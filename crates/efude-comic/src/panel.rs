// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Panels: convex polygons with a border, split by gutters.

use glam::Vec2;
use serde::{Deserialize, Serialize};

/// One panel. Its drawing lives in the folder layer `folder_id`, whose mask
/// is the panel shape; the border is drawn on layer `border_id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub folder_id: u64,
    pub border_id: u64,
    /// Convex polygon in canvas pixels.
    pub polygon: Vec<Vec2>,
}

/// The panels of a page and their shared settings (canvas pixels).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelLayout {
    pub border_width: f32,
    /// Gap between panels side by side (a vertical cut).
    pub gutter_horizontal: f32,
    /// Gap between panels above and below each other (a horizontal cut).
    pub gutter_vertical: f32,
    pub panels: Vec<Panel>,
}

impl PanelLayout {
    /// Typical manga values at `dpi`: 0.5 mm borders, 2 mm gaps between
    /// columns and 5 mm between rows.
    pub fn for_dpi(dpi: f32) -> Self {
        let px = |mm: f32| mm / 25.4 * dpi;
        Self {
            border_width: px(0.5),
            gutter_horizontal: px(2.0),
            gutter_vertical: px(5.0),
            panels: Vec::new(),
        }
    }

    /// Index of the panel containing `point`.
    pub fn panel_at(&self, point: Vec2) -> Option<usize> {
        self.panels
            .iter()
            .position(|panel| contains(&panel.polygon, point))
    }

    /// The gutter for a cut along `a`→`b`: mostly horizontal cuts separate
    /// rows, mostly vertical cuts separate columns.
    pub fn gutter_for(&self, a: Vec2, b: Vec2) -> f32 {
        let d = b - a;
        if d.x.abs() >= d.y.abs() {
            self.gutter_vertical
        } else {
            self.gutter_horizontal
        }
    }
}

/// A rectangle as a polygon (clockwise on screen).
pub fn rectangle(min: Vec2, max: Vec2) -> Vec<Vec2> {
    vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
}

/// Point-in-polygon (even-odd).
pub fn contains(polygon: &[Vec2], p: Vec2) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Area of a polygon (absolute).
pub fn area(polygon: &[Vec2]) -> f32 {
    let n = polygon.len();
    ((0..n)
        .map(|i| polygon[i].perp_dot(polygon[(i + 1) % n]))
        .sum::<f32>()
        * 0.5)
        .abs()
}

/// Keeps the part of `polygon` where `(p - origin) · normal >= 0`.
pub fn clip_half_plane(polygon: &[Vec2], origin: Vec2, normal: Vec2) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(polygon.len() + 1);
    let n = polygon.len();
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        let (da, db) = ((a - origin).dot(normal), (b - origin).dot(normal));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(a + (b - a) * t);
        }
    }
    // Drop repeated points.
    out.dedup_by(|a, b| a.distance_squared(*b) < 1e-6);
    if out.len() > 1 && out[0].distance_squared(*out.last().unwrap()) < 1e-6 {
        out.pop();
    }
    out
}

/// Cuts `polygon` along the infinite line through `a` and `b`, leaving a
/// gap of `gutter` between the parts. Returns `None` unless both parts are
/// real panels (at least `min_area`).
pub fn split(
    polygon: &[Vec2],
    a: Vec2,
    b: Vec2,
    gutter: f32,
    min_area: f32,
) -> Option<[Vec<Vec2>; 2]> {
    let d = (b - a).normalize_or_zero();
    if d == Vec2::ZERO {
        return None;
    }
    let normal = Vec2::new(-d.y, d.x);
    let half = gutter.max(0.0) * 0.5;
    let first = clip_half_plane(polygon, a + normal * half, normal);
    let second = clip_half_plane(polygon, a - normal * half, -normal);
    (first.len() >= 3 && second.len() >= 3 && area(&first) >= min_area && area(&second) >= min_area)
        .then_some([first, second])
}

