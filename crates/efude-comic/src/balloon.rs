// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Speech balloons: a shape with an outline and fill, tails, and text.
//! Balloons drawn on the same layer merge: where one balloon (or a tail)
//! overlaps another, the outlines inside the other's interior disappear.

use crate::raster::{Coverage, fill_polygon};
use crate::text::{FontInfo, TextStyle};
use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Outline of a balloon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BalloonShape {
    /// Text only.
    None,
    #[default]
    Ellipse,
    RoundedRect,
    /// Bumpy outline (thoughts).
    Cloud,
    /// Spiky outline (shouts).
    Flash,
}

impl BalloonShape {
    pub const ALL: [BalloonShape; 5] = [
        BalloonShape::None,
        BalloonShape::Ellipse,
        BalloonShape::RoundedRect,
        BalloonShape::Cloud,
        BalloonShape::Flash,
    ];

    /// How much larger than the text box the shape is drawn so the text
    /// stays inside it.
    fn fit(self) -> f32 {
        match self {
            BalloonShape::None => 1.0,
            BalloonShape::Ellipse => 1.42,
            BalloonShape::RoundedRect => 1.12,
            BalloonShape::Cloud => 1.55,
            BalloonShape::Flash => 1.85,
        }
    }
}

/// A tail from the balloon toward the speaker.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tail {
    /// Tip in canvas pixels.
    pub tip: Vec2,
    /// Width where the tail leaves the balloon.
    pub width: f32,
    /// Sideways bend as a fraction of the tail's length (-1..1).
    #[serde(default)]
    pub bend: f32,
    /// A trail of shrinking bubbles instead of a pointed tail (thoughts).
    #[serde(default)]
    pub bubbles: bool,
}

/// A balloon with its text. `shape: None` is plain text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Balloon {
    pub id: u64,
    /// Layer the balloon is drawn on; balloons sharing a layer merge.
    pub layer_id: u64,
    pub center: Vec2,
    /// Size of the shape when `auto_size` is off.
    pub size: Vec2,
    /// Size the shape to the text.
    pub auto_size: bool,
    pub shape: BalloonShape,
    pub line_width: f32,
    pub line_color: [u8; 3],
    pub fill: [u8; 4],
    pub tails: Vec<Tail>,
    pub text: String,
    pub font: Option<FontInfo>,
    pub style: TextStyle,
    pub text_color: [u8; 3],
    /// Varies bumps and spikes.
    pub seed: u64,
}

impl Balloon {
    pub fn new(id: u64, layer_id: u64, center: Vec2, dpi: f32) -> Self {
        let size = crate::text::points_to_pixels(9.0, dpi);
        Self {
            id,
            layer_id,
            center,
            size: Vec2::new(size * 6.0, size * 8.0),
            auto_size: true,
            shape: BalloonShape::Ellipse,
            line_width: (dpi / 25.4 * 0.3).max(1.0),
            line_color: [0, 0, 0],
            fill: [255, 255, 255, 255],
            tails: Vec::new(),
            text: String::new(),
            font: None,
            style: TextStyle {
                size,
                ..TextStyle::default()
            },
            text_color: [0, 0, 0],
            seed: 1,
        }
    }

    /// Shape size for a text block of `text_size` (or the fixed size).
    pub fn shape_size(&self, text_size: Option<Vec2>) -> Vec2 {
        match (self.auto_size, text_size) {
            (true, Some(text)) => {
                let padding = self.style.size * 0.7;
                text * self.shape.fit() + Vec2::splat(padding)
            }
            (true, None) => Vec2::splat(self.style.size * 3.0),
            (false, _) => self.size,
        }
        .max(Vec2::splat(4.0))
    }

