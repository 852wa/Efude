// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Comic features in the UI: page setup and guides, panels, tones and
//! effect lines (see `docs/spec/comic.md`). The geometry lives in the
//! `efude-comic` engine crate; this module turns it into layers.

use super::*;
use efude_canvas::{DotShape, TilePixels, ToneSettings};
use efude_comic::{
    Coverage, FocusLines, PageSpec, Panel, PanelLayout, SpeedLines, panel as panels, raster,
};

/// Document metadata key of the comic data.
const METADATA_KEY: &str = "comic";

/// Comic data stored with the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ComicDoc {
    pub page: PageSpec,
    pub layout: PanelLayout,
}

/// Which effect-line generator the dialog shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectKind {
    Focus,
    Speed,
}

/// State of the comic dialogs.
pub(crate) struct ComicUi {
    pub show_guides: bool,
    pub setup_open: bool,
    pub setup: PageSpec,
    pub preset: usize,
    pub panel_settings_open: bool,
    pub grid_open: bool,
    pub grid: (u32, u32),
    pub tone_open: bool,
    pub tone: ToneSettings,
    pub tone_density: f32,
    pub effect: Option<EffectKind>,
    pub focus: FocusLines,
    pub speed: SpeedLines,
    /// Layer the effect dialog last drew on (regenerated in place).
    pub effect_layer: Option<u64>,
    /// Start of the split line being dragged (document pixels).
    pub split_start: Option<Vec2>,
    pub split_end: Option<Vec2>,
}

impl Default for ComicUi {
    fn default() -> Self {
        let region = [glam::Vec2::ZERO, glam::Vec2::new(1000.0, 1000.0)];
        Self {
            show_guides: true,
            setup_open: false,
            setup: PageSpec::presets()[0].2.clone(),
            preset: 0,
            panel_settings_open: false,
            grid_open: false,
            grid: (2, 3),
            tone_open: false,
            tone: ToneSettings::default(),
            tone_density: 0.3,
            effect: None,
            focus: FocusLines::new(region),
            speed: SpeedLines::new(region),
            effect_layer: None,
            split_start: None,
            split_end: None,
        }
    }
}

pub(crate) fn to_glam(v: Vec2) -> glam::Vec2 {
    glam::Vec2::new(v.x, v.y)
}

/// End of a split line: nearly level or upright lines snap straight, and
/// with Shift every line snaps to 45° steps.
pub(crate) fn snap_split(start: Vec2, end: Vec2, shift: bool) -> Vec2 {
    let d = end - start;
    let length = d.length();
    if length < 1.0 {
        return end;
    }
    let angle = d.y.atan2(d.x);
    let step = std::f32::consts::FRAC_PI_4;
    let snapped = (angle / step).round() * step;
    let straight = (angle / (2.0 * step)).round() * 2.0 * step;
    let target = if shift {
        snapped
    } else if (angle - straight).abs() < 3f32.to_radians() {
        straight
    } else {
        return end;
    };
    start + Vec2::new(target.cos(), target.sin()) * length
}

impl EfudeApp {
    pub(crate) fn comic_doc(&self) -> Option<ComicDoc> {
        self.doc
            .metadata
            .get(METADATA_KEY)
            .and_then(|text| serde_json::from_str(text).ok())
    }

    /// Stores comic data in the document (undoably; call inside a history
    /// action or on its own).
    fn store_comic(&mut self, comic: &ComicDoc) {
        let text = serde_json::to_string(comic).ok();
        self.history.set_metadata(&mut self.doc, METADATA_KEY, text);
    }

