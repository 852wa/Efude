// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Window layout: menu bar, tool rail on the left, and a docking workspace
//! (canvas, layers, brushes, colour, ...) whose panels can be dragged,
//! split, stacked as tabs or floated. `default_workspace` is the initial
//! layout, and the one "Reset Layout" restores.

use super::*;
use egui_dock::{DockArea, DockState, NodeIndex, TabIndex};
use std::hash::{Hash, Hasher};

/// Accent colour of selected tabs, tools and presets.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(118, 100, 230);
const PANEL: Color32 = Color32::from_rgb(29, 30, 35);
const PANEL_DARK: Color32 = Color32::from_rgb(22, 23, 27);
/// Secondary text (hints, status) that must stay readable on the panels.
pub(crate) const MUTED_TEXT: Color32 = Color32::from_rgb(200, 202, 216);
pub(crate) const CANVAS_BACKGROUND: Color32 = Color32::from_rgb(40, 41, 47);
const RAIL_WIDTH: f32 = 46.0;
const PREVIEW_SIZE: [usize; 2] = [192, 44];

/// A panel of the workspace.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub(crate) enum Pane {
    Canvas,
    Layers,
    Brushes,
    Color,
    Navigator,
    CanvasSettings,
    /// Properties of the current tool.
    Tool,
}

impl Pane {
    /// Panels the user can open and close from the View menu.
    pub(crate) const OPTIONAL: [Pane; 6] = [
        Pane::Layers,
        Pane::Brushes,
        Pane::Color,
        Pane::Tool,
        Pane::Navigator,
        Pane::CanvasSettings,
    ];

    fn title(self, english: bool) -> &'static str {
        match (self, english) {
            (Pane::Canvas, false) => "キャンバス",
            (Pane::Canvas, true) => "Canvas",
            (Pane::Layers, false) => "レイヤー",
            (Pane::Layers, true) => "Layers",
            (Pane::Brushes, false) => "ブラシ",
            (Pane::Brushes, true) => "Brush",
            (Pane::Color, false) => "カラー",
            (Pane::Color, true) => "Color",
            (Pane::Navigator, false) => "ナビゲーター",
            (Pane::Navigator, true) => "Navigator",
            (Pane::CanvasSettings, false) => "用紙",
            (Pane::CanvasSettings, true) => "Canvas Size",
            (Pane::Tool, false) => "ツール",
            (Pane::Tool, true) => "Tool",
        }
    }
}

/// Bumped when the default layout changes enough that saved layouts
/// should be replaced once.
pub(crate) const LAYOUT_VERSION: u32 = 2;

/// Initial layout: the canvas, and on the right the layers and brushes as
/// tabs above, with the colour and the current tool's properties side by
/// side below.
pub(crate) fn default_workspace() -> DockState<Pane> {
    let mut state = DockState::new(vec![Pane::Canvas]);
    let surface = state.main_surface_mut();
    let [_, right] =
        surface.split_right(NodeIndex::root(), 0.68, vec![Pane::Layers, Pane::Brushes]);
    surface.set_active_tab(right, TabIndex(1));
    let [_, below] = surface.split_below(right, 0.5, vec![Pane::Color]);
    surface.split_right(below, 0.5, vec![Pane::Tool]);
    state
}

/// Presets in one column of the brush panel (one set of built-in brushes).
const PRESETS_PER_COLUMN: usize = 10;

/// What the right-click menu of a brush preset asked for.
#[derive(Clone, Copy)]
enum PresetAction {
    Settings(usize),
    Rename(usize),
    Duplicate(usize),
    Export(usize),
    Delete(usize),
}

/// Draws the workspace panels.
struct PaneViewer<'a> {
    app: &'a mut EfudeApp,
    ctx: &'a egui::Context,
}

impl egui_dock::TabViewer for PaneViewer<'_> {
    type Tab = Pane;

    fn title(&mut self, tab: &mut Pane) -> egui::WidgetText {
        tab.title(self.app.language_english).into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Pane) {
        let (app, ctx) = (&mut *self.app, self.ctx);
        if *tab == Pane::Canvas {
            app.document_tabs_ui(ui);
            app.canvas_view(ui, ctx);
            app.zoom_indicator(ui);
            return;
        }
        // Sliders shrink with narrow panels so their values stay in view.
        ui.spacing_mut().slider_width = (ui.available_width() - 130.0).clamp(60.0, 200.0);
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| match tab {
                Pane::Brushes => app.brush_tab(ui, ctx),
                Pane::Layers => {
                    egui::ScrollArea::vertical()
                        .id_salt("pane_layers")
                        .auto_shrink([false, false])
                        .show(ui, |ui| app.layers_ui(ui, ctx));
                }
                Pane::Color => {
                    egui::ScrollArea::vertical()
                        .id_salt("pane_color")
                        .auto_shrink([false, false])
                        .show(ui, |ui| app.color_ui(ui, ctx));
                }
                Pane::Navigator => app.navigator_ui(ui, ctx),
                Pane::Tool => {
                    ui.label(
                        egui::RichText::new(tool_name(app.tool, app.language_english)).strong(),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("pane_tool")
                        .auto_shrink([false, false])
                        .show(ui, |ui| app.tool_options_contents(ui, ctx));
                }
                Pane::CanvasSettings => {
                    egui::ScrollArea::vertical()
                        .id_salt("pane_canvas_settings")
                        .auto_shrink([false, false])
                        .show(ui, |ui| app.canvas_ui(ui, ctx));
                }
                Pane::Canvas => {}
            });
    }

    fn closeable(&mut self, tab: &mut Pane) -> bool {
        *tab != Pane::Canvas
    }

    fn allowed_in_windows(&self, tab: &mut Pane) -> bool {
        *tab != Pane::Canvas
    }

    fn scroll_bars(&self, _tab: &Pane) -> [bool; 2] {
        [false, false]
    }
}

