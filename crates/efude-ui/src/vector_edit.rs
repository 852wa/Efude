// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! The control point tool: selects a vector line and moves its Bézier
//! anchors and handles, or the whole line.

use super::*;
use efude_canvas::VectorStroke;
use efude_canvas::vector::{self, PixelRect};

/// What the control point tool has selected.
#[derive(Default)]
pub(crate) struct VectorEdit {
    layer_id: Option<u64>,
    pub(crate) stroke: Option<usize>,
    pub(crate) anchor: Option<usize>,
    drag: Option<VectorDrag>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum DragTarget {
    Anchor(usize),
    In(usize),
    Out(usize),
    Whole,
}

struct VectorDrag {
    target: DragTarget,
    start: Vec2,
    /// The line when the drag began.
    original: VectorStroke,
    /// All lines of the layer before the drag (for Undo).
    before: Vec<VectorStroke>,
    /// Where the line is drawn now.
    shown: Option<PixelRect>,
}

/// Screen pixels within which a point or line is picked.
const PICK_RADIUS: f32 = 7.0;

impl EfudeApp {
    /// The selected layer, when the control point tool can edit it.
    fn vector_edit_layer(&self) -> Option<usize> {
        let index = self.selected_layer;
        self.doc
            .layers
            .get(index)
            .is_some_and(|layer| layer.is_vector() && !layer.locked)
            .then_some(index)
            .filter(|&index| !self.is_reference_layer(index))
    }

    /// Forgets the selection when another layer is chosen or lines change
    /// under it (Undo, erasing).
    fn sync_vector_edit(&mut self) {
        let layer = self.doc.layers.get(self.selected_layer);
        let id = layer.map(|layer| layer.id);
        let count = layer
            .and_then(|layer| layer.vector.as_ref())
            .map_or(0, Vec::len);
        if self.vector_edit.layer_id != id {
            self.vector_edit = VectorEdit {
                layer_id: id,
                ..Default::default()
            };
        }
        if self
            .vector_edit
            .stroke
            .is_some_and(|stroke| stroke >= count)
        {
            self.vector_edit.stroke = None;
            self.vector_edit.anchor = None;
        }
    }

    /// Pointer down: picks a handle or anchor of the selected line, or a
    /// line, and starts dragging it.
    pub(crate) fn vector_edit_press(&mut self, q: Vec2, scale: f32) {
        self.sync_vector_edit();
        let Some(index) = self.vector_edit_layer() else {
            self.refuse_vector_edit();
            return;
        };
        let reach = PICK_RADIUS / scale.max(1e-3);
        let strokes = self.doc.layers[index].vector.as_deref().unwrap_or_default();
        let near = |x: f32, y: f32| (x - q.x).hypot(y - q.y) <= reach;
        let mut target = None;
        if let Some(stroke) = self.vector_edit.stroke.and_then(|i| strokes.get(i)) {
            for (i, a) in stroke.anchors.iter().enumerate() {
                if (a.in_x != 0.0 || a.in_y != 0.0) && near(a.x + a.in_x, a.y + a.in_y) {
                    target = Some(DragTarget::In(i));
                } else if (a.out_x != 0.0 || a.out_y != 0.0) && near(a.x + a.out_x, a.y + a.out_y) {
                    target = Some(DragTarget::Out(i));
                }
            }
            // Anchors win over handles lying on them.
            for (i, a) in stroke.anchors.iter().enumerate() {
                if near(a.x, a.y) {
                    target = Some(DragTarget::Anchor(i));
                }
            }
        }
        if target.is_none() {
            match vector::hit_stroke(strokes, q.x, q.y, reach) {
                Some(stroke) => {
                    if self.vector_edit.stroke != Some(stroke) {
                        self.vector_edit.anchor = None;
                    }
                    self.vector_edit.stroke = Some(stroke);
                    target = Some(DragTarget::Whole);
                }
                None => {
                    self.vector_edit.stroke = None;
                    self.vector_edit.anchor = None;
                    return;
                }
            }
        }
        let (Some(target), Some(stroke_index)) = (target, self.vector_edit.stroke) else {
            return;
        };
        if let DragTarget::Anchor(i) | DragTarget::In(i) | DragTarget::Out(i) = target {
            self.vector_edit.anchor = Some(i);
        }
        let before = strokes.to_vec();
        self.history.begin();
        // Older lines (points only) get a curve first.
        let (w, h) = (self.doc.width, self.doc.height);
        let layer = &mut self.doc.layers[index];
        let stroke = &mut layer.vector.as_mut().unwrap()[stroke_index];
        let old = stroke.bounds();
        stroke.ensure_curve();
        let original = stroke.clone();
        let shown = original.bounds();
        if before[stroke_index] != original
            && let Some(rect) = vector::union_rect(old, shown)
        {
            vector::render_region(layer, w, h, rect, Some(&mut self.history));
        }
        self.vector_edit.drag = Some(VectorDrag {
            target,
            start: q,
            original,
            before,
            shown,
        });
    }