    fn next_layer_id(&self) -> u64 {
        self.doc
            .layers
            .iter()
            .map(|layer| layer.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    fn layer_index(&self, id: u64) -> Option<usize> {
        self.doc.layers.iter().position(|layer| layer.id == id)
    }

    /// New document laid out as a manga page.
    pub(crate) fn new_comic_document(&mut self, spec: &PageSpec) -> bool {
        let geometry = spec.geometry();
        if !efude_canvas::valid_document_dimensions(geometry.canvas_width, geometry.canvas_height) {
            self.status = self
                .text("原稿が大きすぎます", "The page is too large")
                .into();
            return false;
        }
        self.canvas_width_input = geometry.canvas_width;
        self.canvas_height_input = geometry.canvas_height;
        self.canvas_dpi_input = spec.dpi;
        if !self.new_document() {
            return false;
        }
        let comic = ComicDoc {
            page: spec.clone(),
            layout: PanelLayout::for_dpi(spec.dpi),
        };
        self.doc.metadata.insert(
            METADATA_KEY.into(),
            serde_json::to_string(&comic).unwrap_or_default(),
        );
        self.status = self
            .text("漫画原稿を作成しました", "Manga page created")
            .into();
        true
    }

    // ---- Panels ---------------------------------------------------------

    /// Mask that shows only the inside of `polygon`.
    fn panel_mask(&self, polygon: &[glam::Vec2]) -> TilePixels {
        let (w, h) = (self.doc.width, self.doc.height);
        let coverage = raster::fill_polygon(polygon, w, h);
        tiles_from_coverage(w, h, coverage.as_ref(), [0, 0, 0, 255], |c| [c, c, c, 255])
    }

    /// Writes `coverage` of `color` into layer `index` (replacing it),
    /// recording history.
    fn write_coverage(
        &mut self,
        index: usize,
        coverage: Option<&Coverage>,
        color: [u8; 3],
        clear: bool,
    ) {
        let (w, h) = (self.doc.width, self.doc.height);
        if clear {
            self.history
                .record_all_layer_tiles(&self.doc.layers[index], w, h);
            self.doc.layers[index].pixels = TilePixels::new(w, h);
        }
        let Some(coverage) = coverage else {
            return;
        };
        // Record every tile the coverage touches before writing.
        let size = efude_canvas::TILE_SIZE;
        for ty in coverage.y0 / size..=(coverage.y0 + coverage.height - 1) / size {
            for tx in coverage.x0 / size..=(coverage.x0 + coverage.width - 1) / size {
                let (x, y) = ((tx * size).min(w - 1), (ty * size).min(h - 1));
                self.history
                    .record_pixel(&self.doc.layers[index], ((y * w + x) * 4) as usize);
            }
        }
        let layer = &mut self.doc.layers[index];
        for (x, y, c) in coverage.iter() {
            let old = layer.pixels.pixel(x, y);
            // Keep the stronger of old and new (lines over lines).
            if c >= old[3] {
                layer
                    .pixels
                    .set_pixel(x, y, [color[0], color[1], color[2], c]);
            }
        }
    }

    /// Adds a panel folder for `polygon` with its border and a drawing layer.
    fn add_panel(&mut self, layout: &mut PanelLayout, polygon: Vec<glam::Vec2>) -> Panel {
        let english = self.language_english;
        let number = layout.panels.len() + 1;
        let (w, h) = (self.doc.width, self.doc.height);
        let folder_id = self.next_layer_id();
        let mut folder = efude_canvas::Layer::new(
            folder_id,
            if english {
                format!("Panel {number}")
            } else {
                format!("コマ {number}")
            },
            w,
            h,
        );
        folder.kind = LayerKind::Folder;
        folder.mask = Some(self.panel_mask(&polygon));
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, folder);
        let drawing_id = self.next_layer_id();
        let mut drawing =
            efude_canvas::Layer::new(drawing_id, if english { "Drawing" } else { "作画" }, w, h);
        drawing.parent_id = Some(folder_id);
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, drawing);
        let border_id = self.next_layer_id();
        let mut border =
            efude_canvas::Layer::new(border_id, if english { "Border" } else { "枠線" }, w, h);
        border.parent_id = Some(folder_id);
        border.locked = true;
        if let Some(coverage) = raster::inner_border(&polygon, layout.border_width, w, h) {
            for (x, y, c) in coverage.iter() {
                border.pixels.set_pixel(x, y, [0, 0, 0, c]);
            }
        }
        let at = self.doc.layers.len();
        self.history.insert_layer(&mut self.doc.layers, at, border);
        self.selected_layer = self.layer_index(drawing_id).unwrap_or(self.selected_layer);
        let panel = Panel {
            folder_id,
            border_id,
            polygon,
        };
        layout.panels.push(panel.clone());
        panel
    }

    /// Redraws a panel's mask and border after its shape changed.
    fn update_panel(&mut self, panel: &Panel, border_width: f32) {
        let Some(folder) = self.layer_index(panel.folder_id) else {
            return;
        };
        let mask = self.panel_mask(&panel.polygon);
        self.history
            .replace_layer_mask(&mut self.doc.layers, folder, Some(mask));
        if let Some(border) = self.layer_index(panel.border_id) {
            let coverage = raster::inner_border(
                &panel.polygon,
                border_width,
                self.doc.width,
                self.doc.height,
            );
            self.write_coverage(border, coverage.as_ref(), [0, 0, 0], true);
        }
    }

    /// One panel over the inner frame (or `region`).
    pub(crate) fn create_panel(&mut self, region: Option<[glam::Vec2; 2]>) {
        let Some(mut comic) = self.comic_doc() else {
            self.status = self
                .text(
                    "先に「漫画 → 原稿の設定」で漫画原稿を作ってください",
                    "Create a manga page first (Manga → Page Setup)",
                )
                .into();
            return;
        };
        let [min, max] = region.unwrap_or(comic.page.geometry().inner);
        self.history.begin();
        self.add_panel(&mut comic.layout, panels::rectangle(min, max));
        self.store_comic(&comic);
        self.history.commit();
        self.canvas_texture_dirty = true;
    }

    /// Splits the panel under the middle of `a`→`b` along that line.
    pub(crate) fn split_panel(&mut self, a: glam::Vec2, b: glam::Vec2) -> bool {
        let Some(mut comic) = self.comic_doc() else {
            return false;
        };
        let middle = (a + b) * 0.5;
        let Some(index) = comic
            .layout
            .panel_at(middle)
            .or_else(|| comic.layout.panel_at(a))
        else {
            return false;
        };
        let gutter = comic.layout.gutter_for(a, b);
        let min_area = (comic.layout.border_width * 4.0).powi(2);
        let Some([mut first, mut second]) =
            panels::split(&comic.layout.panels[index].polygon, a, b, gutter, min_area)
        else {
            return false;
        };
        // The panel keeps the part read first: the upper one, or across a
        // vertical cut the one on the binding's reading start.
        let centre =
            |p: &[glam::Vec2]| p.iter().fold(glam::Vec2::ZERO, |s, &v| s + v) / p.len() as f32;
        let (c1, c2) = (centre(&first), centre(&second));
        let d = b - a;
        let second_first = if d.x.abs() >= d.y.abs() {
            c2.y < c1.y
        } else if comic.page.binding == efude_comic::Binding::Right {
            c2.x > c1.x
        } else {
            c2.x < c1.x
        };
        if second_first {
            std::mem::swap(&mut first, &mut second);
        }
        self.history.begin();
        let mut panel = comic.layout.panels[index].clone();
        panel.polygon = first;
        comic.layout.panels[index] = panel.clone();
        self.update_panel(&panel, comic.layout.border_width);
        self.add_panel(&mut comic.layout, second);
        self.store_comic(&comic);
        self.history.commit();
        self.canvas_texture_dirty = true;
        self.status = self.text("コマを分割しました", "Panel split").into();
        true
    }

