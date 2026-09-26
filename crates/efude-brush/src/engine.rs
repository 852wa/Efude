// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Dab rasterization: turns the dabs of one stroke into layer pixels.
//!
//! This is the painting core behind every brush tool. It is independent of
//! any UI: the caller decides which layer is painted, supplies the selection
//! mask and undo history, and feeds dabs from `efude_stroke::StrokeBuilder`.

use crate::{Brush, BrushKind, DynamicSource, PaintMix, ReferenceTarget, SampleRange};
use efude_canvas::{CompositeSampler, Document, History, LayerSampler};
use efude_core::InkPoint;
use glam::Vec2;
use half::f16;
use std::collections::HashMap;
use std::sync::Arc;

/// Dabs thinner than this are drawn at this radius with proportionally less
/// opacity (same ink per dab). A 1 px circle only ever touches one or two
/// pixels, which makes thin lines beaded and jagged.
pub const MIN_DAB_RADIUS: f32 = 0.75;

/// Pen speed, in screen pixels per millisecond, treated as "fast" (1.0) by
/// the speed dynamics.
pub const FAST_SCREEN_SPEED: f32 = 0.5;

/// Paint used per document pixel by mixing brushes (a full charge lasts
/// 2000 px).
const CHARGE_PER_PIXEL: f32 = 1.0 / 2000.0;

/// Per-dab multipliers derived from pressure, speed, tilt and the taper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dynamics {
    pub size: f32,
    pub opacity: f32,
    pub concentration: f32,
    pub mix: f32,
    pub dilution: f32,
}

impl Default for Dynamics {
    fn default() -> Self {
        Self {
            size: 1.0,
            opacity: 1.0,
            concentration: 1.0,
            mix: 1.0,
            dilution: 1.0,
        }
    }
}

/// Computes the dynamics of dab `p` (pressure already mapped through the
/// pressure curves). `view_scale` is screen pixels per document pixel, so
/// speed dynamics respond to how fast the pen moves on screen, not in the
/// document; zooming does not change their feel.
pub fn dynamics(
    brush: &Brush,
    p: &InkPoint,
    previous: Option<InkPoint>,
    view_scale: f32,
) -> Dynamics {
    let (speed, direction, has_previous) = if let Some(previous) = previous {
        let elapsed = p.time_ms.saturating_sub(previous.time_ms).max(1) as f32;
        let delta = p.position - previous.position;
        let screen_speed = delta.length() * view_scale.max(1e-3) / elapsed;
        (
            (screen_speed / FAST_SCREEN_SPEED).clamp(0., 1.),
            (delta.y.atan2(delta.x) / std::f32::consts::TAU).rem_euclid(1.),
            true,
        )
    } else {
        (0., 0.5, false)
    };
    let tilt_amount = p.tilt.length().clamp(0., 1.);
    let random = hash01(p);
    let signal = |source: DynamicSource| match source {
        DynamicSource::None => 1.,
        DynamicSource::Pressure => p.pressure,
        DynamicSource::Speed => speed,
        DynamicSource::Tilt => tilt_amount,
        DynamicSource::Direction => direction,
        DynamicSource::Random => random,
    };
    let mapped = |source: DynamicSource, minimum: f32| {
        if source == DynamicSource::None {
            1.
        } else {
            minimum.clamp(0., 1.) + (1. - minimum.clamp(0., 1.)) * signal(source)
        }
    };
    let taper = p.taper.clamp(0.01, 1.0);
    Dynamics {
        size: mapped(brush.size_source, brush.size_min)
            * if has_previous {
                (1. + brush.speed_size.clamp(-1., 1.) * (speed - 0.5) * 1.5).clamp(0.2, 1.8)
            } else {
                1.
            }
            * (1. + brush.tilt_size.clamp(-1., 1.) * tilt_amount).clamp(0.2, 1.8)
            * taper,
        opacity: mapped(brush.opacity_source, brush.opacity_min)
            * if has_previous {
                (1. + brush.speed_opacity.clamp(-1., 1.) * (speed - 0.5) * 1.5).clamp(0.1, 1.8)
            } else {
                1.
            }
            * (1. + brush.tilt_opacity.clamp(-1., 1.) * tilt_amount).clamp(0.1, 1.8)
            // With no opacity source, taper narrows the line but keeps its
            // density (ink pens).
            * if brush.opacity_source == DynamicSource::None {
                1.0
            } else {
                taper
            },
        concentration: mapped(brush.concentration_source, brush.concentration_min),
        mix: mapped(brush.mix_source, brush.mix_min),
        dilution: mapped(brush.dilution_source, brush.dilution_min),
    }
}

fn hash01(p: &InkPoint) -> f32 {
    let seed = (p.time_ms as u32)
        .wrapping_mul(747796405)
        .wrapping_add((p.position.x as i32 as u32).wrapping_mul(2891336453))
        .wrapping_add((p.position.y as i32 as u32).wrapping_mul(1181783497));
    seed.wrapping_mul(277803737).rotate_left(13) as f32 / u32::MAX as f32
}

/// Paint used up over `distance` document pixels. Only mixing brushes run
/// dry; a pen or plain brush keeps its full charge however long the stroke.
pub fn charge_depletion(brush: &Brush, eraser: bool, distance: f32) -> f32 {
    let mixing = brush.mix.blend > 0.0 || brush.mix.persistence > 0.0;
    // A full charge never runs dry; a partial one lasts proportionally.
    if eraser || !mixing || brush.mix.charge >= 1.0 {
        0.0
    } else {
        distance * CHARGE_PER_PIXEL
    }
}

/// Where a dab is painted.
pub struct DabTarget<'a> {
    pub doc: &'a mut Document,
    /// Index of the raster layer being painted.
    pub layer: usize,
    /// Active selection mask (one byte per document pixel), if any.
    pub selection: Option<&'a [u8]>,
    pub history: &'a mut History,
}

/// What is painted.
#[derive(Clone, Copy)]
pub struct DabStyle<'a> {
    pub brush: &'a Brush,
    /// Brush kind after tool overrides (the blur and smudge tools).
    pub kind: BrushKind,
    pub eraser: bool,
    /// Straight-alpha RGBA paint colour.
    pub color: [u8; 4],
    /// Brush diameter in document pixels before dynamics.
    pub size: f32,
}

/// Placement and shape of one dab, shared by the CPU rasterizer and GPU
/// batches so both draw exactly the same dab.
#[derive(Clone, Copy, Debug)]
pub struct DabGeometry {
    /// Centre after scatter, in document pixels.
    pub center: Vec2,
    /// Drawn radius (at least `MIN_DAB_RADIUS`).
    pub radius: f32,
    /// Opacity factor for dabs thinner than `MIN_DAB_RADIUS`.
    pub thin_alpha: f32,
    pub aspect: f32,
    pub sin: f32,
    pub cos: f32,
    /// Width of the anti-aliased edge.
    pub edge: f32,
    /// Distance (in the dab's elliptical metric) beyond which nothing is drawn.
    pub reach: f32,
    /// Bounding radius in document pixels.
    pub max_radius: f32,
}

impl DabGeometry {
    pub fn new(style: DabStyle, dynamics: &Dynamics, p: &InkPoint) -> Self {
        let brush = style.brush;
        let requested_radius = (style.size * dynamics.size * 0.5).max(0.05);
        // Without anti-aliasing a thin line stays solid (one pixel wide)
        // instead of fading.
        let (radius, thin_alpha) = if brush.antialias == 0 {
            (requested_radius.max(0.5), 1.0)
        } else {
            let radius = requested_radius.max(MIN_DAB_RADIUS);
            (radius, (requested_radius / radius).powi(2).min(1.0))
        };
        let tilt_amount = p.tilt.length().clamp(0., 1.);
        let aspect = (brush.tip_aspect * (1. - brush.tilt_flattening.clamp(0., 0.9) * tilt_amount))
            .clamp(0.1, 10.0);
        let tilt_angle = if tilt_amount > 0.001 {
            p.tilt.y.atan2(p.tilt.x) * brush.tilt_rotation.clamp(0., 1.)
        } else {
            0.
        };
        let angle = brush.tip_rotation.to_radians() + p.rotation + tilt_angle;
        let (sin, cos) = angle.sin_cos();
        let hard_edged = !matches!(
            style.kind,
            BrushKind::Airbrush | BrushKind::Blur | BrushKind::Smudge
        );
        // Anti-aliasing: normal keeps the edge at least one pixel wide (a
        // narrower edge aliases on slanted lines); none makes hard pixel
        // edges; light and strong narrow or widen it.
        let soft = (1. - brush.hardness.clamp(0., 1.)) * radius * 0.75 + 0.5;
        let edge = match brush.antialias {
            0 => (soft - 0.5).max(1e-3),
            1 => soft.max(0.5),
            3 => soft.max(2.0),
            _ => soft.max(1.0),
        };
        // The anti-aliased edge extends half an edge width past `radius`.
        let reach = if hard_edged {
            radius + edge * 0.5
        } else {
            radius
        };
        let max_radius = reach * aspect.max(1.0 / aspect);
        let mut center = p.position;
        if brush.scatter > 0.0 {
            let seed = (p.time_ms as u32)
                .wrapping_mul(747796405)
                .wrapping_add((p.position.x as i32 as u32).wrapping_mul(2891336453))
                .wrapping_add((p.position.y as i32 as u32).wrapping_mul(1181783497));
            let h1 = seed.wrapping_mul(277803737).rotate_left(13);
            let h2 = seed
                .wrapping_add(0x9e3779b9)
                .wrapping_mul(277803737)
                .rotate_left(17);
            let jitter_x = (h1 as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let jitter_y = (h2 as f32 / u32::MAX as f32) * 2.0 - 1.0;
            center += Vec2::new(jitter_x, jitter_y) * radius * brush.scatter.clamp(0.0, 2.0);
        }
        Self {
            center,
            radius,
            thin_alpha,
            aspect,
            sin,
            cos,
            edge,
            reach,
            max_radius,
        }
    }
}

/// Per-dab opacity factors; per pixel they are multiplied by the coverage,
/// grain and selection (the same way on the CPU and the GPU).
#[derive(Clone, Copy, Debug)]
struct AlphaFactors {
    paint: f32,
    erase: f32,
}

impl AlphaFactors {
    fn new(
        style: DabStyle,
        dynamics: &Dynamics,
        mix: PaintMix,
        charge: f32,
        color_alpha: u8,
    ) -> Self {
        let brush = style.brush;
        let watercolor = if matches!(style.kind, BrushKind::Watercolor) {
            0.48
        } else {
            1.0
        };
        Self {
            paint: color_alpha as f32 / 255.
                * brush.opacity
                * (1.0 - mix.dilution.clamp(0.0, 1.0))
                * charge.clamp(0.0, 1.0)
                * dynamics.opacity
                * dynamics.concentration
                * watercolor,
            erase: brush.opacity * dynamics.opacity * dynamics.concentration,
        }
    }
}

/// A pixel written by a dense dab, already converted for storage.
#[derive(Clone, Copy)]
struct Stored {
    accum: [f16; 4],
    bytes: [u8; 4],
    wet: u8,
}

impl Stored {
    /// Same clamping and rounding as `store_stroke_pixel`.
    fn new(mut pixel: [f32; 4], wet: f32) -> Self {
        for channel in &mut pixel {
            *channel = channel.clamp(0.0, 1.0);
        }
        if pixel[3] == 0.0 {
            pixel[..3].fill(0.0);
        }
        let accum = pixel.map(f16::from_f32);
        Self {
            accum,
            bytes: accum.map(|channel| (channel.to_f32() * 255.0).round() as u8),
            wet: (wet.clamp(0.0, 1.0) * 255.0).round() as u8,
        }
    }
}

/// A dab for [`StrokeRaster::stamp_dense`].
#[derive(Clone, Copy)]
struct DenseDab {
    geometry: DabGeometry,
    wet: bool,
    dab_spacing: f32,
    lift: f32,
    /// Opacity factor of the airbrush.
    paint_factor: f32,
    color: [u8; 4],
}

/// One pixel touched by the current stroke: the layer pixel before the
/// stroke, and the strongest paint any dab of the stroke has put there.
#[derive(Clone, Copy)]
struct StrokeTexel {
    base: [u8; 4],
    color: [f16; 3],
    alpha: f16,
}

/// Per-stroke coverage in 64 px tiles. A stroke's opacity is the *maximum*
/// of its dabs at each pixel, composited once over the pixel as it was
/// before the stroke. Without this cap the many overlapping dabs of one
/// stroke pile up: light pressure still ends at full opacity, thin lines turn
/// uneven, and wet brushes bury the colours they should mix with.
#[derive(Clone, Default)]
struct StrokeCover {
    tiles: HashMap<(u32, u32), Arc<Vec<Option<StrokeTexel>>>>,
}

impl StrokeCover {
    const TILE: u32 = 64;