    /// Pointer moved while dragging: reshapes the line from where it was.
    pub(crate) fn vector_edit_drag(&mut self, q: Vec2) {
        let layer_index = self.vector_edit_layer();
        let (Some(drag), Some(stroke_index), Some(index)) = (
            self.vector_edit.drag.as_mut(),
            self.vector_edit.stroke,
            layer_index,
        ) else {
            return;
        };
        let delta = q - drag.start;
        let mut stroke = drag.original.clone();
        let length = |x: f32, y: f32| x.hypot(y);
        match drag.target {
            DragTarget::Anchor(i) => {
                if let Some(a) = stroke.anchors.get_mut(i) {
                    a.x += delta.x;
                    a.y += delta.y;
                }
            }
            DragTarget::Whole => {
                for a in &mut stroke.anchors {
                    a.x += delta.x;
                    a.y += delta.y;
                }
            }
            DragTarget::Out(i) => {
                if let Some(a) = stroke.anchors.get_mut(i) {
                    a.out_x += delta.x;
                    a.out_y += delta.y;
                    let (out, inside) = (length(a.out_x, a.out_y), length(a.in_x, a.in_y));
                    if !a.corner && out > 1e-3 && inside > 1e-3 {
                        a.in_x = -a.out_x / out * inside;
                        a.in_y = -a.out_y / out * inside;
                    }
                }
            }
            DragTarget::In(i) => {
                if let Some(a) = stroke.anchors.get_mut(i) {
                    a.in_x += delta.x;
                    a.in_y += delta.y;
                    let (inside, out) = (length(a.in_x, a.in_y), length(a.out_x, a.out_y));
                    if !a.corner && out > 1e-3 && inside > 1e-3 {
                        a.out_x = -a.in_x / inside * out;
                        a.out_y = -a.in_y / inside * out;
                    }
                }
            }
        }
        stroke.rebuild_points();
        let now = stroke.bounds();
        let dirty = vector::union_rect(drag.shown, now);
        drag.shown = now;
        let (w, h) = (self.doc.width, self.doc.height);
        let layer = &mut self.doc.layers[index];
        if let Some(slot) = layer
            .vector
            .as_mut()
            .and_then(|strokes| strokes.get_mut(stroke_index))
        {
            *slot = stroke;
        }
        if let Some(rect) = dirty {
            vector::render_region(layer, w, h, rect, Some(&mut self.history));
        }
    }

    /// Pointer up: the drag becomes one Undo step.
    pub(crate) fn vector_edit_release(&mut self) {
        let Some(drag) = self.vector_edit.drag.take() else {
            return;
        };
        if let Some(index) = self.vector_edit_layer() {
            let layer = &self.doc.layers[index];
            self.history
                .record_vector(layer.id, Some(&drag.before), layer.vector.as_deref());
        }
        self.history.commit();
    }

    fn refuse_vector_edit(&mut self) {
        self.status = self
            .text(
                "制御点ツールはベクターレイヤーで使えます",
                "The control point tool works on vector layers",
            )
            .into();
    }

