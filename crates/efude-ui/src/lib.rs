// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use efude_brush::{Brush, BrushKind, DynamicSource, ReferenceTarget, SampleRange};
use efude_canvas::{BlendMode, Document, History, LayerKind, Selection};
use efude_core::InkPoint;
use serde::{Deserialize, Serialize};
enum IoCompletion {
    Saved {
        path: std::path::PathBuf,
        backup: bool,
        state_token: u64,
        document_id: u64,
    },
    Exported {
        path: std::path::PathBuf,
        format: &'static str,
        warnings: Vec<String>,
    },
    Loaded {
        path: std::path::PathBuf,
        document: Document,
    },
    PsdLoaded {
        path: std::path::PathBuf,
        imported: efude_io::PsdImport,
    },
    ImageDocumentLoaded {
        path: std::path::PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    ImageLoaded {
        path: std::path::PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    ReferenceLoaded {
        path: std::path::PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    FilterApplied {
        layer_id: u64,
        document_id: u64,
        state_token: u64,
        pixels: efude_canvas::TilePixels,
    },
    Failed(String),
}
/// The I/O thread has stopped.
#[derive(Debug)]
struct IoWorkerGone;

impl std::fmt::Display for IoWorkerGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the I/O worker has stopped")
    }
}

/// Sends work to the I/O thread and counts what is still in progress
/// (for the busy indicator).
struct IoSender {
    sender: std::sync::mpsc::Sender<IoTask>,
    busy: std::cell::Cell<usize>,
}

impl IoSender {
    fn send(&self, task: IoTask) -> Result<(), IoWorkerGone> {
        let counts = !matches!(task, IoTask::Shutdown);
        self.sender.send(task).map_err(|_| IoWorkerGone)?;
        if counts {
            self.busy.set(self.busy.get() + 1);
        }
        Ok(())
    }
}

enum IoTask {
    Save {
        path: std::path::PathBuf,
        document: Document,
        backup: bool,
        backup_generations: usize,
        state_token: u64,
        document_id: u64,
        repaint: egui::Context,
    },
    Export {
        path: std::path::PathBuf,
        document: Document,
        format: ExportFormat,
        repaint: egui::Context,
    },
    LoadEfude {
        path: std::path::PathBuf,
        repaint: egui::Context,
    },
    LoadPsd {
        path: std::path::PathBuf,
        repaint: egui::Context,
    },
    LoadImageLayer {
        path: std::path::PathBuf,
        max_width: u32,
        max_height: u32,
        repaint: egui::Context,
    },
    LoadImageDocument {
        path: std::path::PathBuf,
        repaint: egui::Context,
    },
    LoadReference {
        path: std::path::PathBuf,
        repaint: egui::Context,
    },
    ApplyFilter {
        layer: Box<efude_canvas::Layer>,
        before: Option<efude_canvas::TilePixels>,
        selection: Option<Vec<u8>>,
        width: u32,
        height: u32,
        operation: FilterOperation,
        document_id: u64,
        state_token: u64,
        repaint: egui::Context,
    },
    Shutdown,
}
#[derive(Clone, Copy)]
enum ExportFormat {
    Png,
    Jpeg,
    Psd,
}
#[derive(Clone, Copy, Debug)]
enum FilterOperation {
    AutoLevels,
    Levels {
        input_black: f32,
        input_white: f32,
        gamma: f32,
        output_black: f32,
        output_white: f32,
    },
    /// Hue shift, then brightness, contrast, saturation and gamma.
    Adjust {
        hue: f32,
        brightness: f32,
        contrast: f32,
        saturation: f32,
        gamma: f32,
    },
    Curve([f32; 5]),
    Image(efude_canvas::filters::Filter),
}

/// Runs a filter over a whole layer (`origin`: where the layer's pixel
/// (0, 0) is in the document, for previews of a part).
fn run_filter(
    layer: &mut efude_canvas::Layer,
    width: u32,
    height: u32,
    operation: FilterOperation,
    origin: (i64, i64),
) {
    match operation {
        FilterOperation::AutoLevels => efude_canvas::auto_levels(layer),
        FilterOperation::Levels {
            input_black,
            input_white,
            gamma,
            output_black,
            output_white,
        } => efude_canvas::levels(
            layer,
            input_black,
            input_white,
            gamma,
            output_black,
            output_white,
        ),
        FilterOperation::Adjust {
            hue,
            brightness,
            contrast,
            saturation,
            gamma,
        } => {
            if hue != 0.0 {
                efude_canvas::hue_saturation(layer, hue, 1.0);
            }
            efude_canvas::color_adjust(layer, brightness, contrast, saturation, gamma);
        }
        FilterOperation::Curve(points) => efude_canvas::apply_tone_curve(layer, points),
        FilterOperation::Image(filter) => {
            let mut dense = layer.pixels.to_dense();
            filter.apply(&mut dense, width as usize, height as usize, origin);
            layer.pixels = efude_canvas::TilePixels::from_dense(width, height, &dense);
        }
    }
}

fn decode_limited_image(
    path: &std::path::Path,
    max_width: u32,
    max_height: u32,
) -> Result<(u32, u32, Vec<u8>), String> {
    let dimensions = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    let source_pixels = (dimensions.0 as u64)
        .checked_mul(dimensions.1 as u64)
        .ok_or("画像サイズが大きすぎます")?;
    if dimensions.0 == 0 || dimensions.1 == 0 || source_pixels > 100_000_000 {
        return Err("画像は1億ピクセル以下にしてください".into());
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(30000);
    limits.max_image_height = Some(30000);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    // Only shrink: `thumbnail` would also enlarge small images (a 64 px
    // grain would become a blurred 4096 px one).
    let image = if image.width() > max_width.max(1) || image.height() > max_height.max(1) {
        image.thumbnail(max_width.max(1), max_height.max(1))
    } else {
        image
    }
    .to_rgba8();
    Ok((image.width(), image.height(), image.into_raw()))
}

fn load_brush_texture(path: &std::path::Path) -> Result<efude_brush::BrushTip, String> {
    const MAX_DIMENSION: u32 = 4096;
    const MAX_PIXELS: u64 = 16 * 1024 * 1024;

    let dimensions = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    let pixels = u64::from(dimensions.0)
        .checked_mul(u64::from(dimensions.1))
        .ok_or("ブラシ画像のサイズが大きすぎます")?;
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > MAX_DIMENSION
        || dimensions.1 > MAX_DIMENSION
        || pixels > MAX_PIXELS
    {
        return Err("ブラシ画像は4096×4096ピクセル以下にしてください".into());
    }

    let (width, height, rgba) = decode_limited_image(path, MAX_DIMENSION, MAX_DIMENSION)?;
    let coverage = rgba
        .chunks_exact(4)
        .map(|pixel| {
            (((pixel[0] as u16 + pixel[1] as u16 + pixel[2] as u16) / 3 * pixel[3] as u16) / 255)
                as u8
        })
        .collect();
    Ok(efude_brush::BrushTip {
        width,
        height,
        coverage,
    })
}

/// Built-in seamless grain images (see `assets/PROVENANCE.md`).
const GRAIN_TEMPLATES: [(&str, &str, &[u8]); 4] = [
    (
        "雲",
        "Clouds",
        include_bytes!("../../../assets/grains/cloud.png"),
    ),
    (
        "布目",
        "Canvas weave",
        include_bytes!("../../../assets/grains/canvas_weave.png"),
    ),
    (
        "水彩紙",
        "Watercolour",
        include_bytes!("../../../assets/grains/watercolor.png"),
    ),
    (
        "チョーク",
        "Chalk",
        include_bytes!("../../../assets/grains/chalk.png"),
    ),
];

/// Grain template `index` as paint coverage (dark paints).
fn grain_template(index: usize) -> Option<efude_brush::BrushTip> {
    let (_, _, bytes) = GRAIN_TEMPLATES.get(index)?;
    let image = image::load_from_memory(bytes).ok()?.to_luma8();
    Some(efude_brush::BrushTip {
        width: image.width(),
        height: image.height(),
        coverage: image.pixels().map(|p| 255 - p.0[0]).collect(),
    })
}

/// A grain image as paint coverage: dark opaque pixels paint (255), white
/// or transparent pixels do not (0).
fn load_grain_texture(path: &std::path::Path) -> Result<efude_brush::BrushTip, String> {
    let mut texture = load_brush_texture(path)?;
    let (_, _, rgba) = decode_limited_image(path, 4096, 4096)?;
    texture.coverage = rgba
        .chunks_exact(4)
        .map(|pixel| {
            let luma =
                (pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000;
            ((255 - luma) * pixel[3] as u32 / 255) as u8
        })
        .collect();
    Ok(texture)
}

fn dynamic_source_name(source: DynamicSource, english: bool) -> &'static str {
    match source {
        DynamicSource::None => {
            if english {
                "None"
            } else {
                "なし"
            }
        }
        DynamicSource::Pressure => {
            if english {
                "Pressure"
            } else {
                "筆圧"
            }
        }
        DynamicSource::Speed => {
            if english {
                "Speed"
            } else {
                "速度"
            }
        }
        DynamicSource::Tilt => {
            if english {
                "Tilt"
            } else {
                "傾き"
            }
        }
        DynamicSource::Direction => {
            if english {
                "Direction"
            } else {
                "方向"
            }
        }
        DynamicSource::Random => {
            if english {
                "Random"
            } else {
                "ランダム"
            }
        }
    }
}
fn blend_mode_name(mode: BlendMode, english: bool) -> &'static str {
    let names = match mode {
        BlendMode::Normal => ("通常", "Normal"),
        BlendMode::Multiply => ("乗算", "Multiply"),
        BlendMode::Screen => ("スクリーン", "Screen"),
        BlendMode::Overlay => ("オーバーレイ", "Overlay"),
        BlendMode::Darken => ("比較（暗）", "Darken"),
        BlendMode::Lighten => ("比較（明）", "Lighten"),
        BlendMode::ColorDodge => ("覆い焼きカラー", "Color Dodge"),
        BlendMode::ColorBurn => ("焼き込みカラー", "Color Burn"),
        BlendMode::HardLight => ("ハードライト", "Hard Light"),
        BlendMode::SoftLight => ("ソフトライト", "Soft Light"),
        BlendMode::Difference => ("差の絶対値", "Difference"),
        BlendMode::Exclusion => ("除外", "Exclusion"),
        BlendMode::Add => ("加算", "Add"),
        BlendMode::Subtract => ("減算", "Subtract"),
    };
    if english { names.1 } else { names.0 }
}
fn localize_io_warning(warning: &str, english: bool) -> String {
    if !english {
        return warning.to_owned();
    }
    match warning {
        "リニア合成設定はPSDに保存されません" => {
            "Linear-light blending is not preserved in PSD.".into()
        }
        "下描き・参照レイヤー属性はPSDに保存されません" => {
            "Sketch and reference layer flags are not preserved in PSD.".into()
        }
        "レイヤーロック設定はPSDに保存されません" => {
            "Layer lock settings are not preserved in PSD.".into()
        }
        "フォルダーの合成モードはPSDに保存されません" => {
            "Folder blend modes are not preserved in PSD.".into()
        }
        "PSDの16bitレイヤーチャンネルを8bitへ変換しました" => {
            "Converted 16-bit PSD layer channels to 8-bit.".into()
        }
        "未対応の調整・塗りつぶし・スマートオブジェクト・レイヤー効果・合成モードなどを統合画像へ置き換えました" => {
            "Unsupported PSD features were replaced with the merged composite.".into()
        }
        "PSDの文字レイヤーをラスター化して読み込みました" => {
            "PSD text layers were imported as raster layers.".into()
        }
        _ => warning.to_owned(),
    }
}
fn dynamic_source_control(
    ui: &mut egui::Ui,
    label: &str,
    source: &mut DynamicSource,
    english: bool,
) {
    egui::ComboBox::from_label(label)
        .selected_text(dynamic_source_name(*source, english))
        .show_ui(ui, |ui| {
            for value in [
                DynamicSource::None,
                DynamicSource::Pressure,
                DynamicSource::Speed,
                DynamicSource::Tilt,
                DynamicSource::Direction,
                DynamicSource::Random,
            ] {
                ui.selectable_value(source, value, dynamic_source_name(value, english));
            }
        });
}
#[derive(Serialize, Deserialize)]
struct PersistedSettings {
    #[serde(default)]
    language_english: bool,
    brushes: Vec<Brush>,
    selected_brush: usize,
    /// Brush of the pen, eraser, blur and smudge tools.
    #[serde(default)]
    tool_brushes: [Option<usize>; 4],
    /// Version of the preset layout the saved brushes grew from (see
    /// `PRESET_VERSION`).
    #[serde(default)]
    preset_version: u32,
    color: [u8; 4],
    palette: Vec<[u8; 4]>,
    #[serde(default = "default_true")]
    show_tools_panel: bool,
    #[serde(default = "default_true")]
    show_layers_panel: bool,
    /// Workspace layout; absent in settings from older versions.
    #[serde(default)]
    workspace: Option<egui_dock::DockState<layout::Pane>>,
    /// Version of the default layout the saved one grew from; older
    /// layouts are replaced once by the current default.
    #[serde(default)]
    layout_version: u32,
    #[serde(default = "default_panel_width")]
    tools_panel_width: f32,
    #[serde(default = "default_panel_width")]
    layers_panel_width: f32,
    #[serde(default = "default_secondary_color")]
    secondary_color: [u8; 4],
    #[serde(default = "default_intermediate_mix")]
    intermediate_mix: f32,
    size: f32,
    zoom: f32,
    view_rotation: f32,
    flip_x: bool,
    flip_y: bool,
    #[serde(default = "default_true")]
    transparency_checker: bool,
    show_grid: bool,
    grid_size: u32,
    #[serde(default)]
    grid_snap: bool,
    symmetry_x: bool,
    symmetry_y: bool,
    #[serde(default = "default_symmetry_count")]
    symmetry_count: u32,
    #[serde(default = "default_symmetry_center")]
    symmetry_center: [f32; 2],
    #[serde(default)]
    perspective_points: Vec<[i32; 2]>,
    #[serde(default)]
    perspective_selected: usize,
    #[serde(default)]
    shortcuts: ShortcutSettings,
    #[serde(default = "default_tone_curve")]
    tone_curve: [f32; 5],
    #[serde(default = "default_pressure_curve_points")]
    pressure_curve_points: [f32; 4],
    #[serde(default = "default_true")]
    use_windows_ink: bool,
    #[serde(default)]
    use_wintab: bool,
    #[serde(default)]
    eyedropper_radius: u32,
    #[serde(default)]
    eyedropper_composite: bool,
    #[serde(default)]
    selection_reference_mode: u8,
    #[serde(default)]
    selection_reference_layer: u64,
    #[serde(default)]
    fill_reference_layer: u64,
    #[serde(default)]
    fill_reference_mode: u8,
    #[serde(default = "default_fill_tolerance")]
    fill_tolerance: u8,
    #[serde(default)]
    fill_gap_close: u8,
    #[serde(default = "default_fill_tolerance")]
    selection_tolerance: u8,
    #[serde(default = "default_reference_zoom")]
    reference_zoom: f32,
    #[serde(default = "default_reference_opacity")]
    reference_opacity: f32,
    #[serde(default)]
    reference_position: Option<[f32; 2]>,
    #[serde(default)]
    reference_image_path: Option<std::path::PathBuf>,
    #[serde(default = "default_backup_interval_minutes")]
    backup_interval_minutes: u32,
    #[serde(default = "default_backup_generations")]
    backup_generations: usize,
    /// Saved erasers were updated to ignore pressure for density (the new
    /// default); done once so a later choice is kept.
    #[serde(default)]
    eraser_pressure_updated: bool,
}
fn default_tone_curve() -> [f32; 5] {
    [0., 0.25, 0.5, 0.75, 1.]
}
fn default_symmetry_count() -> u32 {
    1
}
fn default_symmetry_center() -> [f32; 2] {
    [599.5, 449.5]
}
fn default_pressure_curve_points() -> [f32; 4] {
    [0.25, 0.25, 0.75, 0.75]
}
fn default_fill_tolerance() -> u8 {
    24
}
fn default_reference_zoom() -> f32 {
    0.3
}
fn default_reference_opacity() -> f32 {
    0.75
}
fn default_true() -> bool {
    true
}
fn default_panel_width() -> f32 {
    190.0
}
fn default_secondary_color() -> [u8; 4] {
    [255, 255, 255, 255]
}
fn default_intermediate_mix() -> f32 {
    0.5
}
fn default_backup_interval_minutes() -> u32 {
    5
}
fn default_backup_generations() -> usize {
    10
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct ShortcutSettings {
    undo: String,
    redo: String,
    save: String,
    copy: String,
    cut: String,
    paste: String,
    select_all: String,
    deselect: String,
    eraser: String,
    brush_tool: String,
    rectangle_tool: String,
    fill_tool: String,
    eyedropper_tool: String,
    move_tool: String,
    pan_tool: String,
    blur_tool: String,
    smudge_tool: String,
    ellipse_tool: String,
    lasso_tool: String,
    polygon_tool: String,
    magic_wand_tool: String,
    color_range_tool: String,
    line_ruler_tool: String,
    ellipse_ruler_tool: String,
    bezier_ruler_tool: String,
    perspective_ruler_tool: String,
    selection_brush_tool: String,
    quick_mask_tool: String,
}
impl Default for ShortcutSettings {
    fn default() -> Self {
        Self {
            undo: "Z".into(),
            redo: "Y".into(),
            save: "S".into(),
            copy: "C".into(),
            cut: "X".into(),
            paste: "V".into(),
            select_all: "A".into(),
            deselect: "D".into(),
            eraser: "E".into(),
            brush_tool: "1".into(),
            rectangle_tool: "2".into(),
            fill_tool: "3".into(),
            eyedropper_tool: "4".into(),
            move_tool: "5".into(),
            pan_tool: "6".into(),
            blur_tool: "7".into(),
            smudge_tool: "8".into(),
            ellipse_tool: "9".into(),
            lasso_tool: "Q".into(),
            polygon_tool: "W".into(),
            magic_wand_tool: "R".into(),
            color_range_tool: "T".into(),
            line_ruler_tool: "Y".into(),
            ellipse_ruler_tool: "U".into(),
            bezier_ruler_tool: "I".into(),
            perspective_ruler_tool: "O".into(),
            selection_brush_tool: "G".into(),
            quick_mask_tool: "H".into(),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Brush,
    Blur,
    Smudge,
    RectangleSelect,
    EllipseSelect,
    LassoSelect,
    PolygonSelect,
    MagicWand,
    ColorRange,
    Move,
    Pan,
    Fill,
    Eyedropper,
    Line,
    EllipseRuler,
    BezierRuler,
    PerspectiveRuler,
    PanelSplit,
    Balloon,
    Text,
    SelectionBrush,
    QuickMask,
    Eraser,
    /// Edits the Bézier curves of vector lines.
    VectorEdit,
}
/// See `EfudeApp::move_origin`.
struct MoveOrigin {
    layer: usize,
    pixels: efude_canvas::TilePixels,
    mask: Option<efude_canvas::TilePixels>,
    selection: Vec<u8>,
    selection_active: bool,
    /// Selected area (`[x0, y0, x1, y1)`), `None` for the whole layer.
    bounds: Option<[i32; 4]>,
    start: (i32, i32),
    offset: (i32, i32),
    /// The lines of a vector layer before the move.
    vector: Option<Vec<efude_canvas::VectorStroke>>,
}

/// Takes Ctrl with "+" (also "=" and ";", which give "+" with Shift on
/// common layouts, and the keypad plus), "-" and "0" from the input and
/// returns (zoom in, zoom out, fit). Logical and physical keys both count,
/// so the shortcuts work whatever the keyboard layout.
fn canvas_zoom_keys(input: &mut egui::InputState) -> (bool, bool, bool) {
    use egui::Key;
    let mut found = (false, false, false);
    input.events.retain(|event| {
        let egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            modifiers,
            ..
        } = event
        else {
            return true;
        };
        if !modifiers.command || modifiers.alt {
            return true;
        }
        let is = |wanted: &[Key]| {
            wanted.contains(key) || physical_key.is_some_and(|k| wanted.contains(&k))
        };
        if is(&[Key::Plus, Key::Equals, Key::Semicolon]) {
            found.0 = true;
        } else if is(&[Key::Minus]) {
            found.1 = true;
        } else if is(&[Key::Num0]) {
            found.2 = true;
        } else {
            return true;
        }
        false
    });
    found
}

/// Index of a brush tool in `EfudeApp::tool_brushes`.
fn brush_tool_slot(tool: Tool) -> Option<usize> {
    match tool {
        Tool::Brush => Some(0),
        Tool::Eraser => Some(1),
        Tool::Blur => Some(2),
        Tool::Smudge => Some(3),
        _ => None,
    }
}

/// The brush tool that draws with a brush of `kind`.
fn tool_for_brush(kind: BrushKind) -> Tool {
    match kind {
        BrushKind::Eraser => Tool::Eraser,
        BrushKind::Blur => Tool::Blur,
        BrushKind::Smudge => Tool::Smudge,
        _ => Tool::Brush,
    }
}

fn brush_fits_tool(kind: BrushKind, tool: Tool) -> bool {
    tool_for_brush(kind) == tool
}

/// A tool key being held down (see `update_held_tool_key`).
#[derive(Clone, Copy)]
struct HeldToolKey {
    key: egui::Key,
    /// Tool to return to when the hold ends.
    previous: Tool,
    pressed_at: std::time::Instant,
    /// The tool was used on the canvas while the key was held.
    used: bool,
    /// The key was let go; waiting for the pen to lift.
    released: bool,
    /// Only works while held (Space).
    hold_only: bool,
}

/// Mip levels of the canvas display texture (down to about 16 px).
fn canvas_mip_levels(width: u32, height: u32) -> u32 {
    let largest = width.max(height).max(1);
    (32 - largest.leading_zeros())
        .saturating_sub(4)
        .clamp(1, 12)
}

/// What the cached grain preview shows: size, a checksum and the switches.
type GrainPreviewKey = (u32, u32, u64, bool, bool);

/// Largest brush diameter in pixels (big enough for 600 dpi pages).
const MAX_BRUSH_SIZE: f32 = 1000.0;

/// A key held at least this long switches tools only while it is held.
const TOOL_KEY_HOLD: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Clone, Copy, Default)]
enum SelectionCombineMode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}
struct GpuCanvasSurface {
    texture: wgpu::Texture,
    storage_view: wgpu::TextureView,
    texture_id: egui::TextureId,
    width: u32,
    height: u32,
}
/// Counts shown in the tools panel so pressure problems can be diagnosed
/// without a debugger: tablet samples used, tablet samples that landed
/// outside the canvas, and pressure-less window-input fallbacks.
#[derive(Clone, Copy, Default)]
struct InputDiagnostics {
    tablet_samples: u32,
    outside_canvas: u32,
    window_fallbacks: u32,
    min_pressure: Option<f32>,
    max_pressure: Option<f32>,
}

impl InputDiagnostics {
    fn record_pressure(&mut self, pressure: f32) {
        self.min_pressure = Some(self.min_pressure.map_or(pressure, |p| p.min(pressure)));
        self.max_pressure = Some(self.max_pressure.map_or(pressure, |p| p.max(pressure)));
    }
}

/// Everything a provisional stroke tail changes, so it can be taken back
/// before the next update draws a new tail.
struct ProvisionalSnapshot {
    layer: usize,
    pixels: efude_canvas::TilePixels,
    mask: Option<efude_canvas::TilePixels>,
    raster: efude_brush::engine::StrokeRaster,
}