    /// Index of the panel that holds the selected layer (or is it).
    pub(crate) fn current_panel(&self, comic: &ComicDoc) -> Option<usize> {
        let mut id = self
            .doc
            .layers
            .get(self.selected_layer)
            .map(|layer| layer.id);
        let mut guard = self.doc.layers.len();
        while let Some(current) = id {
            if let Some(index) = comic
                .layout
                .panels
                .iter()
                .position(|panel| panel.folder_id == current)
            {
                return Some(index);
            }
            id = self
                .layer_index(current)
                .and_then(|index| self.doc.layers[index].parent_id);
            guard = guard.checked_sub(1)?;
        }
        None
    }

    /// Splits the current panel into a grid of columns × rows.
    pub(crate) fn grid_split_panel(&mut self, columns: u32, rows: u32) -> bool {
        let Some(mut comic) = self.comic_doc() else {
            return false;
        };
        let Some(index) = self
            .current_panel(&comic)
            .or((comic.layout.panels.len() == 1).then_some(0))
        else {
            self.status = self
                .text(
                    "分割するコマのレイヤーを選んでください",
                    "Select a layer of the panel to split",
                )
                .into();
            return false;
        };
        let right_to_left = comic.page.binding == efude_comic::Binding::Right;
        let cells = panels::grid(
            &comic.layout.panels[index].polygon,
            columns,
            rows,
            comic.layout.gutter_horizontal,
            comic.layout.gutter_vertical,
            right_to_left,
        );
        if cells.len() < 2 {
            return false;
        }
        self.history.begin();
        let mut cells = cells.into_iter();
        let mut panel = comic.layout.panels[index].clone();
        panel.polygon = cells.next().unwrap();
        comic.layout.panels[index] = panel.clone();
        self.update_panel(&panel, comic.layout.border_width);
        for cell in cells {
            self.add_panel(&mut comic.layout, cell);
        }
        self.store_comic(&comic);
        self.history.commit();
        self.canvas_texture_dirty = true;
        true
    }

    /// Applies new border width and gutters (borders are redrawn).
    fn apply_panel_settings(&mut self, border_mm: f32, gutter_h_mm: f32, gutter_v_mm: f32) {
        let Some(mut comic) = self.comic_doc() else {
            return;
        };
        let px = |mm: f32| mm / 25.4 * comic.page.dpi;
        comic.layout.border_width = px(border_mm).max(0.5);
        comic.layout.gutter_horizontal = px(gutter_h_mm).max(0.0);
        comic.layout.gutter_vertical = px(gutter_v_mm).max(0.0);
        self.history.begin();
        for panel in comic.layout.panels.clone() {
            if let Some(border) = self.layer_index(panel.border_id) {
                let coverage = raster::inner_border(
                    &panel.polygon,
                    comic.layout.border_width,
                    self.doc.width,
                    self.doc.height,
                );
                self.write_coverage(border, coverage.as_ref(), [0, 0, 0], true);
            }
        }
        self.store_comic(&comic);
        self.history.commit();
        self.canvas_texture_dirty = true;
    }

    // ---- Tones ----------------------------------------------------------

    /// A new tone layer above the selected layer, filled at `density`
    /// inside the selection, else the current panel, else the whole page.
    pub(crate) fn tone_fill(&mut self, settings: ToneSettings, density: f32) {
        let (w, h) = (self.doc.width, self.doc.height);
        let english = self.language_english;
        let parent = self.doc.layers.get(self.selected_layer).and_then(|layer| {
            if layer.kind == LayerKind::Folder {
                Some(layer.id)
            } else {
                layer.parent_id
            }
        });
        let id = self.next_layer_id();
        let mut layer = efude_canvas::Layer::new(id, if english { "Tone" } else { "トーン" }, w, h);
        layer.parent_id = parent;
        layer.tone = Some(settings);
        let alpha = |coverage: u8| ((coverage as f32 / 255.0) * density * 255.0).round() as u8;
        if self.selection.active {
            for (i, &value) in self.selection.mask.iter().enumerate() {
                if value > 0 {
                    let (x, y) = (i as u32 % w, i as u32 / w);
                    layer.pixels.set_pixel(x, y, [0, 0, 0, alpha(value)]);
                }
            }
        } else if let Some(polygon) = self.comic_doc().and_then(|comic| {
            self.current_panel(&comic)
                .map(|i| comic.layout.panels[i].polygon.clone())
        }) {
            let coverage = raster::fill_polygon(&polygon, w, h);
            layer.pixels =
                tiles_from_coverage(w, h, coverage.as_ref(), [0; 4], |c| [0, 0, 0, alpha(c)]);
        } else {
            layer.pixels.fill_shared([0, 0, 0, alpha(255)]);
        }
        self.history.begin();
        let at = (self.selected_layer + 1).min(self.doc.layers.len());
        self.history.insert_layer(&mut self.doc.layers, at, layer);
        self.history.commit();
        self.selected_layer = at;
        self.canvas_texture_dirty = true;
    }