fn dock_style(ctx: &egui::Context) -> egui_dock::Style {
    let mut style = egui_dock::Style::from_egui(ctx.style().as_ref());
    style.tab_bar.bg_fill = PANEL_DARK;
    style.tab_bar.hline_color = Color32::from_rgb(46, 47, 56);
    style.tab_bar.height = 30.0;
    style.separator.width = 3.0;
    style.separator.color_idle = PANEL_DARK;
    style.separator.color_hovered = ACCENT.gamma_multiply(0.7);
    style.separator.color_dragged = ACCENT;
    style.tab.tab_body.bg_fill = PANEL;
    style.tab.tab_body.inner_margin = egui::Margin::ZERO;
    style.tab.tab_body.stroke = Stroke::NONE;
    for interaction in [&mut style.tab.active, &mut style.tab.focused] {
        interaction.bg_fill = PANEL;
        interaction.text_color = Color32::WHITE;
        interaction.outline_color = ACCENT;
    }
    style.tab.hovered.text_color = Color32::from_rgb(235, 235, 242);
    style.tab.inactive.text_color = Color32::from_rgb(196, 198, 212);
    style.tab.inactive.bg_fill = PANEL_DARK;
    style.tab.hline_below_active_tab_name = true;
    style.overlay.selection_color = ACCENT.gamma_multiply(0.4);
    style
}

pub(crate) struct BrushPreview {
    signature: u64,
    texture: egui::TextureHandle,
}

/// Dark theme with rounded widgets and a violet accent.
pub(crate) fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = PANEL_DARK;
    visuals.faint_bg_color = Color32::from_rgb(34, 35, 41);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.hyperlink_color = Color32::from_rgb(160, 150, 255);
    visuals.window_corner_radius = 10.into();
    visuals.menu_corner_radius = 8.into();
    let widgets = &mut visuals.widgets;
    for (state, fill) in [
        (&mut widgets.noninteractive, PANEL),
        (&mut widgets.inactive, Color32::from_rgb(44, 45, 53)),
        (&mut widgets.hovered, Color32::from_rgb(56, 57, 68)),
        (&mut widgets.active, Color32::from_rgb(70, 66, 110)),
        (&mut widgets.open, Color32::from_rgb(52, 53, 63)),
    ] {
        state.corner_radius = 6.into();
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
    }
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(46, 47, 55));
    // Text: bright enough to read on the dark panels at small sizes.
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(240, 240, 246));
    widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(246, 246, 250));
    widgets.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    widgets.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    widgets.open.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    // Always dark, whatever the OS theme: with a light Windows theme egui
    // would otherwise switch to its light style and draw dark text on these
    // dark panels.
    ctx.options_mut(|options| {
        options.theme_preference = egui::ThemePreference::Dark;
        options.fallback_theme = egui::Theme::Dark;
    });
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_visuals_of(theme, visuals.clone());
    }
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(8.0, 4.0);
        style.spacing.slider_width = 150.0;
    });
}

/// Tools on the rail, in groups separated by a gap.
const TOOL_GROUPS: &[&[(Tool, &str, &str)]] = &[
    &[
        (Tool::Brush, "ブラシ", "Brush"),
        (Tool::Eraser, "消しゴム", "Eraser"),
        (Tool::Blur, "ぼかし", "Blur"),
        (Tool::Smudge, "指先", "Smudge"),
        (Tool::Fill, "塗りつぶし", "Fill"),
        (Tool::Eyedropper, "スポイト", "Eyedropper"),
    ],
    &[
        (Tool::Move, "移動", "Move"),
        (Tool::VectorEdit, "制御点", "Control Points"),
        (Tool::Pan, "手のひら", "Pan"),
    ],
    &[
        (Tool::RectangleSelect, "矩形選択", "Rectangle Select"),
        (Tool::EllipseSelect, "楕円選択", "Ellipse Select"),
        (Tool::LassoSelect, "投げ縄", "Lasso"),
        (Tool::PolygonSelect, "多角形選択", "Polygon Select"),
        (Tool::MagicWand, "自動選択", "Magic Wand"),
        (Tool::ColorRange, "色域選択", "Color Range"),
        (Tool::SelectionBrush, "選択範囲ペン", "Selection Brush"),
        (Tool::QuickMask, "クイックマスク", "Quick Mask"),
    ],
    &[
        (Tool::Line, "直線定規", "Line Ruler"),
        (Tool::EllipseRuler, "楕円定規", "Ellipse Ruler"),
        (Tool::BezierRuler, "ベジェ曲線定規", "Bezier Ruler"),
        (Tool::PerspectiveRuler, "透視定規", "Perspective Ruler"),
    ],
    &[
        (Tool::PanelSplit, "コマ分割", "Split Panel"),
        (Tool::Balloon, "フキダシ", "Balloon"),
        (Tool::Text, "テキスト", "Text"),
    ],
];

