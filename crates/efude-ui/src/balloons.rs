// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Text and speech balloons in the UI. Each balloon is stored in the
//! document metadata and drawn on its own layer (balloons sharing a layer
//! merge); editing a balloon redraws its layer (see `docs/spec/comic.md`).

use super::*;
use efude_comic::balloon::{self, Balloon, BalloonShape, Tail};
use efude_comic::text::{self, FontInfo};

/// Document metadata key of the balloons.
const METADATA_KEY: &str = "balloons";

/// Fonts tried, in order, for new text.
const PREFERRED_FONTS: &[&str] = &[
    "Noto Sans CJK JP",
    "源ノ角ゴシック",
    "游ゴシック",
    "Yu Gothic",
    "メイリオ",
    "Meiryo",
    "ヒラギノ角ゴ",
    "Hiragino",
    "ＭＳ ゴシック",
    "MS Gothic",
];

/// What a drag on the canvas is doing to a balloon.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    Move { id: u64, offset: glam::Vec2 },
    Tip { id: u64, tail: usize },
    Resize { id: u64 },
    Create { start: glam::Vec2 },
}

/// Balloon tool state.
#[derive(Default)]
pub(crate) struct BalloonUi {
    pub selected: Option<u64>,
    gesture: Option<Gesture>,
    /// Where the pointer is during a gesture (document pixels).
    pointer: Option<glam::Vec2>,
    fonts: Option<Vec<FontInfo>>,
    font_filter: String,
    font_data: std::collections::HashMap<(std::path::PathBuf, u32), std::sync::Arc<Vec<u8>>>,
    text_sizes: std::collections::HashMap<String, Option<glam::Vec2>>,
    /// Settings new balloons start with.
    pub new_shape: BalloonShape,
    pub new_points: f32,
    pub new_vertical: bool,
    new_font: Option<FontInfo>,
}

impl BalloonUi {
    pub fn new() -> Self {
        Self {
            new_shape: BalloonShape::Ellipse,
            new_points: 9.0,
            new_vertical: true,
            ..Self::default()
        }
    }
}

fn to_glam(v: Vec2) -> glam::Vec2 {
    glam::Vec2::new(v.x, v.y)
}

fn to_egui(v: glam::Vec2) -> Vec2 {
    Vec2::new(v.x, v.y)
}

impl EfudeApp {
    pub(crate) fn balloons(&self) -> Vec<Balloon> {
        self.doc
            .metadata
            .get(METADATA_KEY)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default()
    }

    fn store_balloons(&mut self, balloons: &[Balloon]) {
        let text = (!balloons.is_empty())
            .then(|| serde_json::to_string(balloons).ok())
            .flatten();
        self.history.set_metadata(&mut self.doc, METADATA_KEY, text);
    }

    fn system_fonts(&mut self) -> &[FontInfo] {
        self.balloon_ui.fonts.get_or_insert_with(text::system_fonts)
    }

    /// The font new text uses: the last one chosen, else a Japanese font.
    fn default_font(&mut self) -> Option<FontInfo> {
        if let Some(font) = &self.balloon_ui.new_font {
            return Some(font.clone());
        }
        let fonts = self.system_fonts();
        let font = PREFERRED_FONTS
            .iter()
            .find_map(|name| fonts.iter().find(|f| f.name.contains(name)))
            .or(fonts.first())
            .cloned();
        self.balloon_ui.new_font = font.clone();
        font
    }

    fn font_bytes(&mut self, font: &FontInfo) -> Option<std::sync::Arc<Vec<u8>>> {
        let key = (font.path.clone(), font.index);
        if let Some(data) = self.balloon_ui.font_data.get(&key) {
            return Some(data.clone());
        }
        let data = std::sync::Arc::new(std::fs::read(&font.path).ok()?);
        self.balloon_ui.font_data.insert(key, data.clone());
        Some(data)
    }

