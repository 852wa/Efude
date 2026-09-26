// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! The Filter menu: each filter opens its own window with its settings, a
//! before/after preview of the part of the layer in view, and Apply. The
//! work itself runs on the I/O thread; a busy indicator shows meanwhile.

use super::*;
use efude_canvas::filters::{BlurKind, Direction, Filter, GRADE_COUNT, GRADE_NAMES};

/// Filters that have a window.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FilterKind {
    Blur,
    Sharpen,
    Denoise,
    Curve,
    Levels,
    Adjust,
    Grade,
    ChromaticAberration,
    Mosaic,
    Noise,
    LineWidth,
}

impl FilterKind {
    fn name(self, english: bool) -> &'static str {
        match (self, english) {
            (FilterKind::Blur, false) => "ぼかし",
            (FilterKind::Blur, true) => "Blur",
            (FilterKind::Sharpen, false) => "シャープ",
            (FilterKind::Sharpen, true) => "Sharpen",
            (FilterKind::Denoise, false) => "ノイズ除去",
            (FilterKind::Denoise, true) => "Reduce Noise",
            (FilterKind::Curve, false) => "トーンカーブ",
            (FilterKind::Curve, true) => "Tone Curve",
            (FilterKind::Levels, false) => "レベル補正",
            (FilterKind::Levels, true) => "Levels",
            (FilterKind::Adjust, false) => "色相・彩度・明度",
            (FilterKind::Adjust, true) => "Hue, Saturation, Brightness",
            (FilterKind::Grade, false) => "カラーグレーディング",
            (FilterKind::Grade, true) => "Color Grading",
            (FilterKind::ChromaticAberration, false) => "色収差",
            (FilterKind::ChromaticAberration, true) => "Chromatic Aberration",
            (FilterKind::Mosaic, false) => "モザイク",
            (FilterKind::Mosaic, true) => "Mosaic",
            (FilterKind::Noise, false) => "ノイズ",
            (FilterKind::Noise, true) => "Add Noise",
            (FilterKind::LineWidth, false) => "線の太さ（太らせる・細らせる）",
            (FilterKind::LineWidth, true) => "Line Width (Thicken / Thin)",
        }
    }
}

/// Settings of the filters kept between uses.
pub(crate) struct FilterSettings {
    blur_kind: BlurKind,
    blur_amount: f32,
    blur_angle: f32,
    sharpen_amount: f32,
    sharpen_radius: f32,
    denoise: u32,
    grade: usize,
    grade_strength: f32,
    aberration_direction: Direction,
    aberration_amount: f32,
    mosaic: u32,
    noise_amount: f32,
    noise_mono: bool,
    noise_seed: u32,
    line_width: i32,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self {
            blur_kind: BlurKind::Gaussian,
            blur_amount: 8.0,
            blur_angle: 0.0,
            sharpen_amount: 1.0,
            sharpen_radius: 2.0,
            denoise: 1,
            grade: 0,
            grade_strength: 1.0,
            aberration_direction: Direction::Horizontal,
            aberration_amount: 4.0,
            mosaic: 16,
            noise_amount: 0.15,
            noise_mono: false,
            noise_seed: 1,
            line_width: 1,
        }
    }
}

/// The open filter window.
pub(crate) struct FilterDialog {
    kind: FilterKind,
    preview: Option<FilterPreview>,
}

struct FilterPreview {
    key: String,
    before: egui::TextureHandle,
    after: egui::TextureHandle,
}

