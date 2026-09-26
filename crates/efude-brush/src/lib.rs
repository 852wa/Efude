// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
pub mod engine;
use serde::{Deserialize, Serialize};
const MAX_BRUSH_TEXTURE_DIMENSION: u32 = 4096;
const MAX_BRUSH_TEXTURE_PIXELS: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Brush {
    pub name: String,
    pub kind: BrushKind,
    pub size: f32,
    pub opacity: f32,
    pub spacing: f32,
    pub stabilization: usize,
    #[serde(default)]
    pub pull_distance: f32,
    #[serde(default)]
    pub speed_stabilization: f32,
    #[serde(default = "default_pressure_curve")]
    pub pressure_curve: f32,
    #[serde(default = "default_curve_x1")]
    pub pressure_curve_x1: f32,
    #[serde(default = "default_curve_y1")]
    pub pressure_curve_y1: f32,
    #[serde(default = "default_curve_x2")]
    pub pressure_curve_x2: f32,
    #[serde(default = "default_curve_y2")]
    pub pressure_curve_y2: f32,
    #[serde(default)]
    pub taper_start: f32,
    #[serde(default)]
    pub taper_end: f32,
    #[serde(default)]
    pub taper_in_pixels: bool,
    #[serde(default = "default_taper_min")]
    pub taper_min: f32,
    #[serde(default)]
    pub mix: PaintMix,
    #[serde(default)]
    pub speed_size: f32,
    #[serde(default)]
    pub speed_opacity: f32,
    #[serde(default)]
    pub tilt_size: f32,
    #[serde(default)]
    pub tilt_opacity: f32,
    #[serde(default)]
    pub tilt_flattening: f32,
    #[serde(default)]
    pub tilt_rotation: f32,
    #[serde(default = "default_pressure_source")]
    pub size_source: DynamicSource,
    #[serde(default = "default_pressure_source")]
    pub opacity_source: DynamicSource,
    #[serde(default)]
    pub concentration_source: DynamicSource,
    #[serde(default)]
    pub mix_source: DynamicSource,
    #[serde(default)]
    pub dilution_source: DynamicSource,
    #[serde(default = "default_size_min")]
    pub size_min: f32,
    #[serde(default)]
    pub opacity_min: f32,
    #[serde(default)]
    pub concentration_min: f32,
    #[serde(default)]
    pub mix_min: f32,
    #[serde(default)]
    pub dilution_min: f32,
    #[serde(default = "default_hardness")]
    pub hardness: f32,
    #[serde(default)]
    pub grain: f32,
    #[serde(default = "default_grain_scale")]
    pub grain_scale: f32,
    #[serde(default)]
    pub grain_fixed: bool,
    /// Grain as used for painting (255 = paint), derived from
    /// `grain_source` with `grain_invert` and `grain_binary` applied.
    #[serde(default)]
    pub grain_tip: Option<BrushTip>,
    /// Grain image as loaded: dark opaque pixels paint (255), white or
    /// transparent pixels do not (0).
    #[serde(default)]
    pub grain_source: Option<BrushTip>,
    /// 書き味: reshape the line after drawing (tip preview, end taper, wet
    /// edge at pen-up). Off draws every part as final at once.
    #[serde(default = "default_true")]
    pub settle: bool,
    /// Apply `wet_edge` at pen-up.
    #[serde(default = "default_true")]
    pub wet_edge_on: bool,
    /// Anti-aliasing of hard edges: 0 none, 1 light, 2 normal, 3 strong.
    #[serde(default = "default_antialias")]
    pub antialias: u8,
    /// With `grain_fixed`: the grain is turned by a new random angle for
    /// every stroke (still fixed to the canvas while the stroke lasts).
    #[serde(default)]
    pub grain_random_rotation: bool,
    /// Swap painted and unpainted parts of the grain.
    #[serde(default)]
    pub grain_invert: bool,
    /// Two levels only: paint where the grain is at least half on, nothing
    /// elsewhere.
    #[serde(default)]
    pub grain_binary: bool,
    #[serde(default = "default_tip_aspect")]
    pub tip_aspect: f32,
    #[serde(default)]
    pub tip_rotation: f32,
    #[serde(default)]
    pub scatter: f32,
    pub color: [u8; 4],
    #[serde(default)]
    pub tip: Option<BrushTip>,
    /// How strongly pigment gathers at the rim of a wet stroke when the pen
    /// is lifted (0 = off). Only brushes that mix paint use it.
    #[serde(default)]
    pub wet_edge: f32,
    /// Width of that rim in document pixels.
    #[serde(default = "default_wet_edge_width")]
    pub wet_edge_width: f32,
    /// How wet paint spreads into what is already on the canvas.
    #[serde(default)]
    pub bleed: BleedStyle,
}
/// Character of wet mixing: how much the transparency under the brush thins
/// the paint it carries, and how much existing paint it lifts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BleedStyle {
    /// Paint thins out freely into transparent areas and lifts easily:
    /// soft, airy washes.
    Airy,
    #[default]
    Balanced,
    /// Paint keeps its body over transparent areas and builds up.
    Dense,
}
impl BleedStyle {
    /// How much sampled transparency lowers the paint's alpha (0..1).
    pub fn transparency_pickup(self) -> f32 {
        match self {
            Self::Airy => 1.0,
            Self::Balanced => 0.55,
            Self::Dense => 0.15,
        }
    }
    /// How much of the paint already on the canvas a dab replaces rather
    /// than covers (0..1, scaled by the mixing amount).
    pub fn lift(self) -> f32 {
        match self {
            Self::Airy => 1.0,
            Self::Balanced => 0.8,
            Self::Dense => 0.55,
        }
    }
}
fn default_true() -> bool {
    true
}
fn default_antialias() -> u8 {
    2
}
fn default_wet_edge_width() -> f32 {
    6.0
}
#[derive(Clone, Debug)]
pub struct BrushTip {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}
impl Serialize for BrushTip {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        (&self.width, &self.height, &self.coverage).serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for BrushTip {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (width, height, coverage) = <(u32, u32, Vec<u8>)>::deserialize(deserializer)?;
        if width == 0
            || height == 0
            || width > MAX_BRUSH_TEXTURE_DIMENSION
            || height > MAX_BRUSH_TEXTURE_DIMENSION
            || width as usize * height as usize > MAX_BRUSH_TEXTURE_PIXELS
            || width as usize * height as usize != coverage.len()
        {
            return Err(serde::de::Error::custom("invalid brush texture dimensions"));
        }
        Ok(Self {
            width,
            height,
            coverage,
        })
    }
}
fn default_pressure_curve() -> f32 {
    1.0
}
fn default_curve_x1() -> f32 {
    0.25
}
fn default_curve_y1() -> f32 {
    0.25
}
fn default_curve_x2() -> f32 {
    0.75
}
fn default_curve_y2() -> f32 {
    0.75
}
fn default_taper_min() -> f32 {
    0.15
}
fn default_pressure_source() -> DynamicSource {
    DynamicSource::Pressure
}
fn default_size_min() -> f32 {
    0.08
}
fn default_hardness() -> f32 {
    0.85
}
fn default_grain_scale() -> f32 {
    1.0
}
fn default_tip_aspect() -> f32 {
    1.0
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct PaintMix {
    #[serde(default)]
    pub blend: f32,
    #[serde(default)]
    pub dilution: f32,
    #[serde(default)]
    pub persistence: f32,
    #[serde(default = "default_mix_charge")]
    pub charge: f32,
    #[serde(default)]
    pub sample_range: SampleRange,
    #[serde(default)]
    pub reference_target: ReferenceTarget,
    #[serde(default)]
    pub sample_radius: f32,
}
fn default_mix_charge() -> f32 {
    1.0
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleRange {
    #[default]
    Center,
    Average,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceTarget {
    #[default]
    CurrentLayer,
    BelowSelected,
    VisibleLayers,
    Layer(u64),
}
impl Default for PaintMix {
    fn default() -> Self {
        Self {
            blend: 0.0,
            dilution: 0.0,
            persistence: 0.0,
            charge: 1.0,
            sample_range: SampleRange::Center,
            reference_target: ReferenceTarget::CurrentLayer,
            sample_radius: 0.0,
        }
    }
}
impl PaintMix {
    pub fn dab_color(self, paint: [f32; 3], sample: [f32; 3], previous: [f32; 3]) -> [f32; 3] {
        let pick: [f32; 3] = std::array::from_fn(|i| {
            paint[i] * (1. - self.blend.clamp(0., 1.)) + sample[i] * self.blend.clamp(0., 1.)
        });
        let carry: [f32; 3] = std::array::from_fn(|i| {
            pick[i] * (1. - self.persistence.clamp(0., 1.))
                + previous[i] * self.persistence.clamp(0., 1.)
        });
        carry
    }
    pub fn effective_opacity(self, base: f32) -> f32 {
        base.clamp(0., 1.) * (1. - self.dilution.clamp(0., 1.)) * self.charge.clamp(0., 1.)
    }
}

fn validate_brush(brush: &Brush) -> Result<(), Box<dyn std::error::Error>> {
    fn within(value: f32, low: f32, high: f32) -> bool {
        value.is_finite() && (low..=high).contains(&value)
    }

    let taper_max = if brush.taper_in_pixels { 512.0 } else { 0.5 };
    let valid = brush.size.is_finite()
        && brush.size > 0.0
        && within(brush.opacity, 0.0, 1.0)
        && within(brush.spacing, 0.005, 2.0)
        && brush.stabilization <= 15
        && brush.antialias <= 3
        && within(brush.pull_distance, 0.0, 256.0)
        && within(brush.speed_stabilization, 0.0, 1.0)
        && within(brush.pressure_curve, 0.2, 3.0)
        && within(brush.pressure_curve_x1, 0.0, 1.0)
        && within(brush.pressure_curve_y1, 0.0, 1.0)
        && within(brush.pressure_curve_x2, 0.0, 1.0)
        && within(brush.pressure_curve_y2, 0.0, 1.0)
        && within(brush.taper_start, 0.0, taper_max)
        && within(brush.taper_end, 0.0, taper_max)
        && within(brush.taper_min, 0.01, 1.0)
        && within(brush.speed_size, -1.0, 1.0)
        && within(brush.speed_opacity, -1.0, 1.0)
        && within(brush.tilt_size, -1.0, 1.0)
        && within(brush.tilt_opacity, -1.0, 1.0)
        && within(brush.tilt_flattening, 0.0, 0.9)
        && within(brush.tilt_rotation, 0.0, 1.0)
        && within(brush.size_min, 0.0, 1.0)
        && within(brush.opacity_min, 0.0, 1.0)
        && within(brush.concentration_min, 0.0, 1.0)
        && within(brush.mix_min, 0.0, 1.0)
        && within(brush.dilution_min, 0.0, 1.0)
        && within(brush.hardness, 0.0, 1.0)
        && within(brush.grain, 0.0, 1.0)
        && within(brush.grain_scale, 0.05, 8.0)
        && within(brush.tip_aspect, f32::MIN_POSITIVE, 10.0)
        && within(brush.tip_rotation, -180.0, 180.0)
        && within(brush.scatter, 0.0, 2.0)
        && within(brush.mix.blend, 0.0, 1.0)
        && within(brush.mix.dilution, 0.0, 1.0)
        && within(brush.mix.persistence, 0.0, 1.0)
        && within(brush.mix.charge, 0.0, 1.0)
        && within(brush.mix.sample_radius, 0.0, 64.0)
        && within(brush.wet_edge, 0.0, 1.0)
        && within(brush.wet_edge_width, 1.0, 64.0);
    if !valid {
        return Err("brush parameters are outside the supported ranges".into());
    }
    Ok(())
}

impl Brush {
    pub fn map_pressure(&self, pressure: f32) -> f32 {
        let x = pressure.clamp(0.0, 1.0);
        let (x1, y1, x2, y2) = (
            self.pressure_curve_x1.clamp(0.0, 1.0),
            self.pressure_curve_y1.clamp(0.0, 1.0),
            self.pressure_curve_x2.clamp(0.0, 1.0),
            self.pressure_curve_y2.clamp(0.0, 1.0),
        );
        let mut low = 0.0;
        let mut high = 1.0;
        for _ in 0..12 {
            let t = (low + high) * 0.5;
            let inv = 1.0 - t;
            let bx = 3.0 * inv * inv * t * x1 + 3.0 * inv * t * t * x2 + t * t * t;
            if bx < x {
                low = t;
            } else {
                high = t;
            }
        }
        let t = (low + high) * 0.5;
        let inv = 1.0 - t;
        let by = 3.0 * inv * inv * t * y1 + 3.0 * inv * t * t * y2 + t * t * t;
        by.clamp(0.0, 1.0).powf(self.pressure_curve.clamp(0.2, 3.0))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushKind {
    Pen,
    Pencil,
    Brush,
    Watercolor,
    Airbrush,
    Blur,
    Smudge,
    Eraser,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DynamicSource {
    #[default]
    None,
    Pressure,
    Speed,
    Tilt,
    Direction,
    Random,
}
impl Brush {
    /// Sets the grain image (see [`Brush::grain_source`]) and rebuilds the
    /// grain used for painting.
    pub fn set_grain_source(&mut self, source: Option<BrushTip>) {
        self.grain_source = source;
        self.refresh_grain();
    }

    /// Rebuilds `grain_tip` from `grain_source`, `grain_invert` and
    /// `grain_binary`. Brushes saved before grain sources existed keep
    /// their grain as it is.
    pub fn refresh_grain(&mut self) {
        let Some(source) = &self.grain_source else {
            return;
        };
        let (invert, binary) = (self.grain_invert, self.grain_binary);
        self.grain_tip = Some(BrushTip {
            width: source.width,
            height: source.height,
            coverage: source
                .coverage
                .iter()
                .map(|&value| {
                    let value = if invert { 255 - value } else { value };
                    if binary {
                        if value >= 128 { 255 } else { 0 }
                    } else {
                        value
                    }
                })
                .collect(),
        });
    }
}

pub fn defaults() -> Vec<Brush> {
    use BrushKind::{Airbrush, Blur, Eraser, Pen, Pencil, Smudge, Watercolor};
    let mut brushes: Vec<Brush> = [
        ("ペン", Pen, 4.),
        ("鉛筆", Pencil, 7.),
        ("筆", BrushKind::Brush, 22.),
        ("水彩", Watercolor, 28.),
        ("エアブラシ", Airbrush, 40.),
        ("ぼかし", Blur, 24.),
        ("指先", Smudge, 20.),
        ("消しゴム", Eraser, 24.),
    ]
    .into_iter()
    .map(|(name, kind, size)| Brush {
        name: name.into(),
        kind,
        size,
        opacity: 1.,
        spacing: 0.22,
        stabilization: 2,
        pull_distance: 0.0,
        speed_stabilization: 0.0,
        pressure_curve: 1.0,
        pressure_curve_x1: 0.25,
        pressure_curve_y1: 0.25,
        pressure_curve_x2: 0.75,
        pressure_curve_y2: 0.75,
        taper_start: 0.0,
        taper_end: 0.0,
        taper_in_pixels: false,
        taper_min: 0.15,
        mix: PaintMix::default(),
        speed_size: 0.0,
        speed_opacity: 0.0,
        tilt_size: 0.0,
        tilt_opacity: 0.0,
        tilt_flattening: 0.0,
        tilt_rotation: 0.0,
        size_source: DynamicSource::Pressure,
        opacity_source: DynamicSource::Pressure,
        concentration_source: DynamicSource::None,
        mix_source: DynamicSource::None,
        dilution_source: DynamicSource::None,
        size_min: 0.08,
        opacity_min: 0.0,
        concentration_min: 0.0,
        mix_min: 0.0,
        dilution_min: 0.0,
        hardness: 0.85,
        grain: 0.0,
        grain_scale: 1.0,
        grain_fixed: false,
        grain_tip: None,
        grain_source: None,
        grain_random_rotation: false,
        settle: true,
        wet_edge_on: true,
        antialias: 2,
        grain_invert: false,
        grain_binary: false,
        tip_aspect: 1.0,
        tip_rotation: 0.0,
        scatter: 0.0,
        color: [35, 35, 42, 255],
        tip: None,
        wet_edge: 0.0,
        wet_edge_width: default_wet_edge_width(),
        bleed: BleedStyle::Balanced,
    })
    .collect();
    let watercolor = &mut brushes[3];
    watercolor.mix = PaintMix {
        blend: 0.55,
        dilution: 0.35,
        persistence: 0.6,
        charge: 1.0,
        ..PaintMix::default()
    };
    watercolor.hardness = 0.7;
    watercolor.spacing = 0.12;
    watercolor.size_min = 0.35;
    watercolor.opacity_min = 0.15;
    watercolor.wet_edge = 0.5;
    watercolor.wet_edge_width = 5.0;
    // The eraser erases fully at any pressure; pressure sets its width.
    brushes[7].opacity_source = DynamicSource::None;
    // Blender: carries no paint of its own, only drags what is on the canvas.
    let mut blender = brushes[3].clone();
    blender.name = "色混ぜ".into();
    blender.mix = PaintMix {
        blend: 1.0,
        dilution: 1.0,
        persistence: 0.85,
        charge: 1.0,
        ..PaintMix::default()
    };
    blender.wet_edge = 0.0;
    blender.bleed = BleedStyle::Airy;
    blender.size = 30.0;
    brushes.push(blender);
    // Ink pen for line art: pressure changes only the width; every part of
    // the line has the same full density.
    let mut ink = brushes[0].clone();
    ink.name = "ペン画".into();
    ink.opacity_source = DynamicSource::None;
    ink.opacity_min = 1.0;
    ink.size_min = 0.25;
    ink.hardness = 0.95;
    ink.stabilization = 4;
    ink.size = 6.0;
    ink.taper_start = 0.08;
    ink.taper_end = 0.12;
    brushes.push(ink);
    brushes
}
pub fn save_bundle(
    path: &std::path::Path,
    brush: &Brush,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    validate_brush(brush)?;
    if let Some(tip) = &brush.tip {
        validate_texture(tip)?;
    }
    if let Some(grain) = &brush.grain_tip {
        validate_texture(grain)?;
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let f = temp.as_file().try_clone()?;
    let mut z = zip::ZipWriter::new(f);
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    z.start_file("brush.json", opt)?;
    let mut metadata = brush.clone();
    metadata.tip = None;
    metadata.grain_tip = None;
    metadata.grain_source = None;
    z.write_all(
        serde_json::to_vec_pretty(&serde_json::json!({"version":1,"brush":metadata}))?.as_slice(),
    )?;
    if let Some(tip) = &brush.tip {
        z.start_file("tip.png", opt)?;
        z.write_all(&encode_tip(tip)?)?;
    }
    if let Some(grain) = &brush.grain_tip {
        z.start_file("grain.png", opt)?;
        z.write_all(&encode_tip(grain)?)?;
    }
    if let Some(source) = &brush.grain_source {
        z.start_file("grain_source.png", opt)?;
        z.write_all(&encode_tip(source)?)?;
    }
    let f = z.finish()?;
    f.sync_all()?;
    drop(f);
    temp.persist(path)?;
    Ok(())
}
pub fn load_bundle(path: &std::path::Path) -> Result<Brush, Box<dyn std::error::Error>> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    if z.len() > 16 {
        return Err("brush bundle contains too many entries".into());
    }
    let mut entry = z.by_name("brush.json")?;
    if entry.size() > 1024 * 1024 {
        return Err("brush metadata exceeds the size limit".into());
    }
    let mut s = String::new();
    (&mut entry).take(1024 * 1024 + 1).read_to_string(&mut s)?;
    if s.len() > 1024 * 1024 {
        return Err("brush metadata exceeds the size limit".into());
    }
    drop(entry);
    let bundle: serde_json::Value = serde_json::from_str(&s)?;
    if bundle.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("unsupported brush bundle version".into());
    }
    let mut brush: Brush = serde_json::from_value(
        bundle
            .get("brush")
            .cloned()
            .ok_or("missing brush metadata")?,
    )?;
    validate_brush(&brush)?;
    if let Ok(mut entry) = z.by_name("tip.png") {
        if entry.size() > 32 * 1024 * 1024 {
            return Err("brush tip image exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry)
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("brush tip image exceeds the size limit".into());
        }
        brush.tip = Some(decode_tip(&bytes)?);
    }
    if let Ok(mut entry) = z.by_name("grain.png") {
        if entry.size() > 32 * 1024 * 1024 {
            return Err("brush grain image exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry)
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("brush grain image exceeds the size limit".into());
        }
        brush.grain_tip = Some(decode_tip(&bytes)?);
    }
    if let Ok(mut entry) = z.by_name("grain_source.png") {
        if entry.size() > 32 * 1024 * 1024 {
            return Err("brush grain image exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        (&mut entry)
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("brush grain image exceeds the size limit".into());
        }
        brush.grain_source = Some(decode_tip(&bytes)?);
        brush.refresh_grain();
    }
    Ok(brush)
}

/// Largest brush set file entry read (texture or metadata).
const SET_ENTRY_LIMIT: u64 = 32 * 1024 * 1024;
/// Most brushes in one set.
const SET_BRUSH_LIMIT: usize = 512;

/// Saves a list of brushes (a preset set) as one ZIP file: `set.json`
/// holds the settings, `N/tip.png`, `N/grain.png` and `N/grain_source.png`
/// the textures of brush N.
pub fn save_set(
    path: &std::path::Path,
    brushes: &[Brush],
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut temp.as_file(), &set_bytes(brushes)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}

/// The bytes of a brush set file (see [`save_set`]).
pub fn set_bytes(brushes: &[Brush]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Write;
    if brushes.is_empty() || brushes.len() > SET_BRUSH_LIMIT {
        return Err("a brush set holds 1 to 512 brushes".into());
    }
    let mut bytes = Vec::new();
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut metadata = Vec::with_capacity(brushes.len());
    for (index, brush) in brushes.iter().enumerate() {
        validate_brush(brush)?;
        let textures = [
            ("tip.png", &brush.tip),
            ("grain.png", &brush.grain_tip),
            ("grain_source.png", &brush.grain_source),
        ];
        for (name, texture) in textures {
            if let Some(texture) = texture {
                z.start_file(format!("{index}/{name}"), opt)?;
                z.write_all(&encode_tip(texture)?)?;
            }
        }
        let mut plain = brush.clone();
        plain.tip = None;
        plain.grain_tip = None;
        plain.grain_source = None;
        metadata.push(plain);
    }
    z.start_file("set.json", opt)?;
    z.write_all(
        serde_json::to_vec_pretty(&serde_json::json!({"version": 1, "brushes": metadata}))?
            .as_slice(),
    )?;
    z.finish()?;
    Ok(bytes)
}

/// Reads a brush set file (see [`save_set`]).
pub fn load_set(path: &std::path::Path) -> Result<Vec<Brush>, Box<dyn std::error::Error>> {
    let size = std::fs::metadata(path)?.len();
    if size > 256 * 1024 * 1024 {
        return Err("brush set exceeds the size limit".into());
    }
    set_from_bytes(&std::fs::read(path)?)
}

/// Reads the bytes of a brush set file (see [`save_set`]).
pub fn set_from_bytes(bytes: &[u8]) -> Result<Vec<Brush>, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    if z.len() > 1 + SET_BRUSH_LIMIT * 3 {
        return Err("brush set contains too many entries".into());
    }
    let mut read_entry = |name: &str| -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
        let Ok(entry) = z.by_name(name) else {
            return Ok(None);
        };
        if entry.size() > SET_ENTRY_LIMIT {
            return Err("brush set entry exceeds the size limit".into());
        }
        let mut data = Vec::new();
        entry.take(SET_ENTRY_LIMIT + 1).read_to_end(&mut data)?;
        if data.len() as u64 > SET_ENTRY_LIMIT {
            return Err("brush set entry exceeds the size limit".into());
        }
        Ok(Some(data))
    };
    let json = read_entry("set.json")?.ok_or("missing set.json")?;
    let set: serde_json::Value = serde_json::from_slice(&json)?;
    if set.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("unsupported brush set version".into());
    }
    let mut brushes: Vec<Brush> =
        serde_json::from_value(set.get("brushes").cloned().ok_or("missing brushes")?)?;
    if brushes.is_empty() || brushes.len() > SET_BRUSH_LIMIT {
        return Err("a brush set holds 1 to 512 brushes".into());
    }
    for (index, brush) in brushes.iter_mut().enumerate() {
        validate_brush(brush)?;
        if let Some(data) = read_entry(&format!("{index}/tip.png"))? {
            brush.tip = Some(decode_tip(&data)?);
        }
        if let Some(data) = read_entry(&format!("{index}/grain.png"))? {
            brush.grain_tip = Some(decode_tip(&data)?);
        }
        if let Some(data) = read_entry(&format!("{index}/grain_source.png"))? {
            brush.grain_source = Some(decode_tip(&data)?);
            brush.refresh_grain();
        }
    }
    Ok(brushes)
}

fn encode_tip(tip: &BrushTip) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    validate_texture(tip)?;
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut bytes), tip.width, tip.height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&tip.coverage)?;
    Ok(bytes)
}

fn validate_texture(tip: &BrushTip) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = tip.width as usize * tip.height as usize;
    if tip.width == 0
        || tip.height == 0
        || tip.width > MAX_BRUSH_TEXTURE_DIMENSION
        || tip.height > MAX_BRUSH_TEXTURE_DIMENSION
        || pixels > MAX_BRUSH_TEXTURE_PIXELS
        || pixels != tip.coverage.len()
    {
        return Err("brush texture dimensions or coverage data are invalid".into());
    }
    Ok(())
}

fn decode_tip(bytes: &[u8]) -> Result<BrushTip, Box<dyn std::error::Error>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info()?;
    let info = reader.info();
    if info.color_type != png::ColorType::Grayscale {
        return Err("brush texture must be grayscale".into());
    }
    if info.width == 0
        || info.height == 0
        || info.width > MAX_BRUSH_TEXTURE_DIMENSION
        || info.height > MAX_BRUSH_TEXTURE_DIMENSION
        || info.width as usize * info.height as usize > MAX_BRUSH_TEXTURE_PIXELS
    {
        return Err("brush texture dimensions exceed the size limit".into());
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or("brush texture output size is invalid")?;
    let mut frame = vec![0; output_size];
    let info = reader.next_frame(&mut frame)?;
    if info.width == 0
        || info.height == 0
        || info.width as usize * info.height as usize != info.buffer_size()
    {
        return Err("invalid brush texture dimensions".into());
    }
    let coverage = frame[..info.buffer_size()].to_vec();
    Ok(BrushTip {
        width: info.width,
        height: info.height,
        coverage,
    })
}

#[cfg(test)]
mod grain_tests {
    use super::*;

    #[test]
    fn grain_invert_and_two_levels() {
        let mut brush = defaults().remove(1);
        brush.set_grain_source(Some(BrushTip {
            width: 4,
            height: 1,
            coverage: vec![0, 100, 200, 255],
        }));
        assert_eq!(
            brush.grain_tip.as_ref().unwrap().coverage,
            vec![0, 100, 200, 255]
        );
        brush.grain_invert = true;
        brush.refresh_grain();
        assert_eq!(
            brush.grain_tip.as_ref().unwrap().coverage,
            vec![255, 155, 55, 0]
        );
        brush.grain_binary = true;
        brush.refresh_grain();
        assert_eq!(
            brush.grain_tip.as_ref().unwrap().coverage,
            vec![255, 255, 0, 0]
        );
        brush.grain_invert = false;
        brush.refresh_grain();
        assert_eq!(
            brush.grain_tip.as_ref().unwrap().coverage,
            vec![0, 0, 255, 255]
        );
        // The source and the switches survive a bundle round trip.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grain.efudebrush");
        save_bundle(&path, &brush).unwrap();
        let loaded = load_bundle(&path).unwrap();
        assert_eq!(
            loaded.grain_source.unwrap().coverage,
            vec![0, 100, 200, 255]
        );
        assert_eq!(loaded.grain_tip.unwrap().coverage, vec![0, 0, 255, 255]);
        assert!(loaded.grain_binary && !loaded.grain_invert);
    }

    #[test]
    fn brush_sets_keep_every_brush_and_texture() {
        let mut brushes = defaults();
        brushes[1].set_grain_source(Some(BrushTip {
            width: 2,
            height: 2,
            coverage: vec![0, 255, 255, 0],
        }));
        brushes[2].grain_invert = true;
        brushes[2].set_grain_source(Some(BrushTip {
            width: 1,
            height: 2,
            coverage: vec![10, 200],
        }));
        let loaded = set_from_bytes(&set_bytes(&brushes).unwrap()).unwrap();
        assert_eq!(loaded.len(), brushes.len());
        for (a, b) in brushes.iter().zip(&loaded) {
            assert_eq!(a.name, b.name);
            assert_eq!(
                a.grain_tip.as_ref().map(|t| t.coverage.clone()),
                b.grain_tip.as_ref().map(|t| t.coverage.clone())
            );
            assert_eq!(
                serde_json::to_value(a).unwrap(),
                serde_json::to_value(b).unwrap()
            );
        }
        assert!(set_from_bytes(b"not a zip").is_err());
    }

    /// Writes `assets/brushes/default.efudebrushes`, the preset set a new
    /// installation starts with: three columns of the built-in brushes.
    /// Run with `cargo test -p efude-brush -- --ignored write_default_preset_set`
    /// only to start over; normally the file is exported from the app.
    #[test]
    #[ignore]
    fn write_default_preset_set() {
        let one = defaults();
        let three: Vec<Brush> = one.iter().chain(&one).chain(&one).cloned().collect();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/brushes/default.efudebrushes");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        save_set(&path, &three).unwrap();
    }
}