    /// Changes the selected line (or its selected anchor) as one Undo step.
    pub(crate) fn edit_selected_vector(
        &mut self,
        change: impl FnOnce(&mut Vec<VectorStroke>, usize, Option<usize>),
    ) {
        self.sync_vector_edit();
        let (Some(index), Some(stroke)) = (self.vector_edit_layer(), self.vector_edit.stroke)
        else {
            return;
        };
        let (w, h) = (self.doc.width, self.doc.height);
        let Some(before) = self.doc.layers[index].vector.clone() else {
            return;
        };
        let mut strokes = before.clone();
        let old = strokes.get(stroke).and_then(VectorStroke::bounds);
        if let Some(line) = strokes.get_mut(stroke) {
            line.ensure_curve();
        }
        change(&mut strokes, stroke, self.vector_edit.anchor);
        if strokes == before {
            return;
        }
        let new = strokes.get(stroke).and_then(VectorStroke::bounds);
        self.history.begin();
        let layer = &mut self.doc.layers[index];
        layer.vector = Some(strokes);
        if let Some(rect) = vector::union_rect(old, new) {
            vector::render_region(layer, w, h, rect, Some(&mut self.history));
        }
        let layer = &self.doc.layers[index];
        self.history
            .record_vector(layer.id, Some(&before), layer.vector.as_deref());
        self.history.commit();
        self.sync_vector_edit();
    }

    /// Delete key with the control point tool: removes the selected anchor
    /// (a line keeps at least two), or else the selected line.
    pub(crate) fn delete_vector_selection(&mut self) {
        let anchor = self.vector_edit.anchor;
        self.edit_selected_vector(|strokes, stroke, _| {
            let line = &mut strokes[stroke];
            match anchor {
                Some(i) if line.anchors.len() > 2 && i < line.anchors.len() => {
                    line.anchors.remove(i);
                    line.rebuild_points();
                }
                _ => {
                    strokes.remove(stroke);
                }
            }
        });
        self.vector_edit.anchor = None;
        self.sync_vector_edit();
        if anchor.is_none() {
            self.vector_edit.stroke = None;
        }
    }

    /// Options of the control point tool in the tool panel.
    pub(crate) fn vector_edit_options(&mut self, ui: &mut egui::Ui) {
        self.sync_vector_edit();
        ui.label(self.text(
            "ベクターレイヤーの線をクリックして選び、アンカー（□）やハンドル（○）をドラッグして形を変えます。線そのものをドラッグすると線ごと動きます。Deleteで選んだアンカー（なければ線）を消します。",
            "Click a line on a vector layer to select it, then drag its anchors (squares) or handles (circles) to reshape it. Drag the line itself to move it. Delete removes the selected anchor (or the line).",
        ));
        if self.vector_edit_layer().is_none() {
            ui.colored_label(
                Color32::from_rgb(230, 180, 90),
                self.text("ベクターレイヤーを選んでください", "Select a vector layer"),
            );
            return;
        }
        let Some(stroke) = self.vector_edit.stroke else {
            return;
        };
        ui.separator();
        let corner = self.doc.layers[self.selected_layer]
            .vector
            .as_ref()
            .and_then(|strokes| strokes.get(stroke))
            .zip(self.vector_edit.anchor)
            .and_then(|(line, i)| line.anchors.get(i))
            .map(|a| a.corner);
        ui.horizontal_wrapped(|ui| {
            ui.label(self.text("線の太さ", "Width"));
            for (label, factor) in [("−", 0.8f32), ("＋", 1.25)] {
                if ui.button(label).clicked() {
                    self.edit_selected_vector(|strokes, stroke, _| {
                        let line = &mut strokes[stroke];
                        for a in &mut line.anchors {
                            a.width = (a.width * factor).clamp(0.1, 2000.0);
                        }
                        line.rebuild_points();
                    });
                }
            }
            if ui
                .button(self.text("現在の色にする", "Use Current Color"))
                .clicked()
            {
                let color = [
                    self.color.r(),
                    self.color.g(),
                    self.color.b(),
                    self.color.a(),
                ];
                self.edit_selected_vector(|strokes, stroke, _| strokes[stroke].color = color);
            }
        });
        if let Some(corner) = corner {
            let label = if corner {
                self.text("なめらかにする", "Make Smooth")
            } else {
                self.text("角にする", "Make Corner")
            };
            if ui
                .button(label)
                .on_hover_text(self.text(
                    "角のアンカーは2本のハンドルを別々に動かせます",
                    "A corner's two handles move independently",
                ))
                .clicked()
            {
                let anchor = self.vector_edit.anchor;
                self.edit_selected_vector(|strokes, stroke, _| {
                    let line = &mut strokes[stroke];
                    if let Some(a) = anchor.and_then(|i| line.anchors.get_mut(i)) {
                        a.corner = !corner;
                        if corner {
                            // Line the handles up again.
                            let (ox, oy) = (a.out_x, a.out_y);
                            let (ix, iy) = (a.in_x, a.in_y);
                            let (lo, li) = (ox.hypot(oy), ix.hypot(iy));
                            let (dx, dy) = (ox - ix, oy - iy);
                            let d = dx.hypot(dy);
                            if d > 1e-3 {
                                (a.out_x, a.out_y) = (dx / d * lo, dy / d * lo);
                                (a.in_x, a.in_y) = (-dx / d * li, -dy / d * li);
                            }
                        }
                    }
                    line.rebuild_points();
                });
            }
        }
        if ui.button(self.text("線を削除", "Delete Line")).clicked() {
            self.vector_edit.anchor = None;
            self.delete_vector_selection();
        }
    }