pub struct EfudeApp {
    language_english: bool,
    doc: Document,
    brushes: Vec<Brush>,
    selected_brush: usize,
    selected_layer: usize,
    editing_mask: bool,
    color: Color32,
    secondary_color: Color32,
    intermediate_mix: f32,
    palette: Vec<[u8; 4]>,
    show_tools_panel: bool,
    show_layers_panel: bool,
    show_help: bool,
    show_settings: bool,
    workspace: egui_dock::DockState<layout::Pane>,
    /// Where the canvas was drawn last frame (screen rect, screen pixels per
    /// document pixel).
    canvas_screen: Option<(Rect, f32)>,
    /// Undo and redo buttons in the menu bar (for tests).
    history_buttons: [Rect; 2],
    comic_ui: comic::ComicUi,
    balloon_ui: balloons::BalloonUi,
    book_ui: book::BookUi,
    tabs: tabs::Tabs,
    show_new_document: bool,
    /// Brush preset waiting for delete confirmation.
    pending_brush_delete: Option<usize>,
    brush_previews: std::collections::HashMap<usize, layout::BrushPreview>,
    tools_panel_width: f32,
    layers_panel_width: f32,
    size: f32,
    active: Vec<InkPoint>,
    /// Rasterizer state of the stroke being painted (engine crate).
    raster: efude_brush::engine::StrokeRaster,
    /// Screen pixels per document pixel in the canvas view.
    view_scale: f32,
    /// Streaming stroke state for the brush tools; `None` between strokes.
    stroke_builder: Option<efude_stroke::StrokeBuilder>,
    /// State to restore before the provisional stroke tail is redrawn.
    provisional: Option<ProvisionalSnapshot>,
    /// Latest provisional tail, drawn once per frame by `flush_provisional`.
    pending_provisional: Vec<InkPoint>,
    /// Where the samples of the current/last stroke came from.
    input_diagnostics: InputDiagnostics,
    /// Set once the automatic Windows Ink -> WinTab switch has been tried.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    tried_wintab_fallback: bool,
    /// Why WinTab could not be started, shown next to the input status.
    wintab_error: Option<String>,
    stabilized_cursor: Option<Vec2>,
    preview_cursor_estimate: Option<InkPoint>,
    pending_canvas_latency: Option<std::time::Instant>,
    canvas_latency_samples_ms: std::collections::VecDeque<f32>,
    last_canvas_latency_ms: Option<f32>,
    gpu_render_state: Option<eframe::egui_wgpu::RenderState>,
    latency_sender: std::sync::mpsc::Sender<f32>,
    latency_receiver: std::sync::mpsc::Receiver<f32>,
    history: History,
    selection: Selection,
    clipboard: Option<(u32, u32, Vec<u8>)>,
    paste_preview: Option<(u32, u32, Vec<u8>, Vec2)>,
    paste_texture: Option<egui::TextureHandle>,
    tool: Tool,
    held_tool_key: Option<HeldToolKey>,
    selection_start: Option<(i32, i32)>,
    selection_before_gesture: Option<(bool, Vec<u8>)>,
    filter_settings: filters_ui::FilterSettings,
    filter_dialog: Option<filters_ui::FilterDialog>,
    /// Frames of the loading animation, once decoded (see `busy_overlay`).
    loading_animation: Option<Vec<(egui::TextureHandle, u64)>>,
    loading_decode: Option<std::sync::mpsc::Receiver<Vec<(egui::ColorImage, u64)>>>,
    /// When the I/O thread became busy (for the busy indicator).
    busy_since: Option<std::time::Instant>,
    /// Preset being renamed and the name typed so far.
    brush_rename: Option<(usize, String)>,
    /// Zoom keys held last frame (Windows; see `update_ui`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    zoom_keys_down: (bool, bool),
    /// State of the random grain turn (see `begin_grain`).
    grain_seed: u64,
    /// Ctrl+V was held last frame (see `command_shortcuts`).
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    paste_chord_down: bool,
    /// The layer and selection as they were when a move began: every frame
    /// of the drag moves these originals by the whole offset, so what lies
    /// under the moved part is covered, never picked up.
    move_origin: Option<MoveOrigin>,
    /// The vector stroke being drawn or erased.
    vector_live: Option<vector_tools::VectorLive>,
    /// The vector line (and anchor) the control point tool works on.
    pub(crate) vector_edit: vector_edit::VectorEdit,
    /// The eraser removes whole vector lines instead of the parts it
    /// touches.
    pub(crate) vector_erase_whole: bool,
    /// The layer being dragged in the layer panel (by id).
    pub(crate) layer_drag: Option<u64>,
    /// Where each layer row was drawn last frame (by layer id).
    pub(crate) layer_rows: Vec<(u64, Rect)>,
    selection_combine_mode: SelectionCombineMode,
    pan_start: Option<Vec2>,
    selection_points: Vec<(i32, i32)>,
    bezier_points: Vec<Vec2>,
    perspective_points: Vec<(i32, i32)>,
    perspective_selected: usize,
    setting_vanishing_point: bool,
    selection_tolerance: u8,
    selection_reference_mode: u8,
    selection_reference_layer: u64,
    hue_shift: f32,
    levels_input_black: f32,
    levels_input_white: f32,
    levels_gamma: f32,
    levels_output_black: f32,
    levels_output_white: f32,
    brightness: f32,
    contrast: f32,
    saturation: f32,
    tone_gamma: f32,
    tone_curve: [f32; 5],
    pressure_curve_points: [f32; 4],
    zoom: f32,
    navigator_center: Vec2,
    navigator_texture: Option<egui::TextureHandle>,
    canvas_texture: Option<egui::TextureHandle>,
    cpu_mips: display::CpuMips,
    /// Last canvas view: its screen area, the fit scale and points per pixel.
    canvas_viewport: (Rect, f32, f32),
    window_checked: bool,
    show_brush_settings: bool,
    /// Tool seen by the previous frame (see `sync_tool_change`).
    last_tool: Tool,
    /// Brush remembered by each brush tool (pen, eraser, blur, smudge).
    tool_brushes: [Option<usize>; 4],
    show_view_options: bool,
    grain_preview: Option<(GrainPreviewKey, egui::TextureHandle)>,
    /// Tablet packets have arrived in this session.
    tablet_seen: bool,
    stroke_started_at: Option<std::time::Instant>,
    window_samples_started: bool,
    gpu_canvas_surface: Option<GpuCanvasSurface>,
    gpu_canvas_disabled: bool,
    navigator_texture_dirty: bool,
    canvas_texture_dirty: bool,
    dirty_canvas_tiles: std::collections::HashSet<(u32, u32)>,
    gpu_dab_pipeline: Option<efude_gpu::GpuDabPipeline>,
    view_rotation: f32,
    flip_x: bool,
    flip_y: bool,
    /// Transparent parts of the canvas show a checkerboard.
    pub(crate) transparency_checker: bool,
    /// The drawing colour is "transparent": brushes and fills erase.
    pub(crate) transparent_color: bool,
    show_grid: bool,
    grid_size: u32,
    grid_snap: bool,
    gesture_end: Option<(i32, i32)>,
    symmetry_x: bool,
    symmetry_y: bool,
    symmetry_count: u32,
    symmetry_center: Vec2,
    doc_path: Option<std::path::PathBuf>,
    last_backup: std::time::Instant,
    stroke_log: efude_input::StrokeLog,
    pen_queue: efude_input::PenInputQueue,
    frame_pen_packets: Vec<efude_input::PenPacket>,
    use_windows_ink: bool,
    use_wintab: bool,
    io_task_sender: IoSender,
    io_receiver: std::sync::mpsc::Receiver<IoCompletion>,
    io_worker: Option<std::thread::JoinHandle<()>>,
    filter_pending: bool,
    #[cfg(target_os = "windows")]
    windows_ink_hook: Option<efude_input::WindowsInkHook>,
    #[cfg(target_os = "windows")]
    wintab_hook: Option<efude_input::WintabHook>,
    #[cfg(target_os = "windows")]
    windows_ink_hwnd: isize,
    comparison_brush: Option<Brush>,
    recording_stroke: bool,
    status: String,
    logo: Option<egui::TextureHandle>,
    color_wheel: Option<egui::TextureHandle>,
    reference_image: Option<egui::TextureHandle>,
    reference_image_path: Option<std::path::PathBuf>,
    reference_size: Vec2,
    reference_opacity: f32,
    reference_zoom: f32,
    reference_position: Option<Pos2>,
    selection_erase: bool,
    /// GPU painting of opacity-capped strokes, when a GPU is available.
    gpu_cover: Option<efude_gpu::GpuCover>,
    /// Dab area (pixels) below which a batch stays on the CPU, where it is
    /// faster than the upload and readback.
    gpu_min_pixels: f32,
    dynamic_size: f32,
    dynamic_opacity: f32,
    dynamic_concentration: f32,
    dynamic_mix: f32,
    dynamic_dilution: f32,
    fill_tolerance: u8,
    fill_reference_mode: u8,
    fill_reference_layer: u64,
    fill_gap_close: u8,
    selection_radius: u32,
    selection_feather_radius: u32,
    transform_scale_x: f32,
    transform_scale_y: f32,
    transform_angle: f32,
    mesh_offsets: [[f32; 2]; 16],
    eyedropper_radius: u32,
    eyedropper_composite: bool,
    shortcuts: ShortcutSettings,
    canvas_width_input: u32,
    canvas_height_input: u32,
    canvas_dpi_input: f32,
    backup_interval_minutes: u32,
    backup_generations: usize,
    close_after_save: bool,
    close_without_saving: bool,
}
impl Default for EfudeApp {
    fn default() -> Self {
        let (task_sender, task_receiver) = std::sync::mpsc::channel();
        let (completion_sender, io_receiver) = std::sync::mpsc::channel();
        let (latency_sender, latency_receiver) = std::sync::mpsc::channel();
        let io_worker = std::thread::Builder::new()
            .name("efude-io-worker".into())
            .spawn(move || {
                while let Ok(task) = task_receiver.recv() {
                    match task {
                        IoTask::Shutdown => break,
                        IoTask::LoadEfude { path, repaint } => {
                            let completion = match efude_io::load(&path) {
                                Ok(document) => IoCompletion::Loaded { path, document },
                                Err(error) => IoCompletion::Failed(error.to_string()),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::LoadPsd { path, repaint } => {
                            let completion = match efude_io::import_psd_report(&path) {
                                Ok(imported) => IoCompletion::PsdLoaded { path, imported },
                                Err(error) => IoCompletion::Failed(error.to_string()),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::LoadImageLayer {
                            path,
                            max_width,
                            max_height,
                            repaint,
                        } => {
                            let decoded = decode_limited_image(&path, max_width, max_height);
                            let completion = match decoded {
                                Ok((width, height, rgba)) => IoCompletion::ImageLoaded {
                                    path,
                                    width,
                                    height,
                                    rgba,
                                },
                                Err(error) => IoCompletion::Failed(format!(
                                    "画像をレイヤーとして読み込めません: {error}"
                                )),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::LoadImageDocument { path, repaint } => {
                            let decoded = decode_limited_image(
                                &path,
                                efude_canvas::MAX_DOCUMENT_DIMENSION,
                                efude_canvas::MAX_DOCUMENT_DIMENSION,
                            );
                            let completion = match decoded {
                                Ok((width, height, rgba)) => IoCompletion::ImageDocumentLoaded {
                                    path,
                                    width,
                                    height,
                                    rgba,
                                },
                                Err(error) => {
                                    IoCompletion::Failed(format!("画像を開けません: {error}"))
                                }
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::LoadReference { path, repaint } => {
                            let decoded = decode_limited_image(&path, 2048, 2048);
                            let completion = match decoded {
                                Ok((width, height, rgba)) => IoCompletion::ReferenceLoaded {
                                    path,
                                    width,
                                    height,
                                    rgba,
                                },
                                Err(error) => IoCompletion::Failed(format!(
                                    "参照画像を読み込めません: {error}"
                                )),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::Save {
                            path,
                            document,
                            backup,
                            backup_generations,
                            state_token,
                            document_id,
                            repaint,
                        } => {
                            let result = if backup {
                                efude_io::save_backup(&path, &document, backup_generations)
                            } else {
                                efude_io::save(&path, &document)
                            };
                            let completion = match result {
                                Ok(()) => IoCompletion::Saved {
                                    path,
                                    backup,
                                    state_token,
                                    document_id,
                                },
                                Err(error) => IoCompletion::Failed(error.to_string()),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::Export {
                            path,
                            document,
                            format,
                            repaint,
                        } => {
                            let result: Result<Vec<String>, Box<dyn std::error::Error>> =
                                match format {
                                    ExportFormat::Png => {
                                        efude_io::export_png(&path, &document).map(|()| Vec::new())
                                    }
                                    ExportFormat::Jpeg => {
                                        efude_io::export_jpeg(&path, &document, 92)
                                            .map(|()| Vec::new())
                                    }
                                    ExportFormat::Psd => {
                                        efude_io::export_psd_report(&path, &document)
                                    }
                                };
                            let completion = match result {
                                Ok(warnings) => IoCompletion::Exported {
                                    path,
                                    warnings,
                                    format: match format {
                                        ExportFormat::Png => "PNG",
                                        ExportFormat::Jpeg => "JPEG",
                                        ExportFormat::Psd => "PSD",
                                    },
                                },
                                Err(error) => IoCompletion::Failed(error.to_string()),
                            };
                            let _ = completion_sender.send(completion);
                            repaint.request_repaint();
                        }
                        IoTask::ApplyFilter {
                            mut layer,
                            before,
                            selection,
                            width,
                            height,
                            operation,
                            document_id,
                            state_token,
                            repaint,
                        } => {
                            run_filter(&mut layer, width, height, operation, (0, 0));
                            if let (Some(mask), Some(before)) = (selection, before) {
                                let pixels = &mut layer.pixels;
                                for y in 0..height {
                                    for x in 0..width {
                                        let index = (y * width + x) as usize;
                                        let coverage =
                                            mask.get(index).copied().unwrap_or(0) as f32 / 255.0;
                                        if coverage >= 1.0 {
                                            continue;
                                        }
                                        let old = before.pixel(x, y);
                                        let filtered = pixels.pixel(x, y);
                                        let result = std::array::from_fn(|channel| {
                                            (old[channel] as f32 * (1.0 - coverage)
                                                + filtered[channel] as f32 * coverage)
                                                .round()
                                                .clamp(0.0, 255.0)
                                                as u8
                                        });
                                        pixels.set_pixel(x, y, result);
                                    }
                                }
                                pixels.prune_empty_tiles();
                            }
                            let _ = completion_sender.send(IoCompletion::FilterApplied {
                                layer_id: layer.id,
                                document_id,
                                state_token,
                                pixels: layer.pixels,
                            });
                            repaint.request_repaint();
                        }
                    }
                }
            })
            .expect("failed to start Efude I/O worker");
        Self {
            language_english: false,
            doc: Document::new(1200, 900),
            brushes: default_presets(),
            selected_brush: 0,
            selected_layer: 0,
            editing_mask: false,
            color: Color32::from_rgb(35, 35, 42),
            secondary_color: Color32::WHITE,
            intermediate_mix: 0.5,
            palette: vec![
                [35, 35, 42, 255],
                [255, 255, 255, 255],
                [220, 55, 70, 255],
                [245, 160, 40, 255],
                [250, 220, 70, 255],
                [70, 180, 110, 255],
                [50, 150, 220, 255],
                [105, 85, 210, 255],
                [200, 90, 190, 255],
            ],
            show_tools_panel: true,
            show_layers_panel: true,
            show_help: false,
            show_settings: false,
            workspace: layout::default_workspace(),
            canvas_screen: None,
            history_buttons: [Rect::NOTHING; 2],
            comic_ui: comic::ComicUi::default(),
            balloon_ui: balloons::BalloonUi::new(),
            book_ui: book::BookUi::new(),
            tabs: tabs::Tabs::with_one(),
            show_new_document: false,
            pending_brush_delete: None,
            brush_previews: std::collections::HashMap::new(),
            tools_panel_width: default_panel_width(),
            layers_panel_width: default_panel_width(),
            size: 4.,
            active: Vec::new(),
            raster: efude_brush::engine::StrokeRaster::default(),
            view_scale: 1.0,
            stroke_builder: None,
            provisional: None,
            pending_provisional: Vec::new(),
            input_diagnostics: InputDiagnostics::default(),
            tried_wintab_fallback: false,
            wintab_error: None,
            stabilized_cursor: None,
            preview_cursor_estimate: None,
            pending_canvas_latency: None,
            canvas_latency_samples_ms: std::collections::VecDeque::new(),
            last_canvas_latency_ms: None,
            gpu_render_state: None,
            latency_sender,
            latency_receiver,
            history: History::default(),
            selection: Selection::default(),
            clipboard: None,
            paste_preview: None,
            paste_texture: None,
            tool: Tool::Brush,
            held_tool_key: None,
            selection_start: None,
            selection_before_gesture: None,
            move_origin: None,
            vector_live: None,
            vector_edit: Default::default(),
            vector_erase_whole: false,
            layer_drag: None,
            layer_rows: Vec::new(),
            paste_chord_down: false,
            zoom_keys_down: (false, false),
            brush_rename: None,
            filter_settings: Default::default(),
            filter_dialog: None,
            busy_since: None,
            loading_animation: None,
            loading_decode: None,
            grain_seed: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64)
                | 1,
            selection_combine_mode: SelectionCombineMode::Replace,
            pan_start: None,
            selection_points: Vec::new(),
            bezier_points: Vec::new(),
            perspective_points: Vec::new(),
            perspective_selected: 0,
            setting_vanishing_point: false,
            selection_tolerance: 24,
            selection_reference_mode: 0,
            selection_reference_layer: 0,
            hue_shift: 0.0,
            levels_input_black: 0.0,
            levels_input_white: 1.0,
            levels_gamma: 1.0,
            levels_output_black: 0.0,
            levels_output_white: 1.0,
            brightness: 0.0,
            contrast: 0.0,
            saturation: 1.0,
            tone_gamma: 1.0,
            tone_curve: [0., 0.25, 0.5, 0.75, 1.],
            pressure_curve_points: default_pressure_curve_points(),
            zoom: 1.0,
            navigator_center: Vec2::new(600., 450.),
            navigator_texture: None,
            canvas_texture: None,
            cpu_mips: display::CpuMips::default(),
            canvas_viewport: (Rect::NOTHING, 1.0, 1.0),
            window_checked: false,
            show_brush_settings: false,
            last_tool: Tool::Brush,
            tool_brushes: [None; 4],
            show_view_options: false,
            grain_preview: None,
            tablet_seen: false,
            stroke_started_at: None,
            window_samples_started: false,
            gpu_canvas_surface: None,
            gpu_canvas_disabled: false,
            navigator_texture_dirty: true,
            canvas_texture_dirty: true,
            dirty_canvas_tiles: std::collections::HashSet::new(),
            gpu_dab_pipeline: None,
            view_rotation: 0.,
            flip_x: false,
            flip_y: false,
            show_grid: false,
            transparency_checker: true,
            transparent_color: false,
            grid_size: 64,
            grid_snap: false,
            gesture_end: None,
            symmetry_x: false,
            symmetry_y: false,
            symmetry_count: 1,
            symmetry_center: Vec2::new(599.5, 449.5),
            doc_path: None,
            last_backup: std::time::Instant::now(),
            stroke_log: efude_input::StrokeLog::default(),
            pen_queue: efude_input::PenInputQueue::default(),
            frame_pen_packets: Vec::new(),
            use_windows_ink: true,
            use_wintab: false,
            io_task_sender: IoSender {
                sender: task_sender,
                busy: std::cell::Cell::new(0),
            },
            io_receiver,
            io_worker: Some(io_worker),
            filter_pending: false,
            #[cfg(target_os = "windows")]
            windows_ink_hook: None,
            #[cfg(target_os = "windows")]
            wintab_hook: None,
            #[cfg(target_os = "windows")]
            windows_ink_hwnd: 0,
            comparison_brush: None,
            recording_stroke: false,
            status: "新しいキャンバス".into(),
            logo: None,
            color_wheel: None,
            reference_image: None,
            reference_image_path: None,
            reference_size: Vec2::ZERO,
            reference_opacity: 0.75,
            reference_zoom: 0.3,
            reference_position: None,
            selection_erase: false,
            gpu_cover: None,
            gpu_min_pixels: 250_000.0,
            dynamic_size: 1.0,
            dynamic_opacity: 1.0,
            dynamic_concentration: 1.0,
            dynamic_mix: 1.0,
            dynamic_dilution: 1.0,
            fill_tolerance: 24,
            fill_reference_mode: 0,
            fill_reference_layer: 0,
            fill_gap_close: 0,
            selection_radius: 1,
            selection_feather_radius: 8,
            transform_scale_x: 1.0,
            transform_scale_y: 1.0,
            transform_angle: 0.0,
            mesh_offsets: [[0.; 2]; 16],
            eyedropper_radius: 0,
            eyedropper_composite: true,
            shortcuts: ShortcutSettings::default(),
            canvas_width_input: 1200,
            canvas_height_input: 900,
            canvas_dpi_input: 300.0,
            backup_interval_minutes: default_backup_interval_minutes(),
            backup_generations: default_backup_generations(),
            close_after_save: false,
            close_without_saving: false,
        }
    }
}
impl EfudeApp {
    fn text<'a>(&self, japanese: &'a str, english: &'a str) -> &'a str {
        if self.language_english {
            english
        } else {
            japanese
        }
    }

    /// Keeps the layers stacked as the panel shows them (see
    /// `History::tidy_layer_tree`), keeping the selection on its layer.
    fn tidy_layers(&mut self) {
        let selected = self.doc.layers.get(self.selected_layer).map(|l| l.id);
        if self.history.tidy_layer_tree(&mut self.doc.layers) {
            if let Some(index) =
                selected.and_then(|id| self.doc.layers.iter().position(|l| l.id == id))
            {
                self.selected_layer = index;
            }
            self.canvas_texture_dirty = true;
            self.navigator_texture_dirty = true;
        }
    }
    /// Adds a layer just above the selected one, in the same folder, and
    /// selects it. One Undo step.
    pub(crate) fn insert_layer_above_selected(&mut self, mut layer: efude_canvas::Layer) {
        let at = (self.selected_layer + 1).min(self.doc.layers.len());
        layer.parent_id = self
            .doc
            .layers
            .get(self.selected_layer)
            .and_then(|selected| selected.parent_id);
        self.history.insert_layer(&mut self.doc.layers, at, layer);
        self.selected_layer = at;
        self.editing_mask = false;
    }
    /// Whether a brush of this kind erases because the drawing colour is
    /// transparent (blur and smudge do not use the colour).
    pub(crate) fn erases_with_transparent_color(&self, kind: BrushKind) -> bool {
        self.transparent_color
            && !matches!(self.tool, Tool::Blur | Tool::Smudge)
            && !matches!(kind, BrushKind::Blur | BrushKind::Smudge)
    }
    /// Square size of the checkerboard behind transparent parts of the
    /// canvas (0 when it is turned off: white).
    pub(crate) fn display_checker(&self) -> u32 {
        if self.transparency_checker {
            efude_canvas::checker_size(self.doc.width, self.doc.height)
        } else {
            0
        }
    }
    fn select_layer(&mut self, index: usize) {
        self.selected_layer = index.min(self.doc.layers.len().saturating_sub(1));
        self.editing_mask = false;
    }
    fn sync_mask_edit_mode(&mut self) {
        self.selected_layer = self
            .selected_layer
            .min(self.doc.layers.len().saturating_sub(1));
        if self.doc.layers[self.selected_layer].kind == LayerKind::Folder
            || self.doc.layers[self.selected_layer].mask.is_none()
        {
            self.editing_mask = false;
        }
    }
    fn is_reference_layer(&self, index: usize) -> bool {
        let Some(layer) = self.doc.layers.get(index) else {
            return false;
        };
        if layer.reference {
            return true;
        }
        let mut parent_id = layer.parent_id;
        let mut remaining = self.doc.layers.len();
        while let Some(id) = parent_id {
            if remaining == 0 {
                break;
            }
            remaining -= 1;
            let Some(parent) = self.doc.layers.iter().find(|candidate| candidate.id == id) else {
                break;
            };
            if parent.reference {
                return true;
            }
            parent_id = parent.parent_id;
        }
        false
    }
    fn document_snapshot(&self) -> Document {
        Document {
            width: self.doc.width,
            height: self.doc.height,
            dpi: self.doc.dpi,
            layers: self.doc.layers.clone(),
            metadata: self.doc.metadata.clone(),
        }
    }
    fn selection_reference_pixels(&self) -> Vec<u8> {
        match self.selection_reference_mode {
            3 => self
                .doc
                .layers
                .iter()
                .position(|layer| layer.id == self.selection_reference_layer)
                .map(|index| self.layer_reference_pixels(index))
                .unwrap_or_else(|| self.layer_reference_pixels(self.selected_layer)),
            1 => {
                let mut reference = self.doc.clone();
                let has_reference = reference.layers.iter().enumerate().any(|(index, layer)| {
                    layer.kind == LayerKind::Raster && self.is_reference_layer(index)
                });
                if has_reference {
                    for (index, layer) in reference.layers.iter_mut().enumerate() {
                        if layer.kind == LayerKind::Raster && !self.is_reference_layer(index) {
                            layer.visible = false;
                        }
                    }
                    efude_canvas::composite_transparent(&reference)
                } else {
                    self.layer_reference_pixels(self.selected_layer)
                }
            }
            2 => efude_canvas::composite_transparent(&self.doc),
            _ => self.layer_reference_pixels(self.selected_layer),
        }
    }
    fn prepare_gpu_composite_tile(
        &self,
        tile_x: u32,
        tile_y: u32,
    ) -> Option<(u32, u32, Vec<efude_gpu::GpuCompositeLayer>)> {
        let origin_x = tile_x.checked_mul(efude_canvas::TILE_SIZE)?;
        let origin_y = tile_y.checked_mul(efude_canvas::TILE_SIZE)?;
        if origin_x >= self.doc.width || origin_y >= self.doc.height {
            return None;
        }
        let width = efude_canvas::TILE_SIZE.min(self.doc.width - origin_x);
        let height = efude_canvas::TILE_SIZE.min(self.doc.height - origin_y);
        let indices = self
            .doc
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| (layer.id, index))
            .collect::<std::collections::HashMap<_, _>>();
        let mut gpu_layers = Vec::new();
        for layer in self
            .doc
            .layers
            .iter()
            .filter(|layer| layer.kind == LayerKind::Raster)
        {
            let Some(pixels) = layer.pixels.tile_data(tile_x, tile_y) else {
                continue;
            };
            let mut visible = layer.visible;
            let mut opacity = layer.opacity.clamp(0.0, 1.0);
            let mut ancestor_masks = Vec::new();
            let mut parent = layer.parent_id;
            let mut depth = 0;
            while let Some(id) = parent {
                if depth >= self.doc.layers.len() {
                    break;
                }
                depth += 1;
                let Some(group) = indices.get(&id).map(|index| &self.doc.layers[*index]) else {
                    break;
                };
                visible &= group.visible;
                opacity *= group.opacity.clamp(0.0, 1.0);
                if let Some(mask) = &group.mask {
                    ancestor_masks.push(mask);
                }
                parent = group.parent_id;
            }
            if !visible {
                continue;
            }
            let mut mask_tiles = ancestor_masks
                .iter()
                .filter_map(|mask| mask.tile_data(tile_x, tile_y))
                .collect::<Vec<_>>();
            if let Some(mask) = &layer.mask
                && let Some(tile) = mask.tile_data(tile_x, tile_y)
            {
                mask_tiles.push(tile);
            }
            let mut coverage = vec![1.0f32; (efude_canvas::TILE_SIZE.pow(2)) as usize];
            for mask in mask_tiles {
                for (index, value) in coverage.iter_mut().enumerate() {
                    *value *= mask[index * 4] as f32 / 255.0;
                }
            }
            let blend_mode = match layer.blend {
                BlendMode::Normal => 0,
                BlendMode::Multiply => 1,
                BlendMode::Screen => 2,
                BlendMode::Overlay => 3,
                BlendMode::Darken => 4,
                BlendMode::Lighten => 5,
                BlendMode::ColorDodge => 6,
                BlendMode::ColorBurn => 7,
                BlendMode::HardLight => 8,
                BlendMode::SoftLight => 9,
                BlendMode::Difference => 10,
                BlendMode::Exclusion => 11,
                BlendMode::Add => 12,
                BlendMode::Subtract => 13,
            };
            gpu_layers.push(efude_gpu::GpuCompositeLayer {
                pixels: efude_canvas::display_tile(
                    layer,
                    self.doc.dpi,
                    tile_x,
                    tile_y,
                    (0, 0),
                    pixels,
                )
                .into_owned(),
                coverage,
                opacity,
                blend_mode,
                linear_blend: layer.linear_blend,
                clipping: layer.clipping,
            });
        }
        if gpu_layers.len() > 200 {
            return None;
        }
        Some((width, height, gpu_layers))
    }
    fn ensure_gpu_canvas_surface(&mut self) -> bool {
        if self.gpu_canvas_disabled {
            return false;
        }
        let Some(render_state) = self.gpu_render_state.clone() else {
            return false;
        };
        if let Some(surface) = &self.gpu_canvas_surface
            && surface.width == self.doc.width
            && surface.height == self.doc.height
        {
            return true;
        }
        let device = &render_state.device;
        if self.doc.width > device.limits().max_texture_dimension_2d
            || self.doc.height > device.limits().max_texture_dimension_2d
        {
            self.disable_gpu_canvas();
            return false;
        }
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("efude-canvas-surface"),
            size: wgpu::Extent3d {
                width: self.doc.width,
                height: self.doc.height,
                depth_or_array_layers: 1,
            },
            // Mip levels keep the zoomed-out view free of aliasing.
            mip_level_count: canvas_mip_levels(self.doc.width, self.doc.height),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
        });
        let storage_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            mip_level_count: Some(1),
            ..Default::default()
        });
        // sRGB views cannot carry the storage usage of the texture.
        let sample_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
            ..Default::default()
        });
        let clear_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            mip_level_count: Some(1),
            usage: Some(wgpu::TextureUsages::RENDER_ATTACHMENT),
            ..Default::default()
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("efude-clear-canvas-surface"),
        });
        {
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &clear_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("efude-clear-canvas-pass"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        render_state.queue.submit(Some(encoder.finish()));
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            eprintln!("efude: canvas surface: {error}");
            self.disable_gpu_canvas();
            return false;
        }
        // Exact pixels when zoomed in (no smoothing between pixels), mip
        // levels blended when zoomed out.
        let texture_id = render_state
            .renderer
            .write()
            .register_native_texture_with_sampler_options(
                device,
                &sample_view,
                wgpu::SamplerDescriptor {
                    label: Some("efude-canvas-sampler"),
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Linear,
                    ..Default::default()
                },
            );
        if let Some(previous) = self.gpu_canvas_surface.replace(GpuCanvasSurface {
            texture,
            storage_view,
            texture_id,
            width: self.doc.width,
            height: self.doc.height,
        }) {
            render_state
                .renderer
                .write()
                .free_texture(&previous.texture_id);
        }
        true
    }
    fn disable_gpu_canvas(&mut self) {
        eprintln!("efude: GPU canvas display failed; using the CPU display");
        if let (Some(surface), Some(render_state)) = (
            self.gpu_canvas_surface.take(),
            self.gpu_render_state.as_ref(),
        ) {
            render_state
                .renderer
                .write()
                .free_texture(&surface.texture_id);
        }
        self.gpu_canvas_disabled = true;
    }
    fn update_gpu_canvas_surface(&mut self, full_refresh: bool) -> bool {
        if self.gpu_dab_pipeline.is_none() || !self.ensure_gpu_canvas_surface() {
            return false;
        }
        let mut tiles = if full_refresh {
            // Every tile of the canvas, not only tiles holding pixels: tiles
            // that became empty (new document, deleted layer, undo) must be
            // redrawn too, or the old picture stays on screen.
            let size = efude_canvas::TILE_SIZE;
            let (columns, rows) = (
                self.doc.width.div_ceil(size),
                self.doc.height.div_ceil(size),
            );
            (0..rows)
                .flat_map(|y| (0..columns).map(move |x| (x, y)))
                .collect::<Vec<_>>()
        } else {
            self.dirty_canvas_tiles.drain().collect::<Vec<_>>()
        };
        tiles.sort_unstable();
        let mut failed = false;
        if let (Some(pipeline), Some(surface)) = (
            self.gpu_dab_pipeline.as_ref(),
            self.gpu_canvas_surface.as_ref(),
        ) {
            // Pixel and coverage inputs each occupy 4 bytes per tile pixel per
            // layer. Bound flattened input data per GPU submission during an
            // 8K full refresh instead of retaining every tile's stack at once.
            const GPU_COMPOSITE_INPUT_BUDGET: usize = 128 * 1024 * 1024;
            let layer_count = self
                .doc
                .layers
                .iter()
                .filter(|layer| layer.kind == LayerKind::Raster)
                .count()
                .clamp(1, 200);
            let bytes_per_tile = (efude_canvas::TILE_SIZE as usize).pow(2) * 8 * layer_count;
            let max_batch_tiles = (GPU_COMPOSITE_INPUT_BUDGET / bytes_per_tile).max(1);
            for tile_batch in tiles.chunks(max_batch_tiles) {
                let mut composite_tiles = Vec::with_capacity(tile_batch.len());
                for &(tile_x, tile_y) in tile_batch {
                    let Some((width, height, layers)) =
                        self.prepare_gpu_composite_tile(tile_x, tile_y)
                    else {
                        failed = true;
                        break;
                    };
                    let origin = [
                        tile_x * efude_canvas::TILE_SIZE,
                        tile_y * efude_canvas::TILE_SIZE,
                    ];
                    composite_tiles.push(efude_gpu::GpuCompositeTileInput {
                        layers,
                        width,
                        height,
                        origin,
                        checker: self.display_checker(),
                    });
                }
                if failed {
                    break;
                }
                if let Err(error) =
                    pipeline.composite_tiles_into(&composite_tiles, &surface.storage_view)
                {
                    eprintln!("efude: GPU composite: {error}");
                    failed = true;
                    break;
                }
            }
        }
        if !failed
            && let (Some(pipeline), Some(surface)) = (
                self.gpu_dab_pipeline.as_ref(),
                self.gpu_canvas_surface.as_ref(),
            )
        {
            let size = efude_canvas::TILE_SIZE;
            let rects = if full_refresh || tiles.len() > 64 {
                vec![[0, 0, self.doc.width, self.doc.height]]
            } else {
                tiles
                    .iter()
                    .map(|&(x, y)| {
                        [
                            x * size,
                            y * size,
                            ((x + 1) * size).min(self.doc.width),
                            ((y + 1) * size).min(self.doc.height),
                        ]
                    })
                    .collect()
            };
            if let Err(error) = pipeline.update_mips(&surface.texture, &rects) {
                eprintln!("efude: canvas mip levels: {error}");
                failed = true;
            }
        }
        if failed {
            self.disable_gpu_canvas();
            self.canvas_texture_dirty = true;
            return false;
        }
        self.dirty_canvas_tiles.clear();
        self.canvas_texture_dirty = false;
        true
    }
    fn layer_reference_pixels(&self, index: usize) -> Vec<u8> {
        let Some(layer) = self.doc.layers.get(index) else {
            return vec![0; (self.doc.width * self.doc.height * 4) as usize];
        };
        if layer.kind == LayerKind::Raster {
            let Some(sampler) = efude_canvas::LayerSampler::new(&self.doc, layer.id) else {
                return vec![0; (self.doc.width * self.doc.height * 4) as usize];
            };
            let mut pixels = vec![0; (self.doc.width * self.doc.height * 4) as usize];
            for y in 0..self.doc.height {
                for x in 0..self.doc.width {
                    let offset = ((y * self.doc.width + x) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&sampler.sample(&self.doc, x, y));
                }
            }
            return pixels;
        }

        let mut included = std::collections::HashSet::from([layer.id]);
        loop {
            let old_count = included.len();
            let descendants = self
                .doc
                .layers
                .iter()
                .filter(|candidate| candidate.parent_id.is_some_and(|id| included.contains(&id)))
                .map(|candidate| candidate.id)
                .collect::<Vec<_>>();
            included.extend(descendants);
            if included.len() == old_count {
                break;
            }
        }
        let mut parent = layer.parent_id;
        while let Some(parent_id) = parent {
            let Some(parent_layer) = self.doc.layers.iter().find(|item| item.id == parent_id)
            else {
                break;
            };
            included.insert(parent_id);
            parent = parent_layer.parent_id;
        }
        let mut reference = self.doc.clone();
        for candidate in &mut reference.layers {
            if !included.contains(&candidate.id) {
                candidate.visible = false;
            }
        }
        efude_canvas::composite_transparent(&reference)
    }
    fn queue_document_save(
        &self,
        path: std::path::PathBuf,
        backup: bool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let document = self.document_snapshot();
        let state_token = self.history.state_token();
        let document_id = self.history.document_id();
        self.io_task_sender
            .send(IoTask::Save {
                path,
                document,
                backup,
                backup_generations: self.backup_generations,
                state_token,
                document_id,
                repaint: ctx.clone(),
            })
            .map_err(|_| "保存ワーカーへタスクを送信できませんでした".to_string())
    }
    fn choose_native_save_path(&self) -> Option<std::path::PathBuf> {
        self.doc_path
            .clone()
            .filter(|path| {
                path.extension().is_some_and(|extension| {
                    extension.to_string_lossy().eq_ignore_ascii_case("efude")
                })
            })
            .or_else(|| {
                rfd::FileDialog::new()
                    .add_filter("Efude", &["efude"])
                    .set_file_name("Artwork.efude")
                    .save_file()
            })
    }
    fn request_native_save(&mut self, ctx: &egui::Context) -> Result<bool, String> {
        let Some(path) = self.choose_native_save_path() else {
            return Ok(false);
        };
        self.queue_document_save(path, false, ctx)?;
        self.last_backup = std::time::Instant::now();
        Ok(true)
    }
    /// Opened documents go to a new tab, so nothing is replaced.
    fn confirm_document_replacement(&self) -> bool {
        true
    }
    fn queue_document_load(
        &self,
        path: std::path::PathBuf,
        psd: bool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let task = if psd {
            IoTask::LoadPsd {
                path,
                repaint: ctx.clone(),
            }
        } else {
            IoTask::LoadEfude {
                path,
                repaint: ctx.clone(),
            }
        };
        self.io_task_sender
            .send(task)
            .map_err(|_| "読み込みワーカーへタスクを送信できませんでした".to_string())
    }
    fn queue_image_layer(
        &self,
        path: std::path::PathBuf,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.io_task_sender
            .send(IoTask::LoadImageLayer {
                path,
                max_width: self.doc.width,
                max_height: self.doc.height,
                repaint: ctx.clone(),
            })
            .map_err(|_| "画像読み込みワーカーへタスクを送信できませんでした".to_string())
    }
    fn queue_image_document(
        &self,
        path: std::path::PathBuf,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        self.io_task_sender
            .send(IoTask::LoadImageDocument {
                path,
                repaint: ctx.clone(),
            })
            .map_err(|_| "画像読み込みワーカーへタスクを送信できませんでした".to_string())
    }
    fn queue_export(
        &self,
        path: std::path::PathBuf,
        format: ExportFormat,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if matches!(format, ExportFormat::Psd) && self.doc.layers.len() > 200 {
            return Err(if self.language_english {
                format!(
                    "PSD export supports at most 200 layers (this document has {})",
                    self.doc.layers.len()
                )
            } else {
                format!(
                    "PSD書き出しは200レイヤーまでです（現在{}レイヤー）",
                    self.doc.layers.len()
                )
            });
        }
        self.io_task_sender
            .send(IoTask::Export {
                path,
                document: self.document_snapshot(),
                format,
                repaint: ctx.clone(),
            })
            .map_err(|_| "書き出しワーカーへタスクを送信できませんでした".to_string())
    }
    /// Opens `document` in a new tab (or the untouched active one).
    fn install_document(&mut self, document: Document, path: std::path::PathBuf) {
        self.open_document_tab();
        self.replace_document(document, Some(path));
    }
    /// Puts `document` in the active tab, discarding what was there.
    fn replace_document(&mut self, document: Document, path: Option<std::path::PathBuf>) {
        self.doc = document;
        self.canvas_width_input = self.doc.width;
        self.canvas_height_input = self.doc.height;
        self.canvas_dpi_input = self.doc.dpi;
        self.navigator_center = Vec2::new(self.doc.width as f32 / 2., self.doc.height as f32 / 2.);
        self.symmetry_center = Vec2::new(
            self.doc.width.saturating_sub(1) as f32 / 2.0,
            self.doc.height.saturating_sub(1) as f32 / 2.0,
        );
        self.doc_path = path;
        self.selected_layer = 0;
        self.fill_reference_layer = 0;
        self.selection_reference_layer = 0;
        self.history = History::default();
        self.selection.clear();
        self.editing_mask = false;
        self.zoom = 1.0;
        self.view_rotation = 0.0;
        self.flip_x = false;
        self.flip_y = false;
        self.canvas_texture_dirty = true;
        self.navigator_texture_dirty = true;
    }
    fn duplicate_layer_subtree(&mut self) {
        let Some(root) = self.doc.layers.get(self.selected_layer) else {
            return;
        };
        let root_id = root.id;
        let mut subtree_ids = vec![root_id];
        let mut cursor = 0;
        while cursor < subtree_ids.len() {
            let parent_id = subtree_ids[cursor];
            for child in self
                .doc
                .layers
                .iter()
                .filter(|layer| layer.parent_id == Some(parent_id))
            {
                if !subtree_ids.contains(&child.id) {
                    subtree_ids.push(child.id);
                }
            }
            cursor += 1;
        }
        let mut source_indices = self
            .doc
            .layers
            .iter()
            .enumerate()
            .filter_map(|(index, layer)| subtree_ids.contains(&layer.id).then_some(index))
            .collect::<Vec<_>>();
        source_indices.sort_unstable();
        let insert_at = source_indices
            .last()
            .copied()
            .unwrap_or(self.selected_layer)
            + 1;
        let Some(first_id) = self
            .doc
            .layers
            .iter()
            .map(|layer| layer.id)
            .max()
            .unwrap_or(0)
            .checked_add(1)
        else {
            self.status = self
                .text(
                    "レイヤーIDを割り当てられません",
                    "Could not assign a layer ID",
                )
                .into();
            return;
        };
        let id_map = source_indices
            .iter()
            .enumerate()
            .map(|(offset, &index)| (self.doc.layers[index].id, first_id + offset as u64))
            .collect::<std::collections::HashMap<_, _>>();
        let mut copies = source_indices
            .iter()
            .map(|&index| {
                let source = &self.doc.layers[index];
                let mut copy = source.clone();
                copy.id = id_map[&source.id];
                copy.parent_id = source
                    .parent_id
                    .and_then(|parent_id| id_map.get(&parent_id).copied())
                    .or(source.parent_id);
                copy
            })
            .collect::<Vec<_>>();
        if let Some(root_position) = copies.iter().position(|layer| layer.id == id_map[&root_id]) {
            copies.swap(0, root_position);
        }
        if let Some(copy_root) = copies.iter_mut().find(|layer| layer.id == id_map[&root_id]) {
            copy_root.name = format!("{} のコピー", copy_root.name);
        }
        self.history.begin();
        for (offset, layer) in copies.into_iter().enumerate() {
            self.history
                .insert_layer(&mut self.doc.layers, insert_at + offset, layer);
        }
        self.history.commit();
        self.selected_layer = insert_at;
    }
    fn install_image_layer(
        &mut self,
        path: std::path::PathBuf,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) {
        let id = self
            .doc
            .layers
            .iter()
            .map(|layer| layer.id)
            .max()
            .unwrap_or(0)
            + 1;
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("画像")
            .to_string();
        let mut layer = efude_canvas::Layer::new(id, name.clone(), self.doc.width, self.doc.height);
        let x_offset = self.doc.width.saturating_sub(width) / 2;
        let y_offset = self.doc.height.saturating_sub(height) / 2;
        self.history.begin();
        let index = self.doc.layers.len();
        self.history
            .insert_layer(&mut self.doc.layers, index, layer.clone());
        layer = self.doc.layers[index].clone();
        for y in 0..height {
            for x in 0..width {
                let src = ((y * width + x) * 4) as usize;
                let pixel: [u8; 4] = rgba[src..src + 4].try_into().unwrap();
                if pixel[3] != 0 {
                    let dx = x + x_offset;
                    let dy = y + y_offset;
                    let pixel_index = (dy * self.doc.width + dx) as usize * 4;
                    self.history.record_pixel(&layer, pixel_index);
                    self.history.record_pixel(&layer, pixel_index + 1);
                    self.history.record_pixel(&layer, pixel_index + 2);
                    self.history.record_pixel(&layer, pixel_index + 3);
                    layer.pixels.set_pixel(dx, dy, pixel);
                }
            }
        }
        layer.pixels.prune_empty_tiles();
        self.doc.layers[index] = layer;
        self.history.commit();
        self.selected_layer = index;
        self.status = format!("画像を新しいレイヤーに読み込みました: {name}");
    }
    fn install_system_font(ctx: &egui::Context) {
        let mut candidates = Vec::new();
        #[cfg(target_os = "windows")]
        {
            let root = std::env::var_os("WINDIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
            for name in ["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc", "segoeui.ttf"] {
                candidates.push(root.join("Fonts").join(name));
            }
        }
        #[cfg(target_os = "macos")]
        candidates.extend([
            std::path::PathBuf::from("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
            std::path::PathBuf::from("/System/Library/Fonts/SFNS.ttf"),
        ]);
        #[cfg(target_os = "linux")]
        candidates.extend([
            std::path::PathBuf::from("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
            std::path::PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        ]);

        let Some(bytes) = candidates
            .into_iter()
            .find_map(|path| std::fs::read(path).ok())
        else {
            return;
        };
        let mut fonts = egui::FontDefinitions::empty();
        fonts.font_data.insert(
            "system-ui".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .insert(egui::FontFamily::Proportional, vec!["system-ui".to_owned()]);
        fonts
            .families
            .insert(egui::FontFamily::Monospace, vec!["system-ui".to_owned()]);
        ctx.set_fonts(fonts);
    }

    pub fn from_creation_context(cc: &eframe::CreationContext<'_>) -> Self {
        // Ctrl +/- zoom the canvas, not the whole interface; a UI zoom left
        // over from an earlier run would shrink the window and every panel.
        cc.egui_ctx
            .options_mut(|options| options.zoom_with_keyboard = false);
        cc.egui_ctx.set_zoom_factor(1.0);
        Self::install_system_font(&cc.egui_ctx);
        layout::apply_theme(&cc.egui_ctx);
        let mut app = Self::default();
        #[cfg(target_os = "windows")]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(handle) = cc.window_handle()
                && let RawWindowHandle::Win32(handle) = handle.as_raw()
            {
                app.windows_ink_hwnd = handle.hwnd.get();
                app.windows_ink_hook = efude_input::WindowsInkHook::install(
                    handle.hwnd.get() as *mut std::ffi::c_void,
                    app.pen_queue.shared(),
                )
                .ok();
            }
        }
        app.gpu_render_state = cc.wgpu_render_state.clone();
        if let Some(state) = &app.gpu_render_state {
            match efude_gpu::GpuDabPipeline::new(&state.device, &state.queue) {
                Ok(pipeline) => app.gpu_dab_pipeline = Some(pipeline),
                Err(error) => {
                    eprintln!("efude: GPU canvas unavailable: {error}");
                    app.status = format!("GPU initialization failed; using CPU rendering: {error}");
                }
            }
            // Large brushes paint on the GPU; anything it cannot do (or any
            // GPU failure) falls back to the identical CPU path.
            app.gpu_cover = efude_gpu::GpuCover::new(&state.device, &state.queue).ok();
        }
        if let Some(settings) = cc
            .storage
            .and_then(|storage| eframe::get_value::<PersistedSettings>(storage, "settings"))
        {
            if !settings.brushes.is_empty() {
                // Saved sets from this version are kept exactly as they are
                // (renamed presets must not bring the originals back).
                app.brushes = if settings.preset_version < PRESET_VERSION {
                    migrate_saved_brushes(settings.brushes)
                } else {
                    settings.brushes
                };
                if settings.preset_version < PRESET_VERSION {
                    // The presets now come in several columns: saved brushes
                    // stay as the first, the rest are added.
                    let presets = default_presets();
                    if app.brushes.len() < presets.len() {
                        let have = app.brushes.len();
                        app.brushes.extend(presets.into_iter().skip(have));
                    }
                }
            }
            app.selected_brush = settings.selected_brush.min(app.brushes.len() - 1);
            app.tool_brushes = settings.tool_brushes;
            // Start with the tool that draws with the remembered brush.
            app.tool = tool_for_brush(app.brushes[app.selected_brush].kind);
            app.last_tool = app.tool;
            app.color = Color32::from_rgba_unmultiplied(
                settings.color[0],
                settings.color[1],
                settings.color[2],
                settings.color[3],
            );
            app.secondary_color = Color32::from_rgba_unmultiplied(
                settings.secondary_color[0],
                settings.secondary_color[1],
                settings.secondary_color[2],
                settings.secondary_color[3],
            );
            app.intermediate_mix = settings.intermediate_mix.clamp(0.0, 1.0);
            if !settings.palette.is_empty() {
                app.palette = settings.palette.into_iter().take(64).collect();
            }
            app.show_tools_panel = settings.show_tools_panel;
            app.show_layers_panel = settings.show_layers_panel;
            if let Some(workspace) = settings.workspace
                && settings.layout_version >= layout::LAYOUT_VERSION
            {
                app.workspace = workspace;
            }
            app.tools_panel_width = settings.tools_panel_width.clamp(120.0, 480.0);
            app.layers_panel_width = settings.layers_panel_width.clamp(120.0, 480.0);
            app.size = settings.size.clamp(1.0, MAX_BRUSH_SIZE);
            app.zoom = settings.zoom.clamp(0.1, 4.0);
            app.view_rotation = settings.view_rotation.clamp(-180.0, 180.0);
            app.flip_x = settings.flip_x;
            app.flip_y = settings.flip_y;
            app.show_grid = settings.show_grid;
            app.transparency_checker = settings.transparency_checker;
            app.grid_size = settings.grid_size.clamp(8, 256);
            app.grid_snap = settings.grid_snap;
            app.symmetry_x = settings.symmetry_x;
            app.symmetry_y = settings.symmetry_y;
            app.symmetry_count = settings.symmetry_count.clamp(1, 32);
            app.symmetry_center = Vec2::new(
                settings.symmetry_center[0].clamp(0.0, app.doc.width.saturating_sub(1) as f32),
                settings.symmetry_center[1].clamp(0.0, app.doc.height.saturating_sub(1) as f32),
            );
            app.perspective_points = settings
                .perspective_points
                .iter()
                .take(3)
                .map(|point| {
                    (
                        point[0].clamp(0, app.doc.width.saturating_sub(1) as i32),
                        point[1].clamp(0, app.doc.height.saturating_sub(1) as i32),
                    )
                })
                .collect();
            app.perspective_selected = settings
                .perspective_selected
                .min(app.perspective_points.len().saturating_sub(1));
            app.shortcuts = settings.shortcuts;
            app.tone_curve = settings.tone_curve;
            app.pressure_curve_points = settings.pressure_curve_points.map(|v| v.clamp(0.0, 1.0));
            app.use_windows_ink = settings.use_windows_ink;
            app.use_wintab = settings.use_wintab;
            app.eyedropper_radius = settings.eyedropper_radius.min(64);
            app.eyedropper_composite = settings.eyedropper_composite;
            app.selection_reference_mode = settings.selection_reference_mode.min(3);
            app.backup_interval_minutes = settings.backup_interval_minutes.clamp(1, 120);
            app.backup_generations = settings.backup_generations.clamp(1, 100);
            if !settings.eraser_pressure_updated {
                for brush in &mut app.brushes {
                    if matches!(brush.kind, BrushKind::Eraser)
                        && brush.opacity_source == DynamicSource::Pressure
                    {
                        brush.opacity_source = DynamicSource::None;
                    }
                }
            }
            app.selection_reference_layer = settings.selection_reference_layer;
            app.fill_reference_layer = settings.fill_reference_layer;
            app.fill_reference_mode = settings.fill_reference_mode.min(3);
            app.fill_tolerance = settings.fill_tolerance.min(96);
            app.fill_gap_close = settings.fill_gap_close.min(8);
            app.selection_tolerance = settings.selection_tolerance.min(96);
            app.reference_zoom = if settings.reference_zoom.is_finite() {
                settings.reference_zoom.clamp(0.1, 1.5)
            } else {
                default_reference_zoom()
            };
            app.reference_opacity = if settings.reference_opacity.is_finite() {
                settings.reference_opacity.clamp(0.1, 1.0)
            } else {
                default_reference_opacity()
            };
            app.reference_position = settings
                .reference_position
                .filter(|position| position.iter().all(|value| value.is_finite()))
                .map(|position| Pos2::new(position[0], position[1]));
            app.reference_image_path = settings.reference_image_path.clone();
            if let Some(path) = app.reference_image_path.clone() {
                let _ = app.io_task_sender.send(IoTask::LoadReference {
                    path,
                    repaint: cc.egui_ctx.clone(),
                });
            }
            app.language_english = settings.language_english;
        }
        #[cfg(target_os = "windows")]
        if !app.use_windows_ink {
            app.windows_ink_hook = None;
        }
        app
    }
    #[cfg(target_os = "windows")]
    fn sync_windows_ink_hook(&mut self) {
        if self.use_wintab {
            self.windows_ink_hook = None;
            if self.wintab_hook.is_none() && self.windows_ink_hwnd != 0 {
                match efude_input::WintabHook::install(
                    self.windows_ink_hwnd as *mut std::ffi::c_void,
                    self.pen_queue.shared(),
                ) {
                    Ok(hook) => {
                        self.wintab_hook = Some(hook);
                        self.wintab_error = None;
                    }
                    Err(error) => {
                        self.wintab_error = Some(error.clone());
                        self.status = format!("Wintab unavailable: {error}");
                        self.use_wintab = false;
                        self.use_windows_ink = false;
                    }
                }
            }
        } else {
            self.wintab_hook = None;
        }
        if self.use_windows_ink {
            if self.windows_ink_hook.is_none() && self.windows_ink_hwnd != 0 {
                self.windows_ink_hook = efude_input::WindowsInkHook::install(
                    self.windows_ink_hwnd as *mut std::ffi::c_void,
                    self.pen_queue.shared(),
                )
                .ok();
            }
        } else {
            self.windows_ink_hook = None;
        }
        if !self.use_windows_ink && !self.use_wintab {
            self.frame_pen_packets.clear();
        }
    }
    fn pressure_at(ctx: &egui::Context, pos: Pos2) -> f32 {
        ctx.input(|input| {
            input
                .events
                .iter()
                .rev()
                .find_map(|event| match event {
                    egui::Event::Touch {
                        pos: touch_pos,
                        force,
                        ..
                    } if touch_pos.distance(pos) < 10. => force.map(|f| f.clamp(0.03, 1.0)),
                    _ => None,
                })
                .unwrap_or(1.0)
        })
    }
    fn global_pressure_curve(&self, pressure: f32) -> f32 {
        let [x1, y1, x2, y2] = self.pressure_curve_points.map(|v| v.clamp(0.0, 1.0));
        let x = pressure.clamp(0.0, 1.0);
        let mut lo = 0.0;
        let mut hi = 1.0;
        for _ in 0..12 {
            let t = (lo + hi) * 0.5;
            let u = 1.0 - t;
            let bx = 3.0 * u * u * t * x1 + 3.0 * u * t * t * x2 + t * t * t;
            if bx < x {
                lo = t;
            } else {
                hi = t;
            }
        }
        let t = (lo + hi) * 0.5;
        let u = 1.0 - t;
        (3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t).clamp(0.0, 1.0)
    }
    /// A movement on screen, in document pixels (undoing view rotation,
    /// flips and zoom).
    fn screen_delta_to_document(&self, delta: Vec2, scale: f32) -> Vec2 {
        let v = delta / scale.max(1e-4);
        let (sa, ca) = self.view_rotation.to_radians().sin_cos();
        let mut v = Vec2::new(v.x * ca + v.y * sa, -v.x * sa + v.y * ca);
        if self.flip_x {
            v.x = -v.x;
        }
        if self.flip_y {
            v.y = -v.y;
        }
        v
    }
    /// Document point under screen position `pos` (no grid snapping).
    fn document_position_unsnapped(&self, pos: Pos2, rect: Rect, scale: f32) -> Vec2 {
        self.screen_delta_to_document(pos - rect.center(), scale)
            + Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0)
    }
    fn document_position(&self, pos: Pos2, rect: Rect, scale: f32) -> Vec2 {
        let mut v = (pos - rect.center()) / scale;
        let ca = self.view_rotation.to_radians().cos();
        let sa = self.view_rotation.to_radians().sin();
        v = Vec2::new(v.x * ca + v.y * sa, -v.x * sa + v.y * ca);
        if self.flip_x {
            v.x = -v.x;
        }
        if self.flip_y {
            v.y = -v.y;
        }
        let mut position = v + Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
        if self.grid_snap && self.show_grid {
            let spacing = self.grid_size.max(1) as f32;
            position = (position / spacing).round() * spacing;
        }
        position
    }
    fn push_live_sample(
        &mut self,
        point: InkPoint,
        received_at: Option<std::time::Instant>,
        ctx: &egui::Context,
    ) {
        if self
            .active
            .last()
            .is_none_or(|last| last.position.distance(point.position) > 0.8)
        {
            self.active.push(point);
            self.update_preview_estimate();
            self.pending_canvas_latency
                .get_or_insert_with(|| received_at.unwrap_or_else(std::time::Instant::now));
            if let Some(builder) = self.stroke_builder.as_mut() {
                // Brush tools: stabilized, evenly spaced dabs. What is drawn
                // here is exactly what stays after pen-up (no re-render).
                let update = builder.push(point);
                let brush = &self.brushes[self.selected_brush];
                self.stabilized_cursor = if brush.stabilization > 0 || brush.pull_distance > 0.0 {
                    builder.brush_position().map(|p| Vec2::new(p.x, p.y))
                } else {
                    None
                };
                self.restore_provisional();
                self.dab_many(&update.committed);
                // Without settling, only final dabs are drawn: nothing on
                // screen changes after it appears.
                self.pending_provisional =
                    if self.brushes[self.selected_brush].settle && !self.vector_target() {
                        update.provisional
                    } else {
                        Vec::new()
                    };
            } else {
                self.update_stabilized_cursor();
                self.dab(point);
            }
            ctx.request_repaint();
        }
    }
    // A short velocity extrapolation is rendered as a guide only. Keep it
    // separate from `active`; this is not an OS-provided point or stroke data.
    fn update_preview_estimate(&mut self) {
        self.preview_cursor_estimate =
            efude_input::estimate_preview_point(&self.active, 15.0, 128.0);
    }
    fn update_stabilized_cursor(&mut self) {
        let Some(latest) = self.active.last().copied() else {
            self.stabilized_cursor = None;
            return;
        };
        let brush = &self.brushes[self.selected_brush];
        let stabilization = brush.stabilization.min(15);
        if stabilization == 0 && brush.pull_distance <= 0.0 {
            self.stabilized_cursor = None;
            return;
        }
        let speed = self
            .active
            .get(self.active.len().saturating_sub(2))
            .map(|previous| {
                let elapsed = latest.time_ms.saturating_sub(previous.time_ms).max(1) as f32;
                (latest.position.distance(previous.position) / elapsed / 0.5).clamp(0.0, 1.0)
            })
            .unwrap_or(0.0);
        let count = (stabilization as f32
            * (1.0 - brush.speed_stabilization.clamp(0.0, 1.0) * speed))
            .round() as usize;
        let start = self.active.len().saturating_sub(count + 1);
        let slice = &self.active[start..];
        let mut position = Vec2::ZERO;
        let mut total_weight = 0.0;
        for (index, point) in slice.iter().enumerate() {
            let weight = (index + 1) as f32;
            position += Vec2::new(point.position.x, point.position.y) * weight;
            total_weight += weight;
        }
        position /= total_weight.max(1.0);
        if let Some(previous) = self.stabilized_cursor {
            let delta = position - previous;
            let distance = delta.length();
            let pull = brush.pull_distance.clamp(0.0, 256.0);
            if pull > 0.0 && distance > pull {
                position = previous + delta * ((distance - pull) / distance);
            }
        }
        self.stabilized_cursor = Some(position);
    }
    fn absorb_history_tile_changes(&mut self) {
        self.dirty_canvas_tiles
            .extend(self.history.take_changed_tiles());
        if self.history.take_full_redraw() {
            self.canvas_texture_dirty = true;
        }
    }
    fn record_canvas_latency(&mut self, elapsed_ms: f32) {
        self.last_canvas_latency_ms = Some(elapsed_ms);
        if self.canvas_latency_samples_ms.len() == 120 {
            self.canvas_latency_samples_ms.pop_front();
        }
        self.canvas_latency_samples_ms.push_back(elapsed_ms);
    }
    fn reset_stroke_buffers(&mut self) {
        self.raster.reset_buffers();
    }
    /// Starts the grain of a stroke at `origin`; brushes whose canvas grain
    /// turns with every stroke get a new random angle.
    fn begin_grain(&mut self, origin: glam::Vec2) {
        self.raster.grain_origin = origin;
        let brush = &self.brushes[self.selected_brush];
        self.raster.grain_rotation = if brush.grain_fixed && brush.grain_random_rotation {
            // xorshift64*: a new angle for every stroke.
            let mut x = self.grain_seed;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.grain_seed = x;
            let unit = (x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32;
            (unit * std::f32::consts::TAU).max(1e-3)
        } else {
            0.0
        };
    }
    /// The settings changed most while drawing, pinned above the scrolling
    /// tools panel so they are always reachable.
    /// Each brush tool remembers its own brush: switching tools swaps the
    /// brush (and its size), so settings made for the eraser stay with the
    /// eraser and never change the pen.
    fn sync_tool_change(&mut self) {
        if self.tool == self.last_tool {
            return;
        }
        let previous = self.last_tool;
        self.last_tool = self.tool;
        if let Some(brush) = self.brushes.get_mut(self.selected_brush) {
            brush.size = self.size;
        }
        if let Some(slot) = brush_tool_slot(previous)
            && self
                .brushes
                .get(self.selected_brush)
                .is_some_and(|brush| brush_fits_tool(brush.kind, previous))
        {
            self.tool_brushes[slot] = Some(self.selected_brush);
        }
        let Some(slot) = brush_tool_slot(self.tool) else {
            return;
        };
        let tool = self.tool;
        let remembered = self.tool_brushes[slot].filter(|&index| {
            self.brushes
                .get(index)
                .is_some_and(|brush| brush_fits_tool(brush.kind, tool))
        });
        let current_fits = self
            .brushes
            .get(self.selected_brush)
            .is_some_and(|brush| brush_fits_tool(brush.kind, tool));
        let wanted = remembered.or_else(|| {
            if current_fits {
                Some(self.selected_brush)
            } else {
                self.brushes
                    .iter()
                    .position(|brush| brush_fits_tool(brush.kind, tool))
            }
        });
        if let Some(index) = wanted {
            self.selected_brush = index;
            self.size = self.brushes[index].size;
            self.tool_brushes[slot] = Some(index);
        }
    }

    /// Starts a move of the selected pixels (or the whole layer).
    fn begin_move(&mut self, start: (i32, i32)) {
        let index = self.selected_layer;
        let layer = &self.doc.layers[index];
        if layer.locked || self.is_reference_layer(index) || layer.kind != LayerKind::Raster {
            self.move_origin = None;
            return;
        }
        let width = self.doc.width as usize;
        let bounds = if self.selection.active {
            let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
            for (i, _) in self
                .selection
                .mask
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
            {
                let (x, y) = ((i % width) as i32, (i / width) as i32);
                b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
            }
            if b[0] == i32::MAX {
                // Nothing selected: nothing to move.
                self.move_origin = None;
                return;
            }
            Some(b)
        } else {
            None
        };
        self.move_origin = Some(MoveOrigin {
            layer: index,
            pixels: layer.pixels.clone(),
            mask: layer.mask.clone(),
            selection: self.selection.mask.clone(),
            selection_active: self.selection.active,
            bounds,
            start,
            offset: (0, 0),
            vector: layer.vector.clone(),
        });
    }

    /// Places the moved pixels at the pointer: always the original pixels
    /// moved by the whole offset, composited over what was there before.
    fn continue_move(&mut self, at: (i32, i32)) {
        let Some(mut origin) = self.move_origin.take() else {
            return;
        };
        let offset = (at.0 - origin.start.0, at.1 - origin.start.1);
        if offset != origin.offset && origin.layer < self.doc.layers.len() {
            let previous = origin.offset;
            origin.offset = offset;
            let (width, height) = (self.doc.width, self.doc.height);
            let layer = &self.doc.layers[origin.layer];
            match origin.bounds {
                Some(bounds) => {
                    // Tiles under the selection, its last place and its new
                    // place change (and must be redrawn).
                    for (dx, dy) in [(0, 0), previous, offset] {
                        let x0 = (bounds[0] + dx).max(0);
                        let y0 = (bounds[1] + dy).max(0);
                        let x1 = (bounds[2] + dx).min(width as i32);
                        let y1 = (bounds[3] + dy).min(height as i32);
                        if x1 <= x0 || y1 <= y0 {
                            continue;
                        }
                        let step = efude_canvas::TILE_SIZE as usize;
                        let xs = (x0..x1).step_by(step).chain(std::iter::once(x1 - 1));
                        let xs: Vec<i32> = xs.collect();
                        let ys = (y0..y1).step_by(step).chain(std::iter::once(y1 - 1));
                        for y in ys {
                            for &x in &xs {
                                let base = (y as usize * width as usize + x as usize) * 4;
                                self.history.record_pixel(layer, base);
                                self.history.record_mask(layer, base);
                            }
                        }
                    }
                }
                None => {
                    self.history.record_all_layer_tiles(layer, width, height);
                    self.history.record_all_mask_tiles(layer, width, height);
                }
            }
            let layer = &mut self.doc.layers[origin.layer];
            layer.pixels.clone_from(&origin.pixels);
            layer.mask.clone_from(&origin.mask);
            self.selection.mask.clone_from(&origin.selection);
            self.selection.active = origin.selection_active;
            efude_canvas::translate_selection(
                layer,
                &mut self.selection,
                width,
                height,
                offset.0,
                offset.1,
            );
        }
        self.move_origin = Some(origin);
    }

    /// Ctrl shortcuts for editing, files, layers and tabs. `keys` are the
    /// configured undo, redo, save, copy, cut, paste, select-all and
    /// deselect keys. Variants with Shift are checked first, since a plain
    /// Ctrl pattern also accepts Shift.
    fn command_shortcuts(&mut self, ctx: &egui::Context, keys: [egui::Key; 8]) {
        use egui::{Key, Modifiers};
        let [undo, redo, save, copy, cut, paste, select_all, deselect] = keys;
        let command = Modifiers::COMMAND;
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        // The window system turns Ctrl+C, Ctrl+X and Ctrl+V into copy, cut
        // and paste events instead of key presses.
        let (copy_event, cut_event, paste_event) = ctx.input_mut(|input| {
            let mut found = (false, false, false);
            input.events.retain(|event| match event {
                egui::Event::Copy => {
                    found.0 = true;
                    false
                }
                egui::Event::Cut => {
                    found.1 = true;
                    false
                }
                egui::Event::Paste(_) => {
                    found.2 = true;
                    false
                }
                _ => true,
            });
            found
        });
        // With an image on the clipboard no paste event arrives at all, so
        // the keys themselves are watched too.
        #[cfg(target_os = "windows")]
        let chord_paste = {
            let down = ctx.input(|input| input.focused) && efude_input::ctrl_key_down(b'V');
            let fresh = down && !self.paste_chord_down;
            self.paste_chord_down = down;
            fresh && paste == Key::V
        };
        #[cfg(not(target_os = "windows"))]
        let chord_paste = false;
        let pressed = |modifiers: Modifiers, key: Key| {
            ctx.input_mut(|input| input.consume_key(modifiers, key))
        };
        if pressed(shift, Key::Z) || pressed(command, redo) {
            self.redo();
        } else if pressed(command, undo) {
            self.undo();
        }
        if pressed(shift, Key::S) {
            self.save_as_dialog(ctx);
        } else if pressed(command, save) {
            self.status = match self.request_native_save(ctx) {
                Ok(true) => self.text("保存中…", "Saving…").into(),
                Ok(false) => self.status.clone(),
                Err(error) => error,
            };
        }
        if copy_event || pressed(command, copy) {
            self.copy_selection(false);
        }
        if cut_event || pressed(command, cut) {
            self.copy_selection(true);
        }
        if paste_event || chord_paste || pressed(command, paste) {
            self.paste_clipboard(ctx);
        }
        if pressed(shift, Key::I) {
            self.change_selection(|selection, width, height| selection.invert(width, height));
        }
        if pressed(command, select_all) {
            self.select_all();
        }
        if pressed(command, deselect) {
            self.change_selection(|selection, _, _| selection.clear());
        }
        if pressed(shift, Key::N) {
            self.add_raster_layer();
        } else if pressed(command, Key::N) {
            self.show_new_document = true;
        }
        if pressed(command, Key::O) {
            self.open_document_dialog(ctx);
        }
        if pressed(command, Key::J) {
            self.duplicate_layer_subtree();
        }
        if pressed(command, Key::W) {
            self.request_close_tab(self.tabs.active);
        }
        if pressed(shift, Key::Tab) {
            let count = self.tabs.slots.len();
            self.switch_tab((self.tabs.active + count - 1) % count);
        } else if pressed(command, Key::Tab) {
            self.switch_tab((self.tabs.active + 1) % self.tabs.slots.len());
        }
    }

    /// Selects a brush preset and the tool that draws with it.
    pub(crate) fn select_brush_preset(&mut self, index: usize) {
        let Some(kind) = self.brushes.get(index).map(|brush| brush.kind) else {
            return;
        };
        if let Some(brush) = self.brushes.get_mut(self.selected_brush) {
            brush.size = self.size;
        }
        if let Some(slot) = brush_tool_slot(self.tool)
            && self
                .brushes
                .get(self.selected_brush)
                .is_some_and(|brush| brush_fits_tool(brush.kind, self.tool))
        {
            self.tool_brushes[slot] = Some(self.selected_brush);
        }
        let tool = tool_for_brush(kind);
        self.selected_brush = index;
        self.size = self.brushes[index].size;
        if let Some(slot) = brush_tool_slot(tool) {
            self.tool_brushes[slot] = Some(index);
        }
        if self.tool != tool {
            if self.tool == Tool::PolygonSelect {
                self.selection_points.clear();
            }
            self.tool = tool;
        }
        self.last_tool = tool;
    }

    fn quick_brush_controls(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let name = self.brushes[self.selected_brush].name.clone();
        ui.label(egui::RichText::new(name).strong());
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
        let d = self.input_diagnostics;
        let source = if self.use_wintab {
            "WinTab"
        } else if self.use_windows_ink {
            "Windows Ink"
        } else if english {
            "Window"
        } else {
            "ウィンドウ"
        };
        let pressure = match (d.min_pressure, d.max_pressure) {
            (Some(min), Some(max)) => format!("{min:.2}–{max:.2}"),
            _ => "-".into(),
        };
        ui.label(
            egui::RichText::new(if english {
                format!(
                    "Last stroke: {source} · tablet {} · outside {} · no-pressure {} · pressure {pressure}",
                    d.tablet_samples, d.outside_canvas, d.window_fallbacks
                )
            } else {
                format!(
                    "直前の線: {source}・ペン点 {}・範囲外 {}・筆圧なし {}・筆圧 {pressure}",
                    d.tablet_samples, d.outside_canvas, d.window_fallbacks
                )
            })
            .small()
            .color(layout::MUTED_TEXT),
        );
        if let Some(error) = &self.wintab_error {
            ui.label(
                egui::RichText::new(if english {
                    format!("WinTab failed: {error}")
                } else {
                    format!("WinTabを開始できません: {error}")
                })
                .small()
                .color(Color32::from_rgb(220, 90, 80)),
            );
        }
        ui.separator();
    }
    /// A Wacom driver with "Windows Ink" turned off delivers the pen as a
    /// mouse, so Windows Ink mode receives no pen samples and no pressure.
    /// After such a stroke, switch to WinTab once, automatically.
    fn fall_back_to_wintab_if_ink_is_silent(&mut self) {
        #[cfg(target_os = "windows")]
        {
            let d = self.input_diagnostics;
            if !self.use_windows_ink
                || self.use_wintab
                || self.tried_wintab_fallback
                || d.tablet_samples > 0
                || d.window_fallbacks < 20
            {
                return;
            }
            self.tried_wintab_fallback = true;
            self.use_windows_ink = false;
            self.use_wintab = true;
            self.sync_windows_ink_hook();
            self.frame_pen_packets.clear();
            self.pen_queue.clear();
            if self.use_wintab {
                self.status = self
                    .text(
                        "Windows Inkでペン入力が届かないため、WinTabに切り替えました",
                        "No pen input arrived through Windows Ink, so Efude switched to WinTab",
                    )
                    .into();
            } else {
                let reason = std::mem::take(&mut self.status);
                self.use_windows_ink = true;
                self.sync_windows_ink_hook();
                self.status = if self.language_english {
                    format!(
                        "No pen pressure: enable Windows Ink in the tablet driver settings ({reason})"
                    )
                } else {
                    format!(
                        "筆圧が届いていません。タブレットのドライバ設定でWindows Inkを有効にしてください({reason})"
                    )
                };
            }
        }
    }
    fn stroke_params(&self) -> efude_stroke::StrokeParams {
        let brush = &self.brushes[self.selected_brush];
        let (taper_start_px, taper_end_px, taper_start_fraction, taper_end_fraction) =
            if !brush.settle {
                // Drawn as final at once: the start taper needs no future
                // (its length follows the brush size), the end taper would
                // reshape the line afterwards, so it is left out.
                let start = if brush.taper_in_pixels {
                    brush.taper_start
                } else {
                    brush.taper_start * self.size * 12.0
                };
                (start, 0.0, 0.0, 0.0)
            } else if brush.taper_in_pixels {
                (brush.taper_start, brush.taper_end, 0.0, 0.0)
            } else {
                (0.0, 0.0, brush.taper_start, brush.taper_end)
            };
        efude_stroke::StrokeParams {
            stabilization_ms: efude_stroke::StrokeParams::stabilization_from_level(
                brush.stabilization,
            ),
            pull_distance: brush.pull_distance,
            speed_adaptation: brush.speed_stabilization,
            // "Fast" is 0.5 screen px/ms at any zoom, like the speed dynamics.
            fast_speed: efude_brush::engine::FAST_SCREEN_SPEED / self.view_scale.max(1e-3),
            spacing: self.stroke_spacing(),
            taper_start_px,
            taper_end_px,
            taper_start_fraction,
            taper_end_fraction,
            taper_min: brush.taper_min,
        }
    }
    /// Runs recorded or programmatic points through the same stroke engine
    /// as live drawing and returns every dab.
    fn offline_stroke_dabs(&self, points: &[InkPoint]) -> Vec<InkPoint> {
        let mut builder = efude_stroke::StrokeBuilder::new(self.stroke_params());
        let mut dabs = Vec::new();
        for point in points {
            dabs.extend(builder.push(*point).committed);
        }
        dabs.extend(builder.finish());
        dabs
    }
    fn begin_brush_stroke(&mut self) {
        self.stroke_builder = Some(efude_stroke::StrokeBuilder::new(self.stroke_params()));
        self.provisional = None;
    }
    /// Draws the latest unfinished tail (once per frame, however many pen
    /// samples arrived) after saving what it will overwrite.
    fn flush_provisional(&mut self) {
        let dabs = std::mem::take(&mut self.pending_provisional);
        if dabs.is_empty() || self.stroke_builder.is_none() {
            return;
        }
        self.restore_provisional();
        let layer = self.selected_layer;
        self.provisional = Some(ProvisionalSnapshot {
            layer,
            pixels: self.doc.layers[layer].pixels.clone(),
            mask: self.doc.layers[layer].mask.clone(),
            raster: self.raster.clone(),
        });
        self.dab_many(&dabs);
    }
    /// Takes back the provisional tail drawn by the previous update.
    fn restore_provisional(&mut self) {
        let Some(snapshot) = self.provisional.take() else {
            return;
        };
        let layer = &mut self.doc.layers[snapshot.layer];
        let mut changed = layer.pixels.tiles_changed_since(&snapshot.pixels);
        if let (Some(mask), Some(before)) = (&layer.mask, &snapshot.mask) {
            changed.extend(mask.tiles_changed_since(before));
        }
        layer.pixels = snapshot.pixels;
        layer.mask = snapshot.mask;
        self.raster = snapshot.raster;
        self.dirty_canvas_tiles.extend(changed);
    }
    fn stroke_spacing(&self) -> f32 {
        let brush = &self.brushes[self.selected_brush];
        let source_min = if brush.size_source == DynamicSource::None {
            1.0
        } else {
            brush.size_min.clamp(0.0, 1.0)
        };
        let speed_min = 1.0 - 0.75 * brush.speed_size.abs().clamp(0.0, 1.0);
        let tilt_min = 1.0 - brush.tilt_size.abs().clamp(0.0, 1.0);
        (self.size * brush.spacing.clamp(0.005, 2.0) * source_min * speed_min * tilt_min).max(0.25)
    }
    fn configured_key(value: &str, fallback: egui::Key) -> egui::Key {
        match value.trim().to_ascii_uppercase().as_str() {
            "0" => egui::Key::Num0,
            "1" => egui::Key::Num1,
            "2" => egui::Key::Num2,
            "3" => egui::Key::Num3,
            "4" => egui::Key::Num4,
            "5" => egui::Key::Num5,
            "6" => egui::Key::Num6,
            "7" => egui::Key::Num7,
            "8" => egui::Key::Num8,
            "9" => egui::Key::Num9,
            "A" => egui::Key::A,
            "B" => egui::Key::B,
            "C" => egui::Key::C,
            "D" => egui::Key::D,
            "E" => egui::Key::E,
            "F" => egui::Key::F,
            "G" => egui::Key::G,
            "H" => egui::Key::H,
            "I" => egui::Key::I,
            "J" => egui::Key::J,
            "K" => egui::Key::K,
            "L" => egui::Key::L,
            "M" => egui::Key::M,
            "N" => egui::Key::N,
            "O" => egui::Key::O,
            "P" => egui::Key::P,
            "Q" => egui::Key::Q,
            "R" => egui::Key::R,
            "S" => egui::Key::S,
            "T" => egui::Key::T,
            "U" => egui::Key::U,
            "V" => egui::Key::V,
            "W" => egui::Key::W,
            "X" => egui::Key::X,
            "Y" => egui::Key::Y,
            "Z" => egui::Key::Z,
            _ => fallback,
        }
    }
    /// Tap a tool key to switch tools; hold it to switch only while held.
    fn update_held_tool_key(
        &mut self,
        ctx: &egui::Context,
        enabled: bool,
        tool_keys: &[(egui::Key, Tool, bool)],
    ) {
        let pointer_down = ctx.input(|input| input.pointer.any_down());
        if let Some(held) = &mut self.held_tool_key {
            if pointer_down {
                held.used = true;
            }
            if !ctx.input(|input| input.key_down(held.key)) {
                held.released = true;
            }
            // Never switch in the middle of a stroke or drag.
            if held.released && !pointer_down {
                let held = self.held_tool_key.take().unwrap();
                let spring_back =
                    held.hold_only || held.used || held.pressed_at.elapsed() >= TOOL_KEY_HOLD;
                if spring_back {
                    self.switch_tool(held.previous);
                }
            }
            return;
        }
        if !enabled || pointer_down {
            return;
        }
        for &(key, tool, hold_only) in tool_keys {
            if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                self.held_tool_key = Some(HeldToolKey {
                    key,
                    previous: self.tool,
                    pressed_at: std::time::Instant::now(),
                    used: false,
                    released: false,
                    hold_only,
                });
                self.switch_tool(tool);
                break;
            }
        }
    }
    fn switch_tool(&mut self, tool: Tool) {
        if self.tool == Tool::PolygonSelect || tool == Tool::PolygonSelect {
            self.selection_points.clear();
        }
        self.tool = tool;
    }
    fn conflicting_tool_keys(&self) -> std::collections::HashSet<egui::Key> {
        let bindings = [
            (
                Self::configured_key(&self.shortcuts.eraser, egui::Key::E),
                "eraser",
            ),
            (
                Self::configured_key(&self.shortcuts.brush_tool, egui::Key::Num1),
                "brush",
            ),
            (
                Self::configured_key(&self.shortcuts.rectangle_tool, egui::Key::Num2),
                "rectangle",
            ),
            (
                Self::configured_key(&self.shortcuts.fill_tool, egui::Key::Num3),
                "fill",
            ),
            (
                Self::configured_key(&self.shortcuts.eyedropper_tool, egui::Key::Num4),
                "eyedropper",
            ),
            (
                Self::configured_key(&self.shortcuts.move_tool, egui::Key::Num5),
                "move",
            ),
            (
                Self::configured_key(&self.shortcuts.pan_tool, egui::Key::Num6),
                "pan",
            ),
            (
                Self::configured_key(&self.shortcuts.blur_tool, egui::Key::Num7),
                "blur",
            ),
            (
                Self::configured_key(&self.shortcuts.smudge_tool, egui::Key::Num8),
                "smudge",
            ),
            (
                Self::configured_key(&self.shortcuts.ellipse_tool, egui::Key::Num9),
                "ellipse",
            ),
            (
                Self::configured_key(&self.shortcuts.lasso_tool, egui::Key::Q),
                "lasso",
            ),
            (
                Self::configured_key(&self.shortcuts.polygon_tool, egui::Key::W),
                "polygon",
            ),
            (
                Self::configured_key(&self.shortcuts.magic_wand_tool, egui::Key::R),
                "wand",
            ),
            (
                Self::configured_key(&self.shortcuts.color_range_tool, egui::Key::T),
                "color range",
            ),
            (
                Self::configured_key(&self.shortcuts.line_ruler_tool, egui::Key::Y),
                "line ruler",
            ),
            (
                Self::configured_key(&self.shortcuts.ellipse_ruler_tool, egui::Key::U),
                "ellipse ruler",
            ),
            (
                Self::configured_key(&self.shortcuts.bezier_ruler_tool, egui::Key::I),
                "bezier ruler",
            ),
            (
                Self::configured_key(&self.shortcuts.perspective_ruler_tool, egui::Key::O),
                "perspective ruler",
            ),
            (
                Self::configured_key(&self.shortcuts.selection_brush_tool, egui::Key::G),
                "selection brush",
            ),
            (
                Self::configured_key(&self.shortcuts.quick_mask_tool, egui::Key::H),
                "quick mask",
            ),
        ];
        let mut counts = std::collections::HashMap::new();
        for (key, _) in bindings {
            *counts.entry(key).or_insert(0usize) += 1;
        }
        counts
            .into_iter()
            .filter_map(|(key, count)| (count > 1).then_some(key))
            .collect()
    }
    fn symmetric_points(&self, p: InkPoint) -> Vec<InkPoint> {
        let mut mirrored = vec![p];
        for (flip_x, flip_y) in [
            (self.symmetry_x, false),
            (false, self.symmetry_y),
            (self.symmetry_x, self.symmetry_y),
        ] {
            if !flip_x && !flip_y {
                continue;
            }
            let rotation = match (flip_x, flip_y) {
                (true, true) => p.rotation + std::f32::consts::PI,
                (true, false) => std::f32::consts::PI - p.rotation,
                (false, true) => -p.rotation,
                (false, false) => p.rotation,
            };
            mirrored.push(InkPoint {
                position: glam::Vec2::new(
                    if flip_x {
                        2. * self.symmetry_center.x - p.position.x
                    } else {
                        p.position.x
                    },
                    if flip_y {
                        2. * self.symmetry_center.y - p.position.y
                    } else {
                        p.position.y
                    },
                ),
                pressure: p.pressure,
                taper: p.taper,
                tilt: glam::Vec2::new(
                    if flip_x { -p.tilt.x } else { p.tilt.x },
                    if flip_y { -p.tilt.y } else { p.tilt.y },
                ),
                rotation,
                time_ms: p.time_ms,
            });
        }
        let count = self.symmetry_count.clamp(1, 32);
        let center = glam::Vec2::new(self.symmetry_center.x, self.symmetry_center.y);
        let mut points = Vec::with_capacity(mirrored.len() * count as usize);
        for base in mirrored {
            for i in 0..count {
                let angle = std::f32::consts::TAU * i as f32 / count as f32;
                let (sin, cos) = angle.sin_cos();
                let offset = base.position - center;
                points.push(InkPoint {
                    position: center
                        + glam::Vec2::new(
                            offset.x * cos - offset.y * sin,
                            offset.x * sin + offset.y * cos,
                        ),
                    tilt: glam::Vec2::new(
                        base.tilt.x * cos - base.tilt.y * sin,
                        base.tilt.x * sin + base.tilt.y * cos,
                    ),
                    rotation: base.rotation + angle,
                    ..base
                });
            }
        }
        points
    }
    fn apply_selection_symmetry(&mut self) {
        if !self.selection.active
            || (!self.symmetry_x && !self.symmetry_y && self.symmetry_count <= 1)
        {
            return;
        }
        let source = self.selection.mask.clone();
        let width = self.doc.width as i32;
        let height = self.doc.height as i32;
        let center = self.symmetry_center;
        let mirrors = [
            (false, false),
            (self.symmetry_x, false),
            (false, self.symmetry_y),
            (self.symmetry_x, self.symmetry_y),
        ];
        let mut output = vec![0u8; source.len()];
        let divisions = self.symmetry_count.clamp(1, 32);
        for (index, &coverage) in source.iter().enumerate() {
            if coverage == 0 {
                continue;
            }
            let x = (index % width as usize) as f32;
            let y = (index / width as usize) as f32;
            for &(flip_x, flip_y) in &mirrors {
                if (flip_x && !self.symmetry_x) || (flip_y && !self.symmetry_y) {
                    continue;
                }
                let base_x = if flip_x { 2.0 * center.x - x } else { x };
                let base_y = if flip_y { 2.0 * center.y - y } else { y };
                let dx = base_x - center.x;
                let dy = base_y - center.y;
                for division in 0..divisions {
                    let angle = std::f32::consts::TAU * division as f32 / divisions as f32;
                    let rx = center.x + dx * angle.cos() - dy * angle.sin();
                    let ry = center.y + dx * angle.sin() + dy * angle.cos();
                    let tx = rx.round() as i32;
                    let ty = ry.round() as i32;
                    if tx >= 0 && ty >= 0 && tx < width && ty < height {
                        let target = ty as usize * width as usize + tx as usize;
                        output[target] = output[target].max(coverage);
                    }
                }
            }
        }
        self.selection.mask = output;
    }
    fn begin_selection_operation(&mut self, modifiers: egui::Modifiers) {
        self.selection_combine_mode = if modifiers.shift && modifiers.ctrl {
            SelectionCombineMode::Intersect
        } else if modifiers.shift {
            SelectionCombineMode::Add
        } else if modifiers.ctrl {
            SelectionCombineMode::Subtract
        } else {
            SelectionCombineMode::Replace
        };
        self.selection_before_gesture = Some((self.selection.active, self.selection.mask.clone()));
    }
    fn finish_selection_operation(&mut self) {
        let Some((was_active, before)) = self.selection_before_gesture.take() else {
            return;
        };
        let width = self.doc.width as usize;
        let height = self.doc.height as usize;
        let len = width.saturating_mul(height);
        let after_active = self.selection.active;
        let after = std::mem::take(&mut self.selection.mask);
        let mut combined = vec![0; len];
        let had_pixels = was_active && before.iter().any(|value| *value != 0);
        match self.selection_combine_mode {
            SelectionCombineMode::Replace => {
                let copy_len = len.min(after.len());
                combined[..copy_len].copy_from_slice(&after[..copy_len]);
            }
            SelectionCombineMode::Add => {
                for (index, value) in combined.iter_mut().enumerate() {
                    let old = if was_active {
                        before.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    let new = if after_active {
                        after.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    *value = old.max(new);
                }
            }
            SelectionCombineMode::Subtract => {
                for (index, value) in combined.iter_mut().enumerate() {
                    let old = if was_active {
                        before.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    let new = if after_active {
                        after.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    *value = (old as f32 * (1.0 - new as f32 / 255.0))
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
            SelectionCombineMode::Intersect => {
                for (index, value) in combined.iter_mut().enumerate() {
                    let old = if was_active {
                        before.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    let new = if after_active {
                        after.get(index).copied().unwrap_or(0)
                    } else {
                        0
                    };
                    *value = (old as u16 * new as u16 / 255) as u8;
                }
            }
        }
        let has_pixels = combined.iter().any(|value| *value != 0);
        self.selection.mask = combined;
        self.selection.active = match self.selection_combine_mode {
            SelectionCombineMode::Replace => after_active && has_pixels,
            SelectionCombineMode::Add => (was_active || after_active) && has_pixels,
            SelectionCombineMode::Subtract => had_pixels && has_pixels,
            SelectionCombineMode::Intersect => was_active && after_active && has_pixels,
        };
        if !self.selection.active {
            self.selection.mask.clear();
        }
        self.history.record_selection_change(
            was_active,
            before,
            self.selection.active,
            self.selection.mask.clone(),
        );
    }
    fn change_selection(&mut self, change: impl FnOnce(&mut Selection, u32, u32)) {
        let before_active = self.selection.active;
        let before = self.selection.mask.clone();
        change(&mut self.selection, self.doc.width, self.doc.height);
        self.history.record_selection_change(
            before_active,
            before,
            self.selection.active,
            self.selection.mask.clone(),
        );
    }
    fn apply_selection_history_update(&mut self) {
        if let Some((active, mask)) = self.history.take_selection_update() {
            self.selection.active = active;
            self.selection.mask = mask;
        }
    }
    /// Paints several stroke dabs. Brushes the GPU can paint are sent as one
    /// batch; everything else goes dab by dab through `dab`.
    fn dab_many(&mut self, points: &[InkPoint]) {
        if self.vector_target() {
            self.vector_dabs(points);
            return;
        }
        let layer = self.selected_layer;
        let brush = &self.brushes[self.selected_brush];
        let kind = match self.tool {
            Tool::Blur => BrushKind::Blur,
            Tool::Smudge => BrushKind::Smudge,
            _ => brush.kind,
        };
        let paintable = !self.editing_mask
            && !matches!(self.tool, Tool::SelectionBrush | Tool::QuickMask)
            && !self.doc.layers[layer].locked
            && !self.is_reference_layer(layer)
            && self.doc.layers[layer].kind != LayerKind::Folder;
        let style = efude_brush::engine::DabStyle {
            brush,
            kind,
            eraser: self.tool == Tool::Eraser
                || matches!(kind, BrushKind::Eraser)
                || self.erases_with_transparent_color(kind),
            color: [
                self.color.r(),
                self.color.g(),
                self.color.b(),
                self.color.a(),
            ],
            size: self.size,
        };
        if !paintable || !efude_brush::engine::accelerator_can_paint(style) {
            for point in points {
                self.dab(*point);
            }
            return;
        }
        let mut stamps = Vec::with_capacity(points.len());
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
            for mirrored in self.symmetric_points(p) {
                stamps.push((mirrored, dynamics));
            }
            self.raster.finish_dab(style, p);
        }
        let mut target = efude_brush::engine::DabTarget {
            doc: &mut self.doc,
            layer,
            selection: self
                .selection
                .active
                .then_some(self.selection.mask.as_slice()),
            history: &mut self.history,
        };
        let accelerator = self
            .gpu_cover
            .as_ref()
            .map(|gpu| gpu as &dyn efude_brush::engine::CoverAccelerator);
        self.raster.stamp_many_with_fallback(
            &mut target,
            style,
            &stamps,
            accelerator,
            self.gpu_min_pixels,
            Some(&efude_brush::engine::CpuCover),
        );
    }
    fn dab(&mut self, p: InkPoint) {
        if self.vector_target() {
            self.vector_dabs(&[p]);
            return;
        }
        let mut p = p;
        p.pressure =
            self.brushes[self.selected_brush].map_pressure(self.global_pressure_curve(p.pressure));
        let dynamics = efude_brush::engine::dynamics(
            &self.brushes[self.selected_brush],
            &p,
            self.raster.last_dab,
            self.view_scale,
        );
        self.dynamic_size = dynamics.size;
        self.dynamic_opacity = dynamics.opacity;
        self.dynamic_concentration = dynamics.concentration;
        self.dynamic_mix = dynamics.mix;
        self.dynamic_dilution = dynamics.dilution;
        if matches!(self.tool, Tool::SelectionBrush | Tool::QuickMask) {
            for point in self.symmetric_points(p) {
                self.selection_dab(point);
            }
            self.raster.last_dab = Some(p);
            return;
        }
        for point in self.symmetric_points(p) {
            self.dab_one(point, &dynamics);
        }
        let brush = &self.brushes[self.selected_brush];
        let style = efude_brush::engine::DabStyle {
            brush,
            kind: brush.kind,
            eraser: self.tool == Tool::Eraser
                || matches!(brush.kind, BrushKind::Eraser)
                || self.erases_with_transparent_color(brush.kind),
            color: [0; 4],
            size: self.size,
        };
        self.raster.finish_dab(style, p);
    }
    fn selection_dab(&mut self, p: InkPoint) {
        let w = self.doc.width;
        let h = self.doc.height;
        if !self.selection.active {
            self.selection.mask = vec![0; (w * h) as usize];
            self.selection.active = true;
        }
        let brush = &self.brushes[self.selected_brush];
        let r = (self.size * self.dynamic_size * 0.5).max(0.5);
        let tilt = p.tilt.length().clamp(0.0, 1.0);
        let aspect = (brush.tip_aspect * (1.0 - brush.tilt_flattening.clamp(0.0, 0.9) * tilt))
            .clamp(0.1, 10.0);
        let angle = p.rotation
            + brush.tip_rotation.to_radians()
            + if tilt > 0.001 {
                p.tilt.y.atan2(p.tilt.x) * brush.tilt_rotation.clamp(0.0, 1.0)
            } else {
                0.0
            };
        let (sin, cos) = angle.sin_cos();
        let cx = p.position.x.round() as i32;
        let cy = p.position.y.round() as i32;
        let extent = r * aspect.max(1.0 / aspect);
        for y in cy - extent.ceil() as i32..=cy + extent.ceil() as i32 {
            for x in cx - extent.ceil() as i32..=cx + extent.ceil() as i32 {
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let dx = x as f32 - p.position.x;
                let dy = y as f32 - p.position.y;
                let local_x = dx * cos + dy * sin;
                let local_y = -dx * sin + dy * cos;
                let distance = (local_x * local_x / (aspect * aspect)
                    + local_y * local_y * aspect * aspect)
                    .sqrt();
                if distance > r + 1.0 {
                    continue;
                }
                let edge = ((1.0 - brush.hardness.clamp(0.0, 1.0)) * r * 0.75).max(0.5);
                let mut coverage = ((r - distance) / edge + 0.5).clamp(0.0, 1.0);
                if let Some(tip) = &brush.tip {
                    let u = ((local_x / (r * aspect) * 0.5 + 0.5)
                        * tip.width.saturating_sub(1) as f32)
                        .round()
                        .clamp(0.0, tip.width.saturating_sub(1) as f32)
                        as usize;
                    let v = ((local_y * aspect / r * 0.5 + 0.5)
                        * tip.height.saturating_sub(1) as f32)
                        .round()
                        .clamp(0.0, tip.height.saturating_sub(1) as f32)
                        as usize;
                    coverage *= tip.coverage[v * tip.width as usize + u] as f32 / 255.0;
                }
                let strength = (coverage * brush.opacity.clamp(0.0, 1.0) * self.dynamic_opacity)
                    .clamp(0.0, 1.0);
                let index = (y as u32 * w + x as u32) as usize;
                let old = self.selection.mask[index] as f32 / 255.0;
                let next = if self.selection_erase {
                    old * (1.0 - strength)
                } else {
                    old + (1.0 - old) * strength
                };
                self.selection.mask[index] = (next * 255.0).round() as u8;
            }
        }
    }
    /// Paints one (possibly mirrored) dab on the selected layer or its mask.
    fn dab_one(&mut self, p: InkPoint, dynamics: &efude_brush::engine::Dynamics) {
        if self.editing_mask {
            self.dab_mask(p);
            return;
        }
        let layer = self.selected_layer;
        if self.doc.layers[layer].locked
            || self.is_reference_layer(layer)
            || self.doc.layers[layer].kind == LayerKind::Folder
        {
            return;
        }
        let brush = &self.brushes[self.selected_brush];
        let kind = match self.tool {
            Tool::Blur => BrushKind::Blur,
            Tool::Smudge => BrushKind::Smudge,
            _ => brush.kind,
        };
        let style = efude_brush::engine::DabStyle {
            brush,
            kind,
            eraser: self.tool == Tool::Eraser
                || matches!(kind, BrushKind::Eraser)
                || self.erases_with_transparent_color(kind),
            color: [
                self.color.r(),
                self.color.g(),
                self.color.b(),
                self.color.a(),
            ],
            size: self.size,
        };
        let mut target = efude_brush::engine::DabTarget {
            doc: &mut self.doc,
            layer,
            selection: self
                .selection
                .active
                .then_some(self.selection.mask.as_slice()),
            history: &mut self.history,
        };
        self.raster.stamp(&mut target, style, dynamics, p);
    }
    fn dab_mask(&mut self, p: InkPoint) {
        let li = self.selected_layer;
        if self.doc.layers[li].locked
            || self.is_reference_layer(li)
            || self.doc.layers[li].kind == LayerKind::Folder
        {
            return;
        }
        if self.doc.layers[li].mask.is_none() {
            self.doc.layers[li].mask = Some(efude_canvas::TilePixels::new(
                self.doc.width,
                self.doc.height,
            ));
        }
        let brush = &self.brushes[self.selected_brush];
        let radius = (self.size * self.dynamic_size * 0.5).max(0.5);
        let tilt_amount = p.tilt.length().clamp(0.0, 1.0);
        let aspect = (brush.tip_aspect
            * (1.0 - brush.tilt_flattening.clamp(0.0, 0.9) * tilt_amount))
            .clamp(0.1, 10.0);
        let angle = p.rotation + brush.tip_rotation.to_radians();
        let (sin, cos) = angle.sin_cos();
        let max_radius = radius * aspect.max(1.0 / aspect);
        let (cx, cy) = (p.position.x.round() as i32, p.position.y.round() as i32);
        let eraser = self.tool == Tool::Eraser || matches!(brush.kind, BrushKind::Eraser);
        let target = if eraser {
            0.0
        } else {
            ((self.color.r() as u16 + self.color.g() as u16 + self.color.b() as u16) / 3) as f32
        };
        for y in cy - max_radius.ceil() as i32..=cy + max_radius.ceil() as i32 {
            for x in cx - max_radius.ceil() as i32..=cx + max_radius.ceil() as i32 {
                if x < 0 || y < 0 || x >= self.doc.width as i32 || y >= self.doc.height as i32 {
                    continue;
                }
                let dx = x as f32 - p.position.x;
                let dy = y as f32 - p.position.y;
                let local_x = dx * cos + dy * sin;
                let local_y = -dx * sin + dy * cos;
                let distance = (local_x * local_x / (aspect * aspect)
                    + local_y * local_y * (aspect * aspect))
                    .sqrt();
                if distance > radius {
                    continue;
                }
                let pi = (y as u32 * self.doc.width + x as u32) as usize;
                let selection_coverage = if self.selection.active {
                    self.selection.mask.get(pi).copied().unwrap_or(0) as f32 / 255.0
                } else {
                    1.0
                };
                if selection_coverage <= 0.0 {
                    continue;
                }
                let hardness = brush.hardness.clamp(0.0, 1.0);
                let edge = (1.0 - hardness) * radius * 0.75 + 0.5;
                let mut coverage = ((radius - distance) / edge + 0.5).clamp(0.0, 1.0);
                if let Some(tip) = &brush.tip {
                    let u = ((local_x / (radius * aspect) * 0.5 + 0.5)
                        * tip.width.saturating_sub(1) as f32)
                        .round()
                        .clamp(0.0, tip.width.saturating_sub(1) as f32)
                        as usize;
                    let v = ((local_y * aspect / radius * 0.5 + 0.5)
                        * tip.height.saturating_sub(1) as f32)
                        .round()
                        .clamp(0.0, tip.height.saturating_sub(1) as f32)
                        as usize;
                    coverage *= tip.coverage[v * tip.width as usize + u] as f32 / 255.0;
                }
                if brush.grain > 0.0 {
                    let scale = brush.grain_scale.max(0.05);
                    let (gx, gy) = if brush.grain_fixed {
                        (x as f32, y as f32)
                    } else {
                        // Following the brush: measured from this dab.
                        (x as f32 - p.position.x, y as f32 - p.position.y)
                    };
                    let noise = if let Some(texture) = &brush.grain_tip {
                        let tx = (gx * scale).floor() as i32;
                        let ty = (gy * scale).floor() as i32;
                        texture.coverage[(ty.rem_euclid(texture.height as i32) as usize)
                            * texture.width as usize
                            + tx.rem_euclid(texture.width as i32) as usize]
                            as f32
                            / 255.0
                    } else {
                        let ix = (gx * scale).floor() as i32;
                        let iy = (gy * scale).floor() as i32;
                        let bits =
                            (ix.wrapping_mul(374761393) ^ iy.wrapping_mul(668265263) ^ 0x51ed270b)
                                .wrapping_mul(1274126177) as u32;
                        bits as f32 / u32::MAX as f32
                    };
                    coverage *= 1.0 - brush.grain.clamp(0.0, 1.0) * (1.0 - noise);
                }
                let strength = (brush.opacity
                    * self.dynamic_opacity
                    * self.dynamic_concentration
                    * coverage
                    * selection_coverage)
                    .clamp(0.0, 1.0);
                if strength <= 0.0 {
                    continue;
                }
                let index = pi * 4;
                let layer = &self.doc.layers[li];
                self.history.record_mask(layer, index);
                let mask = self.doc.layers[li].mask.as_mut().unwrap();
                if !mask.has_tile(x as u32, y as u32) {
                    mask.ensure_tile_filled(x as u32, y as u32, [255; 4]);
                }
                let old = mask[index] as f32;
                mask[index] = (old + (target - old) * strength).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    fn apply_transform(&mut self, sx: f32, sy: f32, angle: f32) {
        if self.doc.layers[self.selected_layer].locked
            || self.is_reference_layer(self.selected_layer)
            || self.doc.layers[self.selected_layer].kind == LayerKind::Folder
        {
            return;
        }
        self.history.begin();
        let before_active = self.selection.active;
        let before_selection = self.selection.mask.clone();
        let vector = self.doc.layers[self.selected_layer].is_vector();
        efude_canvas::transform_selection(
            &mut self.doc.layers[self.selected_layer],
            &mut self.selection,
            self.doc.width,
            self.doc.height,
            sx,
            sy,
            angle,
            &mut self.history,
        );
        if vector {
            self.transform_vector_lines(&before_selection, before_active, (sx, sy), angle);
        }
        self.history.record_selection_change(
            before_active,
            before_selection,
            self.selection.active,
            self.selection.mask.clone(),
        );
        self.history.commit();
    }
    fn apply_mesh_warp(&mut self) {
        if self.refuse_on_vector_layer() {
            return;
        }
        if self.doc.layers[self.selected_layer].locked
            || self.is_reference_layer(self.selected_layer)
            || self.doc.layers[self.selected_layer].kind == LayerKind::Folder
        {
            return;
        }
        self.history.begin();
        let before_active = self.selection.active;
        let before_selection = self.selection.mask.clone();
        efude_canvas::mesh_warp_grid(
            &mut self.doc.layers[self.selected_layer],
            &mut self.selection,
            self.doc.width,
            self.doc.height,
            4,
            4,
            &self.mesh_offsets,
            &mut self.history,
        );
        self.history.record_selection_change(
            before_active,
            before_selection,
            self.selection.active,
            self.selection.mask.clone(),
        );
        self.history.commit();
        self.mesh_offsets = [[0.; 2]; 16];
        self.status = self
            .text("メッシュ変形を適用しました", "Mesh transform applied")
            .into();
    }
    fn copy_selection(&mut self, cut: bool) {
        let w = self.doc.width;
        let h = self.doc.height;
        let layer = &self.doc.layers[self.selected_layer];
        if cut
            && (layer.locked
                || self.is_reference_layer(self.selected_layer)
                || layer.kind == LayerKind::Folder)
        {
            return;
        }
        let mut bounds = (0i32, 0i32, w as i32 - 1, h as i32 - 1);
        if self.selection.active {
            bounds = (w as i32, h as i32, -1, -1);
            for (i, &v) in self.selection.mask.iter().enumerate() {
                if v != 0 {
                    let x = (i % w as usize) as i32;
                    let y = (i / w as usize) as i32;
                    bounds.0 = bounds.0.min(x);
                    bounds.1 = bounds.1.min(y);
                    bounds.2 = bounds.2.max(x);
                    bounds.3 = bounds.3.max(y);
                }
            }
            if bounds.2 < 0 {
                return;
            }
        }
        let cw = (bounds.2 - bounds.0 + 1) as u32;
        let ch = (bounds.3 - bounds.1 + 1) as u32;
        let mut pixels = vec![0; (cw * ch * 4) as usize];
        for y in 0..ch {
            for x in 0..cw {
                let sx = bounds.0 as u32 + x;
                let sy = bounds.1 as u32 + y;
                let si = ((sy * w + sx) * 4) as usize;
                let di = ((y * cw + x) * 4) as usize;
                let coverage = if self.selection.active {
                    self.selection
                        .mask
                        .get((sy * w + sx) as usize)
                        .copied()
                        .unwrap_or(0) as f32
                        / 255.0
                } else {
                    1.0
                };
                if coverage > 0.0 {
                    pixels[di..di + 4].copy_from_slice(&layer.pixels[si..si + 4]);
                    pixels[di + 3] = (pixels[di + 3] as f32 * coverage).round() as u8;
                }
            }
        }
        self.clipboard = Some((cw, ch, pixels));
        #[cfg(target_os = "windows")]
        if let Some((clip_w, clip_h, clip_pixels)) = &self.clipboard {
            let _ = efude_input::set_clipboard_image(*clip_w, *clip_h, clip_pixels);
        }
        if cut && self.doc.layers[self.selected_layer].is_vector() {
            self.erase_vector_selection();
        } else if cut {
            self.clear_pixels(bounds, cw, ch);
        }
    }
    /// Deletes the selected pixels of the current layer (the whole layer
    /// when nothing is selected). Undoable.
    fn delete_selected_pixels(&mut self) {
        let (w, h) = (self.doc.width, self.doc.height);
        let layer = &self.doc.layers[self.selected_layer];
        if layer.locked
            || self.is_reference_layer(self.selected_layer)
            || layer.kind != LayerKind::Raster
        {
            return;
        }
        if layer.is_vector() {
            self.erase_vector_selection();
            return;
        }
        let mut bounds = (0i32, 0i32, w as i32 - 1, h as i32 - 1);
        if self.selection.active {
            bounds = (w as i32, h as i32, -1, -1);
            for (i, &v) in self.selection.mask.iter().enumerate() {
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
        }
        let cw = (bounds.2 - bounds.0 + 1) as u32;
        let ch = (bounds.3 - bounds.1 + 1) as u32;
        self.clear_pixels(bounds, cw, ch);
        self.canvas_texture_dirty = true;
    }
    fn clear_pixels(&mut self, bounds: (i32, i32, i32, i32), cw: u32, ch: u32) {
        let w = self.doc.width;
        {
            self.history.begin();
            for y in 0..ch {
                for x in 0..cw {
                    let sx = bounds.0 as u32 + x;
                    let sy = bounds.1 as u32 + y;
                    let pi = (sy * w + sx) as usize;
                    let coverage = if self.selection.active {
                        self.selection.mask.get(pi).copied().unwrap_or(0) as f32 / 255.0
                    } else {
                        1.0
                    };
                    if coverage > 0.0 {
                        let i = pi * 4;
                        {
                            let l = &self.doc.layers[self.selected_layer];
                            for c in 0..4 {
                                self.history.record_pixel(l, i + c);
                            }
                        }
                        let pixels = &mut self.doc.layers[self.selected_layer].pixels;
                        let alpha = pixels[i + 3] as f32 * (1.0 - coverage);
                        pixels[i + 3] = alpha.round() as u8;
                        if pixels[i + 3] == 0 {
                            pixels[i..i + 3].fill(0);
                        }
                    }
                }
            }
            self.history.commit();
        }
    }
    fn paste_clipboard(&mut self, ctx: &egui::Context) {
        if self.refuse_on_vector_layer() {
            return;
        }
        if self.doc.layers[self.selected_layer].locked
            || self.is_reference_layer(self.selected_layer)
            || self.doc.layers[self.selected_layer].kind != LayerKind::Raster
        {
            return;
        }
        #[cfg(target_os = "windows")]
        let os_image = efude_input::get_clipboard_image().ok().flatten();
        #[cfg(not(target_os = "windows"))]
        let os_image: Option<(u32, u32, Vec<u8>)> = None;
        let Some((w, h, pixels)) = os_image.or_else(|| self.clipboard.clone()) else {
            return;
        };
        if w == 0
            || h == 0
            || (w as u64 * h as u64) > 100_000_000
            || pixels.len() != (w as usize * h as usize * 4)
        {
            self.status = self
                .text(
                    "クリップボード画像のサイズが大きすぎます",
                    "Clipboard image is too large",
                )
                .into();
            return;
        }
        let ox = self.doc.width.saturating_sub(w) / 2;
        let oy = self.doc.height.saturating_sub(h) / 2;
        // The pasted image is placed freely; an old selection would clip it
        // away when it lands somewhere else.
        if self.selection.active {
            self.change_selection(|selection, _, _| selection.clear());
        }
        self.paste_preview = Some((w, h, pixels, Vec2::new(ox as f32, oy as f32)));
        if let Some((w, h, pixels, _)) = &self.paste_preview {
            let mut rgba = vec![0u8; pixels.len()];
            for (source, target) in pixels.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                target.copy_from_slice(source);
                target[3] = (target[3] as f32 * 0.7) as u8;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([*w as usize, *h as usize], &rgba);
            self.paste_texture =
                Some(ctx.load_texture("paste-preview", image, egui::TextureOptions::LINEAR));
        }
    }
    fn commit_paste_preview(&mut self) {
        if self.doc.layers[self.selected_layer].locked
            || self.is_reference_layer(self.selected_layer)
            || self.doc.layers[self.selected_layer].kind != LayerKind::Raster
        {
            self.paste_preview = None;
            self.paste_texture = None;
            return;
        }
        let Some((w, h, pixels, position)) = self.paste_preview.take() else {
            return;
        };
        self.paste_texture = None;
        let ox = position.x.round() as i32;
        let oy = position.y.round() as i32;
        self.history.begin();
        for y in 0..h.min(self.doc.height) {
            for x in 0..w.min(self.doc.width) {
                let dx = ox + x as i32;
                let dy = oy + y as i32;
                if dx < 0 || dy < 0 || dx >= self.doc.width as i32 || dy >= self.doc.height as i32 {
                    continue;
                }
                let i = ((dy as u32 * self.doc.width + dx as u32) * 4) as usize;
                let si = ((y * w + x) * 4) as usize;
                if pixels[si + 3] == 0 {
                    continue;
                }
                let x_doc = dx as u32;
                let y_doc = dy as u32;
                let selection_coverage = if self.selection.active {
                    self.selection
                        .mask
                        .get((y_doc * self.doc.width + x_doc) as usize)
                        .copied()
                        .unwrap_or(0) as f32
                        / 255.0
                } else {
                    1.0
                };
                let source_alpha = pixels[si + 3] as f32 / 255.0 * selection_coverage;
                if source_alpha <= 0.0 {
                    continue;
                }
                let l = &self.doc.layers[self.selected_layer];
                for c in 0..4 {
                    self.history.record_pixel(l, i + c);
                }
                let destination = self.doc.layers[self.selected_layer]
                    .pixels
                    .pixel(x_doc, y_doc);
                let destination_alpha = destination[3] as f32 / 255.0;
                let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
                let mut output = [0u8; 4];
                for channel in 0..3 {
                    output[channel] = ((pixels[si + channel] as f32 * source_alpha
                        + destination[channel] as f32 * destination_alpha * (1.0 - source_alpha))
                        / output_alpha)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                output[3] = (output_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
                self.doc.layers[self.selected_layer]
                    .pixels
                    .set_pixel(x_doc, y_doc, output);
            }
        }
        self.history.commit();
    }
    fn finish_stroke(&mut self) {
        let points = std::mem::take(&mut self.active);
        self.stabilized_cursor = None;
        self.preview_cursor_estimate = None;
        if let Some(mut builder) = self.stroke_builder.take() {
            // Live brush stroke: everything before the tail is already final.
            // Replace the provisional tail with the finished one (end taper,
            // catch-up to the pen-up point) and commit.
            self.restore_provisional();
            self.pending_provisional.clear();
            if points.is_empty() {
                self.rollback_vector_stroke();
                self.vector_live = None;
                self.reset_stroke_buffers();
                return;
            }
            // Every stroke is kept (the last 128) so it can be saved as an
            // input log for comparisons and regression tests.
            self.stroke_log.push(&points);
            let tail = builder.finish();
            self.dab_many(&tail);
            // The wet edge darkens the rim after the pen lifts.
            if self.brushes[self.selected_brush].settle {
                self.finish_wet_edge();
            }
            self.finish_vector_stroke();
            self.doc.layers[self.selected_layer]
                .pixels
                .prune_empty_tiles();
            self.history.commit();
            self.reset_stroke_buffers();
            self.status = self.text("描画しました", "Painted").into();
            self.fall_back_to_wintab_if_ink_is_silent();
            return;
        }
        if points.is_empty() {
            self.reset_stroke_buffers();
            return;
        }
        self.stroke_log.push(&points);
        // Ruler and shape tools preview raw dabs, then render the final
        // stroke here in one pass.
        self.reset_stroke_buffers();
        self.history.rollback_active(&mut self.doc);
        self.rollback_vector_stroke();
        let spaced = self.offline_stroke_dabs(&points);
        self.raster.last_dab = None;
        self.raster.previous_mix_color = [
            self.color.r() as f32 / 255.,
            self.color.g() as f32 / 255.,
            self.color.b() as f32 / 255.,
        ];
        self.raster.remaining_charge = self.brushes[self.selected_brush].mix.charge;
        self.dab_many(&spaced);
        self.finish_wet_edge();
        self.finish_vector_stroke();
        self.doc.layers[self.selected_layer]
            .pixels
            .prune_empty_tiles();
        self.history.commit();
        self.reset_stroke_buffers();
        self.status = self.text("描画しました", "Painted").into();
    }
    /// Pen-up for wet brushes: pigment gathers along the stroke's rim.
    fn finish_wet_edge(&mut self) {
        let layer = self.selected_layer;
        if self.editing_mask
            || matches!(
                self.tool,
                Tool::SelectionBrush | Tool::QuickMask | Tool::Blur | Tool::Smudge
            )
            || self.doc.layers[layer].locked
            || self.is_reference_layer(layer)
            || self.doc.layers[layer].kind == LayerKind::Folder
            || self.doc.layers[layer].is_vector()
            || self.erases_with_transparent_color(self.brushes[self.selected_brush].kind)
        {
            return;
        }
        let brush = &self.brushes[self.selected_brush];
        let mut target = efude_brush::engine::DabTarget {
            doc: &mut self.doc,
            layer,
            selection: self
                .selection
                .active
                .then_some(self.selection.mask.as_slice()),
            history: &mut self.history,
        };
        self.raster.finish_stroke(&mut target, brush);
    }
    fn draw_bezier_ruler(&mut self, points: [Vec2; 4]) {
        self.reset_stroke_buffers();
        self.begin_grain(glam::Vec2::new(points[0].x, points[0].y));
        self.active.clear();
        self.history.begin();
        self.raster.previous_mix_color = [
            self.color.r() as f32 / 255.,
            self.color.g() as f32 / 255.,
            self.color.b() as f32 / 255.,
        ];
        self.raster.last_dab = None;
        self.raster.remaining_charge = self.brushes[self.selected_brush].mix.charge;
        let length = (points[1] - points[0]).length()
            + (points[2] - points[1]).length()
            + (points[3] - points[2]).length();
        let steps = (length / self.stroke_spacing()).ceil().clamp(32., 2048.) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let u = 1. - t;
            let pos = points[0] * (u * u * u)
                + points[1] * (3. * u * u * t)
                + points[2] * (3. * u * t * t)
                + points[3] * (t * t * t);
            let p = InkPoint::new(pos.x, pos.y, 1., i as u64);
            self.active.push(p);
            self.dab(p);
        }
        self.finish_stroke();
        self.status = self
            .text("ベジェ曲線を描画しました", "Bezier curve drawn")
            .into();
    }
    fn canvas_position(&self, pos: Pos2, rect: Rect, scale: f32) -> Vec2 {
        self.document_position(pos, rect, scale)
    }
    fn sample_eyedropper(&self, x: i32, y: i32) -> Option<Color32> {
        if x < 0 || y < 0 || x >= self.doc.width as i32 || y >= self.doc.height as i32 {
            return None;
        }
        let pixels = if self.eyedropper_composite {
            efude_canvas::composite_transparent(&self.doc)
        } else {
            self.doc.layers[self.selected_layer].pixels.to_dense()
        };
        let radius = self.eyedropper_radius as i32;
        let mut rgb = [0u64; 3];
        let mut alpha = 0u64;
        let mut samples = 0u64;
        let mut rgb_weight = 0u64;
        for sy in (y - radius).max(0)..=(y + radius).min(self.doc.height as i32 - 1) {
            for sx in (x - radius).max(0)..=(x + radius).min(self.doc.width as i32 - 1) {
                let i = ((sy as u32 * self.doc.width + sx as u32) * 4) as usize;
                let a = pixels[i + 3] as u64;
                for c in 0..3 {
                    rgb[c] += pixels[i + c] as u64 * a;
                }
                rgb_weight += a;
                alpha += a;
                samples += 1;
            }
        }
        if samples == 0 {
            return None;
        }
        Some(Color32::from_rgba_unmultiplied(
            rgb[0].checked_div(rgb_weight).unwrap_or(0) as u8,
            rgb[1].checked_div(rgb_weight).unwrap_or(0) as u8,
            rgb[2].checked_div(rgb_weight).unwrap_or(0) as u8,
            (alpha / samples) as u8,
        ))
    }
    fn render_replay(&mut self, points: &[InkPoint]) {
        self.reset_stroke_buffers();
        if let Some(first) = points.first() {
            self.begin_grain(first.position);
        }
        let replay = self.offline_stroke_dabs(points);
        self.raster.previous_mix_color = [
            self.color.r() as f32 / 255.0,
            self.color.g() as f32 / 255.0,
            self.color.b() as f32 / 255.0,
        ];
        self.raster.remaining_charge = self.brushes[self.selected_brush].mix.charge;
        self.raster.last_dab = None;
        self.dab_many(&replay);
        self.finish_wet_edge();
        self.finish_vector_stroke();
        self.doc.layers[self.selected_layer]
            .pixels
            .prune_empty_tiles();
        self.reset_stroke_buffers();
    }
    fn fill_at(&mut self, x: u32, y: u32) {
        if x >= self.doc.width || y >= self.doc.height || self.refuse_on_vector_layer() {
            return;
        }
        let layer = &self.doc.layers[self.selected_layer];
        if layer.locked
            || self.is_reference_layer(self.selected_layer)
            || layer.kind == LayerKind::Folder
        {
            return;
        }
        let w = self.doc.width;
        let h = self.doc.height;
        let sample = if self.fill_reference_mode == 3 {
            self.doc
                .layers
                .iter()
                .find(|layer| layer.id == self.fill_reference_layer)
                .map(|layer| layer.pixels.to_dense())
                .unwrap_or_else(|| self.doc.layers[self.selected_layer].pixels.to_dense())
        } else if self.fill_reference_mode != 0 {
            let mut ref_doc = self.doc.clone();
            if self.fill_reference_mode == 1 {
                for (index, layer) in ref_doc.layers.iter_mut().enumerate() {
                    if layer.kind == LayerKind::Raster && !self.is_reference_layer(index) {
                        layer.visible = false;
                    }
                }
            }
            if self.fill_reference_mode == 1
                && !ref_doc.layers.iter().enumerate().any(|(index, layer)| {
                    layer.kind == LayerKind::Raster && self.is_reference_layer(index)
                })
            {
                self.doc.layers[self.selected_layer].pixels.to_dense()
            } else {
                efude_canvas::composite(&ref_doc)
            }
        } else {
            self.doc.layers[self.selected_layer].pixels.to_dense()
        };
        let si = ((y * w + x) * 4) as usize;
        let seed = &sample[si..si + 4];
        let seed = [seed[0], seed[1], seed[2], seed[3]];
        let tolerance = self.fill_tolerance as i32;
        let matches_seed = |i: usize| -> bool {
            let p = &sample[i..i + 4];
            let d = (p[0] as i32 - seed[0] as i32).pow(2)
                + (p[1] as i32 - seed[1] as i32).pow(2)
                + (p[2] as i32 - seed[2] as i32).pow(2);
            d <= tolerance * tolerance * 3 && (p[3] as i32 - seed[3] as i32).abs() <= tolerance
        };
        let replacement = if self.transparent_color {
            [0; 4]
        } else {
            [
                self.color.r(),
                self.color.g(),
                self.color.b(),
                self.color.a(),
            ]
        };
        if self.fill_reference_mode == 0 && seed == replacement {
            return;
        }
        self.history.begin();
        let len = (w * h) as usize;
        let mut inside = vec![false; len];
        for (i, inside_pixel) in inside.iter_mut().enumerate() {
            *inside_pixel = matches_seed(i * 4);
        }
        // Close short boundary gaps with a square morphological closing. This
        // bridges narrow breaks before flood fill instead of walking through
        // differently colored pixels and painting the boundary itself.
        let radius = self.fill_gap_close as usize;
        if radius > 0 {
            let stride = w as usize + 1;
            let mut integral = vec![0u32; stride * (h as usize + 1)];
            for y in 0..h as usize {
                let mut row_sum = 0u32;
                for x in 0..w as usize {
                    row_sum += u32::from(inside[y * w as usize + x]);
                    integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row_sum;
                }
            }
            let mut dilated = vec![false; len];
            for y in 0..h as usize {
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius + 1).min(h as usize);
                for x in 0..w as usize {
                    let x0 = x.saturating_sub(radius);
                    let x1 = (x + radius + 1).min(w as usize);
                    let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                        - integral[y0 * stride + x1]
                        - integral[y1 * stride + x0];
                    dilated[y * w as usize + x] = sum != 0;
                }
            }
            integral.fill(0);
            for y in 0..h as usize {
                let mut row_sum = 0u32;
                for x in 0..w as usize {
                    row_sum += u32::from(dilated[y * w as usize + x]);
                    integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row_sum;
                }
            }
            let mut closed = vec![true; len];
            for y in 0..h as usize {
                let y0 = y.saturating_sub(radius);
                let y1 = (y + radius + 1).min(h as usize);
                for x in 0..w as usize {
                    let x0 = x.saturating_sub(radius);
                    let x1 = (x + radius + 1).min(w as usize);
                    let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                        - integral[y0 * stride + x1]
                        - integral[y1 * stride + x0];
                    closed[y * w as usize + x] = sum == ((x1 - x0) * (y1 - y0)) as u32;
                }
            }
            inside = closed;
        }
        let seed_index = (y * w + x) as usize;
        if !inside[seed_index] {
            self.history.commit();
            return;
        }
        let mut queue = std::collections::VecDeque::from([seed_index]);
        let mut visited = vec![false; len];
        while let Some(pi) = queue.pop_front() {
            if visited[pi] || !inside[pi] {
                continue;
            }
            visited[pi] = true;
            if self.selection.active && self.selection.mask.get(pi).copied().unwrap_or(0) == 0 {
                continue;
            }
            let i = pi * 4;
            for c in 0..4 {
                self.history
                    .record_pixel(&self.doc.layers[self.selected_layer], i + c);
            }
            self.doc.layers[self.selected_layer].pixels[i..i + 4].copy_from_slice(&replacement);
            let cx = pi % w as usize;
            let cy = pi / w as usize;
            if cx > 0 {
                queue.push_back(pi - 1);
            }
            if cx + 1 < w as usize {
                queue.push_back(pi + 1);
            }
            if cy > 0 {
                queue.push_back(pi - w as usize);
            }
            if cy + 1 < h as usize {
                queue.push_back(pi + w as usize);
            }
        }
        self.history.commit();
    }
    fn merge_visible_layers(&mut self) {
        let merged_pixels = efude_canvas::composite_transparent(&self.doc);
        if !merged_pixels.chunks_exact(4).any(|px| px[3] != 0) {
            self.status = self
                .text(
                    "統合する表示レイヤーがありません",
                    "No visible layers to merge",
                )
                .into();
            return;
        }
        self.history.begin();
        for i in 0..self.doc.layers.len() {
            if self.doc.layers[i].kind != LayerKind::Raster || !self.doc.layers[i].visible {
                continue;
            }
            let mut parent = self.doc.layers[i].parent_id;
            let mut visible = true;
            let mut depth = 0;
            while let Some(id) = parent {
                if depth >= self.doc.layers.len() {
                    break;
                }
                depth += 1;
                if let Some(group) = self.doc.layers.iter().find(|layer| layer.id == id) {
                    visible &= group.visible;
                    parent = group.parent_id;
                } else {
                    break;
                }
            }
            if visible {
                let id = self.doc.layers[i].id;
                let before = self.doc.layers[i].property_state();
                self.doc.layers[i].visible = false;
                let after = self.doc.layers[i].property_state();
                self.history.record_layer_properties(id, before, after);
            }
        }
        let id = self
            .doc
            .layers
            .iter()
            .map(|layer| layer.id)
            .max()
            .unwrap_or(0)
            + 1;
        let mut merged =
            efude_canvas::Layer::new(id, "統合レイヤー", self.doc.width, self.doc.height);
        merged.pixels =
            efude_canvas::TilePixels::from_dense(self.doc.width, self.doc.height, &merged_pixels);
        let index = self.doc.layers.len();
        self.history
            .insert_layer(&mut self.doc.layers, index, merged);
        self.history.commit();
        self.selected_layer = index;
        self.editing_mask = false;
        self.status = self
            .text(
                "表示レイヤーを統合しました（Undoで復元できます）",
                "Visible layers merged (Undo to restore)",
            )
            .into();
    }
    fn queue_filter(&mut self, operation: FilterOperation, ctx: &egui::Context) {
        if self.filter_pending {
            return;
        }
        if self.refuse_on_vector_layer() {
            return;
        }
        let Some(layer) = self.doc.layers.get(self.selected_layer) else {
            return;
        };
        if layer.locked
            || self.is_reference_layer(self.selected_layer)
            || layer.kind != LayerKind::Raster
        {
            return;
        }
        let selection = self.selection.active.then(|| self.selection.mask.clone());
        let before = self.selection.active.then(|| layer.pixels.clone());
        let task = IoTask::ApplyFilter {
            layer: Box::new(layer.clone()),
            before,
            selection,
            width: self.doc.width,
            height: self.doc.height,
            operation,
            document_id: self.history.document_id(),
            state_token: self.history.state_token(),
            repaint: ctx.clone(),
        };
        if self.io_task_sender.send(task).is_ok() {
            self.filter_pending = true;
            self.status = self.text("フィルターを処理中…", "Applying filter…").into();
        }
    }
    pub(crate) fn apply_auto_levels(&mut self, ctx: &egui::Context) {
        self.queue_filter(FilterOperation::AutoLevels, ctx);
    }
}
impl Drop for EfudeApp {
    fn drop(&mut self) {
        if let (Some(surface), Some(render_state)) = (
            self.gpu_canvas_surface.take(),
            self.gpu_render_state.as_ref(),
        ) {
            render_state
                .renderer
                .write()
                .free_texture(&surface.texture_id);
        }
        let _ = self.io_task_sender.send(IoTask::Shutdown);
        if let Some(worker) = self.io_worker.take() {
            let _ = worker.join();
        }
    }
}

impl EfudeApp {
    /// One frame of the whole UI (everything `eframe::App::update` does;
    /// tests drive it directly).
    pub(crate) fn update_ui(&mut self, ctx: &egui::Context) {
        self.sync_tool_change();
        self.tidy_layers();
        let other_dirty_tabs = self
            .dirty_tabs()
            .into_iter()
            .filter(|&index| index != self.tabs.active)
            .collect::<Vec<_>>();
        if ctx.input(|input| input.viewport().close_requested())
            && !other_dirty_tabs.is_empty()
            && !self.close_without_saving
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let english = self.language_english;
            let count = other_dirty_tabs.len() + usize::from(self.history.is_dirty());
            let choice = rfd::MessageDialog::new()
                .set_title(if english { "Unsaved Changes" } else { "未保存の変更" })
                .set_description(if english {
                    format!("{count} canvases have unsaved changes. Close without saving them? Cancel shows the first one.")
                } else {
                    format!("保存していないキャンバスが {count} 個あります。保存せずに終了しますか？ キャンセルすると最初のキャンバスを表示します。")
                })
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show();
            if choice == rfd::MessageDialogResult::Ok {
                self.close_without_saving = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.switch_tab(other_dirty_tabs[0]);
            }
        } else if ctx.input(|input| input.viewport().close_requested())
            && self.history.is_dirty()
            && !self.close_without_saving
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            let english = self.language_english;
            let choice = rfd::MessageDialog::new()
                .set_title(if english { "Unsaved Changes" } else { "未保存の変更" })
                .set_description(if english {
                    "Save changes before closing? Yes: save and close, No: close without saving, Cancel: keep editing."
                } else {
                    "閉じる前に変更を保存しますか？ はい: 保存して閉じる、いいえ: 保存せず閉じる、キャンセル: 編集に戻る"
                })
                .set_buttons(rfd::MessageButtons::YesNoCancel)
                .show();
            match choice {
                rfd::MessageDialogResult::Yes => match self.request_native_save(ctx) {
                    Ok(true) => {
                        self.close_after_save = true;
                        self.status = self
                            .text("保存後に閉じます…", "Saving before closing…")
                            .into();
                    }
                    Ok(false) => {}
                    Err(error) => self.status = error,
                },
                rfd::MessageDialogResult::No => {
                    self.close_without_saving = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                rfd::MessageDialogResult::Cancel | rfd::MessageDialogResult::Custom(_) => {}
                rfd::MessageDialogResult::Ok => {}
            }
        }
        if !self.window_checked {
            self.window_checked = true;
            // A window restored smaller than usable (from an earlier UI zoom
            // or a minimised session) opens at the default size instead.
            if let Some(inner) = ctx.input(|input| input.viewport().inner_rect)
                && (inner.width() < 900.0 || inner.height() < 600.0)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(1280.0, 820.0)));
            }
        }
        // Ctrl +/- zoom the canvas; egui must never take them for the UI.
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        if !ctx.wants_keyboard_input() {
            #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
            let (mut zoom_in, mut zoom_out, fit) = ctx.input_mut(canvas_zoom_keys);
            // Keyboards differ in which key gives "+" (on Japanese ones it
            // is Shift+;), so on Windows the keys are also read directly.
            #[cfg(target_os = "windows")]
            {
                let focused = ctx.input(|input| input.focused);
                let down = |codes: &[u8]| {
                    focused && codes.iter().any(|&code| efude_input::ctrl_key_down(code))
                };
                // VK_OEM_PLUS ("=+", or ";+" on Japanese keyboards) and
                // VK_ADD; VK_OEM_MINUS and VK_SUBTRACT.
                let keys = (down(&[0xBB, 0x6B]), down(&[0xBD, 0x6D]));
                zoom_in |= keys.0 && !self.zoom_keys_down.0;
                zoom_out |= keys.1 && !self.zoom_keys_down.1;
                self.zoom_keys_down = keys;
            }
            if zoom_in {
                self.zoom = (self.zoom * 1.25).clamp(0.01, 64.0);
            }
            if zoom_out {
                self.zoom = (self.zoom / 1.25).clamp(0.01, 64.0);
            }
            if fit {
                self.zoom = 1.0;
                self.navigator_center =
                    Vec2::new(self.doc.width as f32 / 2.0, self.doc.height as f32 / 2.0);
            }
        }
        let history_state_at_frame_start = self.history.state_token();
        let view_transform_at_frame_start = (self.view_rotation, self.flip_x, self.flip_y);
        if self.paste_preview.is_some() {
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.paste_preview = None;
                self.paste_texture = None;
                self.selection_start = None;
                self.gesture_end = None;
                self.active.clear();
            } else if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
                self.commit_paste_preview();
                self.canvas_texture_dirty = true;
            }
        }
        while let Ok(elapsed_ms) = self.latency_receiver.try_recv() {
            self.record_canvas_latency(elapsed_ms);
        }
        if let Some(started) = self.pending_canvas_latency.take() {
            if let Some(render_state) = &self.gpu_render_state {
                let sender = self.latency_sender.clone();
                let repaint = ctx.clone();
                render_state.queue.on_submitted_work_done(move || {
                    let _ = sender.send(started.elapsed().as_secs_f32() * 1000.0);
                    repaint.request_repaint();
                });
            } else {
                self.record_canvas_latency(started.elapsed().as_secs_f32() * 1000.0);
            }
        }
        while let Ok(completion) = self.io_receiver.try_recv() {
            // Every task sends one completion.
            self.io_task_sender
                .busy
                .set(self.io_task_sender.busy.get().saturating_sub(1));
            match completion {
                IoCompletion::Saved {
                    path, backup: true, ..
                } => {
                    self.status = if self.language_english {
                        format!("Automatic backup saved: {}", path.display())
                    } else {
                        format!("自動バックアップしました: {}", path.display())
                    };
                }
                IoCompletion::Saved {
                    path,
                    backup: false,
                    state_token,
                    document_id,
                } => {
                    let same_document = self.history.document_id() == document_id;
                    if same_document {
                        self.history.mark_saved(state_token);
                        self.doc_path = Some(path.clone());
                    } else {
                        self.mark_tab_saved(document_id, state_token, &path);
                    }
                    let mut close_was_blocked_by_new_changes = false;
                    if self.close_after_save {
                        self.close_after_save = false;
                        if same_document && self.history.state_token() == state_token {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            close_was_blocked_by_new_changes = true;
                        }
                    }
                    self.status = if self.language_english {
                        format!("Saved: {}", path.display())
                    } else {
                        format!("保存しました: {}", path.display())
                    };
                    if close_was_blocked_by_new_changes {
                        self.status = self
                            .text(
                                "保存中に変更されたため、文書は開いたままです",
                                "The document changed during saving and remains open",
                            )
                            .into();
                    }
                }
                IoCompletion::Exported {
                    path,
                    format,
                    warnings,
                } => {
                    let warnings = warnings
                        .iter()
                        .map(|warning| localize_io_warning(warning, self.language_english))
                        .collect::<Vec<_>>();
                    self.status = if warnings.is_empty() {
                        if self.language_english {
                            format!("Exported {format}: {}", path.display())
                        } else {
                            format!("{format}を書き出しました: {}", path.display())
                        }
                    } else {
                        if self.language_english {
                            format!(
                                "Exported {format}: {} (warnings: {})",
                                path.display(),
                                warnings.join(" / ")
                            )
                        } else {
                            format!(
                                "{format}を書き出しました: {}（警告: {}）",
                                path.display(),
                                warnings.join(" / ")
                            )
                        }
                    };
                }
                IoCompletion::Loaded { path, document } => {
                    self.install_document(document, path);
                    self.canvas_texture_dirty = true;
                    self.navigator_texture_dirty = true;
                    self.status = self.text("読み込みました", "Loaded").into();
                }
                IoCompletion::PsdLoaded { path, imported } => {
                    let warning = if self.language_english {
                        imported.warnings_en.join(" / ")
                    } else {
                        imported.warnings.join(" / ")
                    };
                    self.install_document(imported.document, path);
                    self.canvas_texture_dirty = true;
                    self.navigator_texture_dirty = true;
                    self.status = if warning.is_empty() {
                        self.text(
                            "PSDを読み込みました（レイヤーを保持）",
                            "PSD loaded with layers preserved",
                        )
                        .into()
                    } else {
                        if self.language_english {
                            format!("PSD loaded: {warning}")
                        } else {
                            format!("PSDを読み込みました: {warning}")
                        }
                    };
                }
                IoCompletion::ImageDocumentLoaded {
                    path,
                    width,
                    height,
                    rgba,
                } => {
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("image");
                    let mut document = Document::new(width, height);
                    document.layers[0].name = name.to_string();
                    document.layers[0].pixels =
                        efude_canvas::TilePixels::from_dense(width, height, &rgba);
                    self.install_document(document, std::path::PathBuf::new());
                    self.doc_path = None;
                    self.editing_mask = false;
                    self.canvas_texture_dirty = true;
                    self.navigator_texture_dirty = true;
                    self.status = if self.language_english {
                        format!("Opened image as a new document: {name}")
                    } else {
                        format!("画像を新規ドキュメントとして開きました: {name}")
                    };
                }
                IoCompletion::ImageLoaded {
                    path,
                    width,
                    height,
                    rgba,
                } => {
                    self.install_image_layer(path, width, height, rgba);
                    self.canvas_texture_dirty = true;
                    self.navigator_texture_dirty = true;
                }
                IoCompletion::ReferenceLoaded {
                    path,
                    width,
                    height,
                    rgba,
                } => {
                    self.reference_image_path = Some(path.clone());
                    self.reference_size = Vec2::new(width as f32, height as f32);
                    self.reference_position = None;
                    self.reference_image = Some(ctx.load_texture(
                        "reference-image",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [width as usize, height as usize],
                            &rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    ));
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image");
                    self.status = if self.language_english {
                        format!("Reference image: {name}")
                    } else {
                        format!("参照画像: {name}")
                    };
                }
                IoCompletion::FilterApplied {
                    layer_id,
                    document_id,
                    state_token,
                    pixels,
                } => {
                    self.filter_pending = false;
                    if self.history.document_id() == document_id
                        && self.history.state_token() == state_token
                        && self.active.is_empty()
                        && let Some(index) = self
                            .doc
                            .layers
                            .iter()
                            .position(|layer| layer.id == layer_id)
                        && !self.doc.layers[index].locked
                        && !self.is_reference_layer(index)
                        && self.doc.layers[index].kind == LayerKind::Raster
                    {
                        self.history.begin();
                        self.history.record_all_layer_tiles(
                            &self.doc.layers[index],
                            self.doc.width,
                            self.doc.height,
                        );
                        self.doc.layers[index].pixels = pixels;
                        self.history.commit();
                        self.canvas_texture_dirty = true;
                        self.navigator_texture_dirty = true;
                        self.status = self
                            .text("フィルターを適用しました", "Filter applied")
                            .into();
                    }
                }
                IoCompletion::Failed(error) => {
                    self.close_after_save = false;
                    self.status = if self.language_english {
                        format!("I/O failed: {error}")
                    } else {
                        format!("I/Oに失敗しました: {error}")
                    }
                }
            }
        }
        // `drain_into` reports whether packets are still waiting.
        let more_packets = self.pen_queue.drain_into(&mut self.frame_pen_packets, 8192);
        if !self.use_windows_ink && !self.use_wintab {
            self.frame_pen_packets.clear();
        }
        // Keep polling the tablet queue only while the pen is in use; an
        // idle window does not redraw.
        if more_packets
            || !self.frame_pen_packets.is_empty()
            || ctx.input(|input| input.pointer.any_down())
        {
            ctx.request_repaint();
        }
        let shortcuts_enabled = !ctx.wants_keyboard_input();
        if shortcuts_enabled {
            // [ and ] change the brush size, as in most painting software.
            let (smaller, larger) = ctx.input(|input| {
                (
                    input.key_pressed(egui::Key::OpenBracket),
                    input.key_pressed(egui::Key::CloseBracket),
                )
            });
            if smaller || larger {
                let factor = if larger { 1.15 } else { 1.0 / 1.15 };
                self.size = (self.size * factor).clamp(1.0, MAX_BRUSH_SIZE);
                self.brushes[self.selected_brush].size = self.size;
            }
        }
        let key_undo = Self::configured_key(&self.shortcuts.undo, egui::Key::Z);
        let key_redo = Self::configured_key(&self.shortcuts.redo, egui::Key::Y);
        let key_save = Self::configured_key(&self.shortcuts.save, egui::Key::S);
        let key_copy = Self::configured_key(&self.shortcuts.copy, egui::Key::C);
        let key_cut = Self::configured_key(&self.shortcuts.cut, egui::Key::X);
        let key_paste = Self::configured_key(&self.shortcuts.paste, egui::Key::V);
        let key_select_all = Self::configured_key(&self.shortcuts.select_all, egui::Key::A);
        let key_deselect = Self::configured_key(&self.shortcuts.deselect, egui::Key::D);
        let key_eraser = Self::configured_key(&self.shortcuts.eraser, egui::Key::E);
        let tool_shortcuts = [
            (&self.shortcuts.brush_tool, egui::Key::Num1, Tool::Brush),
            (
                &self.shortcuts.rectangle_tool,
                egui::Key::Num2,
                Tool::RectangleSelect,
            ),
            (&self.shortcuts.fill_tool, egui::Key::Num3, Tool::Fill),
            (
                &self.shortcuts.eyedropper_tool,
                egui::Key::Num4,
                Tool::Eyedropper,
            ),
            (&self.shortcuts.move_tool, egui::Key::Num5, Tool::Move),
            (&self.shortcuts.pan_tool, egui::Key::Num6, Tool::Pan),
            (&self.shortcuts.blur_tool, egui::Key::Num7, Tool::Blur),
            (&self.shortcuts.smudge_tool, egui::Key::Num8, Tool::Smudge),
            (
                &self.shortcuts.ellipse_tool,
                egui::Key::Num9,
                Tool::EllipseSelect,
            ),
            (&self.shortcuts.lasso_tool, egui::Key::Q, Tool::LassoSelect),
            (
                &self.shortcuts.polygon_tool,
                egui::Key::W,
                Tool::PolygonSelect,
            ),
            (
                &self.shortcuts.magic_wand_tool,
                egui::Key::R,
                Tool::MagicWand,
            ),
            (
                &self.shortcuts.color_range_tool,
                egui::Key::T,
                Tool::ColorRange,
            ),
            (&self.shortcuts.line_ruler_tool, egui::Key::Y, Tool::Line),
            (
                &self.shortcuts.ellipse_ruler_tool,
                egui::Key::U,
                Tool::EllipseRuler,
            ),
            (
                &self.shortcuts.bezier_ruler_tool,
                egui::Key::I,
                Tool::BezierRuler,
            ),
            (
                &self.shortcuts.perspective_ruler_tool,
                egui::Key::O,
                Tool::PerspectiveRuler,
            ),
            (
                &self.shortcuts.selection_brush_tool,
                egui::Key::G,
                Tool::SelectionBrush,
            ),
            (
                &self.shortcuts.quick_mask_tool,
                egui::Key::H,
                Tool::QuickMask,
            ),
        ];
        let conflicting_tool_keys = self.conflicting_tool_keys();
        // Tool keys: a tap switches tools; holding a key (or using the tool
        // while it is held) switches only until the key is released. Space
        // always works as a held hand tool.
        let mut tool_keys: Vec<(egui::Key, Tool, bool)> = tool_shortcuts
            .into_iter()
            .map(|(shortcut, fallback, tool)| {
                (Self::configured_key(shortcut, fallback), tool, false)
            })
            .collect();
        tool_keys.push((key_eraser, Tool::Eraser, false));
        tool_keys.retain(|(key, _, _)| !conflicting_tool_keys.contains(key));
        tool_keys.push((egui::Key::Space, Tool::Pan, true));
        self.update_held_tool_key(
            ctx,
            shortcuts_enabled && self.paste_preview.is_none(),
            &tool_keys,
        );
        if shortcuts_enabled && self.paste_preview.is_none() {
            let (delete, escape) = ctx.input_mut(|input| {
                (
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                        || input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if delete && self.tool == Tool::VectorEdit {
                self.delete_vector_selection();
            } else if delete {
                self.delete_selected_pixels();
            }
            if escape {
                // Cancel half-finished point input.
                self.selection_points.clear();
                self.bezier_points.clear();
                self.setting_vanishing_point = false;
            }
        }
        if self.last_backup.elapsed() >= efude_io::backup_interval(self.backup_interval_minutes) {
            if self.history.is_dirty()
                && let Some(path) = self.doc_path.clone()
            {
                self.status = match self.queue_document_save(path, true, ctx) {
                    Ok(()) => "自動バックアップ中…".into(),
                    Err(error) => error,
                };
            }
            self.last_backup = std::time::Instant::now();
        }
        if shortcuts_enabled {
            self.command_shortcuts(
                ctx,
                [
                    key_undo,
                    key_redo,
                    key_save,
                    key_copy,
                    key_cut,
                    key_paste,
                    key_select_all,
                    key_deselect,
                ],
            );
        }
        if self.tool == Tool::PolygonSelect
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            self.selection
                .polygon(self.doc.width, self.doc.height, &self.selection_points);
            self.apply_selection_symmetry();
            self.finish_selection_operation();
            self.selection_points.clear();
            self.status = self
                .text("多角形選択を確定しました", "Polygon selection completed")
                .into();
        }
        self.layout_ui(ctx);
        let english = self.language_english;
        egui::Window::new(if english { "Efude Help" } else { "Efude ヘルプ" })
            .open(&mut self.show_help)
            .default_width(460.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading(if english { "Start Drawing" } else { "描画を始める" });
                    ui.label(if english { "Choose a tool from the left panel, then drag on the canvas. Change color and size in the tool panel." } else { "左のブラシ一覧からツールを選び、キャンバス上をドラッグして描きます。色とサイズはツールパネルで変更できます。" });
                    ui.label(if english { "Stabilization and pull-string modes show the input and corrected positions with cyan guides." } else { "手ブレ補正や紐引きを使うと、シアンのガイドで入力位置と補正位置のずれを確認できます。" });
                    ui.label(if english { "Use the right layer panel to add layers, toggle visibility, adjust opacity, lock layers, and edit masks." } else { "右のレイヤーパネルからレイヤーの追加、表示切替、不透明度、ロック、マスクを操作します。" });
                    ui.label(if english { "Selections limit painting, erasing, and filter operations to the selected area." } else { "選択ツールで範囲を作ると、その範囲に描画や消去、フィルターが適用されます。" });
                    ui.separator();
                    ui.heading(if english { "Saving and Exporting" } else { "保存と書き出し" });
                    ui.label(if english { "Efude (.efude) preserves editable layers and masks. PSD is for exchanging work with other apps; unsupported settings produce export warnings." } else { "Efude形式(.efude)はレイヤーやマスクを含む編集用の保存形式です。PSDは他のアプリとの受け渡しに使います。PSDで保持できない設定は書き出し時に警告します。" });
                    ui.label(if english { "PNG preserves transparency. JPEG exports with a white background." } else { "PNGは透明部分を保持し、JPEGは白背景で書き出します。" });
                    ui.separator();
                    ui.heading(if english { "Shortcuts" } else { "ショートカット" });
                    ui.label(if english { format!("Undo: Ctrl+{}    Redo: Ctrl+{}", self.shortcuts.undo, self.shortcuts.redo) } else { format!("元に戻す: Ctrl+{}　やり直す: Ctrl+{}", self.shortcuts.undo, self.shortcuts.redo) });
                    ui.label(if english { format!("Save: Ctrl+{}    Copy: Ctrl+{}    Cut: Ctrl+{}    Paste: Ctrl+{}", self.shortcuts.save, self.shortcuts.copy, self.shortcuts.cut, self.shortcuts.paste) } else { format!("保存: Ctrl+{}　コピー: Ctrl+{}　切り取り: Ctrl+{}　貼り付け: Ctrl+{}", self.shortcuts.save, self.shortcuts.copy, self.shortcuts.cut, self.shortcuts.paste) });
                    ui.label(if english { format!("Select all: Ctrl+{}    Deselect: Ctrl+{}    Invert selection: Ctrl+Shift+I", self.shortcuts.select_all, self.shortcuts.deselect) } else { format!("すべてを選択: Ctrl+{}　選択解除: Ctrl+{}　選択範囲を反転: Ctrl+Shift+I", self.shortcuts.select_all, self.shortcuts.deselect) });
                    ui.label(if english { "New: Ctrl+N    Open: Ctrl+O    Save as: Ctrl+Shift+S    Close tab: Ctrl+W    Next tab: Ctrl+Tab" } else { "新規: Ctrl+N　開く: Ctrl+O　別名で保存: Ctrl+Shift+S　タブを閉じる: Ctrl+W　次のタブ: Ctrl+Tab" });
                    ui.label(if english { "New layer: Ctrl+Shift+N    Duplicate layer: Ctrl+J    Redo also: Ctrl+Shift+Z    Brush size: [ ]" } else { "新規レイヤー: Ctrl+Shift+N　レイヤーを複製: Ctrl+J　やり直しは Ctrl+Shift+Z でも　ブラシサイズ: [ ]" });
                    ui.label(if english { "Edit shortcuts under File → Preferences → Shortcut Settings." } else { "ショートカットは「ファイル → 環境設定 → ショートカット設定」で変更できます。" });
                    ui.separator();
                    ui.label(if english { "For layered work, use Save As before overwriting or enable periodic backups." } else { "レイヤー付きの作業データは、上書き前に別名保存するか定期バックアップを有効にしてください。" });
                    ui.label(if english { "See the project README and docs/spec for more details." } else { "詳細はプロジェクトのREADMEとdocs/specを参照してください。" });
                });
            });
        #[cfg(target_os = "windows")]
        self.sync_windows_ink_hook();
        self.absorb_history_tile_changes();
        if self.history.state_token() != history_state_at_frame_start {
            self.navigator_texture_dirty = true;
            if self.dirty_canvas_tiles.is_empty() {
                self.canvas_texture_dirty = true;
            }
            ctx.request_repaint();
        }
        if (self.view_rotation, self.flip_x, self.flip_y) != view_transform_at_frame_start {
            // The canvas is transformed when drawn; only a repaint is needed.
            ctx.request_repaint();
        }
    }
}

impl eframe::App for EfudeApp {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.update_ui(ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(
            storage,
            "settings",
            &PersistedSettings {
                language_english: self.language_english,
                brushes: self.brushes.clone(),
                selected_brush: self.selected_brush,
                tool_brushes: self.tool_brushes,
                preset_version: PRESET_VERSION,
                color: [
                    self.color.r(),
                    self.color.g(),
                    self.color.b(),
                    self.color.a(),
                ],
                palette: self.palette.clone(),
                show_tools_panel: self.show_tools_panel,
                show_layers_panel: self.show_layers_panel,
                workspace: Some(self.workspace.clone()),
                layout_version: layout::LAYOUT_VERSION,
                tools_panel_width: self.tools_panel_width,
                layers_panel_width: self.layers_panel_width,
                secondary_color: [
                    self.secondary_color.r(),
                    self.secondary_color.g(),
                    self.secondary_color.b(),
                    self.secondary_color.a(),
                ],
                intermediate_mix: self.intermediate_mix,
                size: self.size,
                zoom: self.zoom,
                view_rotation: self.view_rotation,
                flip_x: self.flip_x,
                flip_y: self.flip_y,
                show_grid: self.show_grid,
                transparency_checker: self.transparency_checker,
                grid_size: self.grid_size,
                grid_snap: self.grid_snap,
                symmetry_x: self.symmetry_x,
                symmetry_y: self.symmetry_y,
                symmetry_count: self.symmetry_count,
                symmetry_center: [self.symmetry_center.x, self.symmetry_center.y],
                perspective_points: self
                    .perspective_points
                    .iter()
                    .map(|&(x, y)| [x, y])
                    .collect(),
                perspective_selected: self.perspective_selected,
                shortcuts: self.shortcuts.clone(),
                tone_curve: self.tone_curve,
                pressure_curve_points: self.pressure_curve_points,
                use_windows_ink: self.use_windows_ink,
                use_wintab: self.use_wintab,
                eyedropper_radius: self.eyedropper_radius,
                eyedropper_composite: self.eyedropper_composite,
                backup_interval_minutes: self.backup_interval_minutes,
                backup_generations: self.backup_generations,
                eraser_pressure_updated: true,
                selection_reference_mode: self.selection_reference_mode,
                selection_reference_layer: self.selection_reference_layer,
                fill_reference_layer: self.fill_reference_layer,
                fill_reference_mode: self.fill_reference_mode,
                fill_tolerance: self.fill_tolerance,
                fill_gap_close: self.fill_gap_close,
                selection_tolerance: self.selection_tolerance,
                reference_zoom: self.reference_zoom,
                reference_opacity: self.reference_opacity,
                reference_position: self
                    .reference_position
                    .map(|position| [position.x, position.y]),
                reference_image_path: self.reference_image_path.clone(),
            },
        );
    }
}

/// Bumped when the default preset set grows so saved sets get the new
/// presets once.
const PRESET_VERSION: u32 = 2;

/// The preset set a new installation starts with.
const DEFAULT_PRESETS: &[u8] = include_bytes!("../../../assets/brushes/default.efudebrushes");

/// Presets of a new installation: `assets/brushes/default.efudebrushes`
/// (a brush set exported from the app), or three columns of the built-in
/// brushes should it not load.
/// (Tests use the built-in brushes, so they do not change when the
/// shipped presets are retuned.)
pub(crate) fn default_presets() -> Vec<efude_brush::Brush> {
    if cfg!(test) {
        return builtin_presets();
    }
    efude_brush::set_from_bytes(DEFAULT_PRESETS).unwrap_or_else(|_| builtin_presets())
}

/// Three columns of the built-in brushes.
fn builtin_presets() -> Vec<efude_brush::Brush> {
    let one = efude_brush::defaults();
    one.iter().chain(&one).chain(&one).cloned().collect()
}

/// The default preset with the same name (or, failing that, the same kind).
fn default_brush_like(brush: &efude_brush::Brush) -> Option<efude_brush::Brush> {
    let defaults = default_presets();
    defaults
        .iter()
        .find(|candidate| candidate.name == brush.name)
        .or_else(|| {
            defaults.iter().find(|candidate| {
                std::mem::discriminant(&candidate.kind) == std::mem::discriminant(&brush.kind)
            })
        })
        .cloned()
}

/// Brings brushes saved by an older version up to date: the old watercolor
/// preset (never edited) becomes the new one, and built-in presets added
/// since are appended.
fn migrate_saved_brushes(mut brushes: Vec<efude_brush::Brush>) -> Vec<efude_brush::Brush> {
    let defaults = efude_brush::defaults();
    for brush in &mut brushes {
        let old_watercolor = brush.name == "水彩"
            && matches!(brush.kind, BrushKind::Watercolor)
            && (brush.mix.blend - 0.18).abs() < 1e-4
            && (brush.mix.dilution - 0.28).abs() < 1e-4
            && (brush.mix.persistence - 0.12).abs() < 1e-4
            && brush.wet_edge == 0.0;
        if old_watercolor && let Some(new) = defaults.iter().find(|d| d.name == "水彩") {
            let size = brush.size;
            let color = brush.color;
            *brush = new.clone();
            brush.size = size;
            brush.color = color;
        }
    }
    for default in defaults {
        if !brushes.iter().any(|brush| brush.name == default.name) {
            brushes.push(default);
        }
    }
    brushes
}

mod balloons;
mod book;
mod canvas_view;
mod comic;
mod display;
mod filters_ui;
mod icons;
mod layout;
mod panels;
mod tabs;
mod vector_edit;
mod vector_tools;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod tool_tests;

#[cfg(test)]
mod stroke_tests {
    use super::*;

    fn app(width: u32, height: u32) -> EfudeApp {
        let mut app = EfudeApp::default();
        app.doc = Document::new(width, height);
        app.selected_layer = 0;
        app.tool = Tool::Brush;
        app.color = Color32::from_rgb(0, 0, 0);
        app
    }

    /// Draws `points` the way a pen drag does: live samples, then pen-up.
    fn draw(app: &mut EfudeApp, points: &[InkPoint]) {
        let ctx = egui::Context::default();
        app.reset_stroke_buffers();
        app.active.clear();
        app.begin_brush_stroke();
        app.history.begin();
        app.raster.previous_mix_color = [
            app.color.r() as f32 / 255.,
            app.color.g() as f32 / 255.,
            app.color.b() as f32 / 255.,
        ];
        app.raster.last_dab = None;
        app.raster.remaining_charge = app.brushes[app.selected_brush].mix.charge;
        for point in points {
            app.push_live_sample(*point, None, &ctx);
            app.flush_provisional();
        }
        app.finish_stroke();
    }

    fn line(from: (f32, f32), to: (f32, f32), steps: usize) -> Vec<InkPoint> {
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                InkPoint::new(
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                    1.0,
                    i as u64 * 4,
                )
            })
            .collect()
    }

    fn column_ink(app: &EfudeApp, x: u32) -> u32 {
        (0..app.doc.height)
            .map(|y| app.doc.layers[0].pixels.pixel(x, y)[3] as u32)
            .sum()
    }

    fn use_brush(app: &mut EfudeApp, index: usize, size: f32) {
        app.selected_brush = index;
        app.size = size;
        app.brushes[index].stabilization = 0;
    }

    #[test]
    fn thin_line_has_even_ink_along_its_length() {
        for size in [1.0, 1.5, 2.0] {
            let mut app = app(120, 40);
            use_brush(&mut app, 0, size);
            // A slightly slanted line exposes beading and stair-stepping.
            draw(&mut app, &line((10.3, 18.2), (110.7, 23.9), 60));
            let ink: Vec<u32> = (20..100).map(|x| column_ink(&app, x)).collect();
            let min = *ink.iter().min().unwrap() as f32;
            let max = *ink.iter().max().unwrap() as f32;
            assert!(min > 0.0, "size {size}: gap in the line");
            assert!(
                min / max > 0.85,
                "size {size}: uneven thin line (min {min}, max {max})"
            );
        }
    }

    #[test]
    fn light_pressure_gives_a_lighter_line() {
        // Overlapping dabs of one stroke must not pile up to full opacity.
        let mut app = app(200, 40);
        use_brush(&mut app, 0, 8.0);
        let mut points = line((10.0, 20.0), (190.0, 20.0), 90);
        for point in &mut points {
            point.pressure = 0.3;
        }
        draw(&mut app, &points);
        let alpha = app.doc.layers[0].pixels.pixel(100, 20)[3];
        assert!(
            (60..=110).contains(&alpha),
            "30% pressure drew alpha {alpha}"
        );
    }

    #[test]
    fn gpu_painting_matches_cpu_painting_live() {
        let gpu = match efude_gpu::GpuCover::with_new_device() {
            Ok(gpu) => gpu,
            Err(error) if error == "no GPU adapter" => return,
            Err(error) => panic!("{error}"),
        };
        let points: Vec<InkPoint> = (0..60)
            .map(|i| {
                let t = i as f32;
                InkPoint::new(
                    20.0 + t * 3.0,
                    60.0 + (t * 0.15).sin() * 25.0,
                    0.3 + 0.7 * ((i % 20) as f32 / 19.0),
                    i as u64 * 4,
                )
            })
            .collect();
        let setup = |app: &mut EfudeApp| {
            use_brush(app, 2, 40.0);
            app.brushes[2].stabilization = 3;
            app.brushes[2].taper_in_pixels = true;
            app.brushes[2].taper_end = 30.0;
            app.color = Color32::from_rgba_unmultiplied(200, 60, 30, 220);
        };
        let mut cpu = app(240, 120);
        setup(&mut cpu);
        draw(&mut cpu, &points);
        let mut on_gpu = app(240, 120);
        setup(&mut on_gpu);
        on_gpu.gpu_cover = Some(gpu);
        on_gpu.gpu_min_pixels = 0.0;
        draw(&mut on_gpu, &points);
        let a = cpu.doc.layers[0].pixels.to_dense();
        let b = on_gpu.doc.layers[0].pixels.to_dense();
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| (*x as i32 - *y as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 1, "GPU and CPU strokes differ by {worst}");
        assert!(
            on_gpu.gpu_cover.as_ref().unwrap().batches_painted() > 10,
            "the GPU path was not used"
        );
        assert!(a.iter().any(|&v| v != 0));
    }

    #[test]
    fn long_pen_stroke_keeps_full_opacity() {
        let mut app = app(3100, 30);
        use_brush(&mut app, 0, 6.0);
        draw(&mut app, &line((20.0, 15.0), (3080.0, 15.0), 800));
        let start = app.doc.layers[0].pixels.pixel(100, 15)[3];
        let end = app.doc.layers[0].pixels.pixel(3000, 15)[3];
        assert_eq!(start, 255);
        assert_eq!(end, 255, "the stroke faded out");
    }

    #[test]
    fn live_stroke_matches_offline_replay() {
        // What is drawn live must equal a clean re-render of the same input,
        // i.e. nothing changes when the pen lifts.
        let points: Vec<InkPoint> = (0..120)
            .map(|i| {
                let t = i as f32;
                let mut p = InkPoint::new(
                    10.0 + t * 1.6,
                    40.0 + (t * 0.08).sin() * 20.0,
                    0.2 + 0.8 * ((i % 30) as f32 / 29.0),
                    i as u64 * 4,
                );
                p.tilt = glam::Vec2::ZERO;
                p
            })
            .collect();
        let mut live = app(220, 90);
        use_brush(&mut live, 2, 9.0);
        live.brushes[2].stabilization = 4;
        live.brushes[2].taper_in_pixels = true;
        live.brushes[2].taper_start = 15.0;
        live.brushes[2].taper_end = 25.0;
        draw(&mut live, &points);

        let mut offline = app(220, 90);
        use_brush(&mut offline, 2, 9.0);
        offline.brushes[2] = live.brushes[2].clone();
        offline.history.begin();
        offline.raster.previous_mix_color = [0.0; 3];
        offline.render_replay(&points);

        assert!(
            live.doc.layers[0].pixels.to_dense() == offline.doc.layers[0].pixels.to_dense(),
            "live drawing differs from the final render"
        );
    }

    #[test]
    fn wet_strokes_match_offline_replay() {
        // Wet paint reads the canvas and gets its rim at pen-up; the live
        // stroke (with provisional tails taken back every frame) must still
        // end exactly like a clean re-render.
        let points: Vec<InkPoint> = (0..100)
            .map(|i| {
                let t = i as f32;
                InkPoint::new(
                    15.0 + t * 1.8,
                    45.0 + (t * 0.1).sin() * 15.0,
                    0.3 + 0.7 * ((i % 25) as f32 / 24.0),
                    i as u64 * 4,
                )
            })
            .collect();
        for index in [3, 8] {
            let setup = |app: &mut EfudeApp| {
                use_brush(app, 0, 30.0);
                app.color = Color32::from_rgb(200, 30, 30);
                draw(app, &line((20.0, 45.0), (90.0, 45.0), 30));
                use_brush(app, index, 20.0);
                app.brushes[index].stabilization = 3;
                app.color = Color32::from_rgb(30, 60, 210);
            };
            let mut live = app(220, 90);
            setup(&mut live);
            draw(&mut live, &points);

            let mut offline = app(220, 90);
            setup(&mut offline);
            offline.history.begin();
            offline.raster.previous_mix_color = [30.0 / 255.0, 60.0 / 255.0, 210.0 / 255.0];
            offline.render_replay(&points);

            assert!(
                live.doc.layers[0].pixels.to_dense() == offline.doc.layers[0].pixels.to_dense(),
                "brush {index}: live drawing differs from the final render"
            );
        }
    }

    #[test]
    fn saved_brushes_pick_up_new_presets() {
        let mut old = efude_brush::defaults();
        old.truncate(8);
        old[3].mix.blend = 0.18;
        old[3].mix.dilution = 0.28;
        old[3].mix.persistence = 0.12;
        old[3].wet_edge = 0.0;
        old[3].size = 40.0;
        let migrated = migrate_saved_brushes(old);
        assert_eq!(migrated.len(), efude_brush::defaults().len());
        assert!(migrated[3].wet_edge > 0.0);
        assert_eq!(migrated[3].size, 40.0);
        assert_eq!(migrated[8].name, "色混ぜ");
    }

    #[test]
    fn wet_brush_drags_paint_it_crosses() {
        let mut app = app(200, 80);
        // A red vertical band.
        use_brush(&mut app, 0, 14.0);
        app.color = Color32::from_rgb(220, 20, 20);
        draw(&mut app, &line((60.0, 5.0), (60.0, 75.0), 40));
        eprintln!(
            "red band pixel {:?} kind {:?}",
            app.doc.layers[0].pixels.pixel(60, 40),
            app.brushes[3].kind
        );
        // A blue watercolour stroke from left to right across it.
        use_brush(&mut app, 3, 18.0);
        app.brushes[3].mix.blend = 0.5;
        app.brushes[3].mix.persistence = 0.8;
        app.color = Color32::from_rgb(20, 20, 220);
        draw(&mut app, &line((10.0, 40.0), (190.0, 40.0), 90));
        eprintln!(
            "{:?}",
            (40..120)
                .step_by(4)
                .map(|x| app.doc.layers[0].pixels.pixel(x, 40))
                .collect::<Vec<_>>()
        );
        let before = app.doc.layers[0].pixels.pixel(30, 40);
        let after = app.doc.layers[0].pixels.pixel(72, 40);
        assert!(
            after[0] as i32 > before[0] as i32 + 20,
            "no red carried past the band: before {before:?}, after {after:?}"
        );
    }
}
