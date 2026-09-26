// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Effect lines: focus lines toward a centre and parallel speed lines.

use crate::raster::{Coverage, wedge_into};
use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Small deterministic random source (xorshift).
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    /// Uniform in [0, 1).
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Lines converging on an ellipse, drawn inside a rectangular region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusLines {
    /// Region the lines fill, [min, max] in canvas pixels.
    pub region: [Vec2; 2],
    /// Centre of the empty ellipse the lines point at.
    pub center: Vec2,
    /// Radii of that ellipse.
    pub radii: Vec2,
    pub count: u32,
    /// Randomness of the spacing between lines (0..1).
    pub spacing_jitter: f32,
    /// How much further some lines reach toward the centre, as a fraction of
    /// the distance to the ellipse (0 = all stop on the ellipse).
    pub length_jitter: f32,
    /// Width at the outer end, in pixels.
    pub width: f32,
    /// 1 = lines taper to a point at the inner end, 0 = constant width.
    pub taper: f32,
    /// Lines per bundle (1 = evenly spread).
    pub bundle: u32,
    pub seed: u64,
}

impl FocusLines {
    /// Defaults for a region.
    pub fn new(region: [Vec2; 2]) -> Self {
        let size = region[1] - region[0];
        Self {
            region,
            center: (region[0] + region[1]) * 0.5,
            radii: size * 0.22,
            count: 160,
            spacing_jitter: 0.6,
            length_jitter: 0.35,
            width: (size.x.min(size.y) * 0.012).max(2.0),
            taper: 1.0,
            bundle: 1,
            seed: 1,
        }
    }

    pub fn render(&self, canvas_width: u32, canvas_height: u32) -> Option<Coverage> {
        let mut coverage = Coverage::for_points(self.region, 0.0, canvas_width, canvas_height)?;
        let mut random = Random::new(self.seed);
        let count = self.count.clamp(1, 4000);
        let bundle = self.bundle.clamp(1, count);
        let step = std::f32::consts::TAU / count as f32;
        // Far enough to start outside every corner of the region.
        let reach = (self.region[0] - self.center)
            .length()
            .max((self.region[1] - self.center).length())
            .max((Vec2::new(self.region[0].x, self.region[1].y) - self.center).length())
            .max((Vec2::new(self.region[1].x, self.region[0].y) - self.center).length())
            + self.width;
        for i in 0..count {
            // Bundles: lines of a bundle sit close together, with the
            // spare space gathered into the gaps between bundles.
            let base = if bundle > 1 {
                let group = i / bundle;
                let within = i % bundle;
                let group_step = step * bundle as f32;
                group as f32 * group_step + within as f32 * step * 0.35
            } else {
                i as f32 * step
            };
            let angle = base + (random.next() - 0.5) * step * self.spacing_jitter.clamp(0.0, 1.0);
            let direction = Vec2::new(angle.cos(), angle.sin());
            // Where the ray meets the ellipse.
            let ellipse = 1.0
                / ((direction.x / self.radii.x.max(1.0)).powi(2)
                    + (direction.y / self.radii.y.max(1.0)).powi(2))
                .sqrt();
            let inner = ellipse * (1.0 - self.length_jitter.clamp(0.0, 1.0) * random.next());
            let width = self.width * (0.6 + 0.4 * random.next());
            wedge_into(
                &mut coverage,
                self.center + direction * reach,
                self.center + direction * inner,
                width,
                width * (1.0 - self.taper.clamp(0.0, 1.0)),
            );
        }
        Some(coverage)
    }
}

/// Parallel lines across a rectangular region.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedLines {
    pub region: [Vec2; 2],
    /// Direction of motion in degrees (0 = horizontal).
    pub angle_degrees: f32,
    pub count: u32,
    pub spacing_jitter: f32,
    /// Line length as a fraction of the region's extent along the lines.
    pub length: f32,
    pub length_jitter: f32,
    pub width: f32,
    /// 1 = pointed at both ends.
    pub taper: f32,
    pub seed: u64,
}

impl SpeedLines {
    pub fn new(region: [Vec2; 2]) -> Self {
        let size = region[1] - region[0];
        Self {
            region,
            angle_degrees: 0.0,
            count: 60,
            spacing_jitter: 0.7,
            length: 0.7,
            length_jitter: 0.5,
            width: (size.x.min(size.y) * 0.01).max(2.0),
            taper: 1.0,
            seed: 1,
        }
    }

    pub fn render(&self, canvas_width: u32, canvas_height: u32) -> Option<Coverage> {
        let mut coverage = Coverage::for_points(self.region, 0.0, canvas_width, canvas_height)?;
        let mut random = Random::new(self.seed);
        let angle = self.angle_degrees.to_radians();
        let along = Vec2::new(angle.cos(), angle.sin());
        let across = Vec2::new(-along.y, along.x);
        let center = (self.region[0] + self.region[1]) * 0.5;
        let corners = [
            self.region[0],
            Vec2::new(self.region[1].x, self.region[0].y),
            self.region[1],
            Vec2::new(self.region[0].x, self.region[1].y),
        ];
        let extent = |axis: Vec2| {
            corners
                .iter()
                .map(|c| (*c - center).dot(axis).abs())
                .fold(0.0f32, f32::max)
        };
        let (half_along, half_across) = (extent(along), extent(across));
        let count = self.count.clamp(1, 4000);
        let spacing = 2.0 * half_across / count as f32;
        for i in 0..count {
            let offset = -half_across
                + (i as f32 + 0.5 + (random.next() - 0.5) * self.spacing_jitter.clamp(0.0, 1.0))
                    * spacing;
            let length = 2.0
                * half_along
                * self.length.clamp(0.05, 1.0)
                * (1.0 - self.length_jitter.clamp(0.0, 0.95) * random.next());
            let middle = center
                + across * offset
                + along * (random.next() - 0.5) * (2.0 * half_along - length).max(0.0);
            let width = self.width * (0.6 + 0.4 * random.next());
            let end_width = width * (1.0 - self.taper.clamp(0.0, 1.0));
            wedge_into(
                &mut coverage,
                middle,
                middle + along * length * 0.5,
                width,
                end_width,
            );
            wedge_into(
                &mut coverage,
                middle,
                middle - along * length * 0.5,
                width,
                end_width,
            );
        }
        Some(coverage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_lines_leave_the_centre_empty() {
        let mut lines = FocusLines::new([Vec2::ZERO, Vec2::new(400.0, 300.0)]);
        lines.length_jitter = 0.0;
        let c = lines.render(400, 300).unwrap();
        assert_eq!(c.at(200, 150), 0, "centre must stay clear");
        let edge: u32 = (0..400).map(|x| c.at(x, 2) as u32).sum();
        assert!(edge > 255 * 20, "too few lines at the edge: {edge}");
        // Same seed, same lines; another seed, other lines.
        assert_eq!(lines.render(400, 300).unwrap(), c);
        lines.seed = 2;
        assert_ne!(lines.render(400, 300).unwrap(), c);
    }

    #[test]
    fn speed_lines_run_along_the_angle() {
        let mut lines = SpeedLines::new([Vec2::ZERO, Vec2::new(300.0, 300.0)]);
        lines.angle_degrees = 0.0;
        let c = lines.render(300, 300).unwrap();
        // Horizontal lines: rows are either mostly empty or mostly inked
        // along their length; columns cross many lines.
        let column: u32 = (0..300).map(|y| (c.at(150, y) > 128) as u32).sum();
        assert!(column > 10, "{column}");
        let ink: u32 = c.data.iter().map(|&v| v as u32).sum();
        assert!(ink > 0);
    }
}