    fn paint(
        &mut self,
        x: u32,
        y: u32,
        base: impl FnOnce() -> [u8; 4],
        color: [f32; 3],
        alpha: f32,
    ) -> [u8; 4] {
        let texel = self.texel(x, y, base);
        if alpha >= texel.alpha.to_f32() {
            texel.alpha = f16::from_f32(alpha);
            texel.color = color.map(f16::from_f32);
        }
        let a = texel.alpha.to_f32();
        let base = texel.base.map(|channel| channel as f32 / 255.0);
        let out_alpha = a + base[3] * (1.0 - a);
        let mut out = [0u8; 4];
        if out_alpha > 0.0 {
            for channel in 0..3 {
                let value = (texel.color[channel].to_f32() * a
                    + base[channel] * base[3] * (1.0 - a))
                    / out_alpha;
                out[channel] = (value * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        out[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        out
    }

    fn erase(&mut self, x: u32, y: u32, base: impl FnOnce() -> [u8; 4], strength: f32) -> [u8; 4] {
        let texel = self.texel(x, y, base);
        if strength > texel.alpha.to_f32() {
            texel.alpha = f16::from_f32(strength);
        }
        let mut out = texel.base;
        out[3] = (texel.base[3] as f32 * (1.0 - texel.alpha.to_f32()))
            .round()
            .clamp(0.0, 255.0) as u8;
        if out[3] == 0 {
            out = [0; 4];
        }
        out
    }

    fn texel(&mut self, x: u32, y: u32, base: impl FnOnce() -> [u8; 4]) -> &mut StrokeTexel {
        let key = (x / Self::TILE, y / Self::TILE);
        let offset = ((y % Self::TILE) * Self::TILE + x % Self::TILE) as usize;
        let tile = self
            .tiles
            .entry(key)
            .or_insert_with(|| Arc::new(vec![None; (Self::TILE * Self::TILE) as usize]));
        Arc::make_mut(tile)[offset].get_or_insert_with(|| StrokeTexel {
            base: base(),
            color: [f16::ZERO; 3],
            alpha: f16::ZERO,
        })
    }
}

/// A dab in a GPU batch (plain data, see [`CoverAccelerator`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverDab {
    pub center: [f32; 2],
    pub radius: f32,
    pub thin_alpha: f32,
    pub aspect: f32,
    pub sin: f32,
    pub cos: f32,
    pub edge: f32,
    pub reach: f32,
    /// Opacity factor (paint) or erase-strength factor (eraser).
    pub alpha: f32,
    pub color: [f32; 3],
}

/// An 8-bit coverage image (bitmap tip or grain texture).
#[derive(Clone, Copy, Debug)]
pub struct CoverTexture<'a> {
    pub width: u32,
    pub height: u32,
    pub coverage: &'a [u8],
}

/// One 64×64 block of the stroke for the accelerator. Arrays are row-major
/// with a stride of `COVER_TILE` and hold `COVER_TILE²` entries.
#[derive(Clone, Debug)]
pub struct CoverTile {
    /// Document position of the block's top-left pixel.
    pub origin: [u32; 2],
    /// Valid size (smaller at the document's right and bottom edges).
    pub width: u32,
    pub height: u32,
    /// Indices into `CoverBatch::dabs`, in painting order.
    pub dabs: Vec<u32>,
    /// Layer pixel before the stroke.
    pub base: Vec<[u8; 4]>,
    /// Stroke paint so far: straight RGB and alpha (erase strength for the
    /// eraser), each already rounded to f16.
    pub state: Vec<[f32; 4]>,
    /// Selection coverage (255 when there is no selection).
    pub selection: Vec<u8>,
}

/// Everything an accelerator needs to paint a batch of dabs. It must give
/// the same result as [`StrokeRaster::stamp`] (up to rounding).
#[derive(Clone, Debug)]
pub struct CoverBatch<'a> {
    pub eraser: bool,
    /// Pencil paper noise.
    pub pencil: bool,
    pub grain_strength: f32,
    pub grain_scale: f32,
    pub grain_fixed: bool,
    pub grain_origin: [f32; 2],
    /// Turn of a canvas-fixed grain around `grain_origin` (radians, 0 =
    /// none).
    pub grain_rotation: f32,
    pub tip: Option<CoverTexture<'a>>,
    pub grain_texture: Option<CoverTexture<'a>>,
    pub dabs: Vec<CoverDab>,
    pub tiles: Vec<CoverTile>,
}

/// Result for one [`CoverTile`].
#[derive(Clone, Debug, PartialEq)]
pub struct CoverTileResult {
    pub state: Vec<[f32; 4]>,
    /// Pixel was reached by a dab (the CPU would have recorded it).
    pub touched: Vec<bool>,
    /// Composited layer pixel.
    pub out: Vec<[u8; 4]>,
}

/// Paints opacity-capped strokes on another device (the GPU).
pub trait CoverAccelerator {
    /// Returns `None` if the batch could not be painted; the caller then
    /// paints it on the CPU.
    fn paint_cover(&self, batch: &CoverBatch) -> Option<Vec<CoverTileResult>>;
}

/// Paints cover batches on the CPU, one block per task across all cores.
/// Same formulas as [`StrokeRaster::stamp`], but each block reads its layer
/// and stroke state once instead of per pixel and per dab.
pub struct CpuCover;

impl CoverAccelerator for CpuCover {
    fn paint_cover(&self, batch: &CoverBatch) -> Option<Vec<CoverTileResult>> {
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(batch.tiles.len().max(1));
        if threads <= 1 {
            return Some(
                batch
                    .tiles
                    .iter()
                    .map(|t| paint_cover_tile(batch, t))
                    .collect(),
            );
        }
        let chunk = batch.tiles.len().div_ceil(threads);
        let mut results: Vec<Option<CoverTileResult>> = vec![None; batch.tiles.len()];
        std::thread::scope(|scope| {
            for (tiles, out) in batch.tiles.chunks(chunk).zip(results.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (tile, slot) in tiles.iter().zip(out) {
                        *slot = Some(paint_cover_tile(batch, tile));
                    }
                });
            }
        });
        results.into_iter().collect()
    }
}

fn batch_grain_noise(batch: &CoverBatch, x: u32, y: u32, for_grain: bool, center: [f32; 2]) -> f32 {
    let scale = batch.grain_scale.max(0.05);
    let (mut gx, mut gy) = (x as f32, y as f32);
    if !batch.grain_fixed {
        gx -= center[0];
        gy -= center[1];
    } else if batch.grain_rotation != 0.0 {
        let origin = Vec2::new(batch.grain_origin[0], batch.grain_origin[1]);
        let turn = Some((
            origin,
            batch.grain_rotation.sin(),
            batch.grain_rotation.cos(),
        ));
        (gx, gy) = turned(turn, gx, gy);
    }
    if let Some(texture) = &batch.grain_texture
        && texture.width > 0
        && texture.height > 0
    {
        let tx = ((gx * scale).floor() as i32).rem_euclid(texture.width as i32) as usize;
        let ty = ((gy * scale).floor() as i32).rem_euclid(texture.height as i32) as usize;
        return texture.coverage[ty * texture.width as usize + tx] as f32 / 255.0;
    }
    let (ix, iy) = if for_grain {
        ((gx * scale).floor() as i32, (gy * scale).floor() as i32)
    } else {
        (x as i32, y as i32)
    };
    let bits = (ix.wrapping_mul(374761393) ^ iy.wrapping_mul(668265263) ^ 0x51ed270b)
        .wrapping_mul(1274126177) as u32;
    bits as f32 / u32::MAX as f32
}

fn f16_round(value: f32) -> f32 {
    f16::from_f32(value).to_f32()
}