    /// Turns the selected layer into a tone layer (or back).
    pub(crate) fn set_layer_tone(&mut self, tone: Option<ToneSettings>) {
        let Some(layer) = self.doc.layers.get_mut(self.selected_layer) else {
            return;
        };
        if layer.kind != LayerKind::Raster {
            return;
        }
        let before = layer.property_state();
        layer.tone = tone;
        let after = layer.property_state();
        let id = layer.id;
        self.history.record_layer_properties(id, before, after);
        self.canvas_texture_dirty = true;
    }

    // ---- Effect lines ---------------------------------------------------

    /// Region effect lines fill: the selection, the current panel, the
    /// inner frame, or the canvas.
    pub(crate) fn effect_region(&self) -> [glam::Vec2; 2] {
        let (w, h) = (self.doc.width, self.doc.height);
        if self.selection.active {
            let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
            for (i, &value) in self.selection.mask.iter().enumerate() {
                if value > 0 {
                    let p = glam::Vec2::new((i as u32 % w) as f32, (i as u32 / w) as f32);
                    min = min.min(p);
                    max = max.max(p + glam::Vec2::ONE);
                }
            }
            if min.x < max.x {
                return [min, max];
            }
        }
        if let Some(comic) = self.comic_doc() {
            if let Some(index) = self.current_panel(&comic) {
                let polygon = &comic.layout.panels[index].polygon;
                let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
                for &p in polygon {
                    min = min.min(p);
                    max = max.max(p);
                }
                return [min, max];
            }
            return comic.page.geometry().inner;
        }
        [glam::Vec2::ZERO, glam::Vec2::new(w as f32, h as f32)]
    }

    /// Draws the effect lines onto their layer (a new one the first time).
    pub(crate) fn draw_effect_lines(&mut self, kind: EffectKind) {
        let (w, h) = (self.doc.width, self.doc.height);
        let coverage = match kind {
            EffectKind::Focus => self.comic_ui.focus.render(w, h),
            EffectKind::Speed => self.comic_ui.speed.render(w, h),
        };
        let color = [self.color.r(), self.color.g(), self.color.b()];
        self.history.begin();
        let index = match self
            .comic_ui
            .effect_layer
            .and_then(|id| self.layer_index(id))
        {
            Some(index) => index,
            None => {
                let parent = self.doc.layers.get(self.selected_layer).and_then(|layer| {
                    if layer.kind == LayerKind::Folder {
                        Some(layer.id)
                    } else {
                        layer.parent_id
                    }
                });
                let id = self.next_layer_id();
                let name = match (kind, self.language_english) {
                    (EffectKind::Focus, false) => "集中線",
                    (EffectKind::Focus, true) => "Focus Lines",
                    (EffectKind::Speed, false) => "流線",
                    (EffectKind::Speed, true) => "Speed Lines",
                };
                let mut layer = efude_canvas::Layer::new(id, name, w, h);
                layer.parent_id = parent;
                let at = (self.selected_layer + 1).min(self.doc.layers.len());
                self.history.insert_layer(&mut self.doc.layers, at, layer);
                self.comic_ui.effect_layer = Some(id);
                self.selected_layer = at;
                at
            }
        };
        self.write_coverage(index, coverage.as_ref(), color, true);
        self.history.commit();
        self.canvas_texture_dirty = true;
    }

    // ---- Canvas overlay -------------------------------------------------

