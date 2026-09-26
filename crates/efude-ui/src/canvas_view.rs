// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! The canvas view: drawing, input on the canvas and overlays.

use super::*;

impl EfudeApp {
    /// Draws the canvas into `ui` and handles pointer input on it.
    pub(crate) fn canvas_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.painter().rect_filled(
            ui.available_rect_before_wrap(),
            0.0,
            layout::CANVAS_BACKGROUND,
        );
        let available = ui.available_size();
        let (response, painter) = ui.allocate_painter(available, egui::Sense::drag());
        // 100% is one document pixel per physical screen pixel.
        let pixels_per_point = ctx.pixels_per_point();
        let (fit, mut scale) = display::view_scale(
            available,
            (self.doc.width, self.doc.height),
            self.zoom,
            pixels_per_point,
        );
        let ca = self.view_rotation.to_radians().cos();
        let sa = self.view_rotation.to_radians().sin();
        let document_center = Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
        let (flip_x, flip_y) = (self.flip_x, self.flip_y);
        // Document vector -> screen direction (flip, then rotate).
        let to_view = move |mut v: Vec2| {
            if flip_x {
                v.x = -v.x;
            }
            if flip_y {
                v.y = -v.y;
            }
            Vec2::new(v.x * ca - v.y * sa, v.x * sa + v.y * ca)
        };
        // The navigator centre sits at the middle of the view, whatever the
        // rotation.
        self.canvas_viewport = (response.rect, fit, pixels_per_point);
        let upright = self.view_rotation.rem_euclid(360.0) == 0.0;
        let raw_canvas_rect = |scale: f32, navigator_center: Vec2| {
            Rect::from_center_size(
                response.rect.center() + to_view(document_center - navigator_center) * scale,
                Vec2::new(self.doc.width as f32, self.doc.height as f32) * scale,
            )
        };
        let canvas_rect = |scale: f32, navigator_center: Vec2| {
            let rect = raw_canvas_rect(scale, navigator_center);
            // On the pixel grid, so document pixels are not split.
            if upright {
                display::snap_to_pixels(rect, pixels_per_point)
            } else {
                rect
            }
        };
        let mut rect = canvas_rect(scale, self.navigator_center);
        if response.hovered() {
            let scroll = ctx.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.01
                && let Some(pointer) = ctx.input(|input| input.pointer.hover_pos())
            {
                // Keep the document point under the pointer in place.
                // Measured without the pixel-grid snap so repeated steps
                // do not accumulate its sub-pixel offset.
                let anchor = self.document_position_unsnapped(
                    pointer,
                    raw_canvas_rect(scale, self.navigator_center),
                    scale,
                );
                self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.01, 64.0);
                scale = fit * self.zoom;
                self.navigator_center =
                    anchor - self.screen_delta_to_document(pointer - response.rect.center(), scale);
                rect = canvas_rect(scale, self.navigator_center);
            }
        }
        // The canvas senses drags only (so a stroke starts the moment the
        // pen touches down); a click is a drag that ended without moving.
        let canvas_clicked = response.drag_stopped_by(egui::PointerButton::Primary)
            && ctx.input(|input| input.pointer.primary_clicked());
        // Middle-button drag pans the view with any tool.
        if response.dragged_by(egui::PointerButton::Middle) {
            self.navigator_center -= self.screen_delta_to_document(response.drag_delta(), scale);
        }
        // Screen pixels per document pixel, for speed dynamics.
        self.view_scale = scale;
        self.canvas_screen = Some((rect, scale));
        let to_screen =
            |position: Vec2| rect.center() + to_view(position - document_center) * scale;
        let stride = (1.5 / scale.max(0.05)).ceil().clamp(2.0, 64.0) as usize;
        let visible_radius_x = (ca.abs() * available.x + sa.abs() * available.y) * 0.5 / scale;
        let visible_radius_y = (sa.abs() * available.x + ca.abs() * available.y) * 0.5 / scale;
        let visible_x = ((self.navigator_center.x - visible_radius_x)
            .floor()
            .max(0.0) as usize)
            ..((self.navigator_center.x + visible_radius_x)
                .ceil()
                .min(self.doc.width as f32) as usize);
        let visible_y = ((self.navigator_center.y - visible_radius_y)
            .floor()
            .max(0.0) as usize)
            ..((self.navigator_center.y + visible_radius_y)
                .ceil()
                .min(self.doc.height as f32) as usize);
        self.absorb_history_tile_changes();
        // The composite is always stored upright; rotation and flips are
        // applied when it is drawn (see `canvas_quad`).
        let mut gpu_canvas_active = false;
        if self.gpu_dab_pipeline.is_some() && !self.gpu_canvas_disabled {
            // The GPU display has its own texture; only changed tiles are
            // recomposited unless everything must be redrawn.
            let surface_missing = self.gpu_canvas_surface.as_ref().is_none_or(|surface| {
                surface.width != self.doc.width || surface.height != self.doc.height
            });
            if self.canvas_texture_dirty || surface_missing || !self.dirty_canvas_tiles.is_empty() {
                let full_refresh = self.canvas_texture_dirty || surface_missing;
                gpu_canvas_active = self.update_gpu_canvas_surface(full_refresh);
            } else {
                gpu_canvas_active = self.gpu_canvas_surface.is_some();
            }
        }
        if !gpu_canvas_active && (self.canvas_texture_dirty || self.canvas_texture.is_none()) {
            let pixels = efude_canvas::composite_display(&self.doc, self.display_checker());
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [self.doc.width as usize, self.doc.height as usize],
                &pixels,
            );
            self.cpu_mips.update_block(
                &pixels,
                self.doc.width,
                self.doc.height,
                0,
                0,
                (self.doc.width, self.doc.height),
            );
            if let Some(tex) = &mut self.canvas_texture {
                tex.set(image, display::CANVAS_TEXTURE);
            } else {
                self.canvas_texture =
                    Some(ctx.load_texture("canvas", image, display::CANVAS_TEXTURE));
            }
            self.dirty_canvas_tiles.clear();
            self.canvas_texture_dirty = false;
        } else if !gpu_canvas_active && !self.dirty_canvas_tiles.is_empty() {
            let mut tiles = self.dirty_canvas_tiles.drain().collect::<Vec<_>>();
            tiles.sort_unstable();
            let updates = tiles
                .into_iter()
                .filter_map(|(tile_x, tile_y)| {
                    efude_canvas::composite_tile_display(
                        &self.doc,
                        tile_x,
                        tile_y,
                        self.display_checker(),
                    )
                    .map(|(tile_width, tile_height, pixels)| {
                        (tile_x, tile_y, tile_width, tile_height, pixels)
                    })
                })
                .collect::<Vec<_>>();
            for (tile_x, tile_y, tile_width, tile_height, pixels) in &updates {
                self.cpu_mips.update_block(
                    pixels,
                    *tile_width,
                    *tile_height,
                    tile_x * efude_canvas::TILE_SIZE,
                    tile_y * efude_canvas::TILE_SIZE,
                    (self.doc.width, self.doc.height),
                );
            }
            if let Some(texture) = &mut self.canvas_texture {
                for (tile_x, tile_y, tile_width, tile_height, pixels) in updates {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [tile_width as usize, tile_height as usize],
                        &pixels,
                    );
                    texture.set_partial(
                        [
                            (tile_x * efude_canvas::TILE_SIZE) as usize,
                            (tile_y * efude_canvas::TILE_SIZE) as usize,
                        ],
                        image,
                        display::CANVAS_TEXTURE,
                    );
                }
            }
        }
        // Paper and picture as a quad through the view transform, so
        // rotation and flips turn the whole canvas.
        let (doc_w, doc_h) = (self.doc.width as f32, self.doc.height as f32);
        let corners = [
            to_screen(Vec2::ZERO),
            to_screen(Vec2::new(doc_w, 0.0)),
            to_screen(Vec2::new(doc_w, doc_h)),
            to_screen(Vec2::new(0.0, doc_h)),
        ];
        painter.add(canvas_quad(
            egui::TextureId::default(),
            corners,
            Color32::WHITE,
        ));
        let reduced = if gpu_canvas_active {
            None
        } else {
            self.cpu_mips.texture_for(ctx, scale * pixels_per_point)
        };
        let picture = match (&self.gpu_canvas_surface, &self.canvas_texture) {
            (Some(surface), _) if gpu_canvas_active => Some(surface.texture_id),
            (_, Some(texture)) => Some(reduced.unwrap_or(texture.id())),
            _ => None,
        };
        if let Some(texture) = picture {
            painter.add(canvas_quad(texture, corners, Color32::WHITE));
        }
        if let Some((paste_w, paste_h, _, position)) = &self.paste_preview
            && let Some(texture) = &self.paste_texture
        {
            let (w, h) = (*paste_w as f32, *paste_h as f32);
            let paste_corners = [
                to_screen(*position),
                to_screen(*position + Vec2::new(w, 0.0)),
                to_screen(*position + Vec2::new(w, h)),
                to_screen(*position + Vec2::new(0.0, h)),
            ];
            painter.add(canvas_quad(texture.id(), paste_corners, Color32::WHITE));
            painter.add(egui::Shape::closed_line(
                paste_corners.to_vec(),
                Stroke::new(1.5, Color32::YELLOW),
            ));
            ui.painter().text(
                response.rect.left_top() + Vec2::splat(12.0),
                egui::Align2::LEFT_TOP,
                self.text(
                    "貼り付け位置をドラッグし、クリックで確定 / Escで取消",
                    "Drag to position, click to place / Esc to cancel",
                ),
                egui::FontId::proportional(14.0),
                Color32::YELLOW,
            );
        }
        if let Some(reference) = &self.reference_image {
            let max_side = (available.x.min(available.y) * 0.28).clamp(96.0, 240.0);
            let aspect = self.reference_size.x / self.reference_size.y.max(1.0);
            let base = if aspect >= 1.0 {
                Vec2::new(max_side, max_side / aspect)
            } else {
                Vec2::new(max_side * aspect, max_side)
            };
            let size = (base * self.reference_zoom).min(available * 0.8);
            let default_position = Pos2::new(
                response.rect.right() - size.x - 18.0,
                response.rect.top() + 18.0,
            );
            let position = *self.reference_position.get_or_insert(default_position);
            let reference_rect = Rect::from_min_size(position, size);
            painter.rect_filled(reference_rect.expand(4.0), 3.0, Color32::from_gray(35));
            painter.image(
                reference.id(),
                reference_rect,
                egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::from_white_alpha((self.reference_opacity * 255.0) as u8),
            );
            painter.rect_stroke(
                reference_rect,
                2.0,
                Stroke::new(1.0, Color32::from_gray(210)),
                egui::StrokeKind::Inside,
            );
            let move_response = ui.interact(
                reference_rect,
                egui::Id::new("reference-image-drag"),
                egui::Sense::drag(),
            );
            if move_response.dragged() {
                self.reference_position = Some(position + move_response.drag_delta());
            }
        }
        if matches!(
            self.tool,
            Tool::Brush
                | Tool::Blur
                | Tool::Smudge
                | Tool::Eraser
                | Tool::SelectionBrush
                | Tool::QuickMask
        ) && !self.active.is_empty()
            && let Some(stabilized) = self.stabilized_cursor
        {
            let latest = self.active.last().unwrap().position;
            let raw = to_screen(Vec2::new(latest.x, latest.y));
            let filtered = to_screen(stabilized);
            if raw.distance(filtered) > 2.0 {
                let guide = Stroke::new(1.5, Color32::from_rgb(40, 225, 240));
                painter.line_segment([raw, filtered], guide);
                painter.circle_filled(filtered, 3.0, Color32::from_rgb(40, 225, 240));
                painter.circle_stroke(raw, 5.0, guide);
            }
        }
        if matches!(
            self.tool,
            Tool::Brush
                | Tool::Blur
                | Tool::Smudge
                | Tool::Eraser
                | Tool::SelectionBrush
                | Tool::QuickMask
        ) && !self.active.is_empty()
            && let (Some(latest), Some(predicted)) =
                (self.active.last(), self.preview_cursor_estimate)
        {
            let from = to_screen(Vec2::new(latest.position.x, latest.position.y));
            let position = Vec2::new(predicted.position.x, predicted.position.y);
            let to = to_screen(position);
            let tint = match self.tool {
                Tool::Eraser => Color32::from_gray(230),
                Tool::SelectionBrush => Color32::from_rgb(60, 160, 255),
                Tool::QuickMask => Color32::from_rgb(250, 80, 90),
                _ => self.color,
            };
            painter.line_segment(
                [from, to],
                Stroke::new(
                    1.5,
                    Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 110),
                ),
            );
            let radius = (self.size * predicted.pressure * scale * 0.5).clamp(1.0, 96.0);
            painter.circle_filled(
                to,
                radius,
                Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 45),
            );
            painter.circle_stroke(
                to,
                radius,
                Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 190),
                ),
            );
        }
        if self.symmetry_x || self.symmetry_y || self.symmetry_count > 1 {
            let center = to_screen(self.symmetry_center);
            let edge = self.doc.width.max(self.doc.height) as f32 * 1.5;
            let guide = Stroke::new(1.0, Color32::from_rgba_unmultiplied(55, 210, 230, 180));
            if self.symmetry_x {
                let a = to_screen(self.symmetry_center + Vec2::new(0.0, -edge));
                let b = to_screen(self.symmetry_center + Vec2::new(0.0, edge));
                painter.line_segment([a, b], guide);
            }
            if self.symmetry_y {
                let a = to_screen(self.symmetry_center + Vec2::new(-edge, 0.0));
                let b = to_screen(self.symmetry_center + Vec2::new(edge, 0.0));
                painter.line_segment([a, b], guide);
            }
            if self.symmetry_count > 1 {
                for i in 0..self.symmetry_count.min(32) {
                    let angle = std::f32::consts::TAU * i as f32 / self.symmetry_count as f32;
                    let direction = Vec2::new(angle.cos(), angle.sin());
                    painter.line_segment(
                        [center, to_screen(self.symmetry_center + direction * edge)],
                        guide,
                    );
                }
            }
            painter.circle_stroke(
                center,
                5.0,
                Stroke::new(1.5, Color32::from_rgb(55, 210, 230)),
            );
        }
        if self.tool == Tool::PerspectiveRuler {
            for (index, &(x, y)) in self.perspective_points.iter().enumerate() {
                let point = to_screen(Vec2::new(x as f32, y as f32));
                let selected = index == self.perspective_selected;
                let color = if selected {
                    Color32::from_rgb(255, 190, 65)
                } else {
                    Color32::from_rgb(80, 210, 235)
                };
                let stroke = Stroke::new(if selected { 2.0 } else { 1.25 }, color);
                painter.line_segment(
                    [point - Vec2::new(9.0, 0.0), point + Vec2::new(9.0, 0.0)],
                    stroke,
                );
                painter.line_segment(
                    [point - Vec2::new(0.0, 9.0), point + Vec2::new(0.0, 9.0)],
                    stroke,
                );
                painter.circle_filled(point, if selected { 4.0 } else { 3.0 }, color);
                painter.text(
                    point + Vec2::new(8.0, -10.0),
                    egui::Align2::LEFT_BOTTOM,
                    (index + 1).to_string(),
                    egui::FontId::proportional(12.0),
                    color,
                );
            }
        }
        if let Some(end) = self.gesture_end {
            let start = if self.tool == Tool::PerspectiveRuler {
                self.perspective_points
                    .get(self.perspective_selected)
                    .copied()
                    .or(self.selection_start)
            } else {
                self.selection_start
            };
            if let Some((x0, y0)) = start
                && matches!(
                    self.tool,
                    Tool::Line | Tool::EllipseRuler | Tool::PerspectiveRuler
                )
            {
                let preview = Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 220, 130, 210));
                let point_at = |t: f32| {
                    if self.tool == Tool::EllipseRuler {
                        let cx = (x0 + end.0) as f32 * 0.5;
                        let cy = (y0 + end.1) as f32 * 0.5;
                        let rx = (end.0 - x0).abs() as f32 * 0.5;
                        let ry = (end.1 - y0).abs() as f32 * 0.5;
                        let angle = std::f32::consts::TAU * t;
                        Vec2::new(cx + rx * angle.cos(), cy + ry * angle.sin())
                    } else {
                        Vec2::new(
                            x0 as f32 + (end.0 - x0) as f32 * t,
                            y0 as f32 + (end.1 - y0) as f32 * t,
                        )
                    }
                };
                let segments = if self.tool == Tool::EllipseRuler {
                    64
                } else {
                    1
                };
                let mut previous = to_screen(point_at(0.0));
                for step in 1..=segments {
                    let next = to_screen(point_at(step as f32 / segments as f32));
                    painter.line_segment([previous, next], preview);
                    previous = next;
                }
            }
        }
        if self.tool == Tool::BezierRuler && !self.bezier_points.is_empty() {
            let screen_points: Vec<_> = self
                .bezier_points
                .iter()
                .map(|point| to_screen(*point))
                .collect();
            for (index, point) in screen_points.iter().enumerate() {
                painter.circle_filled(*point, 4.0, Color32::from_rgb(255, 188, 60));
                if index > 0 {
                    painter.line_segment(
                        [screen_points[index - 1], *point],
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 188, 60, 150)),
                    );
                }
            }
            if screen_points.len() == 4 {
                let mut previous = screen_points[0];
                for step in 1..=64 {
                    let t = step as f32 / 64.0;
                    let u = 1.0 - t;
                    let next = screen_points[0].to_vec2() * (u * u * u)
                        + screen_points[1].to_vec2() * (3.0 * u * u * t)
                        + screen_points[2].to_vec2() * (3.0 * u * t * t)
                        + screen_points[3].to_vec2() * (t * t * t);
                    let next = Pos2::new(next.x, next.y);
                    painter.line_segment(
                        [previous, next],
                        Stroke::new(1.5, Color32::from_rgb(255, 220, 130)),
                    );
                    previous = next;
                }
            }
        }
        if self.tool == Tool::QuickMask && self.selection.active {
            for y in visible_y.clone().step_by(stride) {
                for x in visible_x.clone().step_by(stride) {
                    let selected = self.selection.mask[y * self.doc.width as usize + x];
                    let alpha = ((255 - selected) as f32 * 0.30).round() as u8;
                    if alpha > 0 {
                        let center = to_screen(Vec2::new(x as f32, y as f32));
                        painter.rect_filled(
                            Rect::from_center_size(
                                center,
                                Vec2::splat((stride as f32 * scale).max(1.0)),
                            ),
                            0.0,
                            Color32::from_rgba_unmultiplied(220, 30, 45, alpha),
                        );
                    }
                }
            }
        }
        let canvas_border = Stroke::new(1.0, Color32::from_gray(170));
        let corners = [
            to_screen(Vec2::ZERO),
            to_screen(Vec2::new(self.doc.width as f32, 0.0)),
            to_screen(Vec2::new(self.doc.width as f32, self.doc.height as f32)),
            to_screen(Vec2::new(0.0, self.doc.height as f32)),
        ];
        for index in 0..4 {
            painter.line_segment([corners[index], corners[(index + 1) % 4]], canvas_border);
        }
        if self.show_grid {
            let spacing = self.grid_size.max(1) as usize;
            let first_x = visible_x.start.div_ceil(spacing) * spacing;
            for x in (first_x..visible_x.end).step_by(spacing) {
                painter.line_segment(
                    [
                        to_screen(Vec2::new(x as f32, 0.0)),
                        to_screen(Vec2::new(x as f32, self.doc.height as f32)),
                    ],
                    Stroke::new(0.5, Color32::from_rgba_unmultiplied(40, 80, 120, 90)),
                );
            }
            let first_y = visible_y.start.div_ceil(spacing) * spacing;
            for y in (first_y..visible_y.end).step_by(spacing) {
                painter.line_segment(
                    [
                        to_screen(Vec2::new(0.0, y as f32)),
                        to_screen(Vec2::new(self.doc.width as f32, y as f32)),
                    ],
                    Stroke::new(0.5, Color32::from_rgba_unmultiplied(40, 80, 120, 90)),
                );
            }
        }
        if self.selection.active {
            // Marching ants: the outline in black and white dashes that
            // crawl, visible on any colour.
            let step = (1.0 / scale.max(0.02)).ceil().clamp(1.0, 64.0) as usize;
            let runs = selection_outline(
                &self.selection.mask,
                (self.doc.width as usize, self.doc.height as usize),
                (visible_x.clone(), visible_y.clone()),
                step,
            );
            let time = ctx.input(|input| input.time) as f32;
            let phase = (time * 12.0) % 8.0;
            let white = Stroke::new(1.0, Color32::WHITE);
            let black = Stroke::new(1.0, Color32::from_gray(10));
            let mut shapes = Vec::with_capacity(runs.len() * 3);
            for (a, b) in runs {
                let path = [to_screen(a), to_screen(b)];
                shapes.push(egui::Shape::line_segment(path, white));
                let along = (a.x + a.y) * scale;
                shapes.extend(egui::Shape::dashed_line_with_offset(
                    &path,
                    black,
                    &[4.0],
                    &[4.0],
                    (along + phase).rem_euclid(8.0),
                ));
            }
            painter.extend(shapes);
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        self.paint_comic_overlay(&painter, &to_screen);
        self.paint_balloon_overlay(&painter, &to_screen);
        self.paint_vector_overlay(&painter, &to_screen);
        if response.drag_started_by(egui::PointerButton::Primary)
            && self.paste_preview.is_none()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let q = self.document_position(pos, rect, scale);
            let xy = (q.x.round() as i32, q.y.round() as i32);
            self.selection_start = Some(xy);
            if matches!(
                self.tool,
                Tool::RectangleSelect | Tool::EllipseSelect | Tool::LassoSelect
            ) {
                self.begin_selection_operation(ctx.input(|input| input.modifiers));
            }
            match self.tool {
                Tool::Pan => self.pan_start = Some(self.navigator_center),
                Tool::PanelSplit => {
                    self.comic_ui.split_start = Some(q);
                    self.comic_ui.split_end = Some(q);
                }
                Tool::VectorEdit => {
                    let q = self.document_position_unsnapped(pos, rect, scale);
                    self.vector_edit_press(q, scale);
                }
                Tool::Balloon | Tool::Text => {
                    let q = self.document_position_unsnapped(pos, rect, scale);
                    let modifiers = ctx.input(|input| input.modifiers);
                    self.balloon_drag_start(q, scale, modifiers);
                }
                Tool::Brush
                | Tool::Blur
                | Tool::Smudge
                | Tool::Eraser
                | Tool::Line
                | Tool::EllipseRuler
                | Tool::PerspectiveRuler => {
                    self.reset_stroke_buffers();
                    self.begin_grain(glam::Vec2::new(q.x, q.y));
                    self.active.clear();
                    self.stabilized_cursor = None;
                    self.input_diagnostics = InputDiagnostics::default();
                    if matches!(
                        self.tool,
                        Tool::Brush | Tool::Blur | Tool::Smudge | Tool::Eraser
                    ) {
                        self.begin_brush_stroke();
                    } else {
                        self.stroke_builder = None;
                        self.provisional = None;
                    }
                    self.history.begin();
                    self.raster.previous_mix_color = [
                        self.color.r() as f32 / 255.,
                        self.color.g() as f32 / 255.,
                        self.color.b() as f32 / 255.,
                    ];
                    self.raster.last_dab = None;
                    self.raster.remaining_charge = self.brushes[self.selected_brush].mix.charge;
                    self.stroke_started_at = Some(std::time::Instant::now());
                    self.window_samples_started = false;
                }
                Tool::SelectionBrush | Tool::QuickMask => {
                    self.begin_selection_operation(ctx.input(|input| input.modifiers));
                    if self.selection_erase {
                        self.selection_combine_mode = SelectionCombineMode::Subtract;
                    }
                    self.selection.active = false;
                    self.selection.mask.clear();
                    self.stroke_builder = None;
                    self.provisional = None;
                    self.active.clear();
                    self.stabilized_cursor = None;
                    self.raster.last_dab = None;
                }
                Tool::Move => {
                    self.history.begin();
                    self.selection_before_gesture =
                        Some((self.selection.active, self.selection.mask.clone()));
                    self.begin_move(xy);
                }
                Tool::Fill => self.fill_at(xy.0.max(0) as u32, xy.1.max(0) as u32),
                Tool::Eyedropper => {
                    if let Some(sample) = self.sample_eyedropper(xy.0, xy.1) {
                        self.color = sample;
                        self.transparent_color = false;
                    }
                }
                Tool::LassoSelect => {
                    self.selection_points.clear();
                    self.selection_points.push(xy);
                }
                _ => {}
            }
        }
        // The release frame counts too, so the last stretch of a stroke
        // drawn in that frame is not lost.
        if (response.dragged_by(egui::PointerButton::Primary)
            || response.drag_started_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
            && self.paste_preview.is_none()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let q = self.document_position(pos, rect, scale);
            let xy = (q.x.round() as i32, q.y.round() as i32);
            self.gesture_end = Some(xy);
            let local = InkPoint::new(q.x, q.y, 1.0, 0).position;
            match self.tool {
                Tool::Brush
                | Tool::Blur
                | Tool::Smudge
                | Tool::Eraser
                | Tool::SelectionBrush
                | Tool::QuickMask
                | Tool::Line
                | Tool::EllipseRuler
                | Tool::PerspectiveRuler => {
                    let packets = std::mem::take(&mut self.frame_pen_packets);
                    let mut used_native = false;
                    for packet in packets {
                        let screen = Pos2::new(packet.x, packet.y);
                        if !rect.contains(screen) {
                            self.input_diagnostics.outside_canvas += 1;
                        }
                        if rect.contains(screen) {
                            self.input_diagnostics.tablet_samples += 1;
                            self.input_diagnostics.record_pressure(packet.pressure);
                            let point = self.document_position(screen, rect, scale);
                            let mut sample = InkPoint::new(
                                point.x,
                                point.y,
                                packet.pressure.clamp(0.0, 1.0),
                                packet.time_ms,
                            );
                            sample.tilt = glam::Vec2::new(packet.tilt_x, packet.tilt_y);
                            sample.rotation = packet.rotation;
                            if matches!(self.tool, Tool::Line | Tool::EllipseRuler) {
                                self.active.push(sample);
                            } else if self.tool == Tool::PerspectiveRuler {
                                self.gesture_end =
                                    Some((point.x.round() as i32, point.y.round() as i32));
                            } else {
                                self.push_live_sample(sample, Some(packet.received_at), ctx);
                            }
                            used_native = true;
                        }
                    }
                    // Once tablet packets have arrived in this stroke, a
                    // frame without packets just means none were due;
                    // a window sample here would inject full pressure.
                    if used_native {
                        self.tablet_seen = true;
                    }
                    // With a tablet in use, its packets can arrive a frame
                    // after the press: wait briefly instead of taking the
                    // window's pressure-less sample (full pressure).
                    let waiting_for_tablet = (self.use_windows_ink || self.use_wintab)
                        && self.tablet_seen
                        && self
                            .stroke_started_at
                            .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(150));
                    if !used_native
                        && self.input_diagnostics.tablet_samples == 0
                        && waiting_for_tablet
                    {
                        ctx.request_repaint();
                    } else if !used_native && self.input_diagnostics.tablet_samples == 0 {
                        // Window samples: start where the pointer went down
                        // (a slow frame can deliver the press and the first
                        // moves together).
                        if !self.window_samples_started {
                            self.window_samples_started = true;
                            if matches!(
                                self.tool,
                                Tool::Brush | Tool::Blur | Tool::Smudge | Tool::Eraser
                            ) && let Some(origin) =
                                ctx.input(|input| input.pointer.press_origin())
                                && origin.distance(pos) > 0.5
                                && rect.contains(origin)
                            {
                                let start = self.document_position(origin, rect, scale);
                                let pressure = Self::pressure_at(ctx, origin).clamp(0.0, 1.0);
                                let time_ms = ctx
                                    .input(|input| (input.time * 1000.0) as u64)
                                    .saturating_sub(1);
                                self.push_live_sample(
                                    InkPoint::new(start.x, start.y, pressure, time_ms),
                                    None,
                                    ctx,
                                );
                            }
                        }
                        self.input_diagnostics.window_fallbacks += 1;
                        let pressure = Self::pressure_at(ctx, pos).clamp(0.0, 1.0);
                        self.input_diagnostics.record_pressure(pressure);
                        let time_ms = ctx.input(|input| (input.time * 1000.0) as u64);
                        let sample = InkPoint::new(local.x, local.y, pressure, time_ms);
                        if matches!(self.tool, Tool::Line | Tool::EllipseRuler) {
                            self.active.push(sample);
                        } else if self.tool == Tool::PerspectiveRuler {
                            self.gesture_end = Some((xy.0, xy.1));
                        } else {
                            self.push_live_sample(sample, None, ctx);
                        }
                    }
                    self.flush_provisional();
                }
                Tool::RectangleSelect => {
                    if let Some(a) = self.selection_start {
                        self.selection
                            .rectangle(self.doc.width, self.doc.height, a, xy);
                    }
                }
                Tool::EllipseSelect => {
                    if let Some(a) = self.selection_start {
                        self.selection
                            .ellipse(self.doc.width, self.doc.height, a, xy);
                    }
                }
                Tool::LassoSelect
                    if self
                        .selection_points
                        .last()
                        .is_none_or(|p| (p.0 - xy.0).abs() + (p.1 - xy.1).abs() > 3) =>
                {
                    self.selection_points.push(xy);
                }
                Tool::Move => self.continue_move(xy),
                Tool::VectorEdit if !response.drag_started_by(egui::PointerButton::Primary) => {
                    let q = self.document_position_unsnapped(pos, rect, scale);
                    self.vector_edit_drag(q);
                }
                Tool::Balloon | Tool::Text => {
                    let q = self.document_position_unsnapped(pos, rect, scale);
                    self.balloon_drag(q);
                }
                Tool::PanelSplit => {
                    if let Some(start) = self.comic_ui.split_start {
                        let shift = ctx.input(|input| input.modifiers.shift);
                        self.comic_ui.split_end = Some(comic::snap_split(start, q, shift));
                    }
                }
                Tool::Pan => {
                    if let Some(center) = self.pan_start {
                        // Measured from where the drag began, so the
                        // view follows the pointer exactly, on or off
                        // the picture.
                        if let Some(origin) = ctx.input(|input| input.pointer.press_origin()) {
                            self.navigator_center =
                                center - self.screen_delta_to_document(pos - origin, scale);
                        }
                    }
                }
                _ => {}
            }
        }
        if response.drag_stopped_by(egui::PointerButton::Primary) && self.paste_preview.is_none() {
            if matches!(self.tool, Tool::Balloon | Tool::Text) {
                self.balloon_drag_stop(canvas_clicked);
            }
            if self.tool == Tool::VectorEdit {
                self.vector_edit_release();
            }
            if self.tool == Tool::PanelSplit
                && let (Some(start), Some(end)) = (
                    self.comic_ui.split_start.take(),
                    self.comic_ui.split_end.take(),
                )
                && (end - start).length() > 4.0
                && !self.split_panel(comic::to_glam(start), comic::to_glam(end))
            {
                self.status = self
                    .text(
                        "コマの上を横切るように線を引いてください",
                        "Drag a line across a panel",
                    )
                    .into();
            }
            if matches!(
                self.tool,
                Tool::Brush | Tool::Blur | Tool::Smudge | Tool::Eraser
            ) {
                self.finish_stroke()
            }
            if matches!(self.tool, Tool::SelectionBrush | Tool::QuickMask) {
                self.active.clear();
                self.stabilized_cursor = None;
                self.raster.last_dab = None;
                self.finish_selection_operation();
            }
            if matches!(
                self.tool,
                Tool::Line | Tool::EllipseRuler | Tool::PerspectiveRuler
            ) && canvas_clicked
            {
                // A click (setting a vanishing point, say) draws nothing.
                self.active.clear();
                self.history.rollback_active(&mut self.doc);
                self.rollback_vector_stroke();
                self.vector_live = None;
            } else if matches!(
                self.tool,
                Tool::Line | Tool::EllipseRuler | Tool::PerspectiveRuler
            ) {
                let start = if self.tool == Tool::PerspectiveRuler {
                    self.perspective_points
                        .get(self.perspective_selected)
                        .copied()
                        .or(self.selection_start)
                } else {
                    self.selection_start
                };
                // Only the ruler's own shape is drawn, not the pointer path.
                self.active.clear();
                if let (Some((x0, y0)), Some((x1, y1))) = (start, self.gesture_end) {
                    let n = 128usize;
                    for i in 0..=n {
                        let t = i as f32 / n as f32;
                        let (x, y) = if matches!(self.tool, Tool::Line | Tool::PerspectiveRuler) {
                            (
                                x0 as f32 + (x1 - x0) as f32 * t,
                                y0 as f32 + (y1 - y0) as f32 * t,
                            )
                        } else {
                            let cx = (x0 + x1) as f32 / 2.;
                            let cy = (y0 + y1) as f32 / 2.;
                            let rx = (x1 - x0).abs() as f32 / 2.;
                            let ry = (y1 - y0).abs() as f32 / 2.;
                            let a = std::f32::consts::TAU * t;
                            (cx + rx * a.cos(), cy + ry * a.sin())
                        };
                        let p = InkPoint::new(x, y, 1., i as u64);
                        self.active.push(p);
                        self.dab(p);
                    }
                }
                self.finish_stroke();
            }
            if matches!(self.tool, Tool::LassoSelect) {
                self.selection
                    .polygon(self.doc.width, self.doc.height, &self.selection_points);
                self.selection_points.clear();
            }
            if matches!(
                self.tool,
                Tool::RectangleSelect | Tool::EllipseSelect | Tool::LassoSelect
            ) {
                self.apply_selection_symmetry();
                self.finish_selection_operation();
            }
            if matches!(self.tool, Tool::Move) {
                self.finish_vector_move();
                self.move_origin = None;
                if let Some((before_active, before)) = self.selection_before_gesture.take() {
                    self.history.record_selection_change(
                        before_active,
                        before,
                        self.selection.active,
                        self.selection.mask.clone(),
                    );
                }
                self.history.commit();
            }
            self.selection_start = None;
            self.pan_start = None;
            self.gesture_end = None
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(pointer), Some((paste_w, paste_h, _, _))) =
                (response.interact_pointer_pos(), self.paste_preview.as_ref())
        {
            let next_position = self.canvas_position(pointer, rect, scale)
                - Vec2::new(*paste_w as f32 / 2.0, *paste_h as f32 / 2.0);
            if let Some((_, _, _, position)) = &mut self.paste_preview {
                *position = next_position;
            }
        }
        if canvas_clicked && self.paste_preview.is_some() {
            self.commit_paste_preview();
            self.canvas_texture_dirty = true;
        }
        if canvas_clicked
            && self.paste_preview.is_none()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let q = self.canvas_position(pos, rect, scale);
            if self.tool == Tool::BezierRuler {
                self.bezier_points.push(q);
                if self.bezier_points.len() == 4 {
                    let points = [
                        self.bezier_points[0],
                        self.bezier_points[1],
                        self.bezier_points[2],
                        self.bezier_points[3],
                    ];
                    self.bezier_points.clear();
                    self.draw_bezier_ruler(points);
                }
            } else if self.tool == Tool::PerspectiveRuler && self.setting_vanishing_point {
                if self.perspective_points.len() < 3 {
                    self.perspective_points
                        .push((q.x.round() as i32, q.y.round() as i32));
                }
                self.setting_vanishing_point = false;
                self.status = format!(
                    "透視点を設定しました（{}点）",
                    self.perspective_points.len()
                );
            } else if self.tool == Tool::PolygonSelect {
                let point = (q.x.round() as i32, q.y.round() as i32);
                if self.selection_points.is_empty() {
                    self.begin_selection_operation(ctx.input(|input| input.modifiers));
                }
                if self.selection_points.last().copied() != Some(point) {
                    self.selection_points.push(point);
                }
            } else if matches!(self.tool, Tool::MagicWand | Tool::ColorRange) {
                let x = q.x.round() as i32;
                let y = q.y.round() as i32;
                if x >= 0 && y >= 0 && x < self.doc.width as i32 && y < self.doc.height as i32 {
                    self.begin_selection_operation(ctx.input(|input| input.modifiers));
                    let pixels = self.selection_reference_pixels();
                    if self.tool == Tool::MagicWand {
                        self.selection.contiguous_color_pixels(
                            &pixels,
                            self.doc.width,
                            self.doc.height,
                            x as u32,
                            y as u32,
                            self.selection_tolerance,
                        );
                    } else {
                        let index = ((y as u32 * self.doc.width + x as u32) * 4) as usize;
                        let color = pixels
                            .get(index..index + 4)
                            .and_then(|rgba| rgba.try_into().ok())
                            .unwrap_or([0; 4]);
                        self.selection.color_range_pixels(
                            &pixels,
                            self.doc.width,
                            self.doc.height,
                            color,
                            self.selection_tolerance,
                        );
                    }
                    self.apply_selection_symmetry();
                    self.finish_selection_operation();
                }
            }
        }
        if response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair)
        }
    }
}

