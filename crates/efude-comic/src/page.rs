// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Page setup: finished size, bleed, inner frame and crop marks.

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Which edge the pages are bound on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    /// Bound on the right; pages read right to left.
    #[default]
    Right,
    /// Bound on the left; pages read left to right.
    Left,
}

/// A manga page in physical units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSpec {
    /// Finished (trim) width and height in millimetres.
    pub trim_mm: [f32; 2],
    /// Bleed beyond the trim on every side, in millimetres.
    pub bleed_mm: f32,
    /// Inner frame margins from the trim edge: top, bottom, binding side,
    /// outer side (millimetres).
    pub inner_margins_mm: [f32; 4],
    pub dpi: f32,
    pub binding: Binding,
    /// Right-hand page of a spread (decides which side is the binding).
    #[serde(default)]
    pub right_page: bool,
}

impl PageSpec {
    /// Built-in presets: (Japanese name, English name, spec).
    pub fn presets() -> Vec<(&'static str, &'static str, PageSpec)> {
        let page = |trim: [f32; 2], bleed: f32, frame: [f32; 2], dpi: f32| {
            // Centre the inner frame vertically; shift it slightly toward
            // the outer side, which is common for manga manuscripts.
            let vertical = ((trim[1] - frame[1]) / 2.0).max(0.0);
            let horizontal = (trim[0] - frame[0]).max(0.0);
            PageSpec {
                trim_mm: trim,
                bleed_mm: bleed,
                inner_margins_mm: [vertical, vertical, horizontal * 0.55, horizontal * 0.45],
                dpi,
                binding: Binding::Right,
                right_page: false,
            }
        };
        vec![
            (
                "投稿用（仕上がりB5・600dpi・モノクロ）",
                "Contest manuscript (B5 finish, 600 dpi, mono)",
                page([182.0, 257.0], 5.0, [150.0, 220.0], 600.0),
            ),
            (
                "同人誌 B5（600dpi）",
                "Doujinshi B5 (600 dpi)",
                page([182.0, 257.0], 3.0, [150.0, 220.0], 600.0),
            ),
            (
                "同人誌 A5（600dpi）",
                "Doujinshi A5 (600 dpi)",
                page([148.0, 210.0], 3.0, [120.0, 180.0], 600.0),
            ),
            (
                "カラー B5（350dpi）",
                "Color B5 (350 dpi)",
                page([182.0, 257.0], 3.0, [150.0, 220.0], 350.0),
            ),
        ]
    }

    fn px(&self, mm: f32) -> f32 {
        mm / 25.4 * self.dpi
    }

    /// The page in canvas pixels.
    pub fn geometry(&self) -> PageGeometry {
        let bleed = self.px(self.bleed_mm);
        let trim_size = Vec2::new(self.px(self.trim_mm[0]), self.px(self.trim_mm[1]));
        let canvas = (trim_size + Vec2::splat(2.0 * bleed)).round();
        let trim_min = Vec2::splat(bleed);
        let trim_max = trim_min + trim_size;
        let [top, bottom, binding, outer] = self.inner_margins_mm.map(|mm| self.px(mm));
        // On a right-bound book the binding is on the right of left-hand
        // pages and on the left of right-hand pages; mirrored for left-bound.
        let binding_on_left = match self.binding {
            Binding::Right => self.right_page,
            Binding::Left => !self.right_page,
        };
        let (left, right) = if binding_on_left {
            (binding, outer)
        } else {
            (outer, binding)
        };
        PageGeometry {
            canvas_width: canvas.x as u32,
            canvas_height: canvas.y as u32,
            bleed: [Vec2::ZERO, canvas],
            trim: [trim_min, trim_max],
            inner: [
                trim_min + Vec2::new(left, top),
                trim_max - Vec2::new(right, bottom),
            ],
            mark_length: self.px(10.0),
        }
    }
}

/// Page areas in canvas pixels, each as [min, max] corners.
#[derive(Clone, Debug, PartialEq)]
pub struct PageGeometry {
    pub canvas_width: u32,
    pub canvas_height: u32,
    pub bleed: [Vec2; 2],
    pub trim: [Vec2; 2],
    pub inner: [Vec2; 2],
    /// Length of the crop mark strokes.
    pub mark_length: f32,
}