    /// The text of `balloon` rendered with its font.
    fn balloon_text(&mut self, balloon: &Balloon) -> Option<efude_comic::Coverage> {
        if balloon.text.trim().is_empty() {
            return None;
        }
        let font = balloon.font.clone().or_else(|| self.default_font())?;
        let data = self.font_bytes(&font)?;
        text::render_with(&data, font.index, &balloon.text, &balloon.style)
    }

    /// Current shape size of `balloon` (following its text when auto-sized).
    fn balloon_size(&mut self, balloon: &Balloon) -> glam::Vec2 {
        if !balloon.auto_size {
            return balloon.shape_size(None);
        }
        let key = format!("{:?}{:?}{:?}", balloon.text, balloon.style, balloon.font);
        let text = match self.balloon_ui.text_sizes.get(&key) {
            Some(size) => *size,
            None => {
                let size = self
                    .balloon_text(balloon)
                    .map(|t| glam::Vec2::new(t.width as f32, t.height as f32));
                if self.balloon_ui.text_sizes.len() > 256 {
                    self.balloon_ui.text_sizes.clear();
                }
                self.balloon_ui.text_sizes.insert(key, size);
                size
            }
        };
        balloon.shape_size(text)
    }

    /// Redraws the layer `layer_id` from the balloons on it (inside a
    /// history action).
    fn redraw_balloon_layer(&mut self, balloons: &[Balloon], layer_id: u64) {
        let Some(index) = self.doc.layers.iter().position(|l| l.id == layer_id) else {
            return;
        };
        let group: Vec<&Balloon> = balloons.iter().filter(|b| b.layer_id == layer_id).collect();
        let texts: Vec<_> = group.iter().map(|b| self.balloon_text(b)).collect();
        let (w, h) = (self.doc.width, self.doc.height);
        let region = balloon::render_group(&group, &texts, w, h);
        self.history
            .record_all_layer_tiles(&self.doc.layers[index], w, h);
        let mut pixels = efude_canvas::TilePixels::new(w, h);
        if let Some(region) = region {
            for y in 0..region.height {
                for x in 0..region.width {
                    let i = ((y * region.width + x) * 4) as usize;
                    if region.rgba[i + 3] > 0 {
                        pixels.set_pixel(
                            region.x0 + x,
                            region.y0 + y,
                            [
                                region.rgba[i],
                                region.rgba[i + 1],
                                region.rgba[i + 2],
                                region.rgba[i + 3],
                            ],
                        );
                    }
                }
            }
        }
        // Tiles that were on screen before must be redrawn too.
        for key in self.doc.layers[index].pixels.tile_keys() {
            self.dirty_canvas_tiles.insert(key);
        }
        for key in pixels.tile_keys() {
            self.dirty_canvas_tiles.insert(key);
        }
        self.doc.layers[index].pixels = pixels;
        self.navigator_texture_dirty = true;
    }

    /// Saves `balloons` and redraws the layers in `layers`, as one undo step.
    fn commit_balloons(&mut self, balloons: Vec<Balloon>, layers: &[u64]) {
        self.history.begin();
        // Balloons whose layer was deleted go too.
        let balloons: Vec<Balloon> = balloons
            .into_iter()
            .filter(|b| self.doc.layers.iter().any(|l| l.id == b.layer_id))
            .collect();
        self.store_balloons(&balloons);
        let mut layers = layers.to_vec();
        layers.sort_unstable();
        layers.dedup();
        for layer in layers {
            self.redraw_balloon_layer(&balloons, layer);
        }
        self.history.commit();
    }

    #[cfg(test)]
    pub(crate) fn update_balloon_for_test(&mut self, balloon: Balloon) {
        self.update_balloon(balloon);
    }

    /// Replaces one balloon (matched by id) and redraws.
    fn update_balloon(&mut self, updated: Balloon) {
        let mut balloons = self.balloons();
        let mut layers = vec![updated.layer_id];
        if let Some(existing) = balloons.iter_mut().find(|b| b.id == updated.id) {
            layers.push(existing.layer_id);
            *existing = updated;
        }
        self.commit_balloons(balloons, &layers);
    }