    /// The balloon outline for a shape of `size` (empty for plain text).
    pub fn outline(&self, size: Vec2) -> Vec<Vec2> {
        let (c, r) = (self.center, size * 0.5);
        let steps = ((r.x + r.y) * 0.5).clamp(48.0, 360.0) as usize;
        let ellipse = |radius: &dyn Fn(f32) -> f32| {
            (0..steps)
                .map(|i| {
                    let a = i as f32 / steps as f32 * std::f32::consts::TAU;
                    let k = radius(a);
                    c + Vec2::new(a.cos() * r.x, a.sin() * r.y) * k
                })
                .collect::<Vec<_>>()
        };
        match self.shape {
            BalloonShape::None => Vec::new(),
            BalloonShape::Ellipse => ellipse(&|_| 1.0),
            BalloonShape::RoundedRect => {
                let corner = r.x.min(r.y) * 0.35;
                let mut points = Vec::new();
                for (cx, cy, start) in [
                    (r.x - corner, r.y - corner, 0.0f32),
                    (-(r.x - corner), r.y - corner, 90.0),
                    (-(r.x - corner), -(r.y - corner), 180.0),
                    (r.x - corner, -(r.y - corner), 270.0),
                ] {
                    for i in 0..=12 {
                        let a = (start + i as f32 * 7.5).to_radians();
                        points.push(c + Vec2::new(cx, cy) + Vec2::new(a.cos(), a.sin()) * corner);
                    }
                }
                points
            }
            BalloonShape::Cloud => {
                // Round bumps of equal size along a slightly smaller
                // ellipse, meeting in cusps.
                let inner = r * 0.9;
                let dense: Vec<Vec2> = (0..720)
                    .map(|i| {
                        let a = i as f32 / 720.0 * std::f32::consts::TAU;
                        Vec2::new(a.cos() * inner.x, a.sin() * inner.y)
                    })
                    .collect();
                let mut lengths = vec![0.0f32];
                for i in 1..=dense.len() {
                    let step = dense[i % dense.len()].distance(dense[i - 1]);
                    lengths.push(lengths[i - 1] + step);
                }
                let perimeter = *lengths.last().unwrap();
                let bumps = (perimeter / (self.style.size * 1.7))
                    .round()
                    .clamp(7.0, 48.0) as usize;
                let phase = (self.seed % 97) as f32 / 97.0;
                let at = |fraction: f32| {
                    let target = (fraction.rem_euclid(1.0)) * perimeter;
                    let k = lengths
                        .partition_point(|&l| l < target)
                        .clamp(1, dense.len());
                    dense[k % dense.len()]
                };
                let mut points = Vec::new();
                for b in 0..bumps {
                    let a = at((b as f32 + phase) / bumps as f32);
                    let e = at((b as f32 + 1.0 + phase) / bumps as f32);
                    let chord = e - a;
                    let mut normal = Vec2::new(chord.y, -chord.x).normalize_or_zero();
                    if normal.dot((a + e) * 0.5) < 0.0 {
                        normal = -normal;
                    }
                    let height = chord.length() * 0.38;
                    for k in 0..10 {
                        let t = k as f32 / 10.0;
                        points.push(
                            c + a + chord * t + normal * height * (t * std::f32::consts::PI).sin(),
                        );
                    }
                }
                points
            }
            BalloonShape::Flash => {
                let spikes =
                    (14.0 + (r.x + r.y) / (self.style.size * 1.2)).clamp(14.0, 48.0) as usize;
                let mut random = self.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
                let mut next = || {
                    random ^= random << 13;
                    random ^= random >> 7;
                    random ^= random << 17;
                    (random >> 40) as f32 / (1u64 << 24) as f32
                };
                (0..spikes * 2)
                    .map(|i| {
                        let a = i as f32 / (spikes * 2) as f32 * std::f32::consts::TAU;
                        let k = if i % 2 == 0 {
                            1.0 + 0.18 * next()
                        } else {
                            0.72 + 0.06 * next()
                        };
                        c + Vec2::new(a.cos() * r.x, a.sin() * r.y) * k
                    })
                    .collect()
            }
        }
    }

    /// Shapes of the tails: pointed polygons, or bubble ellipses.
    pub fn tail_shapes(&self, size: Vec2) -> Vec<Vec<Vec2>> {
        let mut shapes = Vec::new();
        for tail in &self.tails {
            let direction = (tail.tip - self.center).normalize_or_zero();
            if direction == Vec2::ZERO {
                continue;
            }
            let normal = Vec2::new(-direction.y, direction.x);
            // Where the ray toward the tip leaves the shape's ellipse.
            let r = size * 0.5;
            let edge = 1.0
                / ((direction.x / r.x.max(1.0)).powi(2) + (direction.y / r.y.max(1.0)).powi(2))
                    .sqrt();
            if tail.bubbles {
                let start = self.center + direction * edge;
                let length = (tail.tip - start).length();
                for (i, (at, scale)) in [(0.2f32, 1.0f32), (0.55, 0.65), (0.88, 0.4)]
                    .iter()
                    .enumerate()
                {
                    let p = start + (tail.tip - start) * *at;
                    let radius = (tail.width * 0.5 * scale).max(2.0).min(length * 0.2);
                    let _ = i;
                    shapes.push(
                        (0..32)
                            .map(|k| {
                                let a = k as f32 / 32.0 * std::f32::consts::TAU;
                                p + Vec2::new(a.cos(), a.sin() * 0.8) * radius
                            })
                            .collect(),
                    );
                }
                continue;
            }
            // Start well inside the balloon so the joint is hidden.
            let base = self.center + direction * edge * 0.6;
            let length = (tail.tip - base).length();
            let control =
                (base + tail.tip) * 0.5 + normal * tail.bend.clamp(-1.0, 1.0) * length * 0.5;
            let steps = 16;
            let (mut left, mut right) = (Vec::new(), Vec::new());
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let p =
                    base * (1.0 - t) * (1.0 - t) + control * 2.0 * t * (1.0 - t) + tail.tip * t * t;
                let tangent = (control - base) * 2.0 * (1.0 - t) + (tail.tip - control) * 2.0 * t;
                let n = Vec2::new(-tangent.y, tangent.x).normalize_or_zero();
                let half = tail.width * 0.5 * (1.0 - t);
                left.push(p + n * half);
                right.push(p - n * half);
            }
            right.reverse();
            left.extend(right);
            shapes.push(left);
        }
        shapes
    }
}