    /// Page guides and the split line, drawn over the canvas.
    pub(crate) fn paint_comic_overlay(
        &self,
        painter: &egui::Painter,
        to_screen: &dyn Fn(Vec2) -> Pos2,
    ) {
        let Some(comic) = self.comic_doc() else {
            return;
        };
        if self.comic_ui.show_guides {
            let g = comic.page.geometry();
            let rect_line = |[min, max]: [glam::Vec2; 2], stroke: Stroke| {
                let corners = [
                    Vec2::new(min.x, min.y),
                    Vec2::new(max.x, min.y),
                    Vec2::new(max.x, max.y),
                    Vec2::new(min.x, max.y),
                ];
                painter.add(egui::Shape::closed_line(
                    corners.iter().map(|&c| to_screen(c)).collect(),
                    stroke,
                ));
            };
            rect_line(g.bleed, Stroke::new(1.0, Color32::from_rgb(230, 90, 90)));
            rect_line(g.trim, Stroke::new(1.0, Color32::from_rgb(60, 120, 230)));
            // Faint: the inner frame often lies under a border.
            rect_line(
                g.inner,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(40, 190, 200, 110)),
            );
            for [a, b] in g.crop_marks() {
                painter.line_segment(
                    [
                        to_screen(Vec2::new(a.x, a.y)),
                        to_screen(Vec2::new(b.x, b.y)),
                    ],
                    Stroke::new(1.0, Color32::from_gray(200)),
                );
            }
        }
        if let (Some(a), Some(b)) = (self.comic_ui.split_start, self.comic_ui.split_end) {
            painter.line_segment(
                [to_screen(a), to_screen(b)],
                Stroke::new(2.0, layout::ACCENT),
            );
        }
    }

    // ---- Menu and dialogs -----------------------------------------------

    pub(crate) fn comic_menu(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        ui.set_min_width(230.0);
        if ui.button(t("原稿の設定…", "Page Setup…")).clicked() {
            self.comic_ui.setup_open = true;
            ui.close_menu();
        }
        ui.checkbox(
            &mut self.comic_ui.show_guides,
            t("ガイドを表示", "Show Guides"),
        );
        if ui
            .button(t("作品（複数ページ）…", "Book (Pages)…"))
            .clicked()
        {
            self.book_ui.open = true;
            ui.close_menu();
        }
        ui.separator();
        let has_comic = self.comic_doc().is_some();
        if ui
            .add_enabled(
                has_comic,
                egui::Button::new(t("コマ枠を作成（基本枠）", "New Panel (Inner Frame)")),
            )
            .clicked()
        {
            self.create_panel(None);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                has_comic && self.selection.active,
                egui::Button::new(t("コマ枠を作成（選択範囲）", "New Panel (Selection)")),
            )
            .clicked()
        {
            let region = self.effect_region();
            self.create_panel(Some(region));
            ui.close_menu();
        }
        if ui
            .add_enabled(
                has_comic,
                egui::Button::new(t("コマを分割（ツール）", "Split Panel (Tool)")),
            )
            .clicked()
        {
            self.switch_tool(Tool::PanelSplit);
            ui.close_menu();
        }
        if ui
            .add_enabled(
                has_comic,
                egui::Button::new(t("コマを等分割…", "Grid Split…")),
            )
            .clicked()
        {
            self.comic_ui.grid_open = true;
            ui.close_menu();
        }
        if ui
            .add_enabled(
                has_comic,
                egui::Button::new(t("枠線と間隔…", "Borders and Gutters…")),
            )
            .clicked()
        {
            self.comic_ui.panel_settings_open = true;
            ui.close_menu();
        }
        ui.separator();
        if ui.button(t("トーンを貼る…", "Tone Fill…")).clicked() {
            self.comic_ui.tone_open = true;
            ui.close_menu();
        }
        let is_tone = self
            .doc
            .layers
            .get(self.selected_layer)
            .is_some_and(|layer| layer.tone.is_some());
        if ui
            .button(if is_tone {
                t("トーンを解除", "Remove Tone")
            } else {
                t("レイヤーをトーンにする", "Layer to Tone")
            })
            .clicked()
        {
            let tone = (!is_tone).then_some(self.comic_ui.tone);
            self.set_layer_tone(tone);
            ui.close_menu();
        }
        ui.separator();
        if ui
            .button(t("フキダシ・テキスト", "Balloons and Text"))
            .clicked()
        {
            self.switch_tool(Tool::Balloon);
            ui.close_menu();
        }
        ui.separator();
        if ui.button(t("集中線…", "Focus Lines…")).clicked() {
            self.open_effect_dialog(EffectKind::Focus);
            ui.close_menu();
        }
        if ui.button(t("流線…", "Speed Lines…")).clicked() {
            self.open_effect_dialog(EffectKind::Speed);
            ui.close_menu();
        }
    }

    pub(crate) fn open_effect_dialog(&mut self, kind: EffectKind) {
        let region = self.effect_region();
        let mut focus = FocusLines::new(region);
        let mut speed = SpeedLines::new(region);
        // Keep the user's parameters, fit them to the new region.
        let old_focus = &self.comic_ui.focus;
        focus.count = old_focus.count;
        focus.spacing_jitter = old_focus.spacing_jitter;
        focus.length_jitter = old_focus.length_jitter;
        focus.taper = old_focus.taper;
        focus.bundle = old_focus.bundle;
        let old_speed = &self.comic_ui.speed;
        speed.angle_degrees = old_speed.angle_degrees;
        speed.count = old_speed.count;
        speed.spacing_jitter = old_speed.spacing_jitter;
        speed.length = old_speed.length;
        speed.length_jitter = old_speed.length_jitter;
        speed.taper = old_speed.taper;
        self.comic_ui.focus = focus;
        self.comic_ui.speed = speed;
        self.comic_ui.effect = Some(kind);
        self.comic_ui.effect_layer = None;
    }

    pub(crate) fn comic_windows(&mut self, ctx: &egui::Context) {
        self.page_setup_window(ctx);
        self.panel_windows(ctx);
        self.tone_window(ctx);
        self.effect_window(ctx);
    }

    fn page_setup_window(&mut self, ctx: &egui::Context) {
        if !self.comic_ui.setup_open {
            return;
        }
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut create = false;
        let mut apply = false;
        egui::Window::new(t("漫画原稿の設定", "Manga Page Setup"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let presets = PageSpec::presets();
                egui::ComboBox::from_label(t("プリセット", "Preset"))
                    .selected_text(if english {
                        presets[self.comic_ui.preset].1
                    } else {
                        presets[self.comic_ui.preset].0
                    })
                    .show_ui(ui, |ui| {
                        for (index, (ja, en, spec)) in presets.iter().enumerate() {
                            if ui
                                .selectable_label(
                                    self.comic_ui.preset == index,
                                    if english { *en } else { *ja },
                                )
                                .clicked()
                            {
                                self.comic_ui.preset = index;
                                self.comic_ui.setup = spec.clone();
                            }
                        }
                    });
                let spec = &mut self.comic_ui.setup;
                egui::Grid::new("page-setup").num_columns(2).show(ui, |ui| {
                    fn mm(value: &mut f32, max: f32) -> egui::DragValue<'_> {
                        egui::DragValue::new(value)
                            .range(0.0..=max)
                            .speed(0.5)
                            .suffix(" mm")
                    }
                    ui.label(t("仕上がり 幅", "Trim width"));
                    ui.add(mm(&mut spec.trim_mm[0], 1000.0));
                    ui.end_row();
                    ui.label(t("仕上がり 高さ", "Trim height"));
                    ui.add(mm(&mut spec.trim_mm[1], 1000.0));
                    ui.end_row();
                    ui.label(t("裁ち落とし", "Bleed"));
                    ui.add(mm(&mut spec.bleed_mm, 50.0));
                    ui.end_row();
                    for (index, (ja, en)) in [
                        ("基本枠 上の余白", "Inner frame top"),
                        ("基本枠 下の余白", "Inner frame bottom"),
                        ("基本枠 ノド側の余白", "Inner frame binding side"),
                        ("基本枠 小口側の余白", "Inner frame outer side"),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        ui.label(if english { en } else { ja });
                        ui.add(mm(&mut spec.inner_margins_mm[index], 200.0));
                        ui.end_row();
                    }
                    ui.label(t("解像度", "Resolution"));
                    ui.add(
                        egui::DragValue::new(&mut spec.dpi)
                            .range(72.0..=1200.0)
                            .suffix(" dpi"),
                    );
                    ui.end_row();
                    ui.label(t("綴じ", "Binding"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut spec.binding,
                            efude_comic::Binding::Right,
                            t("右綴じ", "Right"),
                        );
                        ui.selectable_value(
                            &mut spec.binding,
                            efude_comic::Binding::Left,
                            t("左綴じ", "Left"),
                        );
                    });
                    ui.end_row();
                    ui.label(t("ページ", "Page"));
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut spec.right_page,
                            false,
                            t("左ページ", "Left page"),
                        );
                        ui.selectable_value(
                            &mut spec.right_page,
                            true,
                            t("右ページ", "Right page"),
                        );
                    });
                    ui.end_row();
                });
                let g = spec.geometry();
                ui.label(
                    egui::RichText::new(format!("{} × {} px", g.canvas_width, g.canvas_height))
                        .color(layout::MUTED_TEXT),
                );
                let fits = g.canvas_width == self.doc.width && g.canvas_height == self.doc.height;
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(t("新しいタブに作成", "Create in New Tab"))
                                    .color(Color32::WHITE),
                            )
                            .fill(layout::ACCENT),
                        )
                        .clicked()
                    {
                        create = true;
                    }
                    if ui
                        .add_enabled(
                            fits,
                            egui::Button::new(t("今の原稿に適用", "Apply to This Page")),
                        )
                        .on_disabled_hover_text(t(
                            "キャンバスの大きさが設定と違います",
                            "The canvas size differs from this setup",
                        ))
                        .clicked()
                    {
                        apply = true;
                    }
                });
            });
        let spec = self.comic_ui.setup.clone();
        if create && self.new_comic_document(&spec) {
            open = false;
        }
        if apply {
            let mut comic = self.comic_doc().unwrap_or(ComicDoc {
                page: spec.clone(),
                layout: PanelLayout::for_dpi(spec.dpi),
            });
            comic.page = spec;
            self.history.begin();
            self.store_comic(&comic);
            self.history.commit();
            open = false;
        }
        self.comic_ui.setup_open = open;
    }

    fn panel_windows(&mut self, ctx: &egui::Context) {
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        if self.comic_ui.grid_open {
            let mut open = true;
            let mut split = false;
            egui::Window::new(t("コマを等分割", "Grid Split"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(t(
                        "選んでいるレイヤーのコマを分割します。",
                        "Splits the panel of the selected layer.",
                    ));
                    ui.add(
                        egui::Slider::new(&mut self.comic_ui.grid.0, 1..=6)
                            .text(t("横の数", "Columns")),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.comic_ui.grid.1, 1..=8)
                            .text(t("縦の数", "Rows")),
                    );
                    if ui.button(t("分割", "Split")).clicked() {
                        split = true;
                    }
                });
            if split {
                let (columns, rows) = self.comic_ui.grid;
                if self.grid_split_panel(columns, rows) {
                    open = false;
                }
            }
            self.comic_ui.grid_open = open;
        }
        if self.comic_ui.panel_settings_open {
            let Some(comic) = self.comic_doc() else {
                self.comic_ui.panel_settings_open = false;
                return;
            };
            let to_mm = |px: f32| px / comic.page.dpi * 25.4;
            let id = egui::Id::new("panel-settings-values");
            let mut values: [f32; 3] = ctx.data_mut(|data| {
                *data.get_temp_mut_or_insert_with(id, || {
                    [
                        to_mm(comic.layout.border_width),
                        to_mm(comic.layout.gutter_horizontal),
                        to_mm(comic.layout.gutter_vertical),
                    ]
                })
            });
            let mut open = true;
            let mut apply = false;
            egui::Window::new(t("枠線と間隔", "Borders and Gutters"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    egui::Grid::new("panel-settings")
                        .num_columns(2)
                        .show(ui, |ui| {
                            ui.label(t("枠線の太さ", "Border width"));
                            ui.add(
                                egui::DragValue::new(&mut values[0])
                                    .range(0.05..=5.0)
                                    .speed(0.01)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
                            ui.label(t("左右の間隔", "Gap between columns"));
                            ui.add(
                                egui::DragValue::new(&mut values[1])
                                    .range(0.0..=30.0)
                                    .speed(0.1)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
                            ui.label(t("上下の間隔", "Gap between rows"));
                            ui.add(
                                egui::DragValue::new(&mut values[2])
                                    .range(0.0..=30.0)
                                    .speed(0.1)
                                    .suffix(" mm"),
                            );
                            ui.end_row();
                        });
                    ui.label(
                        egui::RichText::new(t(
                            "間隔は、これから分割するときに使われます。",
                            "Gaps apply to the next splits.",
                        ))
                        .color(layout::MUTED_TEXT),
                    );
                    if ui.button(t("適用", "Apply")).clicked() {
                        apply = true;
                    }
                });
            ctx.data_mut(|data| data.insert_temp(id, values));
            if apply {
                self.apply_panel_settings(values[0], values[1], values[2]);
            }
            if !open {
                ctx.data_mut(|data| data.remove::<[f32; 3]>(id));
            }
            self.comic_ui.panel_settings_open = open;
        }
    }

    fn tone_window(&mut self, ctx: &egui::Context) {
        if !self.comic_ui.tone_open {
            return;
        }
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut fill = false;
        egui::Window::new(t("トーンを貼る", "Tone Fill"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(t(
                        "選択範囲（なければ選んでいるコマ、なければページ全体）に貼ります。",
                        "Fills the selection, else the current panel, else the page.",
                    ))
                    .color(layout::MUTED_TEXT),
                );
                let mut percent = self.comic_ui.tone_density * 100.0;
                ui.add(egui::Slider::new(&mut percent, 1.0..=100.0).text(t("濃度 %", "Density %")));
                self.comic_ui.tone_density = percent / 100.0;
                tone_settings_ui(ui, &mut self.comic_ui.tone, english);
                if ui.button(t("貼る", "Fill")).clicked() {
                    fill = true;
                }
            });
        if fill {
            self.tone_fill(self.comic_ui.tone, self.comic_ui.tone_density);
        }
        self.comic_ui.tone_open = open;
    }

    fn effect_window(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.comic_ui.effect else {
            return;
        };
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut draw = false;
        let mut new_arrangement = false;
        egui::Window::new(match kind {
            EffectKind::Focus => t("集中線", "Focus Lines"),
            EffectKind::Speed => t("流線", "Speed Lines"),
        })
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(t(
                    "範囲: 選択範囲 → 選んでいるコマ → 基本枠 → キャンバス の順に使います。色は描画色です。",
                    "Area: selection, else current panel, else inner frame, else canvas. Uses the current colour.",
                ))
                .color(layout::MUTED_TEXT),
            );
            match kind {
                EffectKind::Focus => {
                    let f = &mut self.comic_ui.focus;
                    ui.add(egui::Slider::new(&mut f.count, 10..=600).text(t("本数", "Lines")));
                    let size = f.region[1] - f.region[0];
                    let mut radius = [f.radii.x / size.x.max(1.0), f.radii.y / size.y.max(1.0)];
                    ui.add(egui::Slider::new(&mut radius[0], 0.02..=0.5).text(t("中心の幅", "Centre width")));
                    ui.add(egui::Slider::new(&mut radius[1], 0.02..=0.5).text(t("中心の高さ", "Centre height")));
                    f.radii = glam::Vec2::new(radius[0] * size.x, radius[1] * size.y);
                    let mut center = [
                        (f.center.x - f.region[0].x) / size.x.max(1.0),
                        (f.center.y - f.region[0].y) / size.y.max(1.0),
                    ];
                    ui.add(egui::Slider::new(&mut center[0], 0.0..=1.0).text(t("中心 横位置", "Centre X")));
                    ui.add(egui::Slider::new(&mut center[1], 0.0..=1.0).text(t("中心 縦位置", "Centre Y")));
                    f.center = f.region[0] + glam::Vec2::new(center[0] * size.x, center[1] * size.y);
                    ui.add(egui::Slider::new(&mut f.spacing_jitter, 0.0..=1.0).text(t("間隔の乱れ", "Spacing randomness")));
                    ui.add(egui::Slider::new(&mut f.length_jitter, 0.0..=1.0).text(t("長さの乱れ", "Length randomness")));
                    ui.add(egui::Slider::new(&mut f.width, 0.5..=60.0).text(t("太さ (px)", "Width (px)")));
                    ui.add(egui::Slider::new(&mut f.taper, 0.0..=1.0).text(t("入り抜き", "Taper")));
                    ui.add(egui::Slider::new(&mut f.bundle, 1..=12).text(t("まとまり", "Bundle")));
                }
                EffectKind::Speed => {
                    let s = &mut self.comic_ui.speed;
                    ui.add(egui::Slider::new(&mut s.angle_degrees, -90.0..=90.0).text(t("角度", "Angle")));
                    ui.add(egui::Slider::new(&mut s.count, 5..=400).text(t("本数", "Lines")));
                    ui.add(egui::Slider::new(&mut s.spacing_jitter, 0.0..=1.0).text(t("間隔の乱れ", "Spacing randomness")));
                    ui.add(egui::Slider::new(&mut s.length, 0.05..=1.0).text(t("長さ", "Length")));
                    ui.add(egui::Slider::new(&mut s.length_jitter, 0.0..=0.95).text(t("長さの乱れ", "Length randomness")));
                    ui.add(egui::Slider::new(&mut s.width, 0.5..=40.0).text(t("太さ (px)", "Width (px)")));
                    ui.add(egui::Slider::new(&mut s.taper, 0.0..=1.0).text(t("入り抜き", "Taper")));
                }
            }
            ui.horizontal(|ui| {
                let label = if self.comic_ui.effect_layer.is_some() {
                    t("描き直す", "Redraw")
                } else {
                    t("描く", "Draw")
                };
                if ui.button(label).clicked() {
                    draw = true;
                }
                if ui.button(t("配置を変える", "New Arrangement")).clicked() {
                    new_arrangement = true;
                }
            });
        });
        if new_arrangement {
            self.comic_ui.focus.seed = self.comic_ui.focus.seed.wrapping_add(1);
            self.comic_ui.speed.seed = self.comic_ui.speed.seed.wrapping_add(1);
            draw = true;
        }
        if draw {
            self.draw_effect_lines(kind);
        }
        if !open {
            self.comic_ui.effect = None;
            self.comic_ui.effect_layer = None;
        }
    }
}

