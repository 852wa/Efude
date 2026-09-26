// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Screentone: a layer whose pixels give a density that is shown as a
//! halftone pattern (see `docs/spec/comic.md`).

use serde::{Deserialize, Serialize};

/// Shape of the halftone dots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DotShape {
    #[default]
    Round,
    Square,
    Diamond,
    Line,
    Cross,
    Noise,
}

impl DotShape {
    pub const ALL: [DotShape; 6] = [
        DotShape::Round,
        DotShape::Square,
        DotShape::Diamond,
        DotShape::Line,
        DotShape::Cross,
        DotShape::Noise,
    ];
}

/// Halftone parameters of a tone layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneSettings {
    /// Screen frequency in lines per inch (dots per inch of the pattern).
    pub lines_per_inch: f32,
    pub angle_degrees: f32,
    pub shape: DotShape,
    /// Ink colour of the dots.
    pub color: [u8; 3],
    /// Smooth dot edges (off gives pure two-level output for print).
    #[serde(default = "default_true")]
    pub antialias: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ToneSettings {
    fn default() -> Self {
        Self {
            lines_per_inch: 60.0,
            angle_degrees: 45.0,
            shape: DotShape::Round,
            color: [0, 0, 0],
            antialias: true,
        }
    }
}

/// Density a pixel stands for: darkness × alpha (0..1).
pub fn density(pixel: [u8; 4]) -> f32 {
    let luma =
        (0.299 * pixel[0] as f32 + 0.587 * pixel[1] as f32 + 0.114 * pixel[2] as f32) / 255.0;
    (1.0 - luma) * pixel[3] as f32 / 255.0
}

fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343) ^ (y as u32).wrapping_mul(0xD816_3841);
    h ^= h >> 13;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    (h & 0xFFFF) as f32 / 65536.0
}

/// Whether the point (in pattern cells, centred on a cell) is inked at
/// `density`. Dot area grows with density; past half, the paper shows as
/// holes that shrink.
fn inked(shape: DotShape, u: f32, v: f32, density: f32) -> bool {
    let (fu, fv) = (u - u.round(), v - v.round());
    // Distance to the nearest cell corner, for the "holes" side.
    let (cu, cv) = (0.5 - fu.abs(), 0.5 - fv.abs());
    match shape {
        DotShape::Round => {
            if density <= 0.5 {
                fu * fu + fv * fv <= density / std::f32::consts::PI
            } else {
                cu * cu + cv * cv > (1.0 - density) / std::f32::consts::PI
            }
        }
        DotShape::Square => {
            let half = density.sqrt() * 0.5;
            fu.abs() <= half && fv.abs() <= half
        }
        DotShape::Diamond => {
            if density <= 0.5 {
                fu.abs() + fv.abs() <= (density / 2.0).sqrt()
            } else {
                cu + cv > ((1.0 - density) / 2.0).sqrt()
            }
        }
        DotShape::Line => fv.abs() <= density * 0.5,
        DotShape::Cross => {
            let half = (1.0 - (1.0 - density).sqrt()) * 0.5;
            fu.abs() <= half || fv.abs() <= half
        }
        DotShape::Noise => unreachable!(),
    }
}