/// Squared Euclidean distance transform along one line (Felzenszwalb).
fn distance_1d(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q - p) as f32);
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f32::INFINITY;
                break;
            }
        }
    }
    k = 0;
    for (q, value) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        *value = (q as f32 - p as f32).powi(2) + f[p];
    }
}

/// Distance from each inside pixel to the nearest outside pixel.
fn inside_distance(inside: &[bool], width: usize, height: usize) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut grid: Vec<f32> = inside.iter().map(|&i| if i { FAR } else { 0.0 }).collect();
    let n = width.max(height);
    let (mut f, mut out, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0; n], vec![0.0; n + 1]);
    for x in 0..width {
        for y in 0..height {
            f[y] = grid[y * width + x];
        }
        distance_1d(&f[..height], &mut out[..height], &mut v, &mut z);
        for y in 0..height {
            grid[y * width + x] = out[y];
        }
    }
    for y in 0..height {
        f[..width].copy_from_slice(&grid[y * width..(y + 1) * width]);
        distance_1d(&f[..width], &mut out[..width], &mut v, &mut z);
        grid[y * width..(y + 1) * width].copy_from_slice(&out[..width]);
    }
    grid.iter().map(|d| d.sqrt()).collect()
}

/// One shape's fill and inner (fill minus outline) coverage over a region.
struct ShapeCoverage {
    fill: Vec<f32>,
    inner: Vec<f32>,
    line_color: [u8; 3],
    fill_color: [u8; 4],
}

/// A shape to draw: polygon, line width, line colour, fill colour.
type ShapeSpec = (Vec<Vec2>, f32, [u8; 3], [u8; 4]);

/// An RGBA image of part of the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA.
    pub rgba: Vec<u8>,
}

fn over(dst: &mut [u8], color: [u8; 3], alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    let da = dst[3] as f32 / 255.0;
    let out = a + da * (1.0 - a);
    for c in 0..3 {
        let value = (color[c] as f32 * a + dst[c] as f32 * da * (1.0 - a)) / out.max(1e-6);
        dst[c] = value.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out * 255.0).round() as u8;
}

