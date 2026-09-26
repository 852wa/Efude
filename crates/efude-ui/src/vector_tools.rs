// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Vector layers in the app: drawing and erasing lines, moving and
//! transforming them, and turning a vector layer into a raster one.

use super::*;
use efude_canvas::vector::{self, PixelRect};
use efude_canvas::{VectorPoint, VectorStroke};

/// The vector stroke being drawn or erased.
pub(crate) struct VectorLive {
    layer_id: u64,
    /// The layer's strokes before this stroke (for Undo).
    before: Option<Vec<VectorStroke>>,
    /// Index of the first line this stroke adds (one per symmetry copy).
    first: usize,
    count: usize,
    /// Points too close to the last kept one, kept in case the stroke
    /// ends there.
    pending: Vec<Option<VectorPoint>>,
}

/// Whether most of the line lies in the selection.
fn in_selection(stroke: &VectorStroke, mask: &[u8], width: u32, height: u32) -> bool {
    let inside = stroke
        .points
        .iter()
        .filter(|p| {
            let (x, y) = (p.x.floor(), p.y.floor());
            x >= 0.0
                && y >= 0.0
                && (x as u32) < width
                && (y as u32) < height
                && mask
                    .get((y as u32 * width + x as u32) as usize)
                    .is_some_and(|&v| v >= 128)
        })
        .count();
    inside * 2 > stroke.points.len()
}

impl EfudeApp {
    /// Whether painting now goes to the strokes of a vector layer.
    pub(crate) fn vector_target(&self) -> bool {
        let index = self.selected_layer;
        !self.editing_mask
            && !matches!(self.tool, Tool::SelectionBrush | Tool::QuickMask)
            && self.doc.layers.get(index).is_some_and(|layer| {
                layer.is_vector() && !layer.locked && layer.kind != LayerKind::Folder
            })
            && !self.is_reference_layer(index)
    }

    /// Tells that a raster-only action does not work on a vector layer.
    /// Returns true (and does nothing else) when the layer is a vector one.
    pub(crate) fn refuse_on_vector_layer(&mut self) -> bool {
        if self
            .doc
            .layers
            .get(self.selected_layer)
            .is_some_and(|layer| layer.is_vector())
        {
            self.status = self
                .text(
                    "ベクターレイヤーでは使えません（ラスタライズすると使えます）",
                    "Not available on a vector layer (rasterize it first)",
                )
                .into();
            return true;
        }
        false
    }

    /// Adds brush points to the vector stroke, or erases lines under them.
    pub(crate) fn vector_dabs(&mut self, points: &[InkPoint]) {
        let index = self.selected_layer;
        let brush = &self.brushes[self.selected_brush];
        let kind = match self.tool {
            Tool::Blur => BrushKind::Blur,
            Tool::Smudge => BrushKind::Smudge,
            _ => brush.kind,
        };
        if matches!(kind, BrushKind::Blur | BrushKind::Smudge) {
            self.refuse_on_vector_layer();
            return;
        }
        let erasing = self.tool == Tool::Eraser
            || matches!(kind, BrushKind::Eraser)
            || self.erases_with_transparent_color(kind);
        let layer_id = self.doc.layers[index].id;
        if self
            .vector_live
            .as_ref()
            .is_none_or(|live| live.layer_id != layer_id)
        {
            self.vector_live = Some(VectorLive {
                layer_id,
                before: self.doc.layers[index].vector.clone(),
                first: 0,
                count: 0,
                pending: Vec::new(),
            });
        }
        let color = [
            self.color.r(),
            self.color.g(),
            self.color.b(),
            (self.color.a() as f32 * brush.opacity.clamp(0.0, 1.0)).round() as u8,
        ];
        let hardness = brush.hardness;
        let mut dirty: Option<PixelRect> = None;
        for point in points {
            let mut p = *point;
            p.pressure = self.brushes[self.selected_brush]
                .map_pressure(self.global_pressure_curve(p.pressure));
            let dynamics = efude_brush::engine::dynamics(
                &self.brushes[self.selected_brush],
                &p,
                self.raster.last_dab,
                self.view_scale,
            );
            self.raster.last_dab = Some(p);
            let width = (self.size * dynamics.size).max(0.1);
            let mirrored = self.symmetric_points(p);
            let Some(strokes) = self.doc.layers[index].vector.as_mut() else {
                return;
            };
            if erasing {
                for m in &mirrored {
                    let changed = vector::erase_circle(
                        strokes,
                        (m.position.x, m.position.y),
                        width * 0.5,
                        self.vector_erase_whole,
                    );
                    dirty = vector::union_rect(dirty, changed);
                }
                continue;
            }
            let Some(live) = self.vector_live.as_mut() else {
                return;
            };
            if live.count == 0 {
                live.first = strokes.len();
                live.count = mirrored.len();
                live.pending = vec![None; mirrored.len()];
                for _ in 0..mirrored.len() {
                    strokes.push(VectorStroke {
                        points: Vec::new(),
                        color,
                        hardness,
                        anchors: Vec::new(),
                    });
                }
            }
            for (k, m) in mirrored.iter().enumerate().take(live.count) {
                let new = VectorPoint {
                    x: m.position.x,
                    y: m.position.y,
                    width,
                };
                let Some(stroke) = strokes.get_mut(live.first + k) else {
                    continue;
                };
                let step = (width * 0.1).clamp(0.6, 3.0);
                match stroke.points.last() {
                    Some(last) if (last.x - new.x).hypot(last.y - new.y) < step => {
                        live.pending[k] = Some(new);
                    }
                    last => {
                        let from = last.copied().unwrap_or(new);
                        stroke.points.push(new);
                        live.pending[k] = None;
                        dirty = vector::union_rect(dirty, segment_rect(&from, &new));
                    }
                }
            }
        }
        if let Some(rect) = dirty {
            let (w, h) = (self.doc.width, self.doc.height);
            vector::render_region(
                &mut self.doc.layers[index],
                w,
                h,
                rect,
                Some(&mut self.history),
            );
        }
    }