impl PageGeometry {
    /// Crop marks as line segments: two per trim corner (pointing away from
    /// the page, starting at the bleed edge) and a short cross at the centre
    /// of each trim edge.
    pub fn crop_marks(&self) -> Vec<[Vec2; 2]> {
        let [t0, t1] = self.trim;
        let [b0, b1] = self.bleed;
        let length = self.mark_length;
        let mut marks = Vec::new();
        for (corner, sx, sy) in [
            (t0, -1.0, -1.0),
            (Vec2::new(t1.x, t0.y), 1.0, -1.0),
            (t1, 1.0, 1.0),
            (Vec2::new(t0.x, t1.y), -1.0, 1.0),
        ] {
            // Horizontal and vertical ticks along the trim lines.
            let bleed_x = if sx < 0.0 { b0.x } else { b1.x };
            let bleed_y = if sy < 0.0 { b0.y } else { b1.y };
            marks.push([
                Vec2::new(bleed_x, corner.y),
                Vec2::new(bleed_x + sx * length, corner.y),
            ]);
            marks.push([
                Vec2::new(corner.x, bleed_y),
                Vec2::new(corner.x, bleed_y + sy * length),
            ]);
        }
        let centre = (t0 + t1) * 0.5;
        let half = length * 0.5;
        for (point, horizontal) in [
            (Vec2::new(centre.x, b0.y - half), false),
            (Vec2::new(centre.x, b1.y + half), false),
            (Vec2::new(b0.x - half, centre.y), true),
            (Vec2::new(b1.x + half, centre.y), true),
        ] {
            if horizontal {
                marks.push([point - Vec2::new(half, 0.0), point + Vec2::new(half, 0.0)]);
                marks.push([point - Vec2::new(0.0, half), point + Vec2::new(0.0, half)]);
            } else {
                marks.push([point - Vec2::new(0.0, half), point + Vec2::new(0.0, half)]);
                marks.push([point - Vec2::new(half, 0.0), point + Vec2::new(half, 0.0)]);
            }
        }
        marks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b5_at_600_dpi() {
        let spec = PageSpec::presets()[1].2.clone();
        let g = spec.geometry();
        // (182 + 6) mm and (257 + 6) mm at 600 dpi.
        assert_eq!((g.canvas_width, g.canvas_height), (4441, 6213));
        assert!((g.trim[0].x - 70.87).abs() < 0.1);
        assert!(g.inner[0].x > g.trim[0].x && g.inner[1].x < g.trim[1].x);
        assert!(g.inner[0].y > g.trim[0].y && g.inner[1].y < g.trim[1].y);
    }

    #[test]
    fn binding_side_moves_the_wider_margin() {
        let mut spec = PageSpec::presets()[1].2.clone();
        spec.inner_margins_mm = [10.0, 10.0, 20.0, 5.0];
        let left_page = spec.geometry();
        spec.right_page = true;
        let right_page = spec.geometry();
        let margin_left = |g: &PageGeometry| g.inner[0].x - g.trim[0].x;
        let margin_right = |g: &PageGeometry| g.trim[1].x - g.inner[1].x;
        // Right-bound: left-hand page has the binding (wide margin) on the right.
        assert!(margin_right(&left_page) > margin_left(&left_page));
        assert!(margin_left(&right_page) > margin_right(&right_page));
    }

    #[test]
    fn crop_marks_sit_outside_the_trim() {
        let g = PageSpec::presets()[1].2.geometry();
        let marks = g.crop_marks();
        assert_eq!(marks.len(), 16);
        for [a, b] in &marks[..8] {
            for p in [a, b] {
                let inside_trim = p.x > g.trim[0].x + 1.0
                    && p.x < g.trim[1].x - 1.0
                    && p.y > g.trim[0].y + 1.0
                    && p.y < g.trim[1].y - 1.0;
                assert!(!inside_trim, "{p:?}");
            }
        }
    }
}