/// A textured quad from document corners (top-left, top-right,
/// bottom-right, bottom-left) already mapped to the screen. The default
/// texture id gives a plain fill.
fn canvas_quad(texture: egui::TextureId, corners: [Pos2; 4], tint: Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::with_texture(texture);
    let uvs = [
        Pos2::new(0.0, 0.0),
        Pos2::new(1.0, 0.0),
        Pos2::new(1.0, 1.0),
        Pos2::new(0.0, 1.0),
    ];
    for (corner, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: corner,
            uv: if texture == egui::TextureId::default() {
                egui::epaint::WHITE_UV
            } else {
                uv
            },
            color: tint,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    egui::Shape::mesh(mesh)
}

/// Outline of the selected area on a grid of `step` pixels within the
/// visible range, as straight runs (document coordinates).
pub(crate) fn selection_outline(
    mask: &[u8],
    (width, height): (usize, usize),
    (visible_x, visible_y): (std::ops::Range<usize>, std::ops::Range<usize>),
    step: usize,
) -> Vec<(Vec2, Vec2)> {
    let step = step.max(1);
    let on = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < width
            && (y as usize) < height
            && mask
                .get(y as usize * width + x as usize)
                .copied()
                .unwrap_or(0)
                != 0
    };
    let s = step as isize;
    let x_start = (visible_x.start / step * step) as isize;
    let y_start = (visible_y.start / step * step) as isize;
    let (x_end, y_end) = (visible_x.end as isize, visible_y.end as isize);
    let clamp_x = |x: isize| (x.min(width as isize)) as f32;
    let clamp_y = |y: isize| (y.min(height as isize)) as f32;
    let mut runs = Vec::new();
    // Horizontal edges: above and below each selected cell.
    let mut y = y_start;
    while y < y_end {
        for (line, neighbour) in [(y, y - s), (y + s, y + s)] {
            let mut open: Option<isize> = None;
            let mut x = x_start;
            loop {
                let edge = x < x_end && on(x, y) && !on(x, neighbour);
                match (edge, open) {
                    (true, None) => open = Some(x),
                    (false, Some(from)) => {
                        runs.push((
                            Vec2::new(from as f32, clamp_y(line)),
                            Vec2::new(clamp_x(x), clamp_y(line)),
                        ));
                        open = None;
                    }
                    _ => {}
                }
                if x >= x_end {
                    break;
                }
                x += s;
            }
        }
        y += s;
    }
    // Vertical edges: left and right of each selected cell.
    let mut x = x_start;
    while x < x_end {
        for (line, neighbour) in [(x, x - s), (x + s, x + s)] {
            let mut open: Option<isize> = None;
            let mut y = y_start;
            loop {
                let edge = y < y_end && on(x, y) && !on(neighbour, y);
                match (edge, open) {
                    (true, None) => open = Some(y),
                    (false, Some(from)) => {
                        runs.push((
                            Vec2::new(clamp_x(line), from as f32),
                            Vec2::new(clamp_x(line), clamp_y(y)),
                        ));
                        open = None;
                    }
                    _ => {}
                }
                if y >= y_end {
                    break;
                }
                y += s;
            }
        }
        x += s;
    }
    runs
}