    /// Ends the vector stroke: keeps its last point and records it for
    /// Undo (the caller commits the history step).
    pub(crate) fn finish_vector_stroke(&mut self) {
        let Some(mut live) = self.vector_live.take() else {
            return;
        };
        let Some(index) = self
            .doc
            .layers
            .iter()
            .position(|layer| layer.id == live.layer_id)
        else {
            return;
        };
        let mut dirty = None;
        if let Some(strokes) = self.doc.layers[index].vector.as_mut() {
            for (k, pending) in live.pending.iter_mut().enumerate() {
                if let (Some(p), Some(stroke)) = (pending.take(), strokes.get_mut(live.first + k)) {
                    let from = stroke.points.last().copied().unwrap_or(p);
                    stroke.points.push(p);
                    dirty = vector::union_rect(dirty, segment_rect(&from, &p));
                }
            }
            // The new lines become Bézier curves, so their points can be
            // edited later with the control point tool.
            for k in 0..live.count {
                if let Some(stroke) = strokes.get_mut(live.first + k)
                    && !stroke.points.is_empty()
                {
                    dirty = vector::union_rect(dirty, stroke.bounds());
                    stroke.fit_curve();
                    dirty = vector::union_rect(dirty, stroke.bounds());
                }
            }
            strokes.retain(|stroke| !stroke.points.is_empty());
        }
        let (w, h) = (self.doc.width, self.doc.height);
        if let Some(rect) = dirty {
            vector::render_region(
                &mut self.doc.layers[index],
                w,
                h,
                rect,
                Some(&mut self.history),
            );
        }
        let layer = &self.doc.layers[index];
        self.history.record_vector(
            live.layer_id,
            live.before.as_deref(),
            layer.vector.as_deref(),
        );
    }

    /// Takes back the vector stroke drawn so far (the pixels are restored
    /// by the history rollback).
    pub(crate) fn rollback_vector_stroke(&mut self) {
        let Some(live) = self.vector_live.as_mut() else {
            return;
        };
        if let Some(layer) = self
            .doc
            .layers
            .iter_mut()
            .find(|layer| layer.id == live.layer_id)
        {
            layer.vector.clone_from(&live.before);
        }
        live.count = 0;
        live.pending.clear();
    }

    /// Erases the selected parts of the lines (all of them without a
    /// selection), as one Undo step.
    pub(crate) fn erase_vector_selection(&mut self) {
        let index = self.selected_layer;
        let (w, h) = (self.doc.width, self.doc.height);
        let Some(before) = self.doc.layers[index].vector.clone() else {
            return;
        };
        let mut strokes = before.clone();
        let dirty = if self.selection.active {
            let mask = &self.selection.mask;
            vector::erase_points(&mut strokes, |x, y| {
                let (x, y) = (x.floor(), y.floor());
                x >= 0.0
                    && y >= 0.0
                    && (x as u32) < w
                    && (y as u32) < h
                    && mask
                        .get((y as u32 * w + x as u32) as usize)
                        .is_some_and(|&v| v >= 128)
            })
        } else {
            let all = strokes
                .iter()
                .fold(None, |r, s| vector::union_rect(r, s.bounds()));
            strokes.clear();
            all
        };
        let Some(rect) = dirty else {
            return;
        };
        self.history.begin();
        self.doc.layers[index].vector = Some(strokes);
        vector::render_region(
            &mut self.doc.layers[index],
            w,
            h,
            rect,
            Some(&mut self.history),
        );
        let id = self.doc.layers[index].id;
        self.history
            .record_vector(id, Some(&before), self.doc.layers[index].vector.as_deref());
        self.history.commit();
        self.canvas_texture_dirty = true;
    }