/// Tiles for a `width`×`height` image that is `map(coverage)` inside the
/// coverage region and `outside` elsewhere. Uniform tiles share one
/// allocation each; tiles of transparent black are left out.
fn tiles_from_coverage(
    width: u32,
    height: u32,
    coverage: Option<&Coverage>,
    outside: [u8; 4],
    map: impl Fn(u8) -> [u8; 4],
) -> TilePixels {
    use std::sync::Arc;
    let size = efude_canvas::TILE_SIZE;
    let tile_pixels = (size * size) as usize;
    let mut pixels = TilePixels::new(width, height);
    if outside != [0; 4] {
        pixels.fill_shared(outside);
    }
    let Some(coverage) = coverage else {
        return pixels;
    };
    let uniform = |value: [u8; 4]| Arc::new(value.repeat(tile_pixels));
    let (full, empty) = (uniform(map(255)), uniform(map(0)));
    let (x1, y1) = (coverage.x0 + coverage.width, coverage.y0 + coverage.height);
    for ty in coverage.y0 / size..y1.div_ceil(size) {
        for tx in coverage.x0 / size..x1.div_ceil(size) {
            let mut data = vec![0u8; tile_pixels * 4];
            let (mut any, mut all) = (false, true);
            for ly in 0..size {
                for lx in 0..size {
                    let (x, y) = (tx * size + lx, ty * size + ly);
                    let value = if x < width && y < height {
                        let c = coverage.at(x, y);
                        any |= c > 0;
                        all &= c == 255;
                        map(c)
                    } else {
                        outside
                    };
                    let i = ((ly * size + lx) * 4) as usize;
                    data[i..i + 4].copy_from_slice(&value);
                }
            }
            // Edge tiles past the canvas keep their real data.
            let whole = (tx + 1) * size <= width && (ty + 1) * size <= height;
            let tile = if whole && all {
                full.clone()
            } else if whole && !any && map(0) == outside {
                if outside == [0; 4] {
                    continue;
                }
                empty.clone()
            } else {
                Arc::new(data)
            };
            pixels.insert_shared_tile(tx, ty, tile);
        }
    }
    pixels
}

