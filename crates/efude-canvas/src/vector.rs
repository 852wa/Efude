// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Vector layers: strokes kept as paths and drawn into the layer's pixels.
//!
//! A vector layer is an ordinary layer whose `vector` holds its strokes. The
//! pixels are a drawing of those strokes, so compositing, saving pictures and
//! exporting work as for any layer; the strokes stay editable (erase part of
//! a line, move, scale) and are drawn again where they change.

use crate::{History, Layer, TILE_SIZE};
use serde::{Deserialize, Serialize};

/// A point of a vector stroke: position in document pixels and the line's
/// width (diameter) there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorPoint {
    pub x: f32,
    pub y: f32,
    pub width: f32,
}

/// One line of a vector layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorStroke {
    pub points: Vec<VectorPoint>,
    /// Straight RGBA; the alpha is the line's opacity.
    pub color: [u8; 4],
    /// 1 draws a crisp edge; lower values soften it.
    #[serde(default = "full_hardness")]
    pub hardness: f32,
    /// The line as a Bézier curve, which the control point tool edits;
    /// `points` are drawn from it. Empty for a line kept only as points.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<VectorAnchor>,
}

/// An anchor point of a vector line's Bézier curve, with its two handles
/// (relative to the anchor) and the line width there.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VectorAnchor {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    /// Handle toward the previous anchor.
    pub in_x: f32,
    pub in_y: f32,
    /// Handle toward the next anchor.
    pub out_x: f32,
    pub out_y: f32,
    /// A corner: its handles turn independently (otherwise they stay in
    /// line, so the curve stays smooth).
    #[serde(default)]
    pub corner: bool,
}

fn full_hardness() -> f32 {
    1.0
}

/// A rectangle of pixels: `[x0, y0, x1, y1)` (the end is exclusive).
pub type PixelRect = [i32; 4];

/// The smallest rectangle holding both (`None` is empty).
pub fn union_rect(a: Option<PixelRect>, b: Option<PixelRect>) -> Option<PixelRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, None) => a,
        (None, b) => b,
    }
}

impl VectorStroke {
    /// The pixels the stroke can touch.
    pub fn bounds(&self) -> Option<PixelRect> {
        let mut rect: Option<[f32; 4]> = None;
        for p in &self.points {
            let r = p.width * 0.5 + 2.0;
            let b = [p.x - r, p.y - r, p.x + r, p.y + r];
            rect = Some(match rect {
                Some(a) => [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ],
                None => b,
            });
        }
        rect.map(|r| {
            [
                r[0].floor() as i32,
                r[1].floor() as i32,
                r[2].ceil() as i32 + 1,
                r[3].ceil() as i32 + 1,
            ]
        })
    }
}

fn intersects(a: PixelRect, b: PixelRect) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// How much of the pixel centred at `(px, py)` a segment of the stroke
/// covers (a round-capped line whose width changes along the segment).
fn segment_coverage(a: &VectorPoint, b: &VectorPoint, px: f32, py: f32, hardness: f32) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared > 1e-9 {
        (((px - a.x) * dx + (py - a.y) * dy) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a.x + dx * t, a.y + dy * t);
    let distance = ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt();
    let radius = (a.width + (b.width - a.width) * t) * 0.5;
    // Lines thinner than a pixel are drawn a pixel wide and fainter.
    let (radius, thin) = if radius < 0.5 {
        (0.5, (radius * 2.0).max(0.0))
    } else {
        (radius, 1.0)
    };
    let edge = ((1.0 - hardness.clamp(0.0, 1.0)) * radius).max(1.0);
    ((radius - distance) / edge + 0.5).clamp(0.0, 1.0) * thin
}