    /// Draws the anchors and handles over the canvas.
    pub(crate) fn paint_vector_overlay(
        &mut self,
        painter: &egui::Painter,
        to_screen: &dyn Fn(Vec2) -> Pos2,
    ) {
        if self.tool != Tool::VectorEdit {
            return;
        }
        self.sync_vector_edit();
        let Some(index) = self.vector_edit_layer() else {
            return;
        };
        let Some(strokes) = self.doc.layers[index].vector.as_ref() else {
            return;
        };
        let accent = crate::layout::ACCENT;
        let pos = |x: f32, y: f32| to_screen(Vec2::new(x, y));
        // Every line's anchors, faintly, so lines can be found.
        let total: usize = strokes.iter().map(|s| s.anchors.len()).sum();
        if total <= 4000 {
            for (i, stroke) in strokes.iter().enumerate() {
                if Some(i) == self.vector_edit.stroke {
                    continue;
                }
                for a in &stroke.anchors {
                    painter.rect_filled(
                        Rect::from_center_size(pos(a.x, a.y), Vec2::splat(4.0)),
                        0.0,
                        Color32::from_rgba_unmultiplied(90, 140, 255, 150),
                    );
                }
            }
        }
        let Some(stroke) = self.vector_edit.stroke.and_then(|i| strokes.get(i)) else {
            return;
        };
        let step = (stroke.points.len() / 1500).max(1);
        let path: Vec<Pos2> = stroke
            .points
            .iter()
            .step_by(step)
            .chain(stroke.points.last())
            .map(|p| pos(p.x, p.y))
            .collect();
        painter.add(egui::Shape::line(path, Stroke::new(1.5, accent)));
        let handle_stroke = Stroke::new(1.0, Color32::from_rgb(200, 200, 215));
        for a in &stroke.anchors {
            let center = pos(a.x, a.y);
            for (hx, hy) in [(a.in_x, a.in_y), (a.out_x, a.out_y)] {
                if hx != 0.0 || hy != 0.0 {
                    let handle = pos(a.x + hx, a.y + hy);
                    painter.line_segment([center, handle], handle_stroke);
                    painter.circle_filled(handle, 3.5, Color32::WHITE);
                    painter.circle_stroke(handle, 3.5, Stroke::new(1.0, accent));
                }
            }
        }
        for (i, a) in stroke.anchors.iter().enumerate() {
            let rect = Rect::from_center_size(pos(a.x, a.y), Vec2::splat(8.0));
            let fill = if Some(i) == self.vector_edit.anchor {
                accent
            } else {
                Color32::WHITE
            };
            if a.corner {
                // Corners are drawn as diamonds.
                let c = rect.center();
                painter.add(egui::Shape::convex_polygon(
                    vec![
                        c + Vec2::new(0.0, -5.0),
                        c + Vec2::new(5.0, 0.0),
                        c + Vec2::new(0.0, 5.0),
                        c + Vec2::new(-5.0, 0.0),
                    ],
                    fill,
                    Stroke::new(1.2, accent),
                ));
            } else {
                painter.rect_filled(rect, 0.0, fill);
                painter.rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(1.2, accent),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}