fn tool_name(tool: Tool, english: bool) -> &'static str {
    TOOL_GROUPS
        .iter()
        .flat_map(|group| group.iter())
        .find(|(candidate, _, _)| *candidate == tool)
        .map_or(
            "",
            |(_, japanese, english_name)| {
                if english { english_name } else { japanese }
            },
        )
}

/// Display name of a built-in brush in the UI language.
pub(crate) fn brush_label(name: &str, english: bool) -> &str {
    if !english {
        return name;
    }
    match name {
        "ペン" => "Pen",
        "鉛筆" => "Pencil",
        "筆" => "Brush",
        "水彩" => "Watercolor",
        "色混ぜ" => "Blender",
        "エアブラシ" => "Airbrush",
        "ぼかし" => "Blur",
        "指先" => "Smudge",
        "消しゴム" => "Eraser",
        custom => custom,
    }
}

impl EfudeApp {
    /// Lays out everything: menu bar, tool rail and the workspace.
    pub(crate) fn layout_ui(&mut self, ctx: &egui::Context) {
        self.menu_bar(ctx);
        if self.show_tools_panel {
            egui::SidePanel::left("tool_rail")
                .exact_width(RAIL_WIDTH)
                .resizable(false)
                .frame(
                    egui::Frame::NONE
                        .fill(PANEL_DARK)
                        .inner_margin(egui::Margin::symmetric(5, 8)),
                )
                .show(ctx, |ui| self.tool_rail(ui));
        }
        if self.workspace.find_tab(&Pane::Canvas).is_none() {
            self.workspace = default_workspace();
        }
        let mut workspace = std::mem::replace(&mut self.workspace, DockState::new(Vec::new()));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL_DARK))
            .show(ctx, |ui| {
                DockArea::new(&mut workspace)
                    .id(egui::Id::new("efude-workspace"))
                    .style(dock_style(ctx))
                    .show_leaf_close_all_buttons(false)
                    .show_leaf_collapse_buttons(false)
                    .show_inside(ui, &mut PaneViewer { app: self, ctx });
            });
        self.workspace = workspace;
        if self.show_new_document {
            self.new_document_window(ctx);
        }
        self.brush_delete_window(ctx);
        self.brush_rename_window(ctx);
        self.comic_windows(ctx);
        self.close_tab_window(ctx);
        self.balloon_window(ctx);
        self.brush_settings_window(ctx);
        self.view_options_window(ctx);
        self.filter_window(ctx);
        self.busy_overlay(ctx);
        self.book_window(ctx);
        if self.show_settings {
            let mut open = true;
            egui::Window::new(self.text("環境設定", "Preferences"))
                .open(&mut open)
                .default_width(360.0)
                .vscroll(true)
                .show(ctx, |ui| self.settings_ui(ui, ctx));
            self.show_settings = open;
        }
    }

    /// Brings the tool properties panel to the front (opening it if it
    /// was closed).
    pub(crate) fn show_tool_pane(&mut self) {
        match self.workspace.find_tab(&Pane::Tool) {
            Some(location) => {
                self.workspace.set_active_tab(location);
            }
            None => self.toggle_pane(Pane::Tool),
        }
    }

    /// Only the properties of the current tool.
    fn tool_options_contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.tool == Tool::VectorEdit {
            self.vector_edit_options(ui);
            return;
        }
        if matches!(
            self.tool,
            Tool::Brush | Tool::Eraser | Tool::Blur | Tool::Smudge
        ) {
            if self.tool == Tool::Eraser
                && self
                    .doc
                    .layers
                    .get(self.selected_layer)
                    .is_some_and(|layer| layer.is_vector())
            {
                let label = self.text(
                    "ベクター: 触れた線をまるごと消す",
                    "Vector: erase whole lines it touches",
                );
                ui.checkbox(&mut self.vector_erase_whole, label);
                ui.add_space(4.0);
            }
            self.tool_brush_properties(ui);
            return;
        }
        self.tool_options_ui(ui, ctx);
        if matches!(
            self.tool,
            Tool::Move
                | Tool::RectangleSelect
                | Tool::EllipseSelect
                | Tool::LassoSelect
                | Tool::PolygonSelect
                | Tool::MagicWand
                | Tool::ColorRange
        ) {
            ui.add_space(6.0);
            egui::CollapsingHeader::new(self.text("変形", "Transform"))
                .default_open(self.tool == Tool::Move)
                .show(ui, |ui| self.transform_ui(ui, ctx));
        }
    }

    /// Properties of a brush tool: its brushes, size, opacity, the main
    /// switches and a way into the full settings.
    fn tool_brush_properties(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let tool = self.tool;
        let mut picked = None;
        let current = brush_label(&self.brushes[self.selected_brush].name, english).to_owned();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("tool-brush")
                .selected_text(current)
                .width((ui.available_width() - 60.0).max(80.0))
                .show_ui(ui, |ui| {
                    for (index, brush) in self.brushes.iter().enumerate() {
                        if brush_fits_tool(brush.kind, tool)
                            && ui
                                .selectable_label(
                                    index == self.selected_brush,
                                    brush_label(&brush.name, english),
                                )
                                .clicked()
                        {
                            picked = Some(index);
                        }
                    }
                });
            if ui
                .small_button(if english { "Rename" } else { "名前" })
                .on_hover_text(if english {
                    "Rename this brush"
                } else {
                    "このブラシの名前を変更"
                })
                .clicked()
            {
                let index = self.selected_brush;
                self.brush_rename = Some((
                    index,
                    brush_label(&self.brushes[index].name, english).to_owned(),
                ));
            }
        });
        if let Some(index) = picked {
            self.select_brush_preset(index);
        }
        ui.separator();
        ui.add(
            egui::Slider::new(&mut self.size, 1.0..=MAX_BRUSH_SIZE)
                .logarithmic(true)
                .text(if english { "Size" } else { "サイズ" }),
        );
        let brush = &mut self.brushes[self.selected_brush];
        brush.size = self.size;
        ui.add(
            egui::Slider::new(&mut brush.opacity, 0.01..=1.0).text(if english {
                "Opacity"
            } else {
                "不透明度"
            }),
        );
        ui.add(
            egui::Slider::new(&mut brush.stabilization, 0..=15).text(if english {
                "Stabilization"
            } else {
                "手ブレ補正"
            }),
        );
        ui.separator();
        self.brush_feel_toggles(ui);
        ui.add_space(4.0);
        if ui
            .button(self.text("ブラシの詳細設定…", "Brush Settings…"))
            .clicked()
        {
            self.show_brush_settings = true;
        }
    }

    /// View, grid, symmetry and sub view settings (from the View menu).
    fn view_options_window(&mut self, ctx: &egui::Context) {
        if !self.show_view_options {
            return;
        }
        let mut open = true;
        egui::Window::new(self.text("表示・グリッド・対称定規", "View, Grid and Symmetry"))
            .id(egui::Id::new("view-options"))
            .open(&mut open)
            .default_width(300.0)
            .vscroll(true)
            .show(ctx, |ui| {
                self.view_ui(ui, ctx);
                egui::CollapsingHeader::new(
                    self.text("サブビュー / 参照画像", "Sub View / Reference Image"),
                )
                .show(ui, |ui| self.subview_ui(ui, ctx));
            });
        self.show_view_options = open;
    }

    /// Opens `pane` if it is closed, or closes it.
    fn toggle_pane(&mut self, pane: Pane) {
        if let Some(location) = self.workspace.find_tab(&pane) {
            self.workspace.remove_tab(location);
            return;
        }
        // Open it next to another side panel, or to the right of the canvas.
        let neighbour = Pane::OPTIONAL
            .iter()
            .find_map(|other| self.workspace.find_tab(other));
        match neighbour {
            Some((surface, node, _)) => {
                self.workspace.set_focused_node_and_surface((surface, node));
                self.workspace.push_to_focused_leaf(pane);
            }
            None => {
                self.workspace
                    .main_surface_mut()
                    .split_right(NodeIndex::root(), 0.75, vec![pane]);
            }
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        if self.logo.is_none()
            && let Ok(decoded) = image::load_from_memory(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/Efude_sub.png"
            )))
        {
            let rgba = decoded.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            self.logo = Some(ctx.load_texture(
                "efude-logo",
                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                egui::TextureOptions::LINEAR,
            ));
        }
        egui::TopBottomPanel::top("menu_bar")
            .frame(
                egui::Frame::NONE
                    .fill(PANEL_DARK)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    if let Some(logo) = &self.logo {
                        ui.image((logo.id(), Vec2::splat(22.0)));
                    }
                    ui.label(egui::RichText::new("Efude").strong().size(15.0));
                    ui.add_space(6.0);
                    ui.menu_button(self.text("ファイル", "File"), |ui| {
                        ui.set_min_width(180.0);
                        self.file_menu_ui(ui, ctx);
                        close_menu_on_click(ui);
                    });
                    ui.menu_button(self.text("編集", "Edit"), |ui| {
                        ui.set_min_width(160.0);
                        self.undo_redo_ui(ui, ctx);
                        ui.separator();
                        let item = |text: &str, keys: &str| {
                            egui::Button::new(text.to_owned()).shortcut_text(keys.to_owned())
                        };
                        if ui
                            .add(item(self.text("コピー", "Copy"), "Ctrl+C"))
                            .clicked()
                        {
                            self.copy_selection(false);
                        }
                        if ui
                            .add(item(self.text("切り取り", "Cut"), "Ctrl+X"))
                            .clicked()
                        {
                            self.copy_selection(true);
                        }
                        if ui
                            .add(item(self.text("貼り付け", "Paste"), "Ctrl+V"))
                            .clicked()
                        {
                            self.paste_clipboard(ctx);
                        }
                        if ui
                            .button(self.text("消去（Delete）", "Delete (Del)"))
                            .clicked()
                        {
                            self.delete_selected_pixels();
                        }
                        ui.separator();
                        if ui
                            .add(item(self.text("すべてを選択", "Select All"), "Ctrl+A"))
                            .clicked()
                        {
                            self.select_all();
                        }
                        if ui
                            .add(item(self.text("選択解除", "Deselect"), "Ctrl+D"))
                            .clicked()
                        {
                            self.change_selection(|selection, _, _| selection.clear());
                        }
                        if ui
                            .add(item(
                                self.text("選択範囲を反転", "Invert Selection"),
                                "Ctrl+Shift+I",
                            ))
                            .clicked()
                        {
                            self.change_selection(|selection, width, height| {
                                selection.invert(width, height)
                            });
                        }
                        close_menu_on_click(ui);
                    });
                    let tool_label = format!(
                        "{}: {}",
                        self.text("ツール", "Tool"),
                        tool_name(self.tool, self.language_english)
                    );
                    if ui
                        .button(tool_label)
                        .on_hover_text(self.text("ツールの設定を表示", "Show tool options"))
                        .clicked()
                    {
                        self.show_tool_pane();
                    }
                    ui.menu_button(self.text("漫画", "Manga"), |ui| self.comic_menu(ui));
                    ui.menu_button(self.text("フィルター", "Filter"), |ui| {
                        ui.set_min_width(240.0);
                        self.filter_menu(ui, ctx);
                    });
                    ui.menu_button(self.text("表示", "View"), |ui| {
                        ui.set_min_width(200.0);
                        let tools = self.text("ツールバー", "Tool Bar");
                        ui.checkbox(&mut self.show_tools_panel, tools);
                        if ui
                            .button(
                                self.text("表示・グリッド・対称定規…", "View, Grid and Symmetry…"),
                            )
                            .clicked()
                        {
                            self.show_view_options = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(self.text("パネル", "Panels"))
                                .color(MUTED_TEXT)
                                .small(),
                        );
                        for pane in Pane::OPTIONAL {
                            let mut open = self.workspace.find_tab(&pane).is_some();
                            if ui
                                .checkbox(&mut open, pane.title(self.language_english))
                                .clicked()
                            {
                                self.toggle_pane(pane);
                            }
                        }
                        if ui
                            .button(self.text("レイアウトを初期状態に戻す", "Reset Layout"))
                            .clicked()
                        {
                            self.workspace = default_workspace();
                            ui.close_menu();
                        }
                    });
                    ui.menu_button(self.text("ヘルプ", "Help"), |ui| {
                        ui.set_min_width(160.0);
                        if ui.button(self.text("ヘルプを表示", "Show Help")).clicked() {
                            self.show_help = true;
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(self.text("言語", "Language"))
                                .color(MUTED_TEXT)
                                .small(),
                        );
                        ui.selectable_value(&mut self.language_english, false, "日本語");
                        ui.selectable_value(&mut self.language_english, true, "English");
                    });
                    let clear = clear_all_button(ui).on_hover_text(self.text(
                        "レイヤー全削除（元に戻す で戻せます）",
                        "Delete All Layers (Undo brings them back)",
                    ));
                    if clear.clicked() {
                        self.clear_all_layers();
                    }
                    // Quick undo/redo, then status on the right.
                    ui.add_space(8.0);
                    let undo = icon_button(ui, false).on_hover_text(self.text("元に戻す", "Undo"));
                    if undo.clicked() {
                        self.undo();
                    }
                    let redo = icon_button(ui, true).on_hover_text(self.text("やり直し", "Redo"));
                    if redo.clicked() {
                        self.redo();
                    }
                    self.history_buttons = [undo.rect, redo.rect];
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.history.is_dirty() {
                            ui.colored_label(
                                Color32::from_rgb(240, 200, 90),
                                self.text("● 未保存", "● Unsaved"),
                            );
                        }
                        ui.add(
                            egui::Label::new(egui::RichText::new(&self.status).color(MUTED_TEXT))
                                .truncate(),
                        );
                    });
                });
            });
    }

    pub(crate) fn undo(&mut self) {
        self.history.undo_document(&mut self.doc);
        self.after_history_step();
    }

    pub(crate) fn redo(&mut self) {
        self.history.redo_document(&mut self.doc);
        self.after_history_step();
    }

    fn after_history_step(&mut self) {
        self.apply_selection_history_update();
        self.sync_mask_edit_mode();
        self.canvas_width_input = self.doc.width;
        self.canvas_height_input = self.doc.height;
        self.canvas_dpi_input = self.doc.dpi;
    }

    fn tool_rail(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        // Shrink the buttons on short windows so every tool stays in view
        // (the rail still scrolls below the smallest size).
        let tools: usize = TOOL_GROUPS.iter().map(|group| group.len()).sum();
        let separators = (TOOL_GROUPS.len() - 1) as f32 * 9.0;
        let button = ((ui.available_height() - separators) / tools as f32 - 4.0).clamp(26.0, 34.0);
        egui::ScrollArea::vertical()
            .id_salt("tool_rail_scroll")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for (index, group) in TOOL_GROUPS.iter().enumerate() {
                        if index > 0 {
                            ui.add_space(4.0);
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::new(24.0, 1.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, Color32::from_gray(58));
                            ui.add_space(4.0);
                        }
                        for &(tool, japanese, english_name) in group.iter() {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(button), egui::Sense::click());
                            let selected = self.tool == tool;
                            let painter = ui.painter();
                            if selected {
                                painter.rect_filled(rect, 8.0, ACCENT);
                            } else if response.hovered() {
                                painter.rect_filled(rect, 8.0, Color32::from_rgb(48, 49, 58));
                            }
                            let color = if selected {
                                Color32::WHITE
                            } else {
                                Color32::from_rgb(222, 222, 234)
                            };
                            crate::icons::paint_tool_icon(
                                painter,
                                rect.shrink(button * 0.235),
                                tool,
                                color,
                            );
                            let response = response.on_hover_text(if english {
                                english_name
                            } else {
                                japanese
                            });
                            if response.clicked() {
                                if self.tool == Tool::PolygonSelect || tool == Tool::PolygonSelect {
                                    self.selection_points.clear();
                                }
                                self.tool = tool;
                                // The tool's own properties come to the front.
                                self.show_tool_pane();
                            }
                        }
                    }
                });
            });
    }

    pub(crate) fn brush_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // The size/opacity block stays pinned at the bottom.
        let bottom_height = 150.0;
        egui::ScrollArea::vertical()
            .id_salt("dock_brush")
            .auto_shrink([false, false])
            .max_height((ui.available_height() - bottom_height).max(120.0))
            .show(ui, |ui| {
                section_title(ui, self.text("ブラシプリセット", "Brush Presets"));
                self.brush_presets_ui(ui, ctx);
                ui.add_space(8.0);
                if ui
                    .add(
                        egui::Button::new(self.text("ブラシの詳細設定…", "Brush Settings…"))
                            .min_size(Vec2::new(ui.available_width(), 28.0)),
                    )
                    .clicked()
                {
                    self.show_brush_settings = !self.show_brush_settings;
                }
                egui::CollapsingHeader::new(self.text(
                    "読み書き・入力ログ・比較",
                    "Import, Export, Input Logs and Comparison",
                ))
                .default_open(false)
                .show(ui, |ui| self.brush_io_ui(ui, ctx));
            });
        ui.separator();
        self.quick_brush_controls(ui);
    }

    fn brush_presets_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut action = None;
        // Presets flow down in columns as wide as the panel allows (each
        // column reads as its own list).
        let count = self.brushes.len();
        let spacing = 8.0;
        // Columns of (up to) ten presets, as many as fit side by side.
        let fit = ((ui.available_width() + spacing) / (170.0 + spacing))
            .floor()
            .clamp(1.0, 6.0) as usize;
        let columns = fit.min(count.div_ceil(PRESETS_PER_COLUMN)).max(1);
        let rows = count.div_ceil(columns.max(1));
        let width = (ui.available_width() - spacing * (columns - 1) as f32) / columns as f32;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            for column in 0..columns {
                ui.vertical(|ui| {
                    ui.set_width(width);
                    for row in 0..rows {
                        let index = column * rows + row;
                        if index < count {
                            self.brush_preset_cell(ui, ctx, index, width, &mut action);
                        }
                    }
                });
            }
        });
        if let Some(action) = action {
            self.apply_preset_action(action);
        }
    }

    /// One brush preset: its sample stroke and name, with a context menu.
    fn brush_preset_cell(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        index: usize,
        width: f32,
        action: &mut Option<PresetAction>,
    ) {
        let english = self.language_english;
        let row_height = 40.0;
        let preview_width = (width * 0.45).clamp(60.0, 116.0);
        let texture = self.brush_preview(index, ctx);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, row_height), egui::Sense::click());
        let selected = index == self.selected_brush;
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 8.0, ACCENT.gamma_multiply(0.28));
            painter.rect_stroke(
                rect,
                8.0,
                Stroke::new(1.5, ACCENT),
                egui::StrokeKind::Inside,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 8.0, Color32::from_rgb(40, 41, 50));
        }
        let preview = Rect::from_min_size(
            rect.min + Vec2::new(8.0, 4.0),
            Vec2::new(preview_width, row_height - 8.0),
        );
        painter.image(
            texture,
            preview,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.text(
            Pos2::new(preview.right() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            brush_label(&self.brushes[index].name, english),
            egui::FontId::proportional(14.0),
            if selected {
                Color32::WHITE
            } else {
                Color32::from_rgb(226, 226, 236)
            },
        );
        if response.clicked() {
            self.select_brush_preset(index);
        }
        let can_delete = self.brushes.len() > 1;
        response.context_menu(|ui| {
            ui.set_min_width(150.0);
            if ui
                .button(if english { "Duplicate" } else { "複製" })
                .clicked()
            {
                *action = Some(PresetAction::Duplicate(index));
                ui.close_menu();
            }
            if ui
                .button(if english {
                    "Settings…"
                } else {
                    "詳細設定…"
                })
                .clicked()
            {
                *action = Some(PresetAction::Settings(index));
                ui.close_menu();
            }
            if ui
                .button(if english {
                    "Rename…"
                } else {
                    "名前を変更…"
                })
                .clicked()
            {
                *action = Some(PresetAction::Rename(index));
                ui.close_menu();
            }
            if ui
                .button(if english {
                    "Export…"
                } else {
                    "書き出し…"
                })
                .clicked()
            {
                *action = Some(PresetAction::Export(index));
                ui.close_menu();
            }
            ui.separator();
            if ui
                .add_enabled(
                    can_delete,
                    egui::Button::new(
                        egui::RichText::new(if english { "Delete" } else { "削除" })
                            .color(Color32::from_rgb(255, 140, 130)),
                    ),
                )
                .clicked()
            {
                *action = Some(PresetAction::Delete(index));
                ui.close_menu();
            }
        });
    }

    fn apply_preset_action(&mut self, action: PresetAction) {
        let english = self.language_english;
        match action {
            PresetAction::Rename(index) => {
                let name = brush_label(&self.brushes[index].name, english).to_owned();
                self.brush_rename = Some((index, name));
                return;
            }
            PresetAction::Settings(index) => {
                self.select_brush_preset(index);
                self.show_brush_settings = true;
            }
            PresetAction::Duplicate(index) => {
                let mut copy = self.brushes[index].clone();
                copy.name = format!(
                    "{} {}",
                    brush_label(&copy.name, english),
                    if english { "copy" } else { "コピー" }
                );
                self.brushes.insert(index + 1, copy);
                for slot in self.tool_brushes.iter_mut().flatten() {
                    if *slot > index {
                        *slot += 1;
                    }
                }
                self.select_brush_preset(index + 1);
            }
            PresetAction::Export(index) => {
                let name = brush_label(&self.brushes[index].name, english).to_owned();
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Efude Brush", &["efudebrush"])
                    .set_file_name(format!("{name}.efudebrush"))
                    .save_file()
                {
                    self.status = match efude_brush::save_bundle(&path, &self.brushes[index]) {
                        Ok(()) => self.text("ブラシを書き出しました", "Brush exported").into(),
                        Err(error) => error.to_string(),
                    };
                }
            }
            PresetAction::Delete(index) => {
                // Confirmed in `brush_delete_window`.
                self.pending_brush_delete = Some(index);
                return;
            }
        }
        // Indices shifted: previews are re-rendered on demand.
        self.brush_previews.clear();
    }

    /// Renames a brush preset.
    fn brush_rename_window(&mut self, ctx: &egui::Context) {
        let Some((index, mut name)) = self.brush_rename.take() else {
            return;
        };
        if index >= self.brushes.len() {
            return;
        }
        let mut done = None;
        egui::Window::new(self.text("ブラシの名前", "Brush Name"))
            .id(egui::Id::new("brush-rename"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let field = ui.add(egui::TextEdit::singleline(&mut name).desired_width(220.0));
                if !field.has_focus() && !field.lost_focus() {
                    field.request_focus();
                }
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() || entered {
                        done = Some(true);
                    }
                    if ui.button(self.text("キャンセル", "Cancel")).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                let name = name.trim();
                if !name.is_empty() {
                    self.brushes[index].name = name.chars().take(64).collect();
                    self.brush_previews.remove(&index);
                }
            }
            Some(false) => {}
            None => self.brush_rename = Some((index, name)),
        }
    }

    /// Asks before deleting a brush preset.
    fn brush_delete_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.pending_brush_delete else {
            return;
        };
        if index >= self.brushes.len() || self.brushes.len() <= 1 {
            self.pending_brush_delete = None;
            return;
        }
        let english = self.language_english;
        let name = brush_label(&self.brushes[index].name, english).to_owned();
        let mut decision = None;
        egui::Window::new(if english {
            "Delete Brush"
        } else {
            "ブラシの削除"
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(if english {
                format!("Delete the brush \"{name}\"?")
            } else {
                format!("ブラシ「{name}」を削除しますか？")
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(if english { "Delete" } else { "削除" })
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(190, 60, 60)),
                    )
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui
                    .button(if english { "Cancel" } else { "キャンセル" })
                    .clicked()
                {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                self.pending_brush_delete = None;
                self.brushes.remove(index);
                // Remembered brushes after the deleted one move up.
                for slot in &mut self.tool_brushes {
                    *slot = match *slot {
                        Some(i) if i == index => None,
                        Some(i) if i > index => Some(i - 1),
                        other => other,
                    };
                }
                if self.selected_brush >= index && self.selected_brush > 0 {
                    self.selected_brush -= 1;
                }
                self.selected_brush = self.selected_brush.min(self.brushes.len() - 1);
                self.size = self.brushes[self.selected_brush].size;
                self.brush_previews.clear();
            }
            Some(false) => self.pending_brush_delete = None,
            None => {}
        }
    }

    /// Texture of a sample stroke painted with brush `index`, re-rendered
    /// only when the brush changes.
    fn brush_preview(&mut self, index: usize, ctx: &egui::Context) -> egui::TextureId {
        let signature = brush_signature(&self.brushes[index]);
        if let Some(preview) = self.brush_previews.get(&index)
            && preview.signature == signature
        {
            return preview.texture.id();
        }
        let pixels = render_brush_preview(&self.brushes[index]);
        let image = egui::ColorImage::from_rgba_unmultiplied(PREVIEW_SIZE, &pixels);
        let texture = match self.brush_previews.remove(&index) {
            Some(mut preview) => {
                preview.texture.set(image, egui::TextureOptions::LINEAR);
                preview.texture
            }
            None => ctx.load_texture(
                format!("brush-preview-{index}"),
                image,
                egui::TextureOptions::LINEAR,
            ),
        };
        let id = texture.id();
        self.brush_previews
            .insert(index, BrushPreview { signature, texture });
        id
    }
}

