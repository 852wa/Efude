// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Anti-aliased coverage of polygons and borders.

use glam::Vec2;

/// 8-bit coverage of a rectangular region of the canvas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Coverage {
    /// Canvas position of the region's top-left pixel.
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height` values (0 = none, 255 = full).
    pub data: Vec<u8>,
}

impl Coverage {
    /// An empty region covering the bounding box of `points`, clamped to a
    /// `canvas_width`×`canvas_height` canvas, grown by `pad` pixels.
    pub fn for_points(
        points: impl IntoIterator<Item = Vec2>,
        pad: f32,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Option<Self> {
        let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in points {
            min = min.min(p);
            max = max.max(p);
        }
        if min.x > max.x || canvas_width == 0 || canvas_height == 0 {
            return None;
        }
        let x0 = (min.x - pad).floor().clamp(0.0, canvas_width as f32) as u32;
        let y0 = (min.y - pad).floor().clamp(0.0, canvas_height as f32) as u32;
        let x1 = (max.x + pad).ceil().clamp(0.0, canvas_width as f32) as u32;
        let y1 = (max.y + pad).ceil().clamp(0.0, canvas_height as f32) as u32;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (width, height) = (x1 - x0, y1 - y0);
        Some(Self {
            x0,
            y0,
            width,
            height,
            data: vec![0; (width * height) as usize],
        })
    }

    /// Coverage at a canvas position (0 outside the region).
    pub fn at(&self, x: u32, y: u32) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.width || y >= self.y0 + self.height {
            return 0;
        }
        self.data[((y - self.y0) * self.width + (x - self.x0)) as usize]
    }

    /// Canvas pixels with non-zero coverage.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32, u8)> + '_ {
        self.data
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(move |(i, &c)| {
                let i = i as u32;
                (self.x0 + i % self.width, self.y0 + i / self.width, c)
            })
    }

    /// Adds `other` (maximum of both) where the regions overlap.
    pub fn merge_max(&mut self, other: &Coverage) {
        for (x, y, c) in other.iter() {
            if x >= self.x0 && y >= self.y0 && x < self.x0 + self.width && y < self.y0 + self.height
            {
                let i = ((y - self.y0) * self.width + (x - self.x0)) as usize;
                self.data[i] = self.data[i].max(c);
            }
        }
    }
}

/// Sub-sample offsets (4×4 grid) used for anti-aliasing.
const SUBSAMPLES: usize = 4;

/// Fraction of each pixel inside `polygon` (any simple polygon, even-odd),
/// by scanline: four sub-rows per pixel row, four sub-columns per pixel.
pub fn fill_polygon(polygon: &[Vec2], canvas_width: u32, canvas_height: u32) -> Option<Coverage> {
    if polygon.len() < 3 {
        return None;
    }
    let mut coverage =
        Coverage::for_points(polygon.iter().copied(), 1.0, canvas_width, canvas_height)?;
    let step = 1.0 / SUBSAMPLES as f32;
    let mut counts = vec![0u16; coverage.width as usize];
    let mut crossings = Vec::new();
    for row in 0..coverage.height {
        counts.fill(0);
        let py = (coverage.y0 + row) as f32;
        for sy in 0..SUBSAMPLES {
            let y = py + (sy as f32 + 0.5) * step;
            crossings.clear();
            let mut j = polygon.len() - 1;
            for i in 0..polygon.len() {
                let (a, b) = (polygon[i], polygon[j]);
                if (a.y > y) != (b.y > y) {
                    crossings.push(a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y));
                }
                j = i;
            }
            crossings.sort_by(f32::total_cmp);
            for span in crossings.chunks_exact(2) {
                // Sub-columns whose centre lies in [span[0], span[1]).
                let first = ((span[0] - coverage.x0 as f32) * SUBSAMPLES as f32 - 0.5).ceil();
                let last = ((span[1] - coverage.x0 as f32) * SUBSAMPLES as f32 - 0.5).ceil();
                let limit = (coverage.width as usize * SUBSAMPLES) as f32;
                let (first, last) = (
                    first.clamp(0.0, limit) as usize,
                    last.clamp(0.0, limit) as usize,
                );
                for sub in first..last {
                    counts[sub / SUBSAMPLES] += 1;
                }
            }
        }
        let base = (row * coverage.width) as usize;
        for (column, &count) in counts.iter().enumerate() {
            coverage.data[base + column] =
                (count as u32 * 255 / (SUBSAMPLES * SUBSAMPLES) as u32) as u8;
        }
    }
    Some(coverage)
}

/// `polygon` shrunk by `distance` (convex polygons only), or `None` if it
/// vanishes.
pub fn inset_convex(polygon: &[Vec2], distance: f32) -> Option<Vec<Vec2>> {
    let n = polygon.len();
    if n < 3 {
        return None;
    }
    let area: f32 = (0..n)
        .map(|i| polygon[i].perp_dot(polygon[(i + 1) % n]))
        .sum::<f32>()
        * 0.5;
    if area.abs() < 1e-3 {
        return None;
    }
    // Inward normal of each edge, and the edge moved inward.
    let orientation = area.signum();
    let lines: Vec<(Vec2, Vec2)> = (0..n)
        .map(|i| {
            let (a, b) = (polygon[i], polygon[(i + 1) % n]);
            let d = (b - a).normalize_or_zero();
            let inward = Vec2::new(-d.y, d.x) * orientation;
            (a + inward * distance, d)
        })
        .collect();
    let mut result = Vec::with_capacity(n);
    for i in 0..n {
        let (p1, d1) = lines[(i + n - 1) % n];
        let (p2, d2) = lines[i];
        let denominator = d1.perp_dot(d2);
        if denominator.abs() < 1e-6 {
            result.push(p2);
            continue;
        }
        let t = (p2 - p1).perp_dot(d2) / denominator;
        result.push(p1 + d1 * t);
    }
    // Collapsed if the orientation flipped.
    let inset_area: f32 = (0..n)
        .map(|i| result[i].perp_dot(result[(i + 1) % n]))
        .sum::<f32>()
        * 0.5;
    (inset_area * area > 0.0 && inset_area.abs() < area.abs()).then_some(result)
}

/// A border of `width` pixels drawn inside the edge of a convex `polygon`.
pub fn inner_border(
    polygon: &[Vec2],
    width: f32,
    canvas_width: u32,
    canvas_height: u32,
) -> Option<Coverage> {
    if width <= 0.0 {
        return None;
    }
    let mut outer = fill_polygon(polygon, canvas_width, canvas_height)?;
    if let Some(inner) = inset_convex(polygon, width)
        .and_then(|inset| fill_polygon(&inset, canvas_width, canvas_height))
    {
        for (x, y, c) in inner.iter() {
            let i = ((y - outer.y0) * outer.width + (x - outer.x0)) as usize;
            outer.data[i] = outer.data[i].saturating_sub(c);
        }
    }
    Some(outer)
}

/// A tapered line: a quadrilateral from `a` (width `width_a`) to `b`
/// (width `width_b`), anti-aliased, added into `coverage` (maximum).
pub fn wedge_into(coverage: &mut Coverage, a: Vec2, b: Vec2, width_a: f32, width_b: f32) {
    let direction = (b - a).normalize_or_zero();
    if direction == Vec2::ZERO {
        return;
    }
    let normal = Vec2::new(-direction.y, direction.x);
    let quad = [
        a + normal * width_a * 0.5,
        b + normal * width_b * 0.5,
        b - normal * width_b * 0.5,
        a - normal * width_a * 0.5,
    ];
    // Thin lines: keep a minimum footprint and fade instead of vanishing.
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in quad {
        min = min.min(p);
        max = max.max(p);
    }
    let x0 = (min.x - 1.0).floor().max(coverage.x0 as f32) as u32;
    let y0 = (min.y - 1.0).floor().max(coverage.y0 as f32) as u32;
    let x1 = ((max.x + 1.0).ceil() as u32).min(coverage.x0 + coverage.width);
    let y1 = ((max.y + 1.0).ceil() as u32).min(coverage.y0 + coverage.height);
    let length = a.distance(b);
    let step = 1.0 / SUBSAMPLES as f32;
    for y in y0..y1 {
        for x in x0..x1 {
            let mut hits = 0.0f32;
            for sy in 0..SUBSAMPLES {
                for sx in 0..SUBSAMPLES {
                    let p = Vec2::new(
                        x as f32 + (sx as f32 + 0.5) * step,
                        y as f32 + (sy as f32 + 0.5) * step,
                    );
                    let along = (p - a).dot(direction);
                    if !(0.0..=length).contains(&along) {
                        continue;
                    }
                    let t = along / length.max(1e-6);
                    let half = (width_a + (width_b - width_a) * t) * 0.5;
                    let across = (p - a).dot(normal).abs();
                    // Sub-pixel widths darken proportionally.
                    if half >= 0.5 {
                        if across <= half {
                            hits += 1.0;
                        }
                    } else if across <= 0.5 {
                        hits += half * 2.0;
                    }
                }
            }
            let value = (hits * 255.0 / (SUBSAMPLES * SUBSAMPLES) as f32).round() as u8;
            let i = ((y - coverage.y0) * coverage.width + (x - coverage.x0)) as usize;
            coverage.data[i] = coverage.data[i].max(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_fill_is_exact_and_antialiased() {
        let square = [
            Vec2::new(10.0, 10.0),
            Vec2::new(20.0, 10.0),
            Vec2::new(20.0, 20.0),
            Vec2::new(10.0, 20.0),
        ];
        let c = fill_polygon(&square, 40, 40).unwrap();
        assert_eq!(c.at(15, 15), 255);
        assert_eq!(c.at(9, 15), 0);
        assert_eq!(c.at(20, 15), 0);
        let half = [
            Vec2::new(10.0, 10.5),
            Vec2::new(20.0, 10.5),
            Vec2::new(20.0, 20.0),
            Vec2::new(10.0, 20.0),
        ];
        let c = fill_polygon(&half, 40, 40).unwrap();
        assert!((110..=145).contains(&c.at(15, 10)), "{}", c.at(15, 10));
    }

    #[test]
    fn inner_border_stays_inside() {
        let square = [
            Vec2::new(10.0, 10.0),
            Vec2::new(30.0, 10.0),
            Vec2::new(30.0, 30.0),
            Vec2::new(10.0, 30.0),
        ];
        let c = inner_border(&square, 3.0, 40, 40).unwrap();
        assert_eq!(c.at(11, 20), 255);
        assert_eq!(c.at(9, 20), 0, "outside the panel");
        assert_eq!(c.at(20, 20), 0, "centre of the panel");
    }

    #[test]
    fn wedge_tapers() {
        let mut c =
            Coverage::for_points([Vec2::ZERO, Vec2::new(100.0, 20.0)], 0.0, 100, 20).unwrap();
        wedge_into(
            &mut c,
            Vec2::new(0.0, 10.0),
            Vec2::new(100.0, 10.0),
            8.0,
            0.0,
        );
        let thick: u32 = (0..20).map(|y| c.at(5, y) as u32).sum();
        let thin: u32 = (0..20).map(|y| c.at(95, y) as u32).sum();
        assert!(thick > thin * 4, "{thick} {thin}");
    }
}