fn paint_cover_tile(batch: &CoverBatch, tile: &CoverTile) -> CoverTileResult {
    let cells = (COVER_TILE * COVER_TILE) as usize;
    let mut state = tile.state.clone();
    let mut touched = vec![false; cells];
    let mut out = vec![[0u8; 4]; cells];
    let dabs: Vec<&CoverDab> = tile.dabs.iter().map(|&i| &batch.dabs[i as usize]).collect();
    for ly in 0..tile.height {
        for lx in 0..tile.width {
            let cell = (ly * COVER_TILE + lx) as usize;
            let (x, y) = (tile.origin[0] + lx, tile.origin[1] + ly);
            let selection = tile.selection[cell] as f32 / 255.0;
            let mut st = state[cell];
            let mut hit = false;
            if selection > 0.0 {
                for dab in &dabs {
                    let (dx, dy) = (
                        x as f32 + 0.5 - dab.center[0],
                        y as f32 + 0.5 - dab.center[1],
                    );
                    let (local_x, local_y) =
                        (dx * dab.cos + dy * dab.sin, -dx * dab.sin + dy * dab.cos);
                    let distance2 = local_x * local_x / (dab.aspect * dab.aspect)
                        + local_y * local_y * (dab.aspect * dab.aspect);
                    if distance2 > dab.reach * dab.reach {
                        continue;
                    }
                    let mut coverage =
                        ((dab.radius - distance2.sqrt()) / dab.edge + 0.5).clamp(0.0, 1.0);
                    if coverage <= 0.0 {
                        continue;
                    }
                    hit = true;
                    coverage *= dab.thin_alpha;
                    if let Some(tip) = &batch.tip {
                        let u = (local_x / (dab.radius * dab.aspect) * 0.5 + 0.5)
                            * tip.width.saturating_sub(1) as f32;
                        let v = (local_y * dab.aspect / dab.radius * 0.5 + 0.5)
                            * tip.height.saturating_sub(1) as f32;
                        coverage *= sample_tip_bilinear(tip.coverage, tip.width, tip.height, u, v);
                    }
                    if batch.grain_strength > 0.0 {
                        let noise = batch_grain_noise(batch, x, y, true, dab.center);
                        coverage *= 1.0 - batch.grain_strength.clamp(0.0, 1.0) * (1.0 - noise);
                    }
                    let grain = if batch.pencil {
                        0.38 + 0.62 * batch_grain_noise(batch, x, y, false, dab.center)
                    } else {
                        1.0
                    };
                    let value = (dab.alpha * coverage * grain * selection).clamp(0.0, 1.0);
                    if batch.eraser {
                        if value > st[3] {
                            st[3] = f16_round(value);
                        }
                    } else if value >= st[3] {
                        st = [
                            f16_round(dab.color[0]),
                            f16_round(dab.color[1]),
                            f16_round(dab.color[2]),
                            f16_round(value),
                        ];
                    }
                }
            }
            state[cell] = st;
            touched[cell] = hit;
            let base = tile.base[cell];
            out[cell] = if batch.eraser {
                let a = (base[3] as f32 * (1.0 - st[3])).round().clamp(0.0, 255.0) as u8;
                if a > 0 {
                    [base[0], base[1], base[2], a]
                } else {
                    [0; 4]
                }
            } else {
                let a = st[3];
                let b = base.map(|channel| channel as f32 / 255.0);
                let out_alpha = a + b[3] * (1.0 - a);
                let mut pixel = [0u8; 4];
                if out_alpha > 0.0 {
                    for channel in 0..3 {
                        let value = (st[channel] * a + b[channel] * b[3] * (1.0 - a)) / out_alpha;
                        pixel[channel] = (value * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
                pixel[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                pixel
            };
        }
    }
    CoverTileResult {
        state,
        touched,
        out,
    }
}

fn cover_texture(tip: &Option<crate::BrushTip>) -> Option<CoverTexture<'_>> {
    tip.as_ref().map(|tip| CoverTexture {
        width: tip.width,
        height: tip.height,
        coverage: &tip.coverage,
    })
}

/// Side of the blocks in [`CoverTile`].
pub const COVER_TILE: u32 = StrokeCover::TILE;

/// Whether dabs in `style` can be painted by a [`CoverAccelerator`]: brushes
/// whose dabs do not read the canvas (no wet mixing, blur or smudge) and
/// that use the opacity-capped stroke (not the airbrush).
pub fn accelerator_can_paint(style: DabStyle) -> bool {
    let mixing = style.brush.mix.blend > 0.0 || style.brush.mix.persistence > 0.0;
    match style.kind {
        BrushKind::Pen | BrushKind::Pencil | BrushKind::Brush | BrushKind::Watercolor => {
            style.eraser || !mixing
        }
        BrushKind::Eraser => true,
        BrushKind::Airbrush | BrushKind::Blur | BrushKind::Smudge => false,
    }
}

type AccumTiles = HashMap<(u32, u32), Arc<Vec<[f16; 4]>>>;
type WetTiles = HashMap<(u32, u32), Arc<Vec<u8>>>;
type BaseTiles = HashMap<(u32, u32), Arc<Vec<Option<[u8; 4]>>>>;

/// Whether dabs in `style` are wet paint: they pick up what is under them,
/// carry it along and lay it down again (see [`StrokeRaster::stamp`]).
/// The smudge tool is wet paint that carries no colour of its own.
pub fn is_wet(style: DabStyle) -> bool {
    if style.eraser {
        return false;
    }
    match style.kind {
        BrushKind::Smudge => true,
        BrushKind::Blur | BrushKind::Eraser => false,
        _ => style.brush.mix.blend > 0.0 || style.brush.mix.persistence > 0.0,
    }
}

/// How far colour carries under the smudge tool when the brush sets no
/// persistence (share left after one brush diameter).
const SMUDGE_PERSISTENCE: f32 = 0.8;

/// Paint the brush picks up at one dab, premultiplied RGBA.
///
/// `paint` is the brush colour and `paint_alpha` its strength after
/// dilution; `sample` is the straight colour and mean alpha under the
/// brush. Colour mixes by how much of each is present, so transparency never
/// tints the paint; alpha drifts toward the sampled alpha, fully where the
/// canvas holds more paint and by `transparency_pickup` where it holds less.
pub fn wet_pickup(
    paint: [f32; 3],
    paint_alpha: f32,
    sample: Option<([f32; 3], f32)>,
    blend: f32,
    transparency_pickup: f32,
) -> [f32; 4] {
    let blend = blend.clamp(0.0, 1.0);
    let (sample_color, sample_alpha) = sample.unwrap_or((paint, 0.0));
    let paint_weight = (1.0 - blend) * paint_alpha;
    let sample_weight = blend * sample_alpha;
    let total = paint_weight + sample_weight;
    let color: [f32; 3] = if total > 1e-6 {
        std::array::from_fn(|i| (paint[i] * paint_weight + sample_color[i] * sample_weight) / total)
    } else {
        paint
    };
    let toward = if sample_alpha >= paint_alpha {
        blend
    } else {
        blend * transparency_pickup.clamp(0.0, 1.0)
    };
    let alpha = (paint_alpha + (sample_alpha - paint_alpha) * toward).clamp(0.0, 1.0);
    [color[0] * alpha, color[1] * alpha, color[2] * alpha, alpha]
}

/// Lays premultiplied `paint` over premultiplied `pixel` with weight `w`.
/// `lift` (0..1) is how much the dab replaces existing paint instead of
/// covering it: 0 is a plain "over" (paint builds up), 1 moves the pixel
/// straight toward the carried paint (smooth blending, transparency too).
pub fn wet_deposit(pixel: [f32; 4], paint: [f32; 4], w: f32, lift: f32) -> [f32; 4] {
    let w = w.clamp(0.0, 1.0);
    let removal = paint[3] + lift.clamp(0.0, 1.0) * (1.0 - paint[3]);
    let keep = 1.0 - w * removal;
    std::array::from_fn(|i| (paint[i] * w + pixel[i] * keep).clamp(0.0, 1.0))
}
const ACCUM_TILE: u32 = 64;

/// State of the stroke being painted. Cheap to clone (tiles are shared
/// `Arc`s), so a caller can snapshot it to take back a provisional tail.
#[derive(Clone)]
pub struct StrokeRaster {
    /// Colour the brush carries between dabs (wet mixing and smudging).
    pub previous_mix_color: [f32; 3],
    /// Paint left on a mixing brush (1 = full).
    pub remaining_charge: f32,
    /// Previous dab of this stroke, for speed and direction.
    pub last_dab: Option<InkPoint>,
    /// The stroke's first point: grain turns around it (see
    /// `grain_rotation`).
    pub grain_origin: Vec2,
    /// Turn of a canvas-fixed grain for this stroke, in radians (brushes
    /// with `grain_random_rotation` get a new one for every stroke).
    pub grain_rotation: f32,
    /// Build-up buffer for the airbrush, blur, smudge and wet paint (f16 per
    /// channel).
    accum: AccumTiles,
    cover: StrokeCover,
    /// Premultiplied paint a wet brush carries between dabs.
    reservoir: Option<[f32; 4]>,
    /// Where wet paint went this stroke (for the rim at pen-up).
    wet: WetTiles,
    /// Layer pixels as they were before this wet stroke touched them. Wet
    /// brushes pick up from these, so a stroke never feeds on its own
    /// paint; how far colour is dragged is set by the persistence alone.
    wet_base: BaseTiles,
    layer_sampler: Option<(u64, LayerSampler)>,
    composite_sampler: Option<CompositeSampler>,
}

impl Default for StrokeRaster {
    fn default() -> Self {
        Self::new([0.0; 3], 1.0, Vec2::ZERO)
    }
}

impl StrokeRaster {
    pub fn new(paint: [f32; 3], charge: f32, grain_origin: Vec2) -> Self {
        Self {
            previous_mix_color: paint,
            remaining_charge: charge,
            last_dab: None,
            grain_origin,
            grain_rotation: 0.0,
            accum: HashMap::new(),
            cover: StrokeCover::default(),
            reservoir: None,
            wet: HashMap::new(),
            wet_base: HashMap::new(),
            layer_sampler: None,
            composite_sampler: None,
        }
    }

    /// Clears per-pixel stroke buffers and samplers (keeps colour state).
    pub fn reset_buffers(&mut self) {
        self.accum = HashMap::new();
        self.cover = StrokeCover::default();
        self.reservoir = None;
        self.wet = HashMap::new();
        self.wet_base = HashMap::new();
        self.layer_sampler = None;
        self.composite_sampler = None;
    }

    /// Paints one dab. Call `finish_dab` once per input dab afterwards (a
    /// symmetric stroke stamps several points per dab).
    pub fn stamp(
        &mut self,
        target: &mut DabTarget,
        style: DabStyle,
        dynamics: &Dynamics,
        p: InkPoint,
    ) {
        if self.composite_sampler.is_none() {
            self.layer_sampler = match style.brush.mix.reference_target {
                ReferenceTarget::Layer(layer_id) => {
                    LayerSampler::new(target.doc, layer_id).map(|sampler| (layer_id, sampler))
                }
                _ => None,
            };
            self.composite_sampler = Some(CompositeSampler::new(target.doc));
        }
        let brush = style.brush;
        let turn = grain_turn(brush, self.grain_origin, self.grain_rotation);
        let kind = style.kind;
        let (width, height) = (target.doc.width, target.doc.height);
        let DabGeometry {
            center,
            radius,
            thin_alpha,
            aspect,
            sin,
            cos,
            edge,
            reach,
            max_radius,
        } = DabGeometry::new(style, dynamics, &p);
        let (x, y) = (center.x.round() as i32, center.y.round() as i32);
        let c = if style.eraser {
            [255, 255, 255, 0]
        } else {
            style.color
        };
        let mut mix = brush.mix;
        mix.blend *= dynamics.mix;
        mix.persistence *= dynamics.mix;
        mix.dilution *= dynamics.dilution;
        let wet = is_wet(style);
        // Wet paint: pick up what is under the brush, blend it into the paint
        // the brush carries, and lay that down again. Dragging from one colour
        // into another therefore pulls a smooth gradient, and dragging into
        // transparency thins the paint out.
        // Travel since the previous dab, as a fraction of the diameter: makes
        // wet paint and blur independent of the dab spacing.
        let travelled = self
            .last_dab
            .map(|previous| previous.position.distance(p.position))
            .unwrap_or(brush.spacing * 2.0 * radius);
        let dab_spacing = (travelled / (2.0 * radius)).clamp(0.02, 1.0);
        let smudge = matches!(kind, BrushKind::Smudge);
        let (blend, persistence, transparency_pickup, lift) = if smudge {
            let persistence = if mix.persistence > 0.0 {
                mix.persistence
            } else {
                SMUDGE_PERSISTENCE
            };
            (1.0, persistence, 1.0, 1.0)
        } else {
            (
                mix.blend,
                mix.persistence,
                brush.bleed.transparency_pickup(),
                mix.blend.clamp(0.0, 1.0) * brush.bleed.lift(),
            )
        };
        if wet {
            let paint = [c[0] as f32 / 255., c[1] as f32 / 255., c[2] as f32 / 255.];
            let paint_alpha = if smudge {
                0.0
            } else {
                c[3] as f32 / 255.0
                    * (1.0 - mix.dilution.clamp(0.0, 1.0))
                    * self.remaining_charge.clamp(0.0, 1.0)
            };
            // Sample the whole footprint, not one pixel at its centre: with
            // dab spacing below the radius the centre has already been
            // painted by the previous dab and would only return the brush's
            // own paint.
            let sample_radius = match mix.sample_range {
                SampleRange::Center => radius.max(1.0),
                SampleRange::Average if mix.sample_radius > 0.0 => mix.sample_radius,
                SampleRange::Average => radius,
            }
            .min(64.0);
            let sample = self.sample_mix_color(target, center, sample_radius, mix);
            let pick = wet_pickup(paint, paint_alpha, sample, blend, transparency_pickup);
            let reservoir = match self.reservoir {
                Some(carried) if persistence > 0.0 => {
                    // Persistence is how much colour survives one brush
                    // diameter of travel.
                    let carry = persistence.clamp(0.0, 0.995).powf(dab_spacing.max(0.02));
                    std::array::from_fn(|i| pick[i] + (carried[i] - pick[i]) * carry)
                }
                _ => pick,
            };
            self.reservoir = Some(reservoir);
            let straight = if reservoir[3] > 1e-6 {
                [0, 1, 2].map(|i| reservoir[i] / reservoir[3])
            } else {
                paint
            };
            self.previous_mix_color = straight;
        }
        // Brushes that build up or mix paint read and write every pixel of
        // the dab; they go through a dense, multi-core path.
        if !style.eraser && (wet || matches!(kind, BrushKind::Blur | BrushKind::Airbrush)) {
            let factors = AlphaFactors::new(style, dynamics, mix, self.remaining_charge, c[3]);
            let geometry = DabGeometry {
                center,
                radius,
                thin_alpha,
                aspect,
                sin,
                cos,
                edge,
                reach,
                max_radius,
            };
            self.stamp_dense(
                target,
                style,
                dynamics,
                DenseDab {
                    geometry,
                    wet,
                    dab_spacing,
                    lift,
                    paint_factor: factors.paint,
                    color: c,
                },
            );
            return;
        }
        // Blur: each pixel moves toward the average of its neighbourhood as
        // it was before this dab (premultiplied, so edges soften into
        // transparency too).
        let blur = if matches!(kind, BrushKind::Blur) {
            let neighbourhood = (radius * 0.3).clamp(1.0, 16.0).round() as i32;
            let reach = max_radius.ceil() as i32 + neighbourhood;
            let (x0, y0) = ((x - reach).max(0), (y - reach).max(0));
            let (x1, y1) = (
                (x + reach).min(width as i32 - 1),
                (y + reach).min(height as i32 - 1),
            );
            if x0 > x1 || y0 > y1 {
                None
            } else {
                let stride = (x1 - x0 + 1) as usize;
                let mut region = Vec::with_capacity(stride * (y1 - y0 + 1) as usize);
                for sy in y0..=y1 {
                    for sx in x0..=x1 {
                        let [r, g, b, a] = self.stroke_pixel(target, sx as u32, sy as u32);
                        region.push([r * a, g * a, b * a, a]);
                    }
                }
                Some(BlurRegion {
                    origin: (x0, y0),
                    end: (x1, y1),
                    stride,
                    pixels: region,
                    radius: neighbourhood,
                })
            }
        } else {
            None
        };
        let layer = target.layer;
        let factors = AlphaFactors::new(style, dynamics, mix, self.remaining_charge, c[3]);
        for yy in y - max_radius.ceil() as i32..=y + max_radius.ceil() as i32 {
            for xx in x - max_radius.ceil() as i32..=x + max_radius.ceil() as i32 {
                if xx < 0 || yy < 0 || xx >= width as i32 || yy >= height as i32 {
                    continue;
                }
                // Measure from the pixel centre, not its corner.
                let (dx, dy) = (xx as f32 + 0.5 - center.x, yy as f32 + 0.5 - center.y);
                let (local_x, local_y) = (dx * cos + dy * sin, -dx * sin + dy * cos);
                let distance2 =
                    local_x * local_x / (aspect * aspect) + local_y * local_y * (aspect * aspect);
                if distance2 > reach * reach {
                    continue;
                }
                let pixel_index = (yy as u32 * width + xx as u32) as usize;
                let selection_coverage = match target.selection {
                    Some(mask) => mask.get(pixel_index).copied().unwrap_or(0) as f32 / 255.0,
                    None => 1.0,
                };
                if selection_coverage <= 0.0 {
                    continue;
                }
                for ch in 0..4 {
                    target
                        .history
                        .record_pixel(&target.doc.layers[layer], pixel_index * 4 + ch);
                }
                let mut coverage = match kind {
                    BrushKind::Airbrush => (-distance2 / (radius * radius * 0.55)).exp(),
                    BrushKind::Blur | BrushKind::Smudge => {
                        (radius + 0.5 - distance2.sqrt()).clamp(0.0, 1.0)
                    }
                    _ => ((radius - distance2.sqrt()) / edge + 0.5).clamp(0., 1.),
                };
                if coverage <= 0.0 {
                    continue;
                }
                coverage *= thin_alpha;
                if let Some(tip) = &brush.tip {
                    let u = (local_x / (radius * aspect) * 0.5 + 0.5)
                        * tip.width.saturating_sub(1) as f32;
                    let v = (local_y * aspect / radius * 0.5 + 0.5)
                        * tip.height.saturating_sub(1) as f32;
                    coverage *= sample_tip_bilinear(&tip.coverage, tip.width, tip.height, u, v);
                }
                if brush.grain > 0. {
                    let noise = grain_noise(brush, center, turn, xx, yy, true);
                    coverage *= 1. - brush.grain.clamp(0., 1.) * (1. - noise);
                }
                let grain = if matches!(kind, BrushKind::Pencil) {
                    0.38 + 0.62 * grain_noise(brush, center, turn, xx, yy, false)
                } else {
                    1.0
                };
                let (px, py) = (xx as u32, yy as u32);
                if style.eraser {
                    let strength =
                        (factors.erase * coverage * grain * selection_coverage).clamp(0.0, 1.0);
                    let layer = &mut target.doc.layers[layer];
                    let out = self
                        .cover
                        .erase(px, py, || layer.pixels.pixel(px, py), strength);
                    layer.pixels.set_pixel(px, py, out);
                    continue;
                }
                let a = (factors.paint * coverage * grain * selection_coverage).clamp(0., 1.);
                if matches!(kind, BrushKind::Blur) {
                    let Some(region) = &blur else {
                        continue;
                    };
                    let strength = (brush.opacity
                        * coverage
                        * grain
                        * dynamics.opacity
                        * selection_coverage
                        * dynamics.concentration)
                        .clamp(0., 1.);
                    let w = 1.0 - (1.0 - strength).powf(dab_spacing);
                    let before = region.at(xx, yy);
                    let average = region.average(xx, yy);
                    let out: [f32; 4] =
                        std::array::from_fn(|i| before[i] + (average[i] - before[i]) * w);
                    let out = if out[3] > 1e-6 {
                        [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
                    } else {
                        [0.0; 4]
                    };
                    self.store_stroke_pixel(target, px, py, out);
                    continue;
                }
                if wet {
                    let Some(paint) = self.reservoir else {
                        continue;
                    };
                    // A pixel is crossed by about 1/spacing dabs; spread the
                    // dab weight over them so pressure sets how far one pass
                    // moves the canvas toward the carried paint.
                    let strength = (brush.opacity
                        * dynamics.opacity
                        * dynamics.concentration
                        * coverage
                        * grain
                        * selection_coverage)
                        .clamp(0.0, 1.0);
                    let w = 1.0 - (1.0 - strength).powf(dab_spacing);
                    let straight = self.stroke_pixel(target, px, py);
                    let pixel = [
                        straight[0] * straight[3],
                        straight[1] * straight[3],
                        straight[2] * straight[3],
                        straight[3],
                    ];
                    let out = wet_deposit(pixel, paint, w, lift);
                    let out = if out[3] > 1e-6 {
                        [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
                    } else {
                        [0.0; 4]
                    };
                    self.remember_base(target, px, py);
                    self.store_stroke_pixel(target, px, py, out);
                    self.mark_wet(px, py, coverage * selection_coverage);
                    continue;
                }
                if !matches!(kind, BrushKind::Airbrush) {
                    // Opacity-capped stroke (see `StrokeCover`). The airbrush
                    // keeps building up, like spray.
                    let color = [c[0], c[1], c[2]].map(|channel| channel as f32 / 255.0);
                    let layer = &mut target.doc.layers[layer];
                    let out = self
                        .cover
                        .paint(px, py, || layer.pixels.pixel(px, py), color, a);
                    layer.pixels.set_pixel(px, py, out);
                    continue;
                }
                let mut pixel = self.stroke_pixel(target, px, py);
                let oa = pixel[3];
                let na = a + oa * (1. - a);
                if na > 0. {
                    for (ch, src) in c.iter().take(3).enumerate() {
                        pixel[ch] = (*src as f32 / 255.0 * a + pixel[ch] * oa * (1. - a)) / na;
                    }
                    pixel[3] = na;
                    self.store_stroke_pixel(target, px, py, pixel);
                }
            }
        }
    }

    /// Current pixels (straight alpha, with this stroke's build-up) of the
    /// `width`×`height` block at (`x0`, `y0`), read a tile at a time.
    fn gather(
        &self,
        target: &DabTarget,
        x0: u32,
        y0: u32,
        width: u32,
        height: u32,
    ) -> Vec<[f32; 4]> {
        let storage = efude_canvas::TILE_SIZE;
        let pixels = &target.doc.layers[target.layer].pixels;
        let mut out = vec![[0.0f32; 4]; (width * height) as usize];
        let mut ty = y0 / ACCUM_TILE;
        while ty * ACCUM_TILE < y0 + height {
            let mut tx = x0 / ACCUM_TILE;
            while tx * ACCUM_TILE < x0 + width {
                let accum = self.accum.get(&(tx, ty));
                let layer = pixels.tile_data(tx * ACCUM_TILE / storage, ty * ACCUM_TILE / storage);
                let (bx0, by0) = ((tx * ACCUM_TILE).max(x0), (ty * ACCUM_TILE).max(y0));
                let (bx1, by1) = (
                    ((tx + 1) * ACCUM_TILE).min(x0 + width),
                    ((ty + 1) * ACCUM_TILE).min(y0 + height),
                );
                for y in by0..by1 {
                    for x in bx0..bx1 {
                        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
                        let value = match accum.map(|tile| tile[offset]) {
                            Some(pixel) if !pixel[3].is_nan() => pixel.map(f16::to_f32),
                            _ => match layer {
                                Some(data) => {
                                    let i = (((y % storage) * storage + x % storage) * 4) as usize;
                                    [data[i], data[i + 1], data[i + 2], data[i + 3]]
                                        .map(|channel| channel as f32 / 255.0)
                                }
                                None => [0.0; 4],
                            },
                        };
                        out[((y - y0) * width + (x - x0)) as usize] = value;
                    }
                }
                tx += 1;
            }
            ty += 1;
        }
        out
    }

    /// [`Self::gather`] split into bands of rows read on several threads.
    fn gather_parallel(
        &self,
        target: &DabTarget,
        x0: u32,
        y0: u32,
        width: u32,
        height: u32,
    ) -> Vec<[f32; 4]> {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        if threads < 2 || width * height < 16_384 {
            return self.gather(target, x0, y0, width, height);
        }
        let band = height.div_ceil(threads as u32).max(1);
        let bands: Vec<Vec<[f32; 4]>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..height)
                .step_by(band as usize)
                .map(|start| {
                    let rows = band.min(height - start);
                    scope.spawn(move || self.gather(target, x0, y0 + start, width, rows))
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        bands.concat()
    }

    /// One dab of a build-up or mixing brush (airbrush, blur, smudge, wet
    /// paint). Pixels of a dab do not depend on each other, so rows are
    /// computed in parallel from a dense copy of the area, then written
    /// back a tile at a time. Same formulas as the per-pixel path.
    fn stamp_dense(
        &mut self,
        target: &mut DabTarget,
        style: DabStyle,
        dynamics: &Dynamics,
        dab: DenseDab,
    ) {
        let brush = style.brush;
        let turn = grain_turn(brush, self.grain_origin, self.grain_rotation);
        let kind = style.kind;
        let g = dab.geometry;
        let (width, height) = (target.doc.width as i32, target.doc.height as i32);
        let (x, y) = (g.center.x.round() as i32, g.center.y.round() as i32);
        let r = g.max_radius.ceil() as i32;
        let (bx0, by0) = ((x - r).max(0), (y - r).max(0));
        let (bx1, by1) = ((x + r).min(width - 1), (y + r).min(height - 1));
        if bx0 > bx1 || by0 > by1 {
            return;
        }
        let neighbourhood = if matches!(kind, BrushKind::Blur) {
            (g.radius * 0.3).clamp(1.0, 16.0).round() as i32
        } else {
            0
        };
        let (gx0, gy0) = ((bx0 - neighbourhood).max(0), (by0 - neighbourhood).max(0));
        let (gx1, gy1) = (
            (bx1 + neighbourhood).min(width - 1),
            (by1 + neighbourhood).min(height - 1),
        );
        let (gw, gh) = ((gx1 - gx0 + 1) as u32, (gy1 - gy0 + 1) as u32);
        let current = self.gather_parallel(target, gx0 as u32, gy0 as u32, gw, gh);
        let blur = matches!(kind, BrushKind::Blur).then(|| BlurRegion {
            origin: (gx0, gy0),
            end: (gx1, gy1),
            stride: gw as usize,
            pixels: current
                .iter()
                .map(|&[r, g, b, a]| [r * a, g * a, b * a, a])
                .collect(),
            radius: neighbourhood,
        });
        let at =
            |xx: i32, yy: i32| current[((yy - gy0) as usize) * gw as usize + (xx - gx0) as usize];
        let selection = target.selection;
        let doc_width = width as usize;
        let reservoir = self.reservoir;
        let (bw, bh) = ((bx1 - bx0 + 1) as usize, (by1 - by0 + 1) as usize);
        // What each reached pixel becomes (converted here, in parallel).
        let compute = |yy: i32, row: &mut [Option<Stored>]| {
            let compute_one = |xx: i32| -> Option<([f32; 4], f32)> {
                let (dx, dy) = (xx as f32 + 0.5 - g.center.x, yy as f32 + 0.5 - g.center.y);
                let (local_x, local_y) = (dx * g.cos + dy * g.sin, -dx * g.sin + dy * g.cos);
                let distance2 = local_x * local_x / (g.aspect * g.aspect)
                    + local_y * local_y * (g.aspect * g.aspect);
                if distance2 > g.reach * g.reach {
                    return None;
                }
                let selection_coverage = match selection {
                    Some(mask) => {
                        mask.get(yy as usize * doc_width + xx as usize)
                            .copied()
                            .unwrap_or(0) as f32
                            / 255.0
                    }
                    None => 1.0,
                };
                if selection_coverage <= 0.0 {
                    return None;
                }
                let mut coverage = match kind {
                    BrushKind::Airbrush => (-distance2 / (g.radius * g.radius * 0.55)).exp(),
                    BrushKind::Blur | BrushKind::Smudge => {
                        (g.radius + 0.5 - distance2.sqrt()).clamp(0.0, 1.0)
                    }
                    _ => ((g.radius - distance2.sqrt()) / g.edge + 0.5).clamp(0., 1.),
                };
                if coverage <= 0.0 {
                    return None;
                }
                coverage *= g.thin_alpha;
                if let Some(tip) = &brush.tip {
                    let u = (local_x / (g.radius * g.aspect) * 0.5 + 0.5)
                        * tip.width.saturating_sub(1) as f32;
                    let v = (local_y * g.aspect / g.radius * 0.5 + 0.5)
                        * tip.height.saturating_sub(1) as f32;
                    coverage *= sample_tip_bilinear(&tip.coverage, tip.width, tip.height, u, v);
                }
                if brush.grain > 0. {
                    let noise = grain_noise(brush, g.center, turn, xx, yy, true);
                    coverage *= 1. - brush.grain.clamp(0., 1.) * (1. - noise);
                }
                let grain = if matches!(kind, BrushKind::Pencil) {
                    0.38 + 0.62 * grain_noise(brush, g.center, turn, xx, yy, false)
                } else {
                    1.0
                };
                if matches!(kind, BrushKind::Blur) {
                    let Some(region) = &blur else {
                        return None;
                    };
                    let strength = (brush.opacity
                        * coverage
                        * grain
                        * dynamics.opacity
                        * selection_coverage
                        * dynamics.concentration)
                        .clamp(0., 1.);
                    let w = 1.0 - (1.0 - strength).powf(dab.dab_spacing);
                    let before = region.at(xx, yy);
                    let average = region.average(xx, yy);
                    let out: [f32; 4] =
                        std::array::from_fn(|i| before[i] + (average[i] - before[i]) * w);
                    let out = if out[3] > 1e-6 {
                        [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
                    } else {
                        [0.0; 4]
                    };
                    return Some((out, 0.0));
                }
                if dab.wet {
                    let paint = reservoir?;
                    let strength = (brush.opacity
                        * dynamics.opacity
                        * dynamics.concentration
                        * coverage
                        * grain
                        * selection_coverage)
                        .clamp(0.0, 1.0);
                    let w = 1.0 - (1.0 - strength).powf(dab.dab_spacing);
                    let straight = at(xx, yy);
                    let pixel = [
                        straight[0] * straight[3],
                        straight[1] * straight[3],
                        straight[2] * straight[3],
                        straight[3],
                    ];
                    let out = wet_deposit(pixel, paint, w, dab.lift);
                    let out = if out[3] > 1e-6 {
                        [out[0] / out[3], out[1] / out[3], out[2] / out[3], out[3]]
                    } else {
                        [0.0; 4]
                    };
                    return Some((out, coverage * selection_coverage));
                }
                // Airbrush: builds up like spray.
                let a = (dab.paint_factor * coverage * grain * selection_coverage).clamp(0., 1.);
                let mut pixel = at(xx, yy);
                let oa = pixel[3];
                let na = a + oa * (1. - a);
                if na > 0. {
                    for (ch, src) in dab.color.iter().take(3).enumerate() {
                        pixel[ch] = (*src as f32 / 255.0 * a + pixel[ch] * oa * (1. - a)) / na;
                    }
                    pixel[3] = na;
                    return Some((pixel, 0.0));
                }
                None
            };
            for xx in bx0..=bx1 {
                row[(xx - bx0) as usize] =
                    compute_one(xx).map(|(pixel, wet)| Stored::new(pixel, wet));
            }
        };
        let mut out: Vec<Option<Stored>> = vec![None; bw * bh];
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        if threads > 1 && bw * bh >= 16_384 {
            let rows_per = bh.div_ceil(threads);
            let compute = &compute;
            std::thread::scope(|scope| {
                for (chunk_index, chunk) in out.chunks_mut(rows_per * bw).enumerate() {
                    scope.spawn(move || {
                        for (row_index, row) in chunk.chunks_mut(bw).enumerate() {
                            compute(by0 + (chunk_index * rows_per + row_index) as i32, row);
                        }
                    });
                }
            });
        } else {
            for (row_index, row) in out.chunks_mut(bw).enumerate() {
                compute(by0 + row_index as i32, row);
            }
        }
        self.scatter(
            target,
            (bx0 as u32, by0 as u32, bw as u32, bh as u32),
            &out,
            dab.wet,
        );
    }

    /// Writes the results of a dense dab back: stroke build-up, layer pixels
    /// (recorded in the history first) and, for wet paint, the pre-stroke
    /// base and the wet area.
    fn scatter(
        &mut self,
        target: &mut DabTarget,
        (x0, y0, width, height): (u32, u32, u32, u32),
        out: &[Option<Stored>],
        wet: bool,
    ) {
        let storage = efude_canvas::TILE_SIZE;
        let layer = target.layer;
        let mut ty = y0 / ACCUM_TILE;
        while ty * ACCUM_TILE < y0 + height {
            let mut tx = x0 / ACCUM_TILE;
            while tx * ACCUM_TILE < x0 + width {
                let (bx0, by0) = ((tx * ACCUM_TILE).max(x0), (ty * ACCUM_TILE).max(y0));
                let (bx1, by1) = (
                    ((tx + 1) * ACCUM_TILE).min(x0 + width),
                    ((ty + 1) * ACCUM_TILE).min(y0 + height),
                );
                let any = (by0..by1).any(|y| {
                    (bx0..bx1).any(|x| out[((y - y0) * width + (x - x0)) as usize].is_some())
                });
                if !any {
                    tx += 1;
                    continue;
                }
                let (sx, sy) = (tx * ACCUM_TILE / storage, ty * ACCUM_TILE / storage);
                target
                    .history
                    .record_tile(&target.doc.layers[layer], sx, sy);
                let cells = (ACCUM_TILE * ACCUM_TILE) as usize;
                let accum = Arc::make_mut(self.accum.entry((tx, ty)).or_insert_with(|| {
                    Arc::new(vec![
                        [
                            f16::ZERO,
                            f16::ZERO,
                            f16::ZERO,
                            f16::from_f32(f32::NAN)
                        ];
                        cells
                    ])
                }));
                let mut wet_base = wet.then(|| {
                    Arc::make_mut(
                        self.wet_base
                            .entry((tx, ty))
                            .or_insert_with(|| Arc::new(vec![None; cells])),
                    )
                });
                let pixels = target.doc.layers[layer].pixels.tile_mut(sx, sy);
                for y in by0..by1 {
                    for x in bx0..bx1 {
                        let Some(stored) = out[((y - y0) * width + (x - x0)) as usize] else {
                            continue;
                        };
                        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
                        let i = (((y % storage) * storage + x % storage) * 4) as usize;
                        if let Some(base) = wet_base.as_deref_mut()
                            && base[offset].is_none()
                        {
                            base[offset] =
                                Some([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]);
                        }
                        accum[offset] = stored.accum;
                        pixels[i..i + 4].copy_from_slice(&stored.bytes);
                    }
                }
                if wet {
                    let tile = Arc::make_mut(
                        self.wet
                            .entry((tx, ty))
                            .or_insert_with(|| Arc::new(vec![0; cells])),
                    );
                    for y in by0..by1 {
                        for x in bx0..bx1 {
                            if let Some(stored) = out[((y - y0) * width + (x - x0)) as usize] {
                                let offset =
                                    ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
                                tile[offset] = tile[offset].max(stored.wet);
                            }
                        }
                    }
                }
                tx += 1;
            }
            ty += 1;
        }
    }

    /// Paints several dabs, on `accelerator` when the brush allows it and
    /// the batch is large enough (`min_pixels` of dab area) to be worth the
    /// transfer, otherwise on the CPU. Results are the same either way.
    pub fn stamp_many(
        &mut self,
        target: &mut DabTarget,
        style: DabStyle,
        stamps: &[(InkPoint, Dynamics)],
        accelerator: Option<&dyn CoverAccelerator>,
        min_pixels: f32,
    ) {
        self.stamp_many_with_fallback(target, style, stamps, accelerator, min_pixels, None);
    }

    /// Like [`Self::stamp_many`], but batches that do not go to
    /// `accelerator` go to `fallback` (typically [`CpuCover`]) before the
    /// per-dab CPU path.
    pub fn stamp_many_with_fallback(
        &mut self,
        target: &mut DabTarget,
        style: DabStyle,
        stamps: &[(InkPoint, Dynamics)],
        accelerator: Option<&dyn CoverAccelerator>,
        min_pixels: f32,
        fallback: Option<&dyn CoverAccelerator>,
    ) {
        if (accelerator.is_some() || fallback.is_some())
            && accelerator_can_paint(style)
            && !stamps.is_empty()
        {
            let geometry: Vec<DabGeometry> = stamps
                .iter()
                .map(|(p, dynamics)| DabGeometry::new(style, dynamics, p))
                .collect();
            let area: f32 = geometry
                .iter()
                .map(|g| (2.0 * g.max_radius + 1.0).powi(2))
                .sum();
            if let Some(accelerator) = accelerator
                && area >= min_pixels
                && self.paint_on(accelerator, target, style, stamps, &geometry)
            {
                return;
            }
            if let Some(fallback) = fallback
                && self.paint_on(fallback, target, style, stamps, &geometry)
            {
                return;
            }
        }
        for (p, dynamics) in stamps {
            self.stamp(target, style, dynamics, *p);
        }
    }

    fn paint_on(
        &mut self,
        accelerator: &dyn CoverAccelerator,
        target: &mut DabTarget,
        style: DabStyle,
        stamps: &[(InkPoint, Dynamics)],
        geometry: &[DabGeometry],
    ) -> bool {
        let brush = style.brush;
        let (width, height) = (target.doc.width, target.doc.height);
        let tile = COVER_TILE;
        let mut dabs = Vec::with_capacity(stamps.len());
        let mut tile_dabs: std::collections::BTreeMap<(u32, u32), Vec<u32>> = Default::default();
        for (index, ((_, dynamics), g)) in stamps.iter().zip(geometry).enumerate() {
            let mut mix = brush.mix;
            mix.dilution *= dynamics.dilution;
            let factors =
                AlphaFactors::new(style, dynamics, mix, self.remaining_charge, style.color[3]);
            dabs.push(CoverDab {
                center: [g.center.x, g.center.y],
                radius: g.radius,
                thin_alpha: g.thin_alpha,
                aspect: g.aspect,
                sin: g.sin,
                cos: g.cos,
                edge: g.edge,
                reach: g.reach,
                alpha: if style.eraser {
                    factors.erase
                } else {
                    factors.paint
                },
                color: [style.color[0], style.color[1], style.color[2]]
                    .map(|channel| channel as f32 / 255.0),
            });
            // Same pixel range as the CPU loop in `stamp`.
            let (x, y) = (g.center.x.round() as i32, g.center.y.round() as i32);
            let r = g.max_radius.ceil() as i32;
            let (x0, y0) = ((x - r).max(0), (y - r).max(0));
            let (x1, y1) = (
                (x + r).min(width as i32 - 1),
                (y + r).min(height as i32 - 1),
            );
            if x0 > x1 || y0 > y1 {
                continue;
            }
            for ty in y0 as u32 / tile..=y1 as u32 / tile {
                for tx in x0 as u32 / tile..=x1 as u32 / tile {
                    tile_dabs.entry((tx, ty)).or_default().push(index as u32);
                }
            }
        }
        let layer = target.layer;
        let mut tiles = Vec::with_capacity(tile_dabs.len());
        for ((tx, ty), dab_indices) in tile_dabs {
            let origin = [tx * tile, ty * tile];
            let (tile_width, tile_height) =
                (tile.min(width - origin[0]), tile.min(height - origin[1]));
            let cells = (tile * tile) as usize;
            let mut base = vec![[0u8; 4]; cells];
            let mut state = vec![[0.0f32; 4]; cells];
            let mut selection = vec![255u8; cells];
            let existing = self.cover.tiles.get(&(tx, ty));
            let storage = efude_canvas::TILE_SIZE;
            let layer_tile = target.doc.layers[layer]
                .pixels
                .tile_data(origin[0] / storage, origin[1] / storage);
            for ly in 0..tile_height {
                for lx in 0..tile_width {
                    let (x, y) = (origin[0] + lx, origin[1] + ly);
                    let cell = (ly * tile + lx) as usize;
                    match existing.and_then(|texels| texels[cell]) {
                        Some(texel) => {
                            base[cell] = texel.base;
                            let [r, g, b] = texel.color.map(f16::to_f32);
                            state[cell] = [r, g, b, texel.alpha.to_f32()];
                        }
                        None => {
                            if let Some(data) = layer_tile {
                                let i = (((y % storage) * storage + x % storage) * 4) as usize;
                                base[cell] = [data[i], data[i + 1], data[i + 2], data[i + 3]];
                            }
                        }
                    }
                    if let Some(mask) = target.selection {
                        selection[cell] = mask.get((y * width + x) as usize).copied().unwrap_or(0);
                    }
                }
            }
            tiles.push(CoverTile {
                origin,
                width: tile_width,
                height: tile_height,
                dabs: dab_indices,
                base,
                state,
                selection,
            });
        }
        let batch = CoverBatch {
            eraser: style.eraser,
            pencil: matches!(style.kind, BrushKind::Pencil),
            grain_strength: brush.grain,
            grain_scale: brush.grain_scale,
            grain_fixed: brush.grain_fixed,
            grain_origin: [self.grain_origin.x, self.grain_origin.y],
            grain_rotation: grain_turn(brush, self.grain_origin, self.grain_rotation)
                .map_or(0.0, |_| self.grain_rotation),
            tip: cover_texture(&brush.tip),
            grain_texture: cover_texture(&brush.grain_tip),
            dabs,
            tiles,
        };
        let Some(results) = accelerator.paint_cover(&batch) else {
            return false;
        };
        if results.len() != batch.tiles.len() {
            return false;
        }
        let storage = efude_canvas::TILE_SIZE;
        for (tile_input, result) in batch.tiles.iter().zip(results) {
            if !result.touched.iter().any(|&touched| touched) {
                continue;
            }
            let key = (tile_input.origin[0] / tile, tile_input.origin[1] / tile);
            let (sx, sy) = (
                tile_input.origin[0] / storage,
                tile_input.origin[1] / storage,
            );
            target
                .history
                .record_tile(&target.doc.layers[layer], sx, sy);
            let texels = Arc::make_mut(self.cover.tiles.entry(key).or_insert_with(|| {
                Arc::new(vec![None; (StrokeCover::TILE * StrokeCover::TILE) as usize])
            }));
            let pixels = target.doc.layers[layer].pixels.tile_mut(sx, sy);
            for ly in 0..tile_input.height {
                for lx in 0..tile_input.width {
                    let cell = (ly * tile + lx) as usize;
                    if !result.touched[cell] {
                        continue;
                    }
                    let (x, y) = (tile_input.origin[0] + lx, tile_input.origin[1] + ly);
                    let [r, g, b, a] = result.state[cell];
                    texels[cell] = Some(StrokeTexel {
                        base: tile_input.base[cell],
                        color: [r, g, b].map(f16::from_f32),
                        alpha: f16::from_f32(a),
                    });
                    let i = (((y % storage) * storage + x % storage) * 4) as usize;
                    pixels[i..i + 4].copy_from_slice(&result.out[cell]);
                }
            }
        }
        true
    }

    /// Records dab `p` as the stroke's latest and uses up paint for the
    /// distance travelled since the previous dab.
    pub fn finish_dab(&mut self, style: DabStyle, p: InkPoint) {
        if let Some(previous) = self.last_dab {
            let travelled = previous.position.distance(p.position);
            self.remaining_charge = (self.remaining_charge
                - charge_depletion(style.brush, style.eraser, travelled))
            .max(0.0);
        }
        self.last_dab = Some(p);
    }

    fn remember_base(&mut self, target: &DabTarget, x: u32, y: u32) {
        let key = (x / ACCUM_TILE, y / ACCUM_TILE);
        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
        let tile = self
            .wet_base
            .entry(key)
            .or_insert_with(|| Arc::new(vec![None; (ACCUM_TILE * ACCUM_TILE) as usize]));
        if tile[offset].is_none() {
            Arc::make_mut(tile)[offset] = Some(target.doc.layers[target.layer].pixels.pixel(x, y));
        }
    }

    /// Layer pixel before this stroke (straight-alpha floats).
    fn base_pixel(&self, target: &DabTarget, x: u32, y: u32) -> [f32; 4] {
        let key = (x / ACCUM_TILE, y / ACCUM_TILE);
        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
        self.wet_base
            .get(&key)
            .and_then(|tile| tile[offset])
            .unwrap_or_else(|| target.doc.layers[target.layer].pixels.pixel(x, y))
            .map(|channel| channel as f32 / 255.0)
    }

    fn mark_wet(&mut self, x: u32, y: u32, amount: f32) {
        let key = (x / ACCUM_TILE, y / ACCUM_TILE);
        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
        let value = (amount.clamp(0.0, 1.0) * 255.0).round() as u8;
        let tile = self
            .wet
            .entry(key)
            .or_insert_with(|| Arc::new(vec![0; (ACCUM_TILE * ACCUM_TILE) as usize]));
        let cell = &mut Arc::make_mut(tile)[offset];
        *cell = (*cell).max(value);
    }

    /// Ends a wet stroke: pigment gathers along the rim of the area the
    /// stroke wetted, darkening and thickening the paint there. Call once at
    /// pen-up, before committing the history. Does nothing for dry brushes
    /// or when `brush.wet_edge` is 0.
    pub fn finish_stroke(&mut self, target: &mut DabTarget, brush: &Brush) {
        let strength = if brush.wet_edge_on {
            brush.wet_edge.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if strength <= 0.0 || self.wet.is_empty() {
            return;
        }
        let wet = std::mem::take(&mut self.wet);
        let (width, height) = (target.doc.width, target.doc.height);
        let halo = brush.wet_edge_width.clamp(1.0, 64.0).round() as u32;
        let box_radius = (halo / 2).max(1) as usize;
        let tile = ACCUM_TILE;
        let wetness_at = |x: i64, y: i64| -> f32 {
            if x < 0 || y < 0 {
                return 0.0;
            }
            let (x, y) = (x as u32, y as u32);
            wet.get(&(x / tile, y / tile)).map_or(0.0, |cells| {
                cells[((y % tile) * tile + x % tile) as usize] as f32 / 255.0
            })
        };
        // Tile by tile, each with a halo, so a stroke across a large canvas
        // never needs a canvas-sized buffer.
        let mut keys: Vec<(u32, u32)> = wet.keys().copied().collect();
        keys.sort_unstable();
        let side = (tile + 2 * halo) as usize;
        let mut mask = vec![0.0f32; side * side];
        for (tx, ty) in keys {
            let (ox, oy) = (
                (tx * tile) as i64 - halo as i64,
                (ty * tile) as i64 - halo as i64,
            );
            for yy in 0..side {
                for xx in 0..side {
                    mask[yy * side + xx] = wetness_at(ox + xx as i64, oy + yy as i64);
                }
            }
            let mut blurred = mask.clone();
            // Two box blurs approximate a Gaussian of about `halo`.
            for _ in 0..2 {
                box_blur(&mut blurred, side, side, box_radius);
            }
            for ly in 0..tile {
                for lx in 0..tile {
                    let (x, y) = (tx * tile + lx, ty * tile + ly);
                    if x >= width || y >= height {
                        continue;
                    }
                    let index = (ly + halo) as usize * side + (lx + halo) as usize;
                    let wetness = mask[index];
                    // Inside the stroke near its boundary the blurred mask
                    // falls below the mask.
                    let rim = ((wetness - blurred[index]) * 2.5).clamp(0.0, 1.0) * wetness;
                    if rim <= 0.0 {
                        continue;
                    }
                    let pixel_index = (y * width + x) as usize;
                    let selection = match target.selection {
                        Some(mask) => mask.get(pixel_index).copied().unwrap_or(0) as f32 / 255.0,
                        None => 1.0,
                    };
                    let k = strength * rim * selection * 3.0;
                    let mut pixel = self.stroke_pixel(target, x, y);
                    if k <= 0.0 || pixel[3] <= 0.0 {
                        continue;
                    }
                    // Denser pigment: the same paint layered on itself.
                    for channel in &mut pixel[..3] {
                        *channel = channel.clamp(0.0, 1.0).powf(1.0 + k);
                    }
                    pixel[3] = 1.0 - (1.0 - pixel[3].clamp(0.0, 1.0)).powf(1.0 + k);
                    for channel in 0..4 {
                        target.history.record_pixel(
                            &target.doc.layers[target.layer],
                            pixel_index * 4 + channel,
                        );
                    }
                    self.store_stroke_pixel(target, x, y, pixel);
                }
            }
        }
    }

    /// Current pixel of the painted layer as straight-alpha floats,
    /// including this stroke's build-up.
    pub fn stroke_pixel(&self, target: &DabTarget, x: u32, y: u32) -> [f32; 4] {
        let key = (x / ACCUM_TILE, y / ACCUM_TILE);
        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
        if let Some(pixel) = self
            .accum
            .get(&key)
            .and_then(|tile| tile.get(offset))
            .filter(|pixel| !pixel[3].is_nan())
        {
            return pixel.map(f16::to_f32);
        }
        target.doc.layers[target.layer]
            .pixels
            .pixel(x, y)
            .map(|channel| channel as f32 / 255.0)
    }

    fn store_stroke_pixel(&mut self, target: &mut DabTarget, x: u32, y: u32, mut pixel: [f32; 4]) {
        for channel in &mut pixel {
            *channel = channel.clamp(0.0, 1.0);
        }
        if pixel[3] == 0.0 {
            pixel[..3].fill(0.0);
        }
        let pixel = pixel.map(f16::from_f32);
        let key = (x / ACCUM_TILE, y / ACCUM_TILE);
        let offset = ((y % ACCUM_TILE) * ACCUM_TILE + x % ACCUM_TILE) as usize;
        let tile = self.accum.entry(key).or_insert_with(|| {
            Arc::new(vec![
                [
                    f16::ZERO,
                    f16::ZERO,
                    f16::ZERO,
                    f16::from_f32(f32::NAN)
                ];
                (ACCUM_TILE * ACCUM_TILE) as usize
            ])
        });
        Arc::make_mut(tile)[offset] = pixel;
        target.doc.layers[target.layer].pixels.set_pixel(
            x,
            y,
            pixel.map(|channel| (channel.to_f32() * 255.0).round() as u8),
        );
    }

    /// Alpha-weighted average colour within `radius` of `center`, and the
    /// average alpha found there. At most ~13×13 samples per dab.
    fn sample_mix_color(
        &self,
        target: &DabTarget,
        center: Vec2,
        radius: f32,
        mix: PaintMix,
    ) -> Option<([f32; 3], f32)> {
        let doc = &*target.doc;
        let (x, y) = (center.x.floor() as i32, center.y.floor() as i32);
        if x < 0 || y < 0 || x >= doc.width as i32 || y >= doc.height as i32 {
            return None;
        }
        let r = radius.ceil().max(0.0) as i32;
        let step = (r / 6).max(1);
        let mut sum = [0.0f32; 3];
        let mut total_alpha = 0.0f32;
        let mut samples = 0u32;
        let mut sy = y - r;
        while sy <= y + r {
            let mut sx = x - r;
            while sx <= x + r {
                let (dx, dy) = (sx - x, sy - y);
                if dx * dx + dy * dy <= r * r
                    && sx >= 0
                    && sy >= 0
                    && sx < doc.width as i32
                    && sy < doc.height as i32
                {
                    samples += 1;
                    let (ux, uy) = (sx as u32, sy as u32);
                    let rgba = match mix.reference_target {
                        ReferenceTarget::CurrentLayer => self.base_pixel(target, ux, uy),
                        ReferenceTarget::BelowSelected => self
                            .composite_sampler
                            .as_ref()
                            .map_or_else(
                                || {
                                    efude_canvas::sample_composite_pixel_below(
                                        doc,
                                        target.layer,
                                        ux,
                                        uy,
                                    )
                                },
                                |sampler| sampler.sample_below(doc, target.layer, ux, uy),
                            )
                            .map(|channel| channel as f32 / 255.0),
                        ReferenceTarget::VisibleLayers => self
                            .composite_sampler
                            .as_ref()
                            .map_or_else(
                                || efude_canvas::sample_composite_pixel(doc, ux, uy),
                                |sampler| sampler.sample(doc, ux, uy),
                            )
                            .map(|channel| channel as f32 / 255.0),
                        ReferenceTarget::Layer(layer_id) => self
                            .layer_sampler
                            .as_ref()
                            .filter(|(cached_id, _)| *cached_id == layer_id)
                            .map(|(_, sampler)| sampler.sample(doc, ux, uy))
                            .unwrap_or([0; 4])
                            .map(|channel| channel as f32 / 255.0),
                    };
                    for channel in 0..3 {
                        sum[channel] += rgba[channel] * rgba[3];
                    }
                    total_alpha += rgba[3];
                }
                sx += step;
            }
            sy += step;
        }
        if total_alpha <= f32::EPSILON || samples == 0 {
            None
        } else {
            Some((
                sum.map(|v| v / total_alpha),
                (total_alpha / samples as f32).clamp(0.0, 1.0),
            ))
        }
    }
}

/// Pixels around a blur dab, premultiplied, as they were before the dab.
struct BlurRegion {
    origin: (i32, i32),
    end: (i32, i32),
    stride: usize,
    pixels: Vec<[f32; 4]>,
    radius: i32,
}

impl BlurRegion {
    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let x = x.clamp(self.origin.0, self.end.0);
        let y = y.clamp(self.origin.1, self.end.1);
        self.pixels[(y - self.origin.1) as usize * self.stride + (x - self.origin.0) as usize]
    }

    /// Mean over a disc of `radius` (at most 7×7 samples).
    fn average(&self, x: i32, y: i32) -> [f32; 4] {
        let step = (self.radius / 3).max(1);
        let mut sum = [0.0f32; 4];
        let mut count = 0.0f32;
        let mut dy = -self.radius;
        while dy <= self.radius {
            let mut dx = -self.radius;
            while dx <= self.radius {
                if dx * dx + dy * dy <= self.radius * self.radius {
                    let pixel = self.at(x + dx, y + dy);
                    for (total, value) in sum.iter_mut().zip(pixel) {
                        *total += value;
                    }
                    count += 1.0;
                }
                dx += step;
            }
            dy += step;
        }
        sum.map(|total| total / count)
    }
}

/// In-place box blur of a `width`×`height` image, clamped at the borders.
fn box_blur(image: &mut [f32], width: usize, height: usize, radius: usize) {
    fn blur_line(
        image: &mut [f32],
        line: &mut Vec<f32>,
        start: usize,
        stride: usize,
        len: usize,
        radius: usize,
    ) {
        line.clear();
        line.extend((0..len).map(|i| image[start + i * stride]));
        let span = (2 * radius + 1) as f32;
        let at = |i: isize| line[i.clamp(0, len as isize - 1) as usize];
        let r = radius as isize;
        let mut sum: f32 = (-r..=r).map(at).sum();
        for i in 0..len {
            image[start + i * stride] = sum / span;
            sum += at(i as isize + r + 1) - at(i as isize - r);
        }
    }
    let mut line = Vec::new();
    for y in 0..height {
        blur_line(image, &mut line, y * width, 1, width, radius);
    }
    for x in 0..width {
        blur_line(image, &mut line, x, width, height, radius);
    }
}

/// Grain texture or procedural noise at a document pixel, 0..1.
/// `for_grain` selects the brush grain scale for procedural noise; pencil
/// noise without a texture uses the raw pixel position.
/// Grain at pixel (`xx`, `yy`): fixed to the canvas, or (not fixed)
/// following the brush, measured from the centre of the dab `origin`.
/// How a canvas-fixed grain is turned for a stroke: around `origin` by
/// the angle given as (sin, cos); `None` when it is not turned.
type GrainTurn = Option<(Vec2, f32, f32)>;

fn grain_turn(brush: &Brush, origin: Vec2, rotation: f32) -> GrainTurn {
    (brush.grain_fixed && brush.grain_random_rotation && rotation != 0.0)
        .then(|| (origin, rotation.sin(), rotation.cos()))
}

/// Turns canvas point (x, y) by `turn`.
fn turned(turn: GrainTurn, x: f32, y: f32) -> (f32, f32) {
    match turn {
        Some((origin, sin, cos)) => {
            let (dx, dy) = (x - origin.x, y - origin.y);
            (
                origin.x + cos * dx - sin * dy,
                origin.y + sin * dx + cos * dy,
            )
        }
        None => (x, y),
    }
}

fn grain_noise(
    brush: &Brush,
    origin: Vec2,
    turn: GrainTurn,
    xx: i32,
    yy: i32,
    for_grain: bool,
) -> f32 {
    let scale = brush.grain_scale.max(0.05);
    let (gx, gy) = if brush.grain_fixed {
        turned(turn, xx as f32, yy as f32)
    } else {
        (xx as f32 - origin.x, yy as f32 - origin.y)
    };
    if let Some(texture) = &brush.grain_tip {
        let tx = ((gx * scale).floor() as i32).rem_euclid(texture.width as i32) as usize;
        let ty = ((gy * scale).floor() as i32).rem_euclid(texture.height as i32) as usize;
        return texture.coverage[ty * texture.width as usize + tx] as f32 / 255.0;
    }
    let (ix, iy) = if for_grain {
        ((gx * scale).floor() as i32, (gy * scale).floor() as i32)
    } else {
        (xx, yy)
    };
    let bits = (ix.wrapping_mul(374761393) ^ iy.wrapping_mul(668265263) ^ 0x51ed270b)
        .wrapping_mul(1274126177) as u32;
    bits as f32 / u32::MAX as f32
}

/// Bilinear lookup in an 8-bit coverage image; `u`/`v` are in pixels.
/// Nearest-pixel lookup makes scaled or rotated bitmap tips blocky.
pub fn sample_tip_bilinear(coverage: &[u8], width: u32, height: u32, u: f32, v: f32) -> f32 {
    if width == 0 || height == 0 {
        return 0.0;
    }
    let max_u = (width - 1) as f32;
    let max_v = (height - 1) as f32;
    if !(-0.5..=max_u + 0.5).contains(&u) || !(-0.5..=max_v + 0.5).contains(&v) {
        return 0.0;
    }
    let u = u.clamp(0.0, max_u);
    let v = v.clamp(0.0, max_v);
    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
    let (x1, y1) = (
        (x0 + 1).min(width as usize - 1),
        (y0 + 1).min(height as usize - 1),
    );
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: usize, y: usize| {
        coverage.get(y * width as usize + x).copied().unwrap_or(0) as f32 / 255.0
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alpha painted by one pen dab at `center` with a striped grain.
    fn grain_dab(fixed: bool, center: f32) -> Vec<u8> {
        let mut brush = crate::defaults().remove(0);
        brush.grain = 1.0;
        brush.grain_fixed = fixed;
        brush.opacity_source = crate::DynamicSource::None;
        brush.size_source = crate::DynamicSource::None;
        brush.set_grain_source(Some(crate::BrushTip {
            width: 5,
            height: 1,
            coverage: vec![255, 255, 0, 0, 0],
        }));
        let mut doc = Document::new(64, 16);
        let mut history = History::default();
        history.begin();
        let style = DabStyle {
            brush: &brush,
            kind: brush.kind,
            eraser: false,
            color: [0, 0, 0, 255],
            size: 12.0,
        };
        let mut raster = StrokeRaster::new([0.0; 3], 1.0, Vec2::ZERO);
        let p = InkPoint::new(center, 8.0, 1.0, 0);
        let d = dynamics(&brush, &p, None, 1.0);
        let mut target = DabTarget {
            doc: &mut doc,
            layer: 0,
            selection: None,
            history: &mut history,
        };
        raster.stamp(&mut target, style, &d, p);
        (0..64)
            .map(|x| doc.layers[0].pixels.pixel(x, 8)[3])
            .collect()
    }

    /// Alpha across one hard dab drawn with anti-aliasing `level`.
    fn aa_dab(level: u8) -> Vec<u8> {
        let mut brush = crate::defaults().remove(0);
        brush.antialias = level;
        brush.hardness = 1.0;
        brush.grain = 0.0;
        brush.opacity_source = crate::DynamicSource::None;
        brush.size_source = crate::DynamicSource::None;
        let mut doc = Document::new(64, 16);
        let mut history = History::default();
        history.begin();
        let style = DabStyle {
            brush: &brush,
            kind: brush.kind,
            eraser: false,
            color: [0, 0, 0, 255],
            size: 16.0,
        };
        let mut raster = StrokeRaster::new([0.0; 3], 1.0, Vec2::ZERO);
        let p = InkPoint::new(32.3, 8.0, 1.0, 0);
        let d = dynamics(&brush, &p, None, 1.0);
        let mut target = DabTarget {
            doc: &mut doc,
            layer: 0,
            selection: None,
            history: &mut history,
        };
        raster.stamp(&mut target, style, &d, p);
        (0..64)
            .map(|x| doc.layers[0].pixels.pixel(x, 8)[3])
            .collect()
    }

    #[test]
    fn antialiasing_levels_widen_the_soft_edge() {
        let partial = |row: &[u8]| row.iter().filter(|&&a| a > 0 && a < 250).count();
        let rows: Vec<Vec<u8>> = (0..4).map(aa_dab).collect();
        // None: every pixel is either painted or not.
        assert_eq!(partial(&rows[0]), 0, "{:?}", rows[0]);
        for level in 1..4 {
            assert!(
                partial(&rows[level]) >= partial(&rows[level - 1]),
                "{level}: {:?}",
                rows
            );
        }
        assert!(partial(&rows[3]) > partial(&rows[1]), "{rows:?}");
    }

    #[test]
    fn a_turned_fixed_grain_changes_direction_per_stroke() {
        let row = |rotation: f32| {
            let mut brush = crate::defaults().remove(0);
            brush.grain = 1.0;
            brush.grain_fixed = true;
            brush.grain_random_rotation = true;
            brush.opacity_source = crate::DynamicSource::None;
            brush.size_source = crate::DynamicSource::None;
            // Vertical stripes.
            brush.set_grain_source(Some(crate::BrushTip {
                width: 4,
                height: 1,
                coverage: vec![255, 255, 0, 0],
            }));
            let mut doc = Document::new(64, 16);
            let mut history = History::default();
            history.begin();
            let style = DabStyle {
                brush: &brush,
                kind: brush.kind,
                eraser: false,
                color: [0, 0, 0, 255],
                size: 12.0,
            };
            let mut raster = StrokeRaster::new([0.0; 3], 1.0, Vec2::new(30.0, 8.0));
            raster.grain_rotation = rotation;
            let p = InkPoint::new(30.0, 8.0, 1.0, 0);
            let d = dynamics(&brush, &p, None, 1.0);
            let mut target = DabTarget {
                doc: &mut doc,
                layer: 0,
                selection: None,
                history: &mut history,
            };
            raster.stamp(&mut target, style, &d, p);
            (26..34)
                .map(|x| doc.layers[0].pixels.pixel(x, 8)[3])
                .collect::<Vec<u8>>()
        };
        let straight = row(0.0);
        assert!(
            straight.contains(&0) && straight.iter().any(|&a| a > 200),
            "{straight:?}"
        );
        // Turned a quarter: the stripes run along the row.
        let turned = row(std::f32::consts::FRAC_PI_2);
        let painted = turned.iter().filter(|&&a| a > 200).count();
        assert!(painted == 0 || painted == turned.len(), "{turned:?}");
    }

    #[test]
    fn fixed_grain_stays_on_the_canvas_and_brush_grain_follows_the_dab() {
        // Fixed: the stripes stay where they are when the dab moves.
        let (a, b) = (grain_dab(true, 30.0), grain_dab(true, 32.0));
        assert!((28..33).all(|x| (a[x] == 0) == (b[x] == 0)), "{a:?}\n{b:?}");
        // Following: the stripes move with the dab.
        let (a, b) = (grain_dab(false, 30.0), grain_dab(false, 32.0));
        assert!(
            (26..33).all(|x| (a[x] == 0) == (b[x + 2] == 0)),
            "{a:?}\n{b:?}"
        );
        assert!((26..33).any(|x| (a[x] == 0) != (b[x] == 0)));
    }

    #[test]
    fn ink_pen_keeps_full_density_at_any_pressure() {
        let ink = crate::defaults()
            .into_iter()
            .find(|brush| brush.name == "ペン画")
            .unwrap();
        for pressure in [0.05, 0.3, 1.0] {
            let mut p = InkPoint::new(10.0, 10.0, pressure, 0);
            p.taper = 0.2;
            let d = dynamics(&ink, &p, None, 1.0);
            assert_eq!(d.opacity, 1.0, "pressure {pressure}");
            assert!(d.size < 1.0);
        }
    }

    fn stroke(
        doc: &mut Document,
        brush: &Brush,
        color: [u8; 4],
        size: f32,
        eraser: bool,
        points: &[InkPoint],
    ) {
        let mut history = History::default();
        history.begin();
        let mut raster = StrokeRaster::new([0.0; 3], brush.mix.charge, Vec2::ZERO);
        let style = DabStyle {
            brush,
            kind: brush.kind,
            eraser,
            color,
            size,
        };
        for p in points {
            let dynamics = dynamics(brush, p, raster.last_dab, 1.0);
            let mut target = DabTarget {
                doc,
                layer: 0,
                selection: None,
                history: &mut history,
            };
            raster.stamp(&mut target, style, &dynamics, *p);
            raster.finish_dab(style, *p);
        }
        let mut target = DabTarget {
            doc,
            layer: 0,
            selection: None,
            history: &mut history,
        };
        raster.finish_stroke(&mut target, brush);
        history.commit();
    }

    fn fill(doc: &mut Document, x0: u32, x1: u32, color: [u8; 4]) {
        for y in 0..doc.height {
            for x in x0..x1 {
                doc.layers[0].pixels.set_pixel(x, y, color);
            }
        }
    }

    fn preset(index: usize) -> Brush {
        crate::defaults().remove(index)
    }

    #[test]
    fn blender_drags_a_smooth_gradient_between_colours() {
        let mut doc = Document::new(120, 40);
        fill(&mut doc, 0, 50, [220, 30, 30, 255]);
        fill(&mut doc, 50, 120, [30, 60, 220, 255]);
        let blender = preset(8);
        assert_eq!(blender.name, "色混ぜ");
        let line = dense_line(Vec2::new(20.0, 20.0), Vec2::new(110.0, 20.0), 1.0);
        stroke(&mut doc, &blender, [0, 0, 0, 255], 20.0, false, &line);
        let reds: Vec<u8> = (40..100)
            .map(|x| doc.layers[0].pixels.pixel(x, 20)[0])
            .collect();
        // Red fades out step by step into the blue: no jumps, no reversals.
        for pair in reds.windows(2) {
            assert!(pair[1] <= pair[0] + 1, "{reds:?}");
            assert!(pair[0] as i32 - pair[1] as i32 <= 12, "{reds:?}");
        }
        let dragged = doc.layers[0].pixels.pixel(65, 20);
        assert!(dragged[0] > 60 && dragged[2] > 60, "{dragged:?}");
        // Outside the stroke nothing changes.
        assert_eq!(doc.layers[0].pixels.pixel(65, 2), [30, 60, 220, 255]);
    }

    #[test]
    fn blender_thins_out_into_transparency() {
        let mut doc = Document::new(120, 40);
        fill(&mut doc, 0, 50, [30, 60, 220, 255]);
        let line = dense_line(Vec2::new(30.0, 20.0), Vec2::new(110.0, 20.0), 1.0);
        stroke(&mut doc, &preset(8), [0, 0, 0, 255], 20.0, false, &line);
        let alphas: Vec<u8> = (50..110)
            .map(|x| doc.layers[0].pixels.pixel(x, 20)[3])
            .collect();
        for pair in alphas.windows(2) {
            assert!(pair[1] <= pair[0] + 1, "{alphas:?}");
        }
        // The colour is pulled well out into the clear area, fading as it goes.
        let (start, reach, end) = (alphas[0], alphas[30], *alphas.last().unwrap());
        assert!(
            start > 200 && reach > 100 && (end as u32) < start as u32 * 3 / 5,
            "{alphas:?}"
        );
        // Transparency does not tint the colour.
        assert!(doc.layers[0].pixels.pixel(80, 20)[2] > 200);
    }

    #[test]
    fn wet_paint_follows_pressure_and_does_not_pile_up() {
        let alpha_at = |pressure: f32| {
            let mut doc = Document::new(120, 60);
            let line = dense_line(Vec2::new(20.0, 30.0), Vec2::new(100.0, 30.0), pressure);
            stroke(&mut doc, &preset(3), [30, 60, 220, 255], 24.0, false, &line);
            let along: Vec<u8> = (40..90)
                .map(|x| doc.layers[0].pixels.pixel(x, 30)[3])
                .collect();
            let (min, max) = (*along.iter().min().unwrap(), *along.iter().max().unwrap());
            assert!(max - min <= 3, "{along:?}");
            max
        };
        let (light, full) = (alpha_at(0.3), alpha_at(1.0));
        assert!(full < 230, "a single wash stays translucent: {full}");
        assert!((light as f32) < full as f32 * 0.45, "{light} vs {full}");
    }

    #[test]
    fn wet_edge_gathers_pigment_at_the_rim() {
        let mut doc = Document::new(120, 60);
        let line = dense_line(Vec2::new(20.0, 30.0), Vec2::new(100.0, 30.0), 1.0);
        stroke(&mut doc, &preset(3), [30, 60, 220, 255], 24.0, false, &line);
        let column: Vec<[u8; 4]> = (15..45)
            .map(|y| doc.layers[0].pixels.pixel(60, y))
            .collect();
        let interior = doc.layers[0].pixels.pixel(60, 30);
        let rim = column.iter().max_by_key(|p| p[3]).unwrap();
        assert!(rim[3] as i32 >= interior[3] as i32 + 20, "{column:?}");
        assert!(rim[2] < interior[2], "{column:?}");

        let mut plain = preset(3);
        plain.wet_edge = 0.0;
        let mut doc = Document::new(120, 60);
        stroke(&mut doc, &plain, [30, 60, 220, 255], 24.0, false, &line);
        let column: Vec<u8> = (15..45)
            .map(|y| doc.layers[0].pixels.pixel(60, y)[3])
            .collect();
        assert!(column.iter().all(|&a| a <= interior[3]), "{column:?}");
    }

    #[test]
    fn blur_softens_an_edge_into_transparency() {
        let mut doc = Document::new(100, 40);
        fill(&mut doc, 0, 50, [30, 60, 220, 255]);
        let line = dense_line(Vec2::new(50.0, 5.0), Vec2::new(50.0, 35.0), 1.0);
        stroke(&mut doc, &preset(5), [0, 0, 0, 255], 24.0, false, &line);
        let alphas: Vec<u8> = (42..58)
            .map(|x| doc.layers[0].pixels.pixel(x, 20)[3])
            .collect();
        // A soft ramp instead of a hard 255 → 0 step.
        assert!(
            alphas.iter().filter(|&&a| a > 20 && a < 235).count() >= 4,
            "{alphas:?}"
        );
        for pair in alphas.windows(2) {
            assert!(pair[1] <= pair[0] + 1, "{alphas:?}");
        }
        // The colour does not darken or shift.
        let edge = doc.layers[0].pixels.pixel(50, 20);
        assert!(edge[2] > 200 && edge[0] < 50, "{edge:?}");
        // Away from the stroke nothing changes.
        assert_eq!(doc.layers[0].pixels.pixel(30, 2), [30, 60, 220, 255]);
    }

    #[test]
    fn smudge_drags_colour_along() {
        let mut doc = Document::new(120, 40);
        fill(&mut doc, 0, 40, [220, 30, 30, 255]);
        fill(&mut doc, 40, 120, [240, 240, 240, 255]);
        let line = dense_line(Vec2::new(25.0, 20.0), Vec2::new(100.0, 20.0), 1.0);
        stroke(&mut doc, &preset(6), [0, 0, 0, 255], 16.0, false, &line);
        let near = doc.layers[0].pixels.pixel(50, 20);
        let far = doc.layers[0].pixels.pixel(90, 20);
        // Red is pulled into the light area and fades with distance; the
        // brush colour (black) never appears.
        assert!(near[0] > 200 && near[1] < 200, "{near:?}");
        assert!(far[1] > near[1], "{near:?} {far:?}");
        assert!(
            near[0] > near[1] && near.iter().take(3).all(|&c| c > 20),
            "{near:?}"
        );
    }

    #[test]
    fn bleed_styles_differ_over_transparency() {
        let alpha = |bleed: crate::BleedStyle| {
            let mut doc = Document::new(80, 40);
            let mut brush = preset(3);
            brush.bleed = bleed;
            brush.wet_edge = 0.0;
            let line = dense_line(Vec2::new(10.0, 20.0), Vec2::new(70.0, 20.0), 1.0);
            stroke(&mut doc, &brush, [30, 60, 220, 255], 20.0, false, &line);
            doc.layers[0].pixels.pixel(40, 20)[3]
        };
        let (airy, balanced, dense) = (
            alpha(crate::BleedStyle::Airy),
            alpha(crate::BleedStyle::Balanced),
            alpha(crate::BleedStyle::Dense),
        );
        assert!(
            airy < balanced && balanced < dense,
            "{airy} {balanced} {dense}"
        );
    }

    /// Dabs every 0.25 px along a straight line, like a stroke builder.
    fn dense_line(from: Vec2, to: Vec2, pressure: f32) -> Vec<InkPoint> {
        let steps = (from.distance(to) / 0.25).ceil() as usize;
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                let p = from.lerp(to, t);
                InkPoint::new(p.x, p.y, pressure, i as u64)
            })
            .collect()
    }

    fn pen() -> Brush {
        crate::defaults().remove(0)
    }

    #[test]
    fn overlapping_dabs_do_not_pile_up_past_the_dab_opacity() {
        let mut doc = Document::new(100, 30);
        stroke(
            &mut doc,
            &pen(),
            [0, 0, 0, 255],
            8.0,
            false,
            &dense_line(Vec2::new(10.0, 15.0), Vec2::new(90.0, 15.0), 0.3),
        );
        let alpha = doc.layers[0].pixels.pixel(50, 15)[3];
        assert!((60..=110).contains(&alpha), "alpha {alpha}");
    }

    #[test]
    fn thin_slanted_line_is_even() {
        let mut doc = Document::new(120, 40);
        stroke(
            &mut doc,
            &pen(),
            [0, 0, 0, 255],
            1.0,
            false,
            &dense_line(Vec2::new(10.3, 18.2), Vec2::new(110.7, 23.9), 1.0),
        );
        let ink: Vec<u32> = (20..100)
            .map(|x| {
                (0..40)
                    .map(|y| doc.layers[0].pixels.pixel(x, y)[3] as u32)
                    .sum()
            })
            .collect();
        let (min, max) = (*ink.iter().min().unwrap(), *ink.iter().max().unwrap());
        assert!(min as f32 / max as f32 > 0.85, "min {min} max {max}");
    }

    #[test]
    fn eraser_strength_is_capped_within_a_stroke() {
        let mut doc = Document::new(60, 20);
        for y in 0..20 {
            for x in 0..60 {
                doc.layers[0].pixels.set_pixel(x, y, [200, 10, 10, 255]);
            }
        }
        let mut eraser = pen();
        eraser.kind = BrushKind::Eraser;
        eraser.opacity = 0.5;
        stroke(
            &mut doc,
            &eraser,
            [0, 0, 0, 255],
            10.0,
            true,
            &dense_line(Vec2::new(5.0, 10.0), Vec2::new(55.0, 10.0), 1.0),
        );
        let alpha = doc.layers[0].pixels.pixel(30, 10)[3];
        assert!((120..=135).contains(&alpha), "alpha {alpha}");
    }

    #[test]
    fn speed_dynamics_follow_screen_speed_not_document_speed() {
        let mut brush = pen();
        brush.size_source = DynamicSource::Speed;
        brush.size_min = 0.0;
        let a = InkPoint::new(0.0, 0.0, 1.0, 0);
        let b = InkPoint::new(1.0, 0.0, 1.0, 10);
        // 0.1 document px/ms: slow at 100 %, fast when zoomed in 5x.
        let at_100 = dynamics(&brush, &b, Some(a), 1.0).size;
        let at_500 = dynamics(&brush, &b, Some(a), 5.0).size;
        assert!((at_100 - 0.2).abs() < 1e-4, "{at_100}");
        assert!((at_500 - 1.0).abs() < 1e-4, "{at_500}");
    }

    #[test]
    fn mixing_brush_runs_dry_by_distance_only() {
        let mut water = crate::defaults().remove(3);
        water.mix.blend = 0.5;
        water.mix.charge = 0.8;
        assert!((charge_depletion(&water, false, 2000.0) - 1.0).abs() < 1e-6);
        assert_eq!(charge_depletion(&pen(), false, 5000.0), 0.0);
        assert_eq!(charge_depletion(&water, true, 5000.0), 0.0);
    }

    #[test]
    fn tip_bilinear_interpolates_between_texels() {
        let coverage = [0u8, 255, 0, 255];
        assert!((sample_tip_bilinear(&coverage, 2, 2, 0.5, 0.0) - 0.5).abs() < 0.01);
        assert_eq!(sample_tip_bilinear(&coverage, 2, 2, 5.0, 0.0), 0.0);
    }
}
#[cfg(test)]
mod bench {
    use super::*;
    #[test]
    #[ignore]
    fn wet_speed() {
        for index in [2usize, 3] {
            let mut doc = Document::new(2000, 400);
            let brush = crate::defaults().remove(index);
            let mut history = History::default();
            history.begin();
            let mut raster = StrokeRaster::new([0.0; 3], 1.0, Vec2::ZERO);
            let style = DabStyle {
                brush: &brush,
                kind: brush.kind,
                eraser: false,
                color: [30, 60, 220, 255],
                size: 80.0,
            };
            let start = std::time::Instant::now();
            let mut x = 50.0;
            let mut n = 0;
            while x < 1950.0 {
                let p = InkPoint::new(x, 200.0, 1.0, n);
                let d = dynamics(&brush, &p, raster.last_dab, 1.0);
                let mut target = DabTarget {
                    doc: &mut doc,
                    layer: 0,
                    selection: None,
                    history: &mut history,
                };
                raster.stamp(&mut target, style, &d, p);
                raster.finish_dab(style, p);
                x += 80.0 * 0.12;
                n += 4;
            }
            let dabs = start.elapsed();
            let mut target = DabTarget {
                doc: &mut doc,
                layer: 0,
                selection: None,
                history: &mut history,
            };
            raster.finish_stroke(&mut target, &brush);
            println!(
                "{index}: {n} dabs {:?}, rim {:?}",
                dabs,
                start.elapsed() - dabs
            );
        }
    }
}

#[cfg(test)]
mod perf {
    use super::*;

    /// Time per dab of the dense brushes (run with `--release --ignored`).
    #[test]
    #[ignore]
    fn perf_dense_dabs() {
        let brushes = crate::defaults();
        for (name, index, kind) in [
            ("watercolor", 3usize, BrushKind::Watercolor),
            ("airbrush", 4, BrushKind::Airbrush),
            ("blur", 5, BrushKind::Blur),
            ("smudge", 6, BrushKind::Smudge),
        ] {
            let mut doc = Document::new(2000, 2000);
            for y in 0..2000 {
                for x in 0..2000 {
                    doc.layers[0].pixels.set_pixel(
                        x,
                        y,
                        [(x % 255) as u8, 90, (y % 200) as u8, 255],
                    );
                }
            }
            let mut history = History::default();
            history.begin();
            let brush = &brushes[index];
            let style = DabStyle {
                brush,
                kind,
                eraser: false,
                color: [20, 30, 200, 255],
                size: 150.0,
            };
            let mut raster = StrokeRaster::new([0.1, 0.2, 0.8], 1.0, Vec2::ZERO);
            let start = std::time::Instant::now();
            let n = 60;
            for i in 0..n {
                let p = InkPoint::new(300.0 + i as f32 * 8.0, 1000.0, 0.8, i as u64 * 4);
                let d = dynamics(brush, &p, raster.last_dab, 1.0);
                let mut target = DabTarget {
                    doc: &mut doc,
                    layer: 0,
                    selection: None,
                    history: &mut history,
                };
                raster.stamp(&mut target, style, &d, p);
                raster.finish_dab(style, p);
            }
            eprintln!(
                "{name}: {:.2} ms/dab",
                start.elapsed().as_secs_f64() * 1000.0 / n as f64
            );
        }
    }
}