/// Coverage (0..1) of the tone at document pixel (x, y) for `density`.
pub fn coverage(settings: &ToneSettings, dpi: f32, x: u32, y: u32, density: f32) -> f32 {
    if density <= 0.0 {
        return 0.0;
    }
    if density >= 1.0 {
        return 1.0;
    }
    let cell = (dpi / settings.lines_per_inch.clamp(5.0, 300.0)).max(1.0);
    if settings.shape == DotShape::Noise {
        // Grain a few pixels wide, scaled with the screen frequency.
        let grain = (cell / 5.0).max(1.0);
        let gx = (x as f32 / grain).floor() as i32;
        let gy = (y as f32 / grain).floor() as i32;
        return (hash(gx, gy) < density) as u8 as f32;
    }
    let (sin, cos) = settings.angle_degrees.to_radians().sin_cos();
    let samples: &[(f32, f32)] = if settings.antialias {
        &[(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
    } else {
        &[(0.5, 0.5)]
    };
    let mut hits = 0;
    for &(ox, oy) in samples {
        let (px, py) = (x as f32 + ox, y as f32 + oy);
        let u = (px * cos + py * sin) / cell;
        let v = (-px * sin + py * cos) / cell;
        hits += inked(settings.shape, u, v, density) as u32;
    }
    hits as f32 / samples.len() as f32
}

/// The displayed colour of a tone-layer pixel at document (x, y).
pub fn tone_pixel(settings: &ToneSettings, dpi: f32, x: u32, y: u32, pixel: [u8; 4]) -> [u8; 4] {
    let alpha = coverage(settings, dpi, x, y, density(pixel));
    let [r, g, b] = settings.color;
    if alpha <= 0.0 {
        [0; 4]
    } else {
        [r, g, b, (alpha * 255.0).round() as u8]
    }
}

/// A whole storage tile of a tone layer converted for display. `origin` is
/// the document position of the tile's top-left pixel; `stride` is the tile
/// side in pixels.
pub fn tone_tile(
    settings: &ToneSettings,
    dpi: f32,
    origin: (u32, u32),
    stride: u32,
    tile: &[u8],
) -> Vec<u8> {
    let mut out = vec![0u8; tile.len()];
    for (index, (source, target)) in tile
        .chunks_exact(4)
        .zip(out.chunks_exact_mut(4))
        .enumerate()
    {
        if source[3] == 0 {
            continue;
        }
        let local = index as u32;
        let (x, y) = (origin.0 + local % stride, origin.1 + local / stride);
        target.copy_from_slice(&tone_pixel(
            settings,
            dpi,
            x,
            y,
            [source[0], source[1], source[2], source[3]],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mean_coverage(settings: &ToneSettings, density: f32) -> f32 {
        let mut total = 0.0;
        for y in 0..200 {
            for x in 0..200 {
                total += coverage(settings, 600.0, x, y, density);
            }
        }
        total / 40_000.0
    }

    #[test]
    fn every_shape_prints_its_density() {
        for shape in DotShape::ALL {
            let settings = ToneSettings {
                shape,
                ..ToneSettings::default()
            };
            for density in [0.1, 0.3, 0.5, 0.7, 0.9] {
                let printed = mean_coverage(&settings, density);
                assert!(
                    (printed - density).abs() < 0.06,
                    "{shape:?} at {density}: {printed}"
                );
            }
        }
    }

    #[test]
    fn pattern_has_the_screen_frequency() {
        // 60 lpi at 600 dpi: one dot every 10 px along the screen axis.
        let settings = ToneSettings {
            angle_degrees: 0.0,
            antialias: false,
            ..ToneSettings::default()
        };
        let row: Vec<bool> = (0..100)
            .map(|x| coverage(&settings, 600.0, x, 0, 0.2) > 0.5)
            .collect();
        let starts = row.windows(2).filter(|w| !w[0] && w[1]).count();
        assert!((9..=11).contains(&starts), "{starts}");
    }

    #[test]
    fn tone_pixels_use_the_tone_colour() {
        let settings = ToneSettings {
            color: [10, 20, 200],
            ..ToneSettings::default()
        };
        assert_eq!(
            tone_pixel(&settings, 600.0, 3, 3, [0, 0, 0, 255]),
            [10, 20, 200, 255]
        );
        assert_eq!(
            tone_pixel(&settings, 600.0, 3, 3, [255, 255, 255, 255]),
            [0; 4]
        );
        assert_eq!(tone_pixel(&settings, 600.0, 3, 3, [0, 0, 0, 0]), [0; 4]);
    }
}