/// Draws the strokes of a vector layer into its pixels inside `rect`
/// (clipped to the canvas), replacing what was there. Changed tiles are
/// recorded in `history` when given.
pub fn render_region(
    layer: &mut Layer,
    width: u32,
    height: u32,
    rect: PixelRect,
    mut history: Option<&mut History>,
) {
    let rect = [
        rect[0].max(0),
        rect[1].max(0),
        rect[2].min(width as i32),
        rect[3].min(height as i32),
    ];
    if rect[2] <= rect[0] || rect[3] <= rect[1] {
        return;
    }
    let Some(strokes) = layer.vector.take() else {
        return;
    };
    let bounds = strokes.iter().map(VectorStroke::bounds).collect::<Vec<_>>();
    let tile = TILE_SIZE as i32;
    let mut coverage = Vec::new();
    for ty in rect[1] / tile..=(rect[3] - 1) / tile {
        for tx in rect[0] / tile..=(rect[2] - 1) / tile {
            let area = [
                rect[0].max(tx * tile),
                rect[1].max(ty * tile),
                rect[2].min((tx + 1) * tile),
                rect[3].min((ty + 1) * tile),
            ];
            if area[2] <= area[0] || area[3] <= area[1] {
                continue;
            }
            let touching = strokes
                .iter()
                .zip(&bounds)
                .filter(|(_, b)| b.is_some_and(|b| intersects(b, area)))
                .map(|(stroke, _)| stroke)
                .collect::<Vec<_>>();
            let key = (tx as u32, ty as u32);
            if touching.is_empty() && !layer.pixels.has_tile(key.0, key.1) {
                continue;
            }
            if let Some(history) = history.as_deref_mut() {
                history.record_tile(layer, key.0, key.1);
            }
            let (aw, ah) = ((area[2] - area[0]) as usize, (area[3] - area[1]) as usize);
            let pixels = layer.pixels.tile_mut(key.0, key.1);
            let local_x = (area[0] - tx * tile) as usize;
            let local_y = (area[1] - ty * tile) as usize;
            for y in 0..ah {
                let start = ((local_y + y) * TILE_SIZE as usize + local_x) * 4;
                pixels[start..start + aw * 4].fill(0);
            }
            for stroke in touching {
                if stroke.color[3] == 0 {
                    continue;
                }
                coverage.clear();
                coverage.resize(aw * ah, 0.0f32);
                let segments: Vec<(&VectorPoint, &VectorPoint)> = if stroke.points.len() == 1 {
                    vec![(&stroke.points[0], &stroke.points[0])]
                } else {
                    stroke.points.windows(2).map(|w| (&w[0], &w[1])).collect()
                };
                for (a, b) in segments {
                    let r = a.width.max(b.width) * 0.5 + 1.5;
                    let x0 = ((a.x.min(b.x) - r).floor() as i32).max(area[0]);
                    let y0 = ((a.y.min(b.y) - r).floor() as i32).max(area[1]);
                    let x1 = ((a.x.max(b.x) + r).ceil() as i32 + 1).min(area[2]);
                    let y1 = ((a.y.max(b.y) + r).ceil() as i32 + 1).min(area[3]);
                    for y in y0..y1 {
                        let row = (y - area[1]) as usize * aw;
                        for x in x0..x1 {
                            let c = segment_coverage(
                                a,
                                b,
                                x as f32 + 0.5,
                                y as f32 + 0.5,
                                stroke.hardness,
                            );
                            let slot = &mut coverage[row + (x - area[0]) as usize];
                            if c > *slot {
                                *slot = c;
                            }
                        }
                    }
                }
                let alpha = stroke.color[3] as f32 / 255.0;
                for y in 0..ah {
                    for x in 0..aw {
                        let c = coverage[y * aw + x];
                        if c <= 0.0 {
                            continue;
                        }
                        let i = ((local_y + y) * TILE_SIZE as usize + local_x + x) * 4;
                        let source_alpha = alpha * c;
                        let destination_alpha = pixels[i + 3] as f32 / 255.0;
                        let out_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
                        if out_alpha <= 0.0 {
                            continue;
                        }
                        for channel in 0..3 {
                            pixels[i + channel] = ((stroke.color[channel] as f32 * source_alpha
                                + pixels[i + channel] as f32
                                    * destination_alpha
                                    * (1.0 - source_alpha))
                                / out_alpha)
                                .round()
                                .clamp(0.0, 255.0)
                                as u8;
                        }
                        pixels[i + 3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            if !pixels.iter().skip(3).step_by(4).any(|&a| a != 0) {
                layer.pixels.remove_tile(key.0, key.1);
            }
        }
    }
    layer.vector = Some(strokes);
}

/// Draws a whole vector layer again from its strokes.
pub fn render_all(layer: &mut Layer, width: u32, height: u32, history: Option<&mut History>) {
    render_region(
        layer,
        width,
        height,
        [0, 0, width as i32, height as i32],
        history,
    );
}

/// Erases the parts of the strokes inside a circle (`whole`: every stroke
/// the circle touches). Returns the pixels that need drawing again.
pub fn erase_circle(
    strokes: &mut Vec<VectorStroke>,
    center: (f32, f32),
    radius: f32,
    whole: bool,
) -> Option<PixelRect> {
    let radius = radius.max(0.5);
    let reach = [
        (center.0 - radius).floor() as i32 - 1,
        (center.1 - radius).floor() as i32 - 1,
        (center.0 + radius).ceil() as i32 + 2,
        (center.1 + radius).ceil() as i32 + 2,
    ];
    let mut dirty = None;
    let mut result = Vec::with_capacity(strokes.len());
    for stroke in strokes.drain(..) {
        let Some(bounds) = stroke.bounds() else {
            continue;
        };
        if !intersects(bounds, reach) {
            result.push(stroke);
            continue;
        }
        if whole {
            let touched = if stroke.points.len() == 1 {
                let p = stroke.points[0];
                ((p.x - center.0).powi(2) + (p.y - center.1).powi(2)).sqrt()
                    < radius + p.width * 0.5
            } else {
                stroke.points.windows(2).any(|w| {
                    let (a, b) = (w[0], w[1]);
                    let (dx, dy) = (b.x - a.x, b.y - a.y);
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 1e-9 {
                        (((center.0 - a.x) * dx + (center.1 - a.y) * dy) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (cx, cy) = (a.x + dx * t, a.y + dy * t);
                    let w = a.width + (b.width - a.width) * t;
                    ((center.0 - cx).powi(2) + (center.1 - cy).powi(2)).sqrt() < radius + w * 0.5
                })
            };
            if touched {
                dirty = union_rect(dirty, Some(bounds));
            } else {
                result.push(stroke);
            }
            continue;
        }
        let pieces = cut_circle(&stroke.points, center, radius);
        if pieces.len() == 1 && pieces[0].len() == stroke.points.len() {
            result.push(stroke);
            continue;
        }
        dirty = union_rect(dirty, Some(bounds));
        for points in pieces {
            result.push(VectorStroke::fitted(points, stroke.color, stroke.hardness));
        }
    }
    *strokes = result;
    dirty
}

fn lerp_point(a: VectorPoint, b: VectorPoint, t: f32) -> VectorPoint {
    VectorPoint {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        width: a.width + (b.width - a.width) * t,
    }
}

/// The pieces of a line left outside a circle.
fn cut_circle(points: &[VectorPoint], center: (f32, f32), radius: f32) -> Vec<Vec<VectorPoint>> {
    let inside =
        |p: &VectorPoint| (p.x - center.0).powi(2) + (p.y - center.1).powi(2) < radius * radius;
    let mut pieces = Vec::new();
    let mut current = Vec::new();
    for (i, &b) in points.iter().enumerate() {
        if i == 0 {
            if !inside(&b) {
                current.push(b);
            }
            continue;
        }
        let a = points[i - 1];
        // Where the segment runs inside the circle: |a + t(b - a) - c| < r.
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let (fx, fy) = (a.x - center.0, a.y - center.1);
        let qa = dx * dx + dy * dy;
        let qb = 2.0 * (fx * dx + fy * dy);
        let qc = fx * fx + fy * fy - radius * radius;
        let span = if qa < 1e-9 {
            (qc < 0.0).then_some((0.0, 1.0))
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc <= 0.0 {
                None
            } else {
                let root = disc.sqrt();
                let t0 = ((-qb - root) / (2.0 * qa)).max(0.0);
                let t1 = ((-qb + root) / (2.0 * qa)).min(1.0);
                (t0 < t1).then_some((t0, t1))
            }
        };
        match span {
            None => current.push(b),
            Some((t0, t1)) => {
                if t0 > 0.0 {
                    current.push(lerp_point(a, b, t0));
                }
                if !current.is_empty() {
                    pieces.push(std::mem::take(&mut current));
                }
                if t1 < 1.0 {
                    current.push(lerp_point(a, b, t1));
                    current.push(b);
                }
            }
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// Erases the points for which `erase` is true, splitting lines where they
/// are cut. Returns the pixels that need drawing again.
pub fn erase_points(
    strokes: &mut Vec<VectorStroke>,
    erase: impl Fn(f32, f32) -> bool,
) -> Option<PixelRect> {
    let mut dirty = None;
    let mut result = Vec::with_capacity(strokes.len());
    for stroke in strokes.drain(..) {
        if !stroke.points.iter().any(|p| erase(p.x, p.y)) {
            result.push(stroke);
            continue;
        }
        dirty = union_rect(dirty, stroke.bounds());
        let mut current = Vec::new();
        for &p in &stroke.points {
            if erase(p.x, p.y) {
                if !current.is_empty() {
                    result.push(VectorStroke::fitted(
                        std::mem::take(&mut current),
                        stroke.color,
                        stroke.hardness,
                    ));
                }
            } else {
                current.push(p);
            }
        }
        if !current.is_empty() {
            result.push(VectorStroke::fitted(current, stroke.color, stroke.hardness));
        }
    }
    *strokes = result;
    dirty
}

/// Moves every point with `map`, and scales line widths by `width_scale`.
pub fn transform_strokes(
    strokes: &mut [VectorStroke],
    map: impl Fn(f32, f32) -> (f32, f32),
    width_scale: f32,
) {
    for stroke in strokes {
        for p in &mut stroke.points {
            (p.x, p.y) = map(p.x, p.y);
            p.width *= width_scale;
        }
        for a in &mut stroke.anchors {
            let (x, y) = map(a.x, a.y);
            let (ix, iy) = map(a.x + a.in_x, a.y + a.in_y);
            let (ox, oy) = map(a.x + a.out_x, a.y + a.out_y);
            *a = VectorAnchor {
                x,
                y,
                width: a.width * width_scale,
                in_x: ix - x,
                in_y: iy - y,
                out_x: ox - x,
                out_y: oy - y,
                corner: a.corner,
            };
        }
    }
}

/// How closely a fitted curve follows the drawn points (in pixels).
fn fit_tolerance(points: &[VectorPoint]) -> f32 {
    let width = points.iter().map(|p| p.width).fold(0.0f32, f32::max);
    0.45 + width * 0.03
}

impl VectorStroke {
    /// A line from points, with its Bézier curve fitted to them.
    pub fn fitted(points: Vec<VectorPoint>, color: [u8; 4], hardness: f32) -> Self {
        let mut stroke = Self {
            points,
            color,
            hardness,
            anchors: Vec::new(),
        };
        stroke.fit_curve();
        stroke
    }

    /// Fits a Bézier curve to the points (keeping corners) and draws the
    /// points again from it, so what is shown is what can be edited.
    pub fn fit_curve(&mut self) {
        self.anchors = fit_anchors(&self.points, fit_tolerance(&self.points));
        self.rebuild_points();
    }

    /// Draws the points again from the curve (after its anchors change).
    pub fn rebuild_points(&mut self) {
        let anchors = &self.anchors;
        match anchors.len() {
            0 => {}
            1 => {
                let a = anchors[0];
                self.points = vec![VectorPoint {
                    x: a.x,
                    y: a.y,
                    width: a.width,
                }];
            }
            _ => {
                let mut points = Vec::new();
                for pair in anchors.windows(2) {
                    let [p0, p1, p2, p3] = segment_controls(&pair[0], &pair[1]);
                    let length = p0.distance(p1) + p1.distance(p2) + p2.distance(p3);
                    let steps = (length / 1.2).ceil().clamp(1.0, 600.0) as usize;
                    let first = if points.is_empty() { 0 } else { 1 };
                    for i in first..=steps {
                        let t = i as f32 / steps as f32;
                        let p = cubic(p0, p1, p2, p3, t);
                        points.push(VectorPoint {
                            x: p.x,
                            y: p.y,
                            width: pair[0].width + (pair[1].width - pair[0].width) * t,
                        });
                    }
                }
                self.points = points;
            }
        }
    }

    /// Gives an older line (points only) a curve to edit.
    pub fn ensure_curve(&mut self) {
        if self.anchors.is_empty() && !self.points.is_empty() {
            self.fit_curve();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct P {
    x: f32,
    y: f32,
}
impl P {
    fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    fn add(self, o: P) -> P {
        P::new(self.x + o.x, self.y + o.y)
    }
    fn sub(self, o: P) -> P {
        P::new(self.x - o.x, self.y - o.y)
    }
    fn scale(self, k: f32) -> P {
        P::new(self.x * k, self.y * k)
    }
    fn dot(self, o: P) -> f32 {
        self.x * o.x + self.y * o.y
    }
    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    fn distance(self, o: P) -> f32 {
        self.sub(o).length()
    }
    fn normalized(self) -> P {
        let l = self.length();
        if l > 1e-9 {
            self.scale(1.0 / l)
        } else {
            P::new(0.0, 0.0)
        }
    }
}

fn segment_controls(a: &VectorAnchor, b: &VectorAnchor) -> [P; 4] {
    [
        P::new(a.x, a.y),
        P::new(a.x + a.out_x, a.y + a.out_y),
        P::new(b.x + b.in_x, b.y + b.in_y),
        P::new(b.x, b.y),
    ]
}

fn cubic(p0: P, p1: P, p2: P, p3: P, t: f32) -> P {
    let u = 1.0 - t;
    p0.scale(u * u * u)
        .add(p1.scale(3.0 * u * u * t))
        .add(p2.scale(3.0 * u * t * t))
        .add(p3.scale(t * t * t))
}

fn cubic_derivative(b: &[P; 4], t: f32) -> P {
    let u = 1.0 - t;
    b[1].sub(b[0])
        .scale(3.0 * u * u)
        .add(b[2].sub(b[1]).scale(6.0 * u * t))
        .add(b[3].sub(b[2]).scale(3.0 * t * t))
}

fn cubic_second(b: &[P; 4], t: f32) -> P {
    let u = 1.0 - t;
    b[2].sub(b[1].scale(2.0))
        .add(b[0])
        .scale(6.0 * u)
        .add(b[3].sub(b[2].scale(2.0)).add(b[1]).scale(6.0 * t))
}

/// One fitted curve segment: control points and the widths at its ends.
struct Segment {
    controls: [P; 4],
    widths: [f32; 2],
}

/// Fits a Bézier curve through drawn points (least squares, splitting where
/// it strays more than `tolerance`; sharp turns become corners).
pub fn fit_anchors(points: &[VectorPoint], tolerance: f32) -> Vec<VectorAnchor> {
    let mut clean: Vec<VectorPoint> = Vec::with_capacity(points.len());
    for p in points {
        if clean
            .last()
            .is_none_or(|q| (q.x - p.x).hypot(q.y - p.y) > 0.05)
        {
            clean.push(*p);
        }
    }
    let anchor = |p: &VectorPoint| VectorAnchor {
        x: p.x,
        y: p.y,
        width: p.width,
        in_x: 0.0,
        in_y: 0.0,
        out_x: 0.0,
        out_y: 0.0,
        corner: false,
    };
    match clean.len() {
        0 => return Vec::new(),
        1 => return vec![anchor(&clean[0])],
        _ => {}
    }
    let at = |i: usize| P::new(clean[i].x, clean[i].y);
    // Corners: where the direction turns sharply between points a few
    // pixels before and after.
    let reach = 4.0f32;
    let mut cuts = vec![0usize];
    let last = clean.len() - 1;
    let mut i = 1;
    while i < last {
        let mut back = i;
        while back > 0 && at(back).distance(at(i)) < reach {
            back -= 1;
        }
        let mut ahead = i;
        while ahead < last && at(ahead).distance(at(i)) < reach {
            ahead += 1;
        }
        let d1 = at(i).sub(at(back)).normalized();
        let d2 = at(ahead).sub(at(i)).normalized();
        if at(back).distance(at(i)) >= reach * 0.75
            && at(ahead).distance(at(i)) >= reach * 0.75
            && d1.dot(d2) < 0.35
        {
            // The sharpest point of this turn.
            let mut best = i;
            let mut best_dot = d1.dot(d2);
            let mut j = i + 1;
            while j < last && at(j).distance(at(i)) < reach {
                let b = at(j.saturating_sub(1).max(back));
                let a = at((j + 1).min(last));
                let dot = at(j).sub(b).normalized().dot(a.sub(at(j)).normalized());
                if dot < best_dot {
                    best = j;
                    best_dot = dot;
                }
                j += 1;
            }
            if best > *cuts.last().unwrap() + 1 && best + 1 < last {
                cuts.push(best);
            }
            i = best + 1;
            continue;
        }
        i += 1;
    }
    cuts.push(last);
    let squared = tolerance * tolerance;
    let mut segments = Vec::new();
    for pair in cuts.windows(2) {
        let part = &clean[pair[0]..=pair[1]];
        let start = end_tangent(part, false);
        let end = end_tangent(part, true);
        fit_cubic(part, start, end, squared, &mut segments);
    }
    let mut anchors = Vec::with_capacity(segments.len() + 1);
    for (k, segment) in segments.iter().enumerate() {
        let [p0, p1, p2, p3] = segment.controls;
        if k == 0 {
            anchors.push(VectorAnchor {
                x: p0.x,
                y: p0.y,
                width: segment.widths[0],
                in_x: 0.0,
                in_y: 0.0,
                out_x: p1.x - p0.x,
                out_y: p1.y - p0.y,
                corner: false,
            });
        } else if let Some(previous) = anchors.last_mut() {
            previous.out_x = p1.x - p0.x;
            previous.out_y = p1.y - p0.y;
        }
        anchors.push(VectorAnchor {
            x: p3.x,
            y: p3.y,
            width: segment.widths[1],
            in_x: p2.x - p3.x,
            in_y: p2.y - p3.y,
            out_x: 0.0,
            out_y: 0.0,
            corner: false,
        });
    }
    // Anchors at the cuts are corners.
    for a in &mut anchors {
        a.corner = cuts[1..cuts.len() - 1]
            .iter()
            .any(|&c| (clean[c].x - a.x).abs() < 1e-3 && (clean[c].y - a.y).abs() < 1e-3);
    }
    anchors
}

/// The direction a stretch of points leaves its start (or, `end`, enters
/// its end from).
fn end_tangent(part: &[VectorPoint], end: bool) -> P {
    let at = |i: usize| P::new(part[i].x, part[i].y);
    let n = part.len();
    let (origin, order): (usize, Box<dyn Iterator<Item = usize>>) = if end {
        (n - 1, Box::new((0..n - 1).rev()))
    } else {
        (0, Box::new(1..n))
    };
    let mut direction = P::new(0.0, 0.0);
    for i in order {
        direction = at(i).sub(at(origin));
        if direction.length() >= 3.0 {
            break;
        }
    }
    direction.normalized()
}

fn fit_cubic(part: &[VectorPoint], start: P, end: P, squared: f32, out: &mut Vec<Segment>) {
    let at = |i: usize| P::new(part[i].x, part[i].y);
    let n = part.len();
    let widths = [part[0].width, part[n - 1].width];
    if n == 2 {
        let d = at(0).distance(at(1)) / 3.0;
        out.push(Segment {
            controls: [
                at(0),
                at(0).add(start.scale(d)),
                at(1).add(end.scale(d)),
                at(1),
            ],
            widths,
        });
        return;
    }
    let mut u = chord_parameters(part);
    let mut controls = generate(part, &u, start, end);
    let (mut error, mut split) = max_error(part, &controls, &u);
    if error < squared {
        out.push(Segment { controls, widths });
        return;
    }
    if error < squared * 16.0 {
        for _ in 0..4 {
            u = reparameterize(part, &controls, &u);
            controls = generate(part, &u, start, end);
            (error, split) = max_error(part, &controls, &u);
            if error < squared {
                out.push(Segment { controls, widths });
                return;
            }
        }
    }
    let split = split.clamp(1, n - 2);
    let mut center = at(split - 1).sub(at(split + 1)).normalized();
    if center.length() < 0.5 {
        center = at(split - 1).sub(at(split)).normalized();
    }
    fit_cubic(&part[..=split], start, center, squared, out);
    fit_cubic(&part[split..], center.scale(-1.0), end, squared, out);
}

fn chord_parameters(part: &[VectorPoint]) -> Vec<f32> {
    let mut u = vec![0.0f32; part.len()];
    for i in 1..part.len() {
        u[i] = u[i - 1] + (part[i].x - part[i - 1].x).hypot(part[i].y - part[i - 1].y);
    }
    let total = *u.last().unwrap();
    if total > 0.0 {
        for value in &mut u {
            *value /= total;
        }
    }
    u
}

fn generate(part: &[VectorPoint], u: &[f32], start: P, end: P) -> [P; 4] {
    let at = |i: usize| P::new(part[i].x, part[i].y);
    let (first, last) = (at(0), at(part.len() - 1));
    let mut c = [[0.0f32; 2]; 2];
    let mut x = [0.0f32; 2];
    for (i, &t) in u.iter().enumerate() {
        let s = 1.0 - t;
        let (b0, b1, b2, b3) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
        let a0 = start.scale(b1);
        let a1 = end.scale(b2);
        c[0][0] += a0.dot(a0);
        c[0][1] += a0.dot(a1);
        c[1][1] += a1.dot(a1);
        let rest = at(i).sub(first.scale(b0 + b1).add(last.scale(b2 + b3)));
        x[0] += a0.dot(rest);
        x[1] += a1.dot(rest);
    }
    c[1][0] = c[0][1];
    let det = c[0][0] * c[1][1] - c[1][0] * c[0][1];
    let chord = first.distance(last);
    let (mut left, mut right) = if det.abs() > 1e-12 {
        (
            (x[0] * c[1][1] - x[1] * c[0][1]) / det,
            (c[0][0] * x[1] - c[1][0] * x[0]) / det,
        )
    } else {
        (0.0, 0.0)
    };
    if left < chord * 1e-3 || right < chord * 1e-3 || !left.is_finite() || !right.is_finite() {
        left = chord / 3.0;
        right = chord / 3.0;
    }
    [
        first,
        first.add(start.scale(left)),
        last.add(end.scale(right)),
        last,
    ]
}

/// The largest squared distance of a point from the curve (a width that
/// strays counts too), and where it is.
fn max_error(part: &[VectorPoint], controls: &[P; 4], u: &[f32]) -> (f32, usize) {
    let (w0, w1) = (part[0].width, part[part.len() - 1].width);
    let mut worst = 0.0f32;
    let mut at = part.len() / 2;
    for i in 1..part.len() - 1 {
        let q = cubic(controls[0], controls[1], controls[2], controls[3], u[i]);
        let d = q.sub(P::new(part[i].x, part[i].y));
        let width_error = (w0 + (w1 - w0) * u[i] - part[i].width) * 0.5;
        let error = d.dot(d) + width_error * width_error;
        if error > worst {
            worst = error;
            at = i;
        }
    }
    (worst, at)
}

fn reparameterize(part: &[VectorPoint], controls: &[P; 4], u: &[f32]) -> Vec<f32> {
    u.iter()
        .enumerate()
        .map(|(i, &t)| {
            let q = cubic(controls[0], controls[1], controls[2], controls[3], t);
            let d1 = cubic_derivative(controls, t);
            let d2 = cubic_second(controls, t);
            let diff = q.sub(P::new(part[i].x, part[i].y));
            let denominator = d1.dot(d1) + diff.dot(d2);
            if denominator.abs() < 1e-9 {
                t
            } else {
                (t - diff.dot(d1) / denominator).clamp(0.0, 1.0)
            }
        })
        .collect()
}

/// The line nearest to a point (within `reach` of its edge), last drawn
/// first.
pub fn hit_stroke(strokes: &[VectorStroke], x: f32, y: f32, reach: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (index, stroke) in strokes.iter().enumerate().rev() {
        let distance = stroke_distance(stroke, x, y);
        if distance <= reach && best.is_none_or(|(_, d)| distance < d) {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// Distance from a point to the edge of a line (0 inside it).
fn stroke_distance(stroke: &VectorStroke, x: f32, y: f32) -> f32 {
    let p = P::new(x, y);
    let mut best = f32::MAX;
    let segment = |a: &VectorPoint, b: &VectorPoint| {
        let (pa, pb) = (P::new(a.x, a.y), P::new(b.x, b.y));
        let ab = pb.sub(pa);
        let t = if ab.dot(ab) > 1e-9 {
            (p.sub(pa).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let width = a.width + (b.width - a.width) * t;
        (p.distance(pa.add(ab.scale(t))) - width * 0.5).max(0.0)
    };
    match stroke.points.len() {
        0 => {}
        1 => best = segment(&stroke.points[0], &stroke.points[0]),
        _ => {
            for pair in stroke.points.windows(2) {
                best = best.min(segment(&pair[0], &pair[1]));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: (f32, f32), to: (f32, f32), width: f32) -> VectorStroke {
        let steps = 20;
        VectorStroke {
            points: (0..=steps)
                .map(|i| {
                    let t = i as f32 / steps as f32;
                    VectorPoint {
                        x: from.0 + (to.0 - from.0) * t,
                        y: from.1 + (to.1 - from.1) * t,
                        width,
                    }
                })
                .collect(),
            color: [20, 40, 200, 255],
            hardness: 1.0,
            anchors: Vec::new(),
        }
    }

    fn vector_layer(strokes: Vec<VectorStroke>) -> Layer {
        let mut layer = Layer::new(1, "v", 300, 300);
        layer.vector = Some(strokes);
        render_all(&mut layer, 300, 300, None);
        layer
    }

    #[test]
    fn strokes_draw_solid_lines_with_soft_edges() {
        let layer = vector_layer(vec![line((10.0, 50.0), (290.0, 50.0), 5.0)]);
        assert_eq!(layer.pixels.pixel(150, 50), [20, 40, 200, 255]);
        assert_eq!(layer.pixels.pixel(150, 60)[3], 0);
        let edge = layer.pixels.pixel(150, 52)[3];
        assert!(edge > 0 && edge < 255, "{edge}");
    }

    #[test]
    fn drawing_a_region_matches_drawing_everything() {
        let strokes = vec![
            line((10.0, 20.0), (280.0, 270.0), 9.0),
            line((270.0, 30.0), (20.0, 260.0), 3.0),
        ];
        let whole = vector_layer(strokes.clone());
        let mut parts = Layer::new(1, "v", 300, 300);
        parts.vector = Some(strokes);
        for rect in [[0, 0, 131, 97], [131, 0, 300, 97], [0, 97, 300, 300]] {
            render_region(&mut parts, 300, 300, rect, None);
        }
        assert_eq!(whole.pixels.to_dense(), parts.pixels.to_dense());
        // Drawing a region again gives the same pixels.
        render_region(&mut parts, 300, 300, [100, 100, 200, 200], None);
        assert_eq!(whole.pixels.to_dense(), parts.pixels.to_dense());
    }

    #[test]
    fn erasing_cuts_a_line_and_whole_erasing_removes_it() {
        let mut strokes = vec![line((10.0, 50.0), (290.0, 50.0), 4.0)];
        let dirty = erase_circle(&mut strokes, (150.0, 50.0), 10.0, false);
        assert!(dirty.is_some());
        assert_eq!(strokes.len(), 2);
        assert!(strokes[0].points.last().unwrap().x <= 140.01);
        assert!(strokes[1].points[0].x >= 159.99);
        let mut layer = vector_layer(strokes.clone());
        assert_eq!(layer.pixels.pixel(150, 50)[3], 0);
        assert_eq!(layer.pixels.pixel(100, 50)[3], 255);
        // Missing the line changes nothing.
        assert!(erase_circle(&mut strokes, (150.0, 200.0), 10.0, false).is_none());
        erase_circle(&mut strokes, (100.0, 51.0), 1.0, true);
        assert_eq!(strokes.len(), 1);
        layer.vector = Some(strokes);
        render_all(&mut layer, 300, 300, None);
        assert_eq!(layer.pixels.pixel(100, 50)[3], 0);
        assert_eq!(layer.pixels.pixel(200, 50)[3], 255);
    }

    #[test]
    fn history_restores_strokes_and_pixels() {
        let mut doc = crate::Document::new(300, 300);
        doc.layers[0].vector = Some(Vec::new());
        let mut history = History::default();
        history.begin();
        let before = doc.layers[0].vector.clone();
        doc.layers[0]
            .vector
            .as_mut()
            .unwrap()
            .push(line((10.0, 50.0), (290.0, 50.0), 6.0));
        render_all(&mut doc.layers[0], 300, 300, Some(&mut history));
        history.record_vector(
            doc.layers[0].id,
            before.as_deref(),
            doc.layers[0].vector.as_deref(),
        );
        history.commit();
        assert_eq!(doc.layers[0].pixels.pixel(150, 50)[3], 255);
        history.undo_document(&mut doc);
        assert_eq!(doc.layers[0].vector.as_ref().unwrap().len(), 0);
        assert_eq!(doc.layers[0].pixels.pixel(150, 50)[3], 0);
        history.redo_document(&mut doc);
        assert_eq!(doc.layers[0].vector.as_ref().unwrap().len(), 1);
        assert_eq!(doc.layers[0].pixels.pixel(150, 50)[3], 255);
    }

    fn deviation(stroke: &VectorStroke, drawn: &[VectorPoint]) -> f32 {
        drawn
            .iter()
            .map(|p| {
                stroke
                    .points
                    .iter()
                    .map(|q| (q.x - p.x).hypot(q.y - p.y))
                    .fold(f32::MAX, f32::min)
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_drawn_curve_becomes_a_few_editable_anchors() {
        let drawn = (0..=300)
            .map(|i| {
                let x = 10.0 + i as f32;
                VectorPoint {
                    x,
                    y: 150.0 + 60.0 * (x / 40.0).sin(),
                    width: 4.0 + (i as f32 / 300.0) * 4.0,
                }
            })
            .collect::<Vec<_>>();
        let stroke = VectorStroke::fitted(drawn.clone(), [0, 0, 0, 255], 1.0);
        assert!(
            stroke.anchors.len() >= 3 && stroke.anchors.len() < 20,
            "{}",
            stroke.anchors.len()
        );
        assert!(
            deviation(&stroke, &drawn) < 1.2,
            "{}",
            deviation(&stroke, &drawn)
        );
        assert!(stroke.anchors.iter().all(|a| !a.corner));
        // The width follows the pressure along the line.
        let (first, last) = (stroke.anchors[0], *stroke.anchors.last().unwrap());
        assert!((first.width - 4.0).abs() < 0.5 && (last.width - 8.0).abs() < 0.5);
    }

    #[test]
    fn sharp_turns_become_corners_and_straight_lines_two_anchors() {
        let mut drawn = Vec::new();
        for i in 0..=100 {
            drawn.push(VectorPoint {
                x: 20.0 + i as f32,
                y: 200.0 - i as f32 * 1.5,
                width: 3.0,
            });
        }
        for i in 1..=100 {
            drawn.push(VectorPoint {
                x: 120.0 + i as f32,
                y: 50.0 + i as f32 * 1.5,
                width: 3.0,
            });
        }
        let v = VectorStroke::fitted(drawn.clone(), [0; 4], 1.0);
        assert!(
            v.anchors
                .iter()
                .any(|a| a.corner && (a.x - 120.0).abs() < 2.0),
            "{:?}",
            v.anchors
        );
        assert!(deviation(&v, &drawn) < 1.2);
        let straight =
            VectorStroke::fitted(line((10.0, 10.0), (200.0, 90.0), 3.0).points, [0; 4], 1.0);
        assert_eq!(straight.anchors.len(), 2);
    }

    #[test]
    fn moving_an_anchor_reshapes_the_line_and_transforms_keep_the_curve() {
        let mut stroke =
            VectorStroke::fitted(line((10.0, 50.0), (290.0, 50.0), 4.0).points, [0; 4], 1.0);
        stroke.anchors[1].y += 40.0;
        stroke.rebuild_points();
        assert!((stroke.points.last().unwrap().y - 90.0).abs() < 1e-3);
        let mut moved = vec![stroke.clone()];
        transform_strokes(&mut moved, |x, y| (x * 2.0, y + 5.0), 2.0);
        let mut rebuilt = moved[0].clone();
        rebuilt.rebuild_points();
        let a = moved[0].points.last().unwrap();
        let b = rebuilt.points.last().unwrap();
        assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
        assert!(hit_stroke(&moved, 580.0, 100.0, 3.0).is_some());
        assert!(hit_stroke(&moved, 300.0, 250.0, 3.0).is_none());
    }
}