/// Splits `polygon` into `columns` × `rows` cells with the given gaps,
/// based on its bounding box. Cells are listed row by row, and within a row
/// from the reading start: right to left when `right_to_left`.
pub fn grid(
    polygon: &[Vec2],
    columns: u32,
    rows: u32,
    gutter_horizontal: f32,
    gutter_vertical: f32,
    right_to_left: bool,
) -> Vec<Vec<Vec2>> {
    let (columns, rows) = (columns.max(1), rows.max(1));
    let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for &p in polygon {
        min = min.min(p);
        max = max.max(p);
    }
    let size = max - min;
    let cell = Vec2::new(
        (size.x - gutter_horizontal * (columns - 1) as f32) / columns as f32,
        (size.y - gutter_vertical * (rows - 1) as f32) / rows as f32,
    );
    if cell.x <= 1.0 || cell.y <= 1.0 {
        return vec![polygon.to_vec()];
    }
    let mut cells = Vec::new();
    for row in 0..rows {
        for index in 0..columns {
            let column = if right_to_left {
                columns - 1 - index
            } else {
                index
            };
            let x0 = min.x + column as f32 * (cell.x + gutter_horizontal);
            let y0 = min.y + row as f32 * (cell.y + gutter_vertical);
            let mut part = polygon.to_vec();
            // Edges of the grid are left alone so the outer shape is kept.
            if column > 0 {
                part = clip_half_plane(&part, Vec2::new(x0, 0.0), Vec2::X);
            }
            if column + 1 < columns {
                part = clip_half_plane(&part, Vec2::new(x0 + cell.x, 0.0), -Vec2::X);
            }
            if row > 0 {
                part = clip_half_plane(&part, Vec2::new(0.0, y0), Vec2::Y);
            }
            if row + 1 < rows {
                part = clip_half_plane(&part, Vec2::new(0.0, y0 + cell.y), -Vec2::Y);
            }
            if part.len() >= 3 {
                cells.push(part);
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_cut_leaves_the_row_gap() {
        let page = rectangle(Vec2::new(100.0, 100.0), Vec2::new(500.0, 700.0));
        let [a, b] = split(
            &page,
            Vec2::new(50.0, 400.0),
            Vec2::new(550.0, 400.0),
            20.0,
            10.0,
        )
        .unwrap();
        let (top, bottom) = if a[0].y < b[0].y { (&a, &b) } else { (&b, &a) };
        let top_max = top.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        let bottom_min = bottom.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!((top_max - 390.0).abs() < 0.01 && (bottom_min - 410.0).abs() < 0.01);
        assert!((area(top) + area(bottom) - 400.0 * 580.0).abs() < 1.0);
    }

    #[test]
    fn diagonal_cut_and_miss() {
        let page = rectangle(Vec2::ZERO, Vec2::new(400.0, 400.0));
        let parts = split(
            &page,
            Vec2::new(0.0, 100.0),
            Vec2::new(400.0, 300.0),
            10.0,
            10.0,
        )
        .unwrap();
        assert!(parts.iter().all(|p| p.len() == 4));
        // A line outside the panel does not split it.
        assert!(
            split(
                &page,
                Vec2::new(0.0, 500.0),
                Vec2::new(400.0, 500.0),
                10.0,
                10.0
            )
            .is_none()
        );
    }

    #[test]
    fn grid_reads_right_to_left() {
        let page = rectangle(Vec2::ZERO, Vec2::new(300.0, 200.0));
        let cells = grid(&page, 3, 2, 10.0, 20.0, true);
        assert_eq!(cells.len(), 6);
        // First cell is the top-right one.
        assert!(cells[0].iter().all(|p| p.x >= 199.0 && p.y <= 90.0 + 0.01));
        let width = cells[1].iter().map(|p| p.x).fold(f32::MIN, f32::max)
            - cells[1].iter().map(|p| p.x).fold(f32::MAX, f32::min);
        assert!((width - (300.0 - 20.0) / 3.0).abs() < 0.01);
    }

    #[test]
    fn layout_finds_panels_and_gutters() {
        let mut layout = PanelLayout::for_dpi(600.0);
        layout.panels.push(Panel {
            folder_id: 1,
            border_id: 2,
            polygon: rectangle(Vec2::ZERO, Vec2::new(100.0, 100.0)),
        });
        assert_eq!(layout.panel_at(Vec2::new(50.0, 50.0)), Some(0));
        assert_eq!(layout.panel_at(Vec2::new(150.0, 50.0)), None);
        assert_eq!(
            layout.gutter_for(Vec2::ZERO, Vec2::new(10.0, 1.0)),
            layout.gutter_vertical
        );
    }
}