    /// Moves the lines as the Move tool moved the pixels (the lines mostly
    /// inside the selection, or all of them) and draws the layer again.
    /// Part of the Move tool's history step.
    pub(crate) fn finish_vector_move(&mut self) {
        let Some(origin) = self.move_origin.as_ref() else {
            return;
        };
        let (Some(before), index) = (origin.vector.clone(), origin.layer) else {
            return;
        };
        if origin.offset == (0, 0) || index >= self.doc.layers.len() {
            return;
        }
        let (dx, dy) = (origin.offset.0 as f32, origin.offset.1 as f32);
        let (w, h) = (self.doc.width, self.doc.height);
        let mut strokes = before.clone();
        for stroke in &mut strokes {
            if origin.bounds.is_none() || in_selection(stroke, &origin.selection, w, h) {
                vector::transform_strokes(
                    std::slice::from_mut(stroke),
                    |x, y| (x + dx, y + dy),
                    1.0,
                );
            }
        }
        let id = self.doc.layers[index].id;
        self.doc.layers[index].vector = Some(strokes);
        vector::render_all(&mut self.doc.layers[index], w, h, Some(&mut self.history));
        self.history
            .record_vector(id, Some(&before), self.doc.layers[index].vector.as_deref());
    }

    /// Scales and rotates the lines like `transform_selection` does the
    /// pixels. Part of the transform's history step.
    pub(crate) fn transform_vector_lines(
        &mut self,
        before_selection: &[u8],
        selected: bool,
        scale: (f32, f32),
        angle: f32,
    ) {
        let index = self.selected_layer;
        let (w, h) = (self.doc.width, self.doc.height);
        let Some(before) = self.doc.layers[index].vector.clone() else {
            return;
        };
        // The same centre as the pixel transform: the middle of the
        // selected pixels (of the canvas without a selection).
        let mut bounds = (w as i32, h as i32, -1i32, -1i32);
        if selected {
            for (i, &v) in before_selection.iter().enumerate() {
                if v != 0 {
                    let (x, y) = ((i % w as usize) as i32, (i / w as usize) as i32);
                    bounds = (
                        bounds.0.min(x),
                        bounds.1.min(y),
                        bounds.2.max(x),
                        bounds.3.max(y),
                    );
                }
            }
            if bounds.2 < 0 {
                return;
            }
        } else {
            bounds = (0, 0, w as i32 - 1, h as i32 - 1);
        }
        let cx = (bounds.0 + bounds.2) as f32 / 2.0 + 0.5;
        let cy = (bounds.1 + bounds.3) as f32 / 2.0 + 0.5;
        let (c, s) = (angle.cos(), angle.sin());
        let width_scale = (scale.0 * scale.1).abs().sqrt();
        let mut strokes = before.clone();
        for stroke in &mut strokes {
            if !selected || in_selection(stroke, before_selection, w, h) {
                vector::transform_strokes(
                    std::slice::from_mut(stroke),
                    |x, y| {
                        let dx = (x - cx) * scale.0;
                        let dy = (y - cy) * scale.1;
                        (cx + dx * c - dy * s, cy + dx * s + dy * c)
                    },
                    width_scale,
                );
            }
        }
        let id = self.doc.layers[index].id;
        self.doc.layers[index].vector = Some(strokes);
        vector::render_all(&mut self.doc.layers[index], w, h, Some(&mut self.history));
        self.history
            .record_vector(id, Some(&before), self.doc.layers[index].vector.as_deref());
    }

    /// Adds an empty vector layer above the selected layer.
    pub(crate) fn add_vector_layer(&mut self) {
        let english = self.language_english;
        let id = self.doc.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let mut layer = efude_canvas::Layer::new(
            id,
            if english {
                format!("Vector {id}")
            } else {
                format!("ベクター {id}")
            },
            self.doc.width,
            self.doc.height,
        );
        layer.vector = Some(Vec::new());
        self.insert_layer_above_selected(layer);
    }

    /// Turns the selected vector layer into a raster layer (its drawing
    /// stays). Undoable.
    pub(crate) fn rasterize_layer(&mut self, index: usize) {
        let Some(layer) = self.doc.layers.get_mut(index) else {
            return;
        };
        let Some(before) = layer.vector.take() else {
            return;
        };
        let id = layer.id;
        self.history.begin();
        self.history.record_vector(id, Some(&before), None);
        self.history.commit();
        self.status = self.text("ラスタライズしました", "Rasterized").into();
    }
}

/// The pixels a segment between two points can touch.
fn segment_rect(a: &VectorPoint, b: &VectorPoint) -> Option<PixelRect> {
    let r = a.width.max(b.width) * 0.5 + 2.0;
    Some([
        (a.x.min(b.x) - r).floor() as i32,
        (a.y.min(b.y) - r).floor() as i32,
        (a.x.max(b.x) + r).ceil() as i32 + 1,
        (a.y.max(b.y) + r).ceil() as i32 + 1,
    ])
}