/// Draws the balloons that share one layer. `texts` holds each balloon's
/// rendered text (see [`crate::text::render`]), placed centred on it.
pub fn render_group(
    balloons: &[&Balloon],
    texts: &[Option<Coverage>],
    canvas_width: u32,
    canvas_height: u32,
) -> Option<Region> {
    let mut shapes: Vec<ShapeSpec> = Vec::new();
    let mut placed_texts = Vec::new();
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (balloon, text) in balloons.iter().zip(texts) {
        let text_size = text
            .as_ref()
            .map(|t| Vec2::new(t.width as f32, t.height as f32));
        let size = balloon.shape_size(text_size);
        if balloon.shape != BalloonShape::None {
            let outline = balloon.outline(size);
            for polygon in std::iter::once(outline).chain(balloon.tail_shapes(size)) {
                for &p in &polygon {
                    min = min.min(p);
                    max = max.max(p);
                }
                shapes.push((
                    polygon,
                    balloon.line_width,
                    balloon.line_color,
                    balloon.fill,
                ));
            }
        }
        if let Some(text) = text {
            let origin = balloon.center - Vec2::new(text.width as f32, text.height as f32) * 0.5;
            min = min.min(origin);
            max = max.max(origin + Vec2::new(text.width as f32, text.height as f32));
            placed_texts.push((origin.round(), text, balloon.text_color));
        }
    }
    if min.x > max.x {
        return None;
    }
    let x0 = (min.x - 2.0).floor().clamp(0.0, canvas_width as f32) as u32;
    let y0 = (min.y - 2.0).floor().clamp(0.0, canvas_height as f32) as u32;
    let x1 = (max.x + 2.0).ceil().clamp(0.0, canvas_width as f32) as u32;
    let y1 = (max.y + 2.0).ceil().clamp(0.0, canvas_height as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (width, height) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let cells = width * height;
    let coverages: Vec<ShapeCoverage> = shapes
        .iter()
        .map(|(polygon, line_width, line_color, fill_color)| {
            let mut fill = vec![0.0f32; cells];
            if let Some(c) = fill_polygon(polygon, canvas_width, canvas_height) {
                for (x, y, value) in c.iter() {
                    if x >= x0 && y >= y0 && x < x1 && y < y1 {
                        fill[(y - y0) as usize * width + (x - x0) as usize] = value as f32 / 255.0;
                    }
                }
            }
            let inside: Vec<bool> = fill.iter().map(|&f| f >= 0.5).collect();
            let distance = inside_distance(&inside, width, height);
            let inner = distance
                .iter()
                .zip(&fill)
                .map(|(&d, &f)| {
                    if f <= 0.0 {
                        0.0
                    } else {
                        (d - line_width + 0.5).clamp(0.0, 1.0).min(f)
                    }
                })
                .collect();
            ShapeCoverage {
                fill,
                inner,
                line_color: *line_color,
                fill_color: *fill_color,
            }
        })
        .collect();
    let mut rgba = vec![0u8; cells * 4];
    for i in 0..cells {
        let pixel = &mut rgba[i * 4..i * 4 + 4];
        // Fill: the strongest shape wins.
        if let Some(shape) = coverages
            .iter()
            .max_by(|a, b| a.fill[i].total_cmp(&b.fill[i]))
            .filter(|s| s.fill[i] > 0.0)
        {
            let c = shape.fill_color;
            over(
                pixel,
                [c[0], c[1], c[2]],
                shape.fill[i] * c[3] as f32 / 255.0,
            );
        }
        // Outline: each shape's ring, hidden inside any other shape.
        let mut line = 0.0f32;
        let mut line_color = [0u8; 3];
        for (s, shape) in coverages.iter().enumerate() {
            let ring = (shape.fill[i] - shape.inner[i]).max(0.0);
            if ring <= 0.0 {
                continue;
            }
            let hidden = coverages
                .iter()
                .enumerate()
                .filter(|(t, _)| *t != s)
                .map(|(_, other)| other.inner[i])
                .fold(0.0f32, f32::max);
            let value = ring * (1.0 - hidden);
            if value > line {
                line = value;
                line_color = shape.line_color;
            }
        }
        over(pixel, line_color, line);
    }
    for (origin, text, color) in placed_texts {
        for ty in 0..text.height {
            for tx in 0..text.width {
                let value = text.data[(ty * text.width + tx) as usize];
                if value == 0 {
                    continue;
                }
                let (x, y) = (origin.x as i64 + tx as i64, origin.y as i64 + ty as i64);
                if x < x0 as i64 || y < y0 as i64 || x >= x1 as i64 || y >= y1 as i64 {
                    continue;
                }
                let i = (y as usize - y0 as usize) * width + (x as usize - x0 as usize);
                over(&mut rgba[i * 4..i * 4 + 4], color, value as f32 / 255.0);
            }
        }
    }
    Some(Region {
        x0,
        y0,
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// Whether `point` hits the balloon (its shape, or its text box for plain
/// text), given the balloon's current shape size.
pub fn hit(balloon: &Balloon, size: Vec2, point: Vec2) -> bool {
    let half = size * 0.5;
    match balloon.shape {
        BalloonShape::None | BalloonShape::RoundedRect => {
            (point - balloon.center).abs().cmple(half).all()
        }
        _ => {
            let d = (point - balloon.center) / half.max(Vec2::ONE);
            d.length_squared() <= 1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balloon(id: u64, center: Vec2) -> Balloon {
        let mut b = Balloon::new(id, 1, center, 300.0);
        b.auto_size = false;
        b.size = Vec2::new(120.0, 80.0);
        b.line_width = 4.0;
        b
    }

    fn at(region: &Region, x: u32, y: u32) -> [u8; 4] {
        let i = ((y - region.y0) * region.width + (x - region.x0)) as usize * 4;
        region.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn ellipse_has_white_fill_and_black_outline() {
        let b = balloon(1, Vec2::new(100.0, 100.0));
        let region = render_group(&[&b], &[None], 300, 300).unwrap();
        assert_eq!(at(&region, 100, 100), [255, 255, 255, 255], "fill");
        let edge = at(&region, 100, 62);
        assert!(edge[0] < 60 && edge[3] > 200, "outline {edge:?}");
        assert_eq!(at(&region, 100, 72)[0], 255, "inside stays white");
    }

    #[test]
    fn overlapping_balloons_merge_their_outlines() {
        let a = balloon(1, Vec2::new(100.0, 100.0));
        let b = balloon(2, Vec2::new(190.0, 100.0));
        let region = render_group(&[&a, &b], &[None, None], 400, 300).unwrap();
        // a's outline at x≈158 lies inside b: hidden.
        let joint = at(&region, 158, 100);
        assert!(
            joint[0] > 200,
            "outline inside the other balloon must vanish: {joint:?}"
        );
        // Outlines away from the overlap stay.
        assert!(at(&region, 42, 100)[0] < 80);
    }

    #[test]
    fn tail_reaches_its_tip_and_joins_without_an_inner_line() {
        let mut b = balloon(1, Vec2::new(100.0, 100.0));
        b.tails.push(Tail {
            tip: Vec2::new(100.0, 220.0),
            width: 30.0,
            bend: 0.0,
            bubbles: false,
        });
        let region = render_group(&[&b], &[None], 300, 300).unwrap();
        assert!(at(&region, 100, 200)[3] > 0, "tail near the tip");
        // The balloon outline where the tail leaves is open.
        assert!(
            at(&region, 100, 139)[0] > 200,
            "{:?}",
            at(&region, 100, 139)
        );
    }

    #[test]
    fn every_shape_draws() {
        for shape in BalloonShape::ALL {
            let mut b = balloon(1, Vec2::new(150.0, 150.0));
            b.shape = shape;
            let region = render_group(&[&b], &[None], 300, 300);
            assert_eq!(region.is_some(), shape != BalloonShape::None, "{shape:?}");
            assert!(hit(&b, b.size, Vec2::new(150.0, 150.0)));
        }
    }

    #[test]
    fn auto_size_fits_the_text() {
        let mut b = balloon(1, Vec2::ZERO);
        b.auto_size = true;
        let size = b.shape_size(Some(Vec2::new(100.0, 200.0)));
        assert!(size.x > 142.0 && size.y > 284.0);
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    use crate::text::{render, system_fonts};

    /// Writes sample balloons to `$EFUDE_BALLOON_PREVIEW` (PPM).
    #[test]
    fn write_preview() {
        let Some(path) = std::env::var_os("EFUDE_BALLOON_PREVIEW") else {
            return;
        };
        let fonts = system_fonts();
        let info = fonts
            .iter()
            .find(|f| f.name.contains("Noto Sans CJK JP"))
            .unwrap();
        let data = std::fs::read(&info.path).unwrap();
        let font = ab_glyph::FontRef::try_from_slice_and_index(&data, info.index).unwrap();
        let (w, h) = (900u32, 420u32);
        let mut image = vec![255u8; (w * h * 3) as usize];
        let samples = [
            (
                BalloonShape::Ellipse,
                "こんにちは\nいい天気！",
                Vec2::new(130.0, 180.0),
            ),
            (BalloonShape::Cloud, "どうしよう…", Vec2::new(330.0, 170.0)),
            (BalloonShape::Flash, "ドーン！！", Vec2::new(540.0, 190.0)),
            (
                BalloonShape::RoundedRect,
                "その頃\n町では",
                Vec2::new(760.0, 150.0),
            ),
        ];
        for (i, (shape, text, center)) in samples.into_iter().enumerate() {
            let mut b = Balloon::new(i as u64, 1, center, 200.0);
            b.shape = shape;
            b.text = text.into();
            b.style.size = 26.0;
            b.line_width = 3.0;
            b.tails.push(Tail {
                tip: center + Vec2::new(40.0, 190.0),
                width: 26.0,
                bend: 0.3,
                bubbles: shape == BalloonShape::Cloud,
            });
            if shape == BalloonShape::RoundedRect {
                b.tails.clear();
                b.style.vertical = false;
            }
            let t = render(&font, &b.text, &b.style);
            let region = render_group(&[&b], &[t], w, h).unwrap();
            for y in 0..region.height {
                for x in 0..region.width {
                    let s = ((y * region.width + x) * 4) as usize;
                    let a = region.rgba[s + 3] as f32 / 255.0;
                    let d = (((region.y0 + y) * w + region.x0 + x) * 3) as usize;
                    for c in 0..3 {
                        image[d + c] =
                            (region.rgba[s + c] as f32 * a + image[d + c] as f32 * (1.0 - a)) as u8;
                    }
                }
            }
        }
        let mut out = format!("P6 {w} {h} 255\n").into_bytes();
        out.extend(image);
        std::fs::write(path, out).unwrap();
    }
}