impl FilterDialog {
    pub(crate) fn open(kind: FilterKind) -> Self {
        Self {
            kind,
            preview: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn has_preview(&self) -> bool {
        self.preview.is_some()
    }
}

/// Size of the preview (document pixels shown 1:1).
const PREVIEW: (i64, i64) = (220, 165);

impl EfudeApp {
    /// Contents of the Filter menu.
    pub(crate) fn filter_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let english = self.language_english;
        let open = |ui: &mut egui::Ui, app: &mut Self, kind: FilterKind| {
            if ui.button(format!("{}…", kind.name(english))).clicked() {
                app.filter_dialog = Some(FilterDialog::open(kind));
                ui.close_menu();
            }
        };
        open(ui, self, FilterKind::Blur);
        open(ui, self, FilterKind::Sharpen);
        open(ui, self, FilterKind::Denoise);
        ui.separator();
        open(ui, self, FilterKind::Curve);
        open(ui, self, FilterKind::Levels);
        open(ui, self, FilterKind::Adjust);
        if ui
            .button(self.text("自動レベル補正", "Auto Levels"))
            .clicked()
        {
            self.apply_auto_levels(ctx);
            ui.close_menu();
        }
        open(ui, self, FilterKind::Grade);
        ui.separator();
        open(ui, self, FilterKind::ChromaticAberration);
        open(ui, self, FilterKind::Mosaic);
        open(ui, self, FilterKind::Noise);
        ui.separator();
        open(ui, self, FilterKind::LineWidth);
    }

    /// The operation the window for `kind` would apply.
    fn filter_operation(&self, kind: FilterKind) -> FilterOperation {
        let s = &self.filter_settings;
        match kind {
            FilterKind::Blur => FilterOperation::Image(Filter::Blur {
                kind: s.blur_kind,
                amount: s.blur_amount,
                angle: s.blur_angle,
            }),
            FilterKind::Sharpen => FilterOperation::Image(Filter::Sharpen {
                amount: s.sharpen_amount,
                radius: s.sharpen_radius,
            }),
            FilterKind::Denoise => FilterOperation::Image(Filter::Denoise {
                strength: s.denoise,
            }),
            FilterKind::Curve => FilterOperation::Curve(self.tone_curve),
            FilterKind::Levels => FilterOperation::Levels {
                input_black: self.levels_input_black,
                input_white: self.levels_input_white,
                gamma: self.levels_gamma,
                output_black: self.levels_output_black,
                output_white: self.levels_output_white,
            },
            FilterKind::Adjust => FilterOperation::Adjust {
                hue: self.hue_shift,
                brightness: self.brightness,
                contrast: self.contrast,
                saturation: self.saturation,
                gamma: self.tone_gamma,
            },
            FilterKind::Grade => FilterOperation::Image(Filter::Grade {
                preset: s.grade,
                strength: s.grade_strength,
            }),
            FilterKind::ChromaticAberration => {
                FilterOperation::Image(Filter::ChromaticAberration {
                    direction: s.aberration_direction,
                    amount: s.aberration_amount,
                })
            }
            FilterKind::Mosaic => FilterOperation::Image(Filter::Mosaic { size: s.mosaic }),
            FilterKind::Noise => FilterOperation::Image(Filter::Noise {
                amount: s.noise_amount,
                mono: s.noise_mono,
                seed: s.noise_seed,
            }),
            FilterKind::LineWidth => FilterOperation::Image(Filter::LineWidth {
                amount: s.line_width,
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn filter_operation_for_tests(&self, kind: FilterKind) -> FilterOperation {
        self.filter_operation(kind)
    }

    /// Whether the selected layer can be filtered.
    fn filterable_layer(&self) -> bool {
        self.doc
            .layers
            .get(self.selected_layer)
            .is_some_and(|layer| {
                !layer.locked
                    && !self.is_reference_layer(self.selected_layer)
                    && layer.kind == LayerKind::Raster
                    && !layer.is_vector()
            })
    }

    /// The window of the filter being set up.
    pub(crate) fn filter_window(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.filter_dialog.as_ref().map(|dialog| dialog.kind) else {
            return;
        };
        let english = self.language_english;
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(kind.name(english))
            .id(egui::Id::new("filter-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(ctx.screen_rect().center() - Vec2::new(240.0, 220.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.spacing_mut().slider_width = 240.0;
                self.filter_controls(ui, kind);
                ui.separator();
                self.update_filter_preview(ctx, kind);
                self.filter_preview_ui(ui);
                ui.separator();
                let usable = self.filterable_layer();
                if !usable {
                    ui.colored_label(
                        Color32::from_rgb(255, 205, 110),
                        self.text(
                            "選んでいるレイヤーには適用できません（ロック・参照・フォルダー）",
                            "The selected layer cannot be filtered (locked, reference or folder).",
                        ),
                    );
                }
                ui.horizontal(|ui| {
                    let busy = self.filter_pending;
                    if ui
                        .add_enabled(
                            usable && !busy,
                            egui::Button::new(self.text("適用", "Apply"))
                                .min_size(Vec2::new(90.0, 26.0)),
                        )
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button(self.text("キャンセル", "Cancel")).clicked() {
                        cancel = true;
                    }
                    if self.selection.active {
                        ui.label(
                            egui::RichText::new(
                                self.text(
                                    "選択範囲の中に適用します",
                                    "Applies inside the selection",
                                ),
                            )
                            .small()
                            .color(crate::layout::MUTED_TEXT),
                        );
                    }
                });
            });
        if apply {
            let operation = self.filter_operation(kind);
            self.queue_filter(operation, ctx);
            self.filter_dialog = None;
        } else if cancel || !open {
            self.filter_dialog = None;
        }
    }

    /// The settings of one filter.
    fn filter_controls(&mut self, ui: &mut egui::Ui, kind: FilterKind) {
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let s = &mut self.filter_settings;
        match kind {
            FilterKind::Blur => {
                ui.horizontal_wrapped(|ui| {
                    for (value, ja, en) in [
                        (BlurKind::Gaussian, "ガウス", "Gaussian"),
                        (BlurKind::Lens, "レンズ", "Lens"),
                        (BlurKind::Smooth, "スムーズ", "Smooth"),
                        (BlurKind::Motion, "移動", "Motion"),
                    ] {
                        ui.selectable_value(&mut s.blur_kind, value, t(ja, en));
                    }
                });
                let max = match s.blur_kind {
                    BlurKind::Gaussian | BlurKind::Motion => 200.0,
                    BlurKind::Lens | BlurKind::Smooth => 64.0,
                };
                s.blur_amount = s.blur_amount.min(max);
                ui.add(
                    egui::Slider::new(&mut s.blur_amount, 0.0..=max)
                        .suffix(" px")
                        .text(t("量", "Amount")),
                );
                if s.blur_kind == BlurKind::Motion {
                    ui.add(
                        egui::Slider::new(&mut s.blur_angle, 0.0..=360.0)
                            .suffix("°")
                            .text(t("方向", "Direction")),
                    );
                }
                ui.label(
                    egui::RichText::new(match s.blur_kind {
                        BlurKind::Gaussian => t("なめらかにぼかします。", "A soft, even blur."),
                        BlurKind::Lens => t(
                            "レンズのピンぼけのように、明るい所が丸く広がります。",
                            "Out of focus like a lens: bright spots spread into discs.",
                        ),
                        BlurKind::Smooth => t(
                            "輪郭を残して面やグラデーションをなめらかにします。",
                            "Smooths surfaces and gradients but keeps edges.",
                        ),
                        BlurKind::Motion => t(
                            "指定した方向に流れるようにぼかします。",
                            "Blurs along a direction, like movement.",
                        ),
                    })
                    .small()
                    .color(crate::layout::MUTED_TEXT),
                );
            }
            FilterKind::Sharpen => {
                ui.add(egui::Slider::new(&mut s.sharpen_amount, 0.0..=5.0).text(t("量", "Amount")));
                ui.add(
                    egui::Slider::new(&mut s.sharpen_radius, 0.5..=20.0)
                        .suffix(" px")
                        .text(t("半径", "Radius")),
                );
            }
            FilterKind::Denoise => {
                ui.add(egui::Slider::new(&mut s.denoise, 1..=3).text(t("強さ", "Strength")));
            }
            FilterKind::Curve => {
                ui.label(t(
                    "点を上下にドラッグします（左が暗い所、右が明るい所）。",
                    "Drag the points up or down (shadows on the left, highlights on the right).",
                ));
                tone_curve_editor(ui, &mut self.tone_curve);
                if ui.small_button(t("直線に戻す", "Reset")).clicked() {
                    self.tone_curve = [0.0, 0.25, 0.5, 0.75, 1.0];
                }
            }
            FilterKind::Levels => {
                ui.add(
                    egui::Slider::new(&mut self.levels_input_black, 0.0..=0.49)
                        .text(t("入力 黒", "Input Black")),
                );
                ui.add(
                    egui::Slider::new(&mut self.levels_input_white, 0.51..=1.0)
                        .text(t("入力 白", "Input White")),
                );
                ui.add(
                    egui::Slider::new(&mut self.levels_gamma, 0.1..=4.0).text(t("ガンマ", "Gamma")),
                );
                ui.add(
                    egui::Slider::new(&mut self.levels_output_black, 0.0..=0.49)
                        .text(t("出力 黒", "Output Black")),
                );
                ui.add(
                    egui::Slider::new(&mut self.levels_output_white, 0.51..=1.0)
                        .text(t("出力 白", "Output White")),
                );
            }
            FilterKind::Adjust => {
                ui.add(
                    egui::Slider::new(&mut self.hue_shift, -180.0..=180.0)
                        .suffix("°")
                        .text(t("色相", "Hue")),
                );
                ui.add(
                    egui::Slider::new(&mut self.saturation, 0.0..=3.0)
                        .text(t("彩度", "Saturation")),
                );
                ui.add(
                    egui::Slider::new(&mut self.brightness, -1.0..=1.0)
                        .text(t("明度", "Brightness")),
                );
                ui.add(
                    egui::Slider::new(&mut self.contrast, -0.9..=2.0)
                        .text(t("コントラスト", "Contrast")),
                );
                ui.add(
                    egui::Slider::new(&mut self.tone_gamma, 0.2..=2.5).text(t("ガンマ", "Gamma")),
                );
                if ui.small_button(t("初期値に戻す", "Reset")).clicked() {
                    self.hue_shift = 0.0;
                    self.saturation = 1.0;
                    self.brightness = 0.0;
                    self.contrast = 0.0;
                    self.tone_gamma = 1.0;
                }
            }
            FilterKind::Grade => {
                egui::Grid::new("grade-looks")
                    .num_columns(3)
                    .show(ui, |ui| {
                        for (index, (ja, en)) in GRADE_NAMES.iter().enumerate().take(GRADE_COUNT) {
                            ui.selectable_value(
                                &mut s.grade,
                                index,
                                if english { *en } else { *ja },
                            );
                            if index % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut s.grade_strength, 0.0..=1.0).text(t("強さ", "Strength")),
                );
            }
            FilterKind::ChromaticAberration => {
                ui.horizontal_wrapped(|ui| {
                    for (value, ja, en) in [
                        (Direction::Horizontal, "横", "Horizontal"),
                        (Direction::Vertical, "縦", "Vertical"),
                        (Direction::DiagonalDown, "斜め ＼", "Diagonal ＼"),
                        (Direction::DiagonalUp, "斜め ／", "Diagonal ／"),
                    ] {
                        ui.selectable_value(&mut s.aberration_direction, value, t(ja, en));
                    }
                });
                ui.add(
                    egui::Slider::new(&mut s.aberration_amount, -50.0..=50.0)
                        .suffix(" px")
                        .text(t("量", "Amount")),
                );
            }
            FilterKind::Mosaic => {
                ui.add(
                    egui::Slider::new(&mut s.mosaic, 2..=200)
                        .suffix(" px")
                        .text(t("マスの大きさ", "Cell Size")),
                );
            }
            FilterKind::Noise => {
                ui.add(egui::Slider::new(&mut s.noise_amount, 0.0..=1.0).text(t("量", "Amount")));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut s.noise_mono, false, t("カラー", "Color"));
                    ui.selectable_value(&mut s.noise_mono, true, t("モノクロ", "Monochrome"));
                    if ui.button(t("別の模様", "New Pattern")).clicked() {
                        s.noise_seed = s.noise_seed.wrapping_add(1);
                    }
                });
            }
            FilterKind::LineWidth => {
                ui.add(
                    egui::Slider::new(&mut s.line_width, -10..=10)
                        .suffix(" px")
                        .text(t("太さ（＋太く／−細く）", "Width (+ thicker / − thinner)")),
                );
                ui.label(
                    egui::RichText::new(t(
                        "透明な部分のあるレイヤーでは描かれた形を、白地の絵では暗い線を太らせ・細らせます。",
                        "On a layer with transparency the painted shapes grow or shrink; on an opaque picture, dark lines do.",
                    ))
                    .small()
                    .color(crate::layout::MUTED_TEXT),
                );
            }
        }
    }

    /// Recomputes the preview when the settings, the layer or the view
    /// changed (not while a slider is being dragged).
    fn update_filter_preview(&mut self, ctx: &egui::Context, kind: FilterKind) {
        if !self.filterable_layer() {
            return;
        }
        let operation = self.filter_operation(kind);
        let (dw, dh) = (self.doc.width as i64, self.doc.height as i64);
        let (pw, ph) = (PREVIEW.0.min(dw), PREVIEW.1.min(dh));
        let left = (self.navigator_center.x as i64 - pw / 2).clamp(0, dw - pw);
        let top = (self.navigator_center.y as i64 - ph / 2).clamp(0, dh - ph);
        let layer = &self.doc.layers[self.selected_layer];
        let key = format!(
            "{operation:?} {left} {top} {} {}",
            layer.id,
            self.history.state_token()
        );
        let current = self
            .filter_dialog
            .as_ref()
            .and_then(|dialog| dialog.preview.as_ref())
            .map(|preview| preview.key == key);
        if current == Some(true) || (current.is_some() && ctx.input(|i| i.pointer.any_down())) {
            return;
        }
        let margin = match operation {
            FilterOperation::Image(filter) => filter.reach().min(200) as i64,
            _ => 0,
        };
        let (x0, y0) = ((left - margin).max(0), (top - margin).max(0));
        let (x1, y1) = ((left + pw + margin).min(dw), (top + ph + margin).min(dh));
        let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut crop = Vec::with_capacity((cw * ch * 4) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                crop.extend_from_slice(&layer.pixels.pixel(x as u32, y as u32));
            }
        }
        let mut part = efude_canvas::Layer::new(0, String::new(), cw, ch);
        part.pixels = efude_canvas::TilePixels::from_dense(cw, ch, &crop);
        run_filter(&mut part, cw, ch, operation, (x0, y0));
        let filtered = part.pixels.to_dense();
        let image = |pixels: &[u8]| {
            let mut out = Vec::with_capacity((pw * ph * 4) as usize);
            for y in 0..ph {
                for x in 0..pw {
                    let (sx, sy) = ((left - x0 + x) as usize, (top - y0 + y) as usize);
                    let i = (sy * cw as usize + sx) * 4;
                    // Over a light checkerboard, so transparency shows.
                    let check = if ((x / 8) + (y / 8)) % 2 == 0 {
                        255.0
                    } else {
                        225.0
                    };
                    let a = pixels[i + 3] as f32 / 255.0;
                    for c in 0..3 {
                        out.push((pixels[i + c] as f32 * a + check * (1.0 - a)).round() as u8);
                    }
                    out.push(255);
                }
            }
            egui::ColorImage::from_rgba_unmultiplied([pw as usize, ph as usize], &out)
        };
        let options = egui::TextureOptions::NEAREST;
        let before = ctx.load_texture("filter-before", image(&crop), options);
        let after = ctx.load_texture("filter-after", image(&filtered), options);
        if let Some(dialog) = &mut self.filter_dialog {
            dialog.preview = Some(FilterPreview { key, before, after });
        }
    }

    fn filter_preview_ui(&self, ui: &mut egui::Ui) {
        let Some(preview) = self.filter_dialog.as_ref().and_then(|d| d.preview.as_ref()) else {
            return;
        };
        ui.horizontal(|ui| {
            for (label, texture) in [
                (self.text("適用前", "Before"), &preview.before),
                (self.text("適用後", "After"), &preview.after),
            ] {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .small()
                            .color(crate::layout::MUTED_TEXT),
                    );
                    ui.image((texture.id(), texture.size_vec2()));
                });
            }
        });
        ui.label(
            egui::RichText::new(self.text(
                "プレビューは表示中の場所を等倍で表示しています。",
                "The preview shows the part of the layer in view at 100%.",
            ))
            .small()
            .color(crate::layout::MUTED_TEXT),
        );
    }

    /// A spinner over the canvas while a filter, load or save is working
    /// (after a moment, so quick tasks do not flash).
    pub(crate) fn busy_overlay(&mut self, ctx: &egui::Context) {
        self.prepare_loading_animation(ctx);
        if self.io_task_sender.busy.get() == 0 {
            self.busy_since = None;
            return;
        }
        let since = *self.busy_since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed() < std::time::Duration::from_millis(150) {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
            return;
        }
        let area = self.canvas_viewport.0;
        let center = if area.is_positive() {
            area.center()
        } else {
            ctx.screen_rect().center()
        };
        egui::Area::new(egui::Id::new("busy-indicator"))
            .order(egui::Order::Foreground)
            .fixed_pos(center)
            .pivot(egui::Align2::CENTER_CENTER)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        match self.loading_frame(ctx, since.elapsed()) {
                            Some(texture) => {
                                ui.add(
                                    egui::Image::new((texture, Vec2::splat(96.0)))
                                        .corner_radius(12.0),
                                );
                            }
                            None => {
                                ui.add(egui::Spinner::new().size(22.0));
                            }
                        }
                        ui.label(self.text("処理中…", "Working…"));
                    });
                });
            });
    }

    /// Decodes the loading animation on a worker thread soon after start
    /// (the spinner shows until it is ready), then uploads it once.
    fn prepare_loading_animation(&mut self, ctx: &egui::Context) {
        // Tests make many windows; they check the decoding on its own.
        if cfg!(test) {
            return;
        }
        if self.loading_animation.is_none() {
            match &self.loading_decode {
                None => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let repaint = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = sender.send(decode_loading_animation());
                        repaint.request_repaint();
                    });
                    self.loading_decode = Some(receiver);
                }
                Some(receiver) => match receiver.try_recv() {
                    Ok(images) => {
                        let frames = images
                            .into_iter()
                            .enumerate()
                            .map(|(index, (image, delay))| {
                                let texture = ctx.load_texture(
                                    format!("loading-{index}"),
                                    image,
                                    egui::TextureOptions::LINEAR,
                                );
                                (texture, delay)
                            })
                            .collect();
                        self.loading_animation = Some(frames);
                        self.loading_decode = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.loading_animation = Some(Vec::new());
                        self.loading_decode = None;
                    }
                },
            }
        }
    }

    /// The frame of the loading animation (`assets/loading.gif`) to show
    /// `elapsed` after it started; asks for a redraw when the next one is
    /// due (`None` until it has been decoded).
    fn loading_frame(
        &mut self,
        ctx: &egui::Context,
        elapsed: std::time::Duration,
    ) -> Option<egui::TextureId> {
        let frames = self.loading_animation.as_ref()?;
        let total: u64 = frames.iter().map(|(_, delay)| *delay).sum();
        if frames.is_empty() || total == 0 {
            return None;
        }
        let mut at = (elapsed.as_millis() as u64) % total;
        for (texture, delay) in frames {
            if at < *delay {
                ctx.request_repaint_after(std::time::Duration::from_millis(delay - at));
                return Some(texture.id());
            }
            at -= delay;
        }
        frames.last().map(|(texture, _)| texture.id())
    }
}