/// Small undo (or redo) button drawn with a vector arrow.
/// A dotted circle: the "delete all layers" button.
fn clear_all_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26.0, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_rgb(48, 49, 58));
    }
    let color = if response.hovered() {
        Color32::from_rgb(255, 150, 140)
    } else {
        Color32::from_rgb(210, 210, 222)
    };
    let (center, radius) = (rect.center(), 6.5);
    for i in 0..12 {
        let angle = i as f32 / 12.0 * std::f32::consts::TAU;
        ui.painter()
            .circle_filled(center + Vec2::angled(angle) * radius, 1.1, color);
    }
    response
}

fn icon_button(ui: &mut egui::Ui, redo: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(26.0, 22.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_rgb(48, 49, 58));
    }
    crate::icons::paint_history_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
        redo,
        Color32::from_rgb(210, 210, 222),
    );
    response
}

fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).strong().size(13.5));
    ui.add_space(2.0);
}

/// Closes the open menu when one of its buttons was clicked.
fn close_menu_on_click(ui: &mut egui::Ui) {
    if ui.input(|input| input.pointer.any_click()) && ui.ui_contains_pointer() {
        ui.close_menu();
    }
}

/// Everything that changes how a brush's preview looks.
fn brush_signature(brush: &Brush) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    brush.name.hash(&mut hasher);
    format!(
        "{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}{}{}{}{}{}",
        brush.kind,
        brush.size_source,
        brush.opacity_source,
        brush.concentration_source,
        brush.mix_source,
        brush.dilution_source,
        brush.bleed,
        brush.mix.sample_range,
        brush.antialias,
        brush.wet_edge_on,
        brush.settle,
        brush.grain_fixed,
        brush.grain_random_rotation
    )
    .hash(&mut hasher);
    for value in [
        brush.size,
        brush.opacity,
        brush.spacing,
        brush.hardness,
        brush.grain,
        brush.grain_scale,
        brush.tip_aspect,
        brush.tip_rotation,
        brush.scatter,
        brush.size_min,
        brush.opacity_min,
        brush.concentration_min,
        brush.mix_min,
        brush.dilution_min,
        brush.pressure_curve,
        brush.pressure_curve_x1,
        brush.pressure_curve_y1,
        brush.pressure_curve_x2,
        brush.pressure_curve_y2,
        brush.mix.blend,
        brush.mix.dilution,
        brush.mix.persistence,
        brush.mix.charge,
        brush.wet_edge,
        brush.wet_edge_width,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    for texture in [&brush.tip, &brush.grain_tip] {
        texture
            .as_ref()
            .map(|t| (t.width, t.height, t.coverage.len()))
            .hash(&mut hasher);
    }
    brush.grain_fixed.hash(&mut hasher);
    hasher.finish()
}