/// Controls for tone parameters (tone dialog and layer card).
pub(crate) fn tone_settings_ui(ui: &mut egui::Ui, tone: &mut ToneSettings, english: bool) -> bool {
    let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
    let before = *tone;
    ui.add(
        egui::Slider::new(&mut tone.lines_per_inch, 10.0..=150.0)
            .text(t("線数 (lpi)", "Lines per inch")),
    );
    ui.add(egui::Slider::new(&mut tone.angle_degrees, -90.0..=90.0).text(t("角度", "Angle")));
    let shape_name = |shape: DotShape| match (shape, english) {
        (DotShape::Round, false) => "円",
        (DotShape::Round, true) => "Round",
        (DotShape::Square, false) => "四角",
        (DotShape::Square, true) => "Square",
        (DotShape::Diamond, false) => "ひし形",
        (DotShape::Diamond, true) => "Diamond",
        (DotShape::Line, false) => "線",
        (DotShape::Line, true) => "Line",
        (DotShape::Cross, false) => "十字",
        (DotShape::Cross, true) => "Cross",
        (DotShape::Noise, false) => "砂目",
        (DotShape::Noise, true) => "Noise",
    };
    egui::ComboBox::from_label(t("網点の形", "Dot shape"))
        .selected_text(shape_name(tone.shape))
        .show_ui(ui, |ui| {
            for shape in DotShape::ALL {
                ui.selectable_value(&mut tone.shape, shape, shape_name(shape));
            }
        });
    ui.checkbox(
        &mut tone.antialias,
        t(
            "網点をなめらかに（印刷用はオフ）",
            "Smooth dots (off for print)",
        ),
    );
    let mut color = Color32::from_rgb(tone.color[0], tone.color[1], tone.color[2]);
    ui.horizontal(|ui| {
        ui.label(t("色", "Colour"));
        ui.color_edit_button_srgba(&mut color);
    });
    tone.color = [color.r(), color.g(), color.b()];
    *tone != before
}