/// Frames of the loading animation and how long each shows (ms); empty if
/// it cannot be decoded.
fn decode_loading_animation() -> Vec<(egui::ColorImage, u64)> {
    use image::AnimationDecoder;
    const GIF: &[u8] = include_bytes!("../../../assets/loading.gif");
    let Ok(decoder) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(GIF)) else {
        return Vec::new();
    };
    let mut frames = Vec::new();
    for frame in decoder.into_frames().take(1000) {
        let Ok(frame) = frame else {
            break;
        };
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let delay = (numerator as u64 / denominator.max(1) as u64).max(20);
        let buffer = frame.into_buffer();
        let size = [buffer.width() as usize, buffer.height() as usize];
        frames.push((
            egui::ColorImage::from_rgba_unmultiplied(size, buffer.as_raw()),
            delay,
        ));
    }
    frames
}

#[cfg(test)]
pub(crate) fn loading_animation_frames() -> usize {
    decode_loading_animation().len()
}

/// The tone curve: a smooth curve through five points that are dragged up
/// and down. Returns whether a point moved.
pub(crate) fn tone_curve_editor(ui: &mut egui::Ui, points: &mut [f32; 5]) -> bool {
    let size = Vec2::new(ui.available_width().min(260.0), 180.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let graph = rect.shrink(8.0);
    let to_screen = |x: f32, y: f32| {
        Pos2::new(
            graph.left() + x * graph.width(),
            graph.bottom() - y * graph.height(),
        )
    };
    let id = response.id.with("point");
    let mut changed = false;
    if (response.drag_started() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let index = (((pointer.x - graph.left()) / graph.width() * 4.0).round() as i32).clamp(0, 4)
            as usize;
        ui.data_mut(|data| data.insert_temp(id, index));
    }
    if (response.dragged() || response.clicked())
        && let Some(pointer) = response.interact_pointer_pos()
        && let Some(index) = ui.data(|data| data.get_temp::<usize>(id))
    {
        points[index] = ((graph.bottom() - pointer.y) / graph.height()).clamp(0.0, 1.0);
        changed = true;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(graph, 4.0, Color32::from_rgb(28, 29, 36));
    let grid = Stroke::new(1.0, Color32::from_gray(52));
    for step in 1..4 {
        let f = step as f32 / 4.0;
        painter.line_segment([to_screen(f, 0.0), to_screen(f, 1.0)], grid);
        painter.line_segment([to_screen(0.0, f), to_screen(1.0, f)], grid);
    }
    painter.line_segment(
        [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
        Stroke::new(1.0, Color32::from_gray(80)),
    );
    let curve: Vec<Pos2> = (0..=128)
        .map(|i| {
            let x = i as f32 / 128.0;
            to_screen(x, efude_canvas::filters::tone_curve_value(*points, x))
        })
        .collect();
    painter.add(egui::Shape::line(
        curve,
        Stroke::new(2.0, crate::layout::ACCENT),
    ));
    for (i, value) in points.iter().enumerate() {
        let center = to_screen(i as f32 / 4.0, *value);
        painter.circle_filled(center, 5.0, Color32::WHITE);
        painter.circle_stroke(center, 5.0, Stroke::new(1.5, crate::layout::ACCENT));
    }
    changed
}