/// Paints an S-shaped sample stroke with pressure rising and falling, the
/// way the brush would paint it. Brushes that only act on existing paint
/// (eraser, blur, smudge, blender) get two colour bands to work on.
fn render_brush_preview(brush: &Brush) -> Vec<u8> {
    use efude_brush::engine::{self, DabStyle, DabTarget, StrokeRaster};
    let [width, height] = PREVIEW_SIZE;
    let mut doc = Document::new(width as u32, height as u32);
    let acts_on_paint = matches!(
        brush.kind,
        BrushKind::Eraser | BrushKind::Blur | BrushKind::Smudge
    ) || (brush.mix.dilution >= 0.99 && brush.mix.blend > 0.0);
    if acts_on_paint {
        for y in 0..height as u32 {
            for x in 0..width as u32 {
                // Blur needs hard edges to soften; the others two bands.
                let left = if matches!(brush.kind, BrushKind::Blur) {
                    (x / 8) % 2 == 0
                } else {
                    x < width as u32 / 2
                };
                let band = if left {
                    [196, 92, 120, 255]
                } else {
                    [92, 110, 205, 255]
                };
                if (8..height as u32 - 8).contains(&y) {
                    doc.layers[0].pixels.set_pixel(x, y, band);
                }
            }
        }
    }
    let diameter = (brush.size * 0.55).clamp(5.0, 22.0);
    let style = DabStyle {
        brush,
        kind: brush.kind,
        eraser: matches!(brush.kind, BrushKind::Eraser),
        color: [226, 226, 240, 255],
        size: diameter,
    };
    let mut history = History::default();
    history.begin();
    let mut raster = StrokeRaster::new([0.88; 3], brush.mix.charge, glam::Vec2::new(12.0, 22.0));
    let step = (diameter * brush.spacing).clamp(0.3, 4.0);
    let (x0, x1) = (14.0f32, width as f32 - 14.0);
    let steps = ((x1 - x0) / step).ceil() as usize;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 + (x1 - x0) * t;
        let y = height as f32 * 0.5 - (t * std::f32::consts::TAU).sin() * (height as f32 * 0.18);
        let pressure = brush.map_pressure((t * std::f32::consts::PI).sin().max(0.0).powf(0.8));
        let point = InkPoint::new(x, y, pressure, (i * 4) as u64);
        let dynamics = engine::dynamics(brush, &point, raster.last_dab, 1.0);
        let mut target = DabTarget {
            doc: &mut doc,
            layer: 0,
            selection: None,
            history: &mut history,
        };
        raster.stamp(&mut target, style, &dynamics, point);
        raster.finish_dab(style, point);
    }
    let mut target = DabTarget {
        doc: &mut doc,
        layer: 0,
        selection: None,
        history: &mut history,
    };
    raster.finish_stroke(&mut target, brush);
    doc.layers[0].pixels.to_dense()
}