    /// A new balloon (or plain text) centred at `center`, on its own layer
    /// at the top of the stack.
    pub(crate) fn create_balloon(
        &mut self,
        center: glam::Vec2,
        size: Option<glam::Vec2>,
        shape: BalloonShape,
    ) -> u64 {
        let font = self.default_font();
        let balloons = self.balloons();
        let id = balloons.iter().map(|b| b.id).max().unwrap_or(0) + 1;
        let layer_id = self.doc.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let english = self.language_english;
        let name = match (shape == BalloonShape::None, english) {
            (true, false) => "テキスト",
            (true, true) => "Text",
            (false, false) => "フキダシ",
            (false, true) => "Balloon",
        };
        let mut b = Balloon::new(id, layer_id, center, self.doc.dpi);
        b.shape = shape;
        b.font = font;
        b.style.size = text::points_to_pixels(self.balloon_ui.new_points, self.doc.dpi);
        b.style.vertical = self.balloon_ui.new_vertical;
        b.line_width = (self.doc.dpi / 25.4 * 0.3).max(1.0);
        if let Some(size) = size {
            b.auto_size = false;
            b.size = size;
        }
        self.history.begin();
        let layer = efude_canvas::Layer::new(layer_id, name, self.doc.width, self.doc.height);
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, layer);
        self.history.commit();
        self.selected_layer = at;
        let mut all = balloons;
        all.push(b);
        self.commit_balloons(all, &[layer_id]);
        self.balloon_ui.selected = Some(id);
        id
    }

    fn delete_balloon(&mut self, id: u64) {
        let mut balloons = self.balloons();
        let Some(position) = balloons.iter().position(|b| b.id == id) else {
            return;
        };
        let removed = balloons.remove(position);
        let layer_empty = !balloons.iter().any(|b| b.layer_id == removed.layer_id);
        self.history.begin();
        self.store_balloons(&balloons);
        if layer_empty {
            if let Some(index) = self
                .doc
                .layers
                .iter()
                .position(|l| l.id == removed.layer_id)
            {
                self.history.delete_layer(&mut self.doc.layers, index);
                self.selected_layer = self
                    .selected_layer
                    .min(self.doc.layers.len().saturating_sub(1));
            }
        } else {
            self.redraw_balloon_layer(&balloons, removed.layer_id);
        }
        self.history.commit();
        self.balloon_ui.selected = None;
        self.canvas_texture_dirty = true;
    }

    /// Puts `id` on the layer of a balloon it overlaps, so their outlines
    /// merge; with `join` false, gives it back a layer of its own.
    fn join_balloon(&mut self, id: u64, join: bool) {
        let mut balloons = self.balloons();
        let Some(index) = balloons.iter().position(|b| b.id == id) else {
            return;
        };
        let old_layer = balloons[index].layer_id;
        if join {
            let me = balloons[index].clone();
            let size = self.balloon_size(&me);
            let partner = balloons.iter().find(|other| {
                other.id != id && other.layer_id != old_layer && {
                    let their = other.shape_size(None);
                    let reach = (size + their) * 0.5;
                    (other.center - me.center).abs().cmple(reach).all()
                }
            });
            let Some(partner) = partner.map(|p| p.layer_id) else {
                self.status = self
                    .text("重なっているフキダシがありません", "No overlapping balloon")
                    .into();
                return;
            };
            balloons[index].layer_id = partner;
            let now_empty = !balloons.iter().any(|b| b.layer_id == old_layer);
            self.history.begin();
            self.store_balloons(&balloons);
            self.redraw_balloon_layer(&balloons, partner);
            if now_empty {
                if let Some(i) = self.doc.layers.iter().position(|l| l.id == old_layer) {
                    self.history.delete_layer(&mut self.doc.layers, i);
                }
            } else {
                self.redraw_balloon_layer(&balloons, old_layer);
            }
            self.history.commit();
        } else {
            let layer_id = self.doc.layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
            let name = if self.language_english {
                "Balloon"
            } else {
                "フキダシ"
            };
            balloons[index].layer_id = layer_id;
            self.history.begin();
            let layer = efude_canvas::Layer::new(layer_id, name, self.doc.width, self.doc.height);
            let at = self
                .doc
                .layers
                .iter()
                .position(|l| l.id == old_layer)
                .map_or(self.doc.layers.len(), |i| i + 1);
            self.history.insert_layer(&mut self.doc.layers, at, layer);
            self.store_balloons(&balloons);
            self.redraw_balloon_layer(&balloons, layer_id);
            self.redraw_balloon_layer(&balloons, old_layer);
            self.history.commit();
        }
        self.canvas_texture_dirty = true;
    }

    /// Topmost balloon under `point`, with its current size.
    fn balloon_at(&mut self, point: glam::Vec2) -> Option<(Balloon, glam::Vec2)> {
        let balloons = self.balloons();
        let order: Vec<u64> = self.doc.layers.iter().map(|l| l.id).collect();
        let mut hits: Vec<(usize, Balloon, glam::Vec2)> = Vec::new();
        for b in balloons {
            let size = self.balloon_size(&b);
            if balloon::hit(&b, size, point) {
                let depth = order.iter().position(|&id| id == b.layer_id).unwrap_or(0);
                hits.push((depth, b, size));
            }
        }
        hits.into_iter()
            .max_by_key(|(depth, _, _)| *depth)
            .map(|(_, b, size)| (b, size))
    }

    /// Handles a drag starting at `q` with the text or balloon tool.
    pub(crate) fn balloon_drag_start(&mut self, q: Vec2, scale: f32, modifiers: egui::Modifiers) {
        let q = to_glam(q);
        let grab = 10.0 / scale.max(0.01);
        self.balloon_ui.pointer = Some(q);
        if let Some(id) = self.balloon_ui.selected
            && let Some(selected) = self.balloons().into_iter().find(|b| b.id == id)
        {
            if let Some(tail) = selected
                .tails
                .iter()
                .position(|t| t.tip.distance(q) <= grab)
            {
                self.balloon_ui.gesture = Some(Gesture::Tip { id, tail });
                return;
            }
            let size = self.balloon_size(&selected);
            if selected.shape != BalloonShape::None
                && (selected.center + size * 0.5).distance(q) <= grab
            {
                self.balloon_ui.gesture = Some(Gesture::Resize { id });
                return;
            }
        }
        if let Some((hit, _)) = self.balloon_at(q) {
            self.balloon_ui.selected = Some(hit.id);
            if let Some(layer) = self.doc.layers.iter().position(|l| l.id == hit.layer_id) {
                self.selected_layer = layer;
            }
            if modifiers.ctrl || modifiers.command {
                // Ctrl-drag from a balloon pulls out a new tail.
                let mut b = hit.clone();
                b.tails.push(Tail {
                    tip: q,
                    width: b.style.size * 0.9,
                    bend: 0.0,
                    bubbles: b.shape == BalloonShape::Cloud,
                });
                let tail = b.tails.len() - 1;
                let id = b.id;
                self.update_balloon(b);
                self.balloon_ui.gesture = Some(Gesture::Tip { id, tail });
            } else {
                self.balloon_ui.gesture = Some(Gesture::Move {
                    id: hit.id,
                    offset: q - hit.center,
                });
            }
            return;
        }
        self.balloon_ui.selected = None;
        self.balloon_ui.gesture = Some(Gesture::Create { start: q });
    }

    pub(crate) fn balloon_drag(&mut self, q: Vec2) {
        self.balloon_ui.pointer = Some(to_glam(q));
    }

    /// Finishes the gesture at the last pointer position.
    pub(crate) fn balloon_drag_stop(&mut self, clicked: bool) {
        let (Some(gesture), Some(q)) = (
            self.balloon_ui.gesture.take(),
            self.balloon_ui.pointer.take(),
        ) else {
            return;
        };
        let find = |app: &Self, id: u64| app.balloons().into_iter().find(|b| b.id == id);
        match gesture {
            Gesture::Move { id, offset } => {
                if let Some(mut b) = find(self, id)
                    && !clicked
                {
                    let delta = (q - offset) - b.center;
                    b.center += delta;
                    // Tails keep pointing at the speaker.
                    self.update_balloon(b);
                }
            }
            Gesture::Tip { id, tail } => {
                if let Some(mut b) = find(self, id)
                    && let Some(t) = b.tails.get_mut(tail)
                {
                    t.tip = q;
                    self.update_balloon(b);
                }
            }
            Gesture::Resize { id } => {
                if let Some(mut b) = find(self, id) {
                    b.auto_size = false;
                    b.size = ((q - b.center).abs() * 2.0).max(glam::Vec2::splat(8.0));
                    self.update_balloon(b);
                }
            }
            Gesture::Create { start } => {
                let shape = if self.tool == Tool::Text {
                    BalloonShape::None
                } else {
                    self.balloon_ui.new_shape
                };
                let size = (q - start).abs();
                if clicked || size.x < 16.0 || size.y < 16.0 {
                    self.create_balloon(start, None, shape);
                } else {
                    self.create_balloon((start + q) * 0.5, Some(size), shape);
                }
            }
        }
        self.canvas_texture_dirty = true;
    }

    /// Selection handles and the outline of a gesture in progress.
    pub(crate) fn paint_balloon_overlay(
        &mut self,
        painter: &egui::Painter,
        to_screen: &dyn Fn(Vec2) -> Pos2,
    ) {
        if !matches!(self.tool, Tool::Text | Tool::Balloon) {
            return;
        }
        let accent = layout::ACCENT;
        let dashed = |painter: &egui::Painter, points: &[Pos2]| {
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                painter.extend(egui::Shape::dashed_line(
                    &[a, b],
                    Stroke::new(1.5, accent),
                    6.0,
                    4.0,
                ));
            }
        };
        if let Some(Gesture::Create { start }) = self.balloon_ui.gesture
            && let Some(q) = self.balloon_ui.pointer
        {
            let (min, max) = (start.min(q), start.max(q));
            dashed(
                painter,
                &[
                    to_screen(to_egui(min)),
                    to_screen(Vec2::new(max.x, min.y)),
                    to_screen(to_egui(max)),
                    to_screen(Vec2::new(min.x, max.y)),
                ],
            );
        }
        let Some(id) = self.balloon_ui.selected else {
            return;
        };
        let Some(mut b) = self.balloons().into_iter().find(|b| b.id == id) else {
            self.balloon_ui.selected = None;
            return;
        };
        // Show the gesture's result live.
        if let (Some(gesture), Some(q)) = (self.balloon_ui.gesture, self.balloon_ui.pointer) {
            match gesture {
                Gesture::Move { offset, .. } => b.center = q - offset,
                Gesture::Tip { tail, .. } => {
                    if let Some(t) = b.tails.get_mut(tail) {
                        t.tip = q;
                    }
                }
                Gesture::Resize { .. } => {
                    b.auto_size = false;
                    b.size = (q - b.center).abs() * 2.0;
                }
                Gesture::Create { .. } => {}
            }
        }
        let size = self.balloon_size(&b);
        let (min, max) = (b.center - size * 0.5, b.center + size * 0.5);
        dashed(
            painter,
            &[
                to_screen(to_egui(min)),
                to_screen(Vec2::new(max.x, min.y)),
                to_screen(to_egui(max)),
                to_screen(Vec2::new(min.x, max.y)),
            ],
        );
        if b.shape != BalloonShape::None {
            painter.rect_filled(
                Rect::from_center_size(to_screen(to_egui(max)), Vec2::splat(9.0)),
                2.0,
                accent,
            );
        }
        for tail in &b.tails {
            let tip = to_screen(to_egui(tail.tip));
            painter.line_segment(
                [to_screen(to_egui(b.center)), tip],
                Stroke::new(1.0, accent),
            );
            painter.circle(tip, 5.5, Color32::WHITE, Stroke::new(2.0, accent));
        }
    }

    /// Editor of the selected balloon.
    pub(crate) fn balloon_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.balloon_ui.selected else {
            return;
        };
        let Some(original) = self.balloons().into_iter().find(|b| b.id == id) else {
            self.balloon_ui.selected = None;
            return;
        };
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let dpi = self.doc.dpi;
        let mut b = original.clone();
        let mut open = true;
        let mut delete = false;
        let mut join = None;
        if self.balloon_ui.fonts.is_none() {
            self.system_fonts();
        }
        let fonts = self.balloon_ui.fonts.clone().unwrap_or_default();
        let mut filter = std::mem::take(&mut self.balloon_ui.font_filter);
        egui::Window::new(if b.shape == BalloonShape::None {
            t("テキスト", "Text")
        } else {
            t("フキダシ", "Balloon")
        })
        .id(egui::Id::new("balloon-editor"))
        .open(&mut open)
        .collapsible(true)
        .resizable(true)
        .default_width(320.0)
        .default_pos(Pos2::new(ctx.screen_rect().right() - 700.0, 90.0))
        .show(ctx, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut b.text)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text(t("セリフを入力（改行で次の行）", "Type the text (Enter for a new line)")),
            );
            ui.horizontal(|ui| {
                ui.selectable_value(&mut b.style.vertical, true, t("縦書き", "Vertical"));
                ui.selectable_value(&mut b.style.vertical, false, t("横書き", "Horizontal"));
            });
            let current = b.font.as_ref().map_or("-".to_string(), |f| f.name.clone());
            egui::CollapsingHeader::new(format!("{}: {current}", t("フォント", "Font")))
                .id_salt("balloon-font")
                .show(ui, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut filter).hint_text(t("絞り込み", "Filter")));
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        let needle = filter.to_lowercase();
                        for font in fonts.iter().filter(|f| needle.is_empty() || f.name.to_lowercase().contains(&needle)).take(400) {
                            let selected = b.font.as_ref().is_some_and(|f| f.name == font.name);
                            if ui.selectable_label(selected, &font.name).clicked() {
                                b.font = Some(font.clone());
                            }
                        }
                    });
                });
            egui::Grid::new("balloon-grid").num_columns(2).show(ui, |ui| {
                ui.label(t("文字サイズ", "Text size"));
                let mut points = b.style.size / dpi * 72.0;
                if ui.add(egui::DragValue::new(&mut points).range(3.0..=200.0).speed(0.1).suffix(" pt")).changed() {
                    b.style.size = text::points_to_pixels(points, dpi);
                }
                ui.end_row();
                ui.label(t("行間", "Line spacing"));
                ui.add(egui::DragValue::new(&mut b.style.line_spacing).range(0.8..=3.0).speed(0.01));
                ui.end_row();
                ui.label(t("字間", "Letter spacing"));
                ui.add(egui::DragValue::new(&mut b.style.letter_spacing).range(-0.3..=1.0).speed(0.01));
                ui.end_row();
                ui.label(t("文字色", "Text colour"));
                let mut c = Color32::from_rgb(b.text_color[0], b.text_color[1], b.text_color[2]);
                ui.color_edit_button_srgba(&mut c);
                b.text_color = [c.r(), c.g(), c.b()];
                ui.end_row();
                ui.label(t("形", "Shape"));
                let name = |shape: BalloonShape| match (shape, english) {
                    (BalloonShape::None, false) => "なし（文字だけ）",
                    (BalloonShape::None, true) => "None (text only)",
                    (BalloonShape::Ellipse, false) => "楕円",
                    (BalloonShape::Ellipse, true) => "Ellipse",
                    (BalloonShape::RoundedRect, false) => "角丸",
                    (BalloonShape::RoundedRect, true) => "Rounded box",
                    (BalloonShape::Cloud, false) => "雲（考え）",
                    (BalloonShape::Cloud, true) => "Cloud (thought)",
                    (BalloonShape::Flash, false) => "トゲ（叫び）",
                    (BalloonShape::Flash, true) => "Spiky (shout)",
                };
                egui::ComboBox::from_id_salt("balloon-shape")
                    .selected_text(name(b.shape))
                    .show_ui(ui, |ui| {
                        for shape in BalloonShape::ALL {
                            ui.selectable_value(&mut b.shape, shape, name(shape));
                        }
                    });
                ui.end_row();
                if b.shape != BalloonShape::None {
                    ui.label(t("線の太さ", "Line width"));
                    let mut mm = b.line_width / dpi * 25.4;
                    if ui.add(egui::DragValue::new(&mut mm).range(0.05..=5.0).speed(0.01).suffix(" mm")).changed() {
                        b.line_width = mm / 25.4 * dpi;
                    }
                    ui.end_row();
                    ui.label(t("線の色", "Line colour"));
                    let mut c = Color32::from_rgb(b.line_color[0], b.line_color[1], b.line_color[2]);
                    ui.color_edit_button_srgba(&mut c);
                    b.line_color = [c.r(), c.g(), c.b()];
                    ui.end_row();
                    ui.label(t("塗り", "Fill"));
                    let mut c = Color32::from_rgba_unmultiplied(b.fill[0], b.fill[1], b.fill[2], b.fill[3]);
                    ui.color_edit_button_srgba(&mut c);
                    b.fill = c.to_srgba_unmultiplied();
                    ui.end_row();
                    ui.label(t("大きさ", "Size"));
                    ui.checkbox(&mut b.auto_size, t("文字に合わせる", "Fit the text"));
                    ui.end_row();
                }
            });
            if b.shape != BalloonShape::None {
                ui.separator();
                ui.label(egui::RichText::new(t("しっぽ", "Tails")).strong());
                let mut remove = None;
                for (i, tail) in b.tails.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(format!("{}", i + 1));
                        ui.add(egui::DragValue::new(&mut tail.width).range(1.0..=2000.0).prefix(t("幅 ", "width ")));
                        ui.add(egui::DragValue::new(&mut tail.bend).range(-1.0..=1.0).speed(0.01).prefix(t("曲がり ", "bend ")));
                        ui.checkbox(&mut tail.bubbles, t("泡", "Bubbles"));
                        if ui.small_button("×").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    b.tails.remove(i);
                }
                if ui.button(t("しっぽを追加", "Add Tail")).clicked() {
                    let size = b.shape_size(None);
                    b.tails.push(Tail {
                        tip: b.center + glam::Vec2::new(-size.x * 0.3, size.y * 0.9),
                        width: b.style.size * 0.9,
                        bend: 0.2,
                        bubbles: b.shape == BalloonShape::Cloud,
                    });
                }
                ui.label(
                    egui::RichText::new(t(
                        "しっぽの先の○をドラッグで動かせます。Ctrl+ドラッグでフキダシから新しいしっぽを引き出せます。",
                        "Drag a tail's circle to move its tip. Ctrl-drag from a balloon pulls out a new tail.",
                    ))
                    .small()
                    .color(layout::MUTED_TEXT),
                );
                ui.horizontal(|ui| {
                    if ui.button(t("重なるフキダシとつなげる", "Join Overlapping Balloon")).clicked() {
                        join = Some(true);
                    }
                    if ui.button(t("切り離す", "Separate")).clicked() {
                        join = Some(false);
                    }
                });
            }
            ui.separator();
            if ui.button(t("削除", "Delete")).clicked() {
                delete = true;
            }
        });
        self.balloon_ui.font_filter = filter;
        if b.font != original.font {
            self.balloon_ui.new_font = b.font.clone();
        }
        if b.style != original.style {
            self.balloon_ui.new_points = b.style.size / dpi * 72.0;
            self.balloon_ui.new_vertical = b.style.vertical;
        }
        if b.shape != original.shape && b.shape != BalloonShape::None {
            self.balloon_ui.new_shape = b.shape;
        }
        if delete {
            self.delete_balloon(id);
        } else if let Some(join) = join {
            self.join_balloon(id, join);
        } else if b != original {
            self.update_balloon(b);
        }
        if !open {
            self.balloon_ui.selected = None;
        }
    }
}
