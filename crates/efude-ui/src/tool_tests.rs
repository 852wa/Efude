// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Behaviour of every tool, driven through the real UI with synthetic mouse
//! and keyboard input (one `update_ui` per frame, as eframe would call it).

use super::*;
use efude_canvas::ToneSettings;

const SCREEN: Vec2 = Vec2::new(1280.0, 820.0);

struct Harness {
    app: EfudeApp,
    ctx: egui::Context,
    time: f64,
    pointer: Pos2,
}

impl Harness {
    fn new(width: u32, height: u32) -> Self {
        let mut app = EfudeApp::default();
        app.doc = Document::new(width, height);
        app.navigator_center = Vec2::new(width as f32 / 2.0, height as f32 / 2.0);
        app.use_windows_ink = false;
        app.use_wintab = false;
        app.selected_layer = 0;
        app.color = Color32::from_rgb(0, 0, 0);
        let ctx = egui::Context::default();
        layout::apply_theme(&ctx);
        let mut harness = Self {
            app,
            ctx,
            time: 0.0,
            pointer: Pos2::new(5.0, 5.0),
        };
        harness.frames(3);
        harness
    }

    fn frame_with(&mut self, events: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let _ = self.ctx.run(input, |ctx| app.update_ui(ctx));
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame_with(Vec::new());
        }
    }

    /// Screen position of document point `p` (as drawn last frame).
    fn screen(&self, p: Vec2) -> Pos2 {
        let (rect, scale) = self.app.canvas_screen.expect("canvas drawn");
        let center = Vec2::new(self.app.doc.width as f32, self.app.doc.height as f32) / 2.0;
        let mut d = p - center;
        if self.app.flip_x {
            d.x = -d.x;
        }
        if self.app.flip_y {
            d.y = -d.y;
        }
        let (sa, ca) = self.app.view_rotation.to_radians().sin_cos();
        rect.center() + Vec2::new(d.x * ca - d.y * sa, d.x * sa + d.y * ca) * scale
    }

    fn move_to(&mut self, pos: Pos2) {
        self.pointer = pos;
        self.frame_with(vec![egui::Event::PointerMoved(pos)]);
    }

    fn button(&mut self, pressed: bool, button: egui::PointerButton) {
        self.frame_with(vec![egui::Event::PointerButton {
            pos: self.pointer,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }

    /// Drags along screen points with the given button.
    fn drag_screen(&mut self, path: &[Pos2], button: egui::PointerButton) {
        self.move_to(path[0]);
        self.button(true, button);
        for &p in &path[1..] {
            self.move_to(p);
        }
        self.button(false, button);
        self.frames(3);
    }

    /// Drags along document points (straight segments, sampled finely).
    fn drag(&mut self, points: &[Vec2]) {
        let mut path = Vec::new();
        for pair in points.windows(2) {
            for i in 0..12 {
                path.push(self.screen(pair[0] + (pair[1] - pair[0]) * (i as f32 / 12.0)));
            }
        }
        path.push(self.screen(*points.last().unwrap()));
        self.drag_screen(&path, egui::PointerButton::Primary);
    }

    fn click(&mut self, p: Vec2) {
        let pos = self.screen(p);
        self.click_screen(pos);
    }

    fn click_screen(&mut self, pos: Pos2) {
        self.move_to(pos);
        self.button(true, egui::PointerButton::Primary);
        self.button(false, egui::PointerButton::Primary);
        self.frames(2);
    }

    fn key(&mut self, key: egui::Key, pressed: bool) {
        self.frame_with(vec![egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);
    }

    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.app.doc.layers[self.app.selected_layer]
            .pixels
            .pixel(x, y)
    }

    fn fill_rect(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, color: [u8; 4]) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.app.doc.layers[0].pixels.set_pixel(x, y, color);
            }
        }
        self.app.canvas_texture_dirty = true;
        self.frames(1);
    }

    fn selected(&self, x: u32, y: u32) -> bool {
        self.app.selection.active
            && self.app.selection.mask[(y * self.app.doc.width + x) as usize] > 127
    }

    fn use_tool(&mut self, tool: Tool) {
        self.app.tool = tool;
        self.frames(1);
    }
}

fn v(x: f32, y: f32) -> Vec2 {
    Vec2::new(x, y)
}

#[test]
fn brush_paints_along_the_drag() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    h.app.selected_brush = 0;
    h.app.size = 8.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    for x in [80, 200, 320] {
        assert!(h.pixel(x, 150)[3] > 150, "x {x}: {:?}", h.pixel(x, 150));
    }
    assert_eq!(h.pixel(200, 100)[3], 0);
}

#[test]
fn eraser_removes_paint() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [200, 30, 30, 255]);
    h.use_tool(Tool::Eraser);
    h.app.size = 16.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.pixel(200, 150)[3] < 40, "{:?}", h.pixel(200, 150));
    assert_eq!(h.pixel(200, 60)[3], 255);
}

#[test]
fn blur_softens_an_edge() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 200, 300, [220, 30, 30, 255]);
    h.fill_rect(200, 0, 400, 300, [30, 30, 220, 255]);
    h.use_tool(Tool::Blur);
    h.app.size = 30.0;
    h.drag(&[v(200.0, 60.0), v(200.0, 240.0)]);
    let p = h.pixel(198, 150);
    assert!(p[2] > 60 && p[0] < 210, "{p:?}");
}

#[test]
fn smudge_drags_colour() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [240, 240, 240, 255]);
    h.fill_rect(0, 0, 150, 300, [220, 30, 30, 255]);
    h.use_tool(Tool::Smudge);
    h.app.size = 24.0;
    h.drag(&[v(100.0, 150.0), v(260.0, 150.0)]);
    let p = h.pixel(170, 150);
    assert!(p[1] < 220, "no red dragged: {p:?}");
}

#[test]
fn fill_fills_the_clicked_region_only() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(198, 0, 202, 300, [0, 0, 0, 255]);
    h.use_tool(Tool::Fill);
    h.app.color = Color32::from_rgb(20, 160, 60);
    h.click(v(100.0, 100.0));
    assert_eq!(h.pixel(100, 100), [20, 160, 60, 255]);
    assert_eq!(h.pixel(300, 100)[3], 0);
}

#[test]
fn eyedropper_picks_the_clicked_colour() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(80, 80, 120, 120, [10, 200, 90, 255]);
    h.use_tool(Tool::Eyedropper);
    h.app.eyedropper_radius = 0;
    h.click(v(100.0, 100.0));
    assert_eq!(h.app.color, Color32::from_rgb(10, 200, 90));
}

#[test]
fn move_translates_the_layer() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(90, 90, 110, 110, [0, 0, 255, 255]);
    h.use_tool(Tool::Move);
    h.drag(&[v(100.0, 100.0), v(150.0, 130.0)]);
    assert_eq!(h.pixel(150, 130)[3], 255, "block did not arrive");
    assert_eq!(h.pixel(95, 95)[3], 0, "block did not leave");
}

#[test]
fn pan_moves_the_view_even_off_the_picture() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Pan);
    let (rect, scale) = h.app.canvas_screen.unwrap();
    // Start below the picture, on the empty canvas area.
    let start = Pos2::new(rect.center().x, rect.bottom() + 20.0);
    let before = h.app.navigator_center;
    let path: Vec<Pos2> = (0..=10)
        .map(|i| start + Vec2::new(8.0 * i as f32, -6.0 * i as f32))
        .collect();
    h.drag_screen(&path, egui::PointerButton::Primary);
    let moved = before - h.app.navigator_center;
    assert!(
        (moved - Vec2::new(80.0, -60.0) / scale).length() < 1.5,
        "{moved:?}"
    );
}

#[test]
fn middle_button_pans_with_any_tool() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    let (rect, _) = h.app.canvas_screen.unwrap();
    let before = h.app.navigator_center;
    let path: Vec<Pos2> = (0..=10)
        .map(|i| rect.center() + Vec2::new(6.0 * i as f32, 0.0))
        .collect();
    h.drag_screen(&path, egui::PointerButton::Middle);
    assert!((before - h.app.navigator_center).x > 20.0);
    assert!(
        h.app.doc.layers[0].pixels.tile_keys().is_empty(),
        "middle drag painted"
    );
}

#[test]
fn rectangle_and_ellipse_select() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(100.0, 100.0), v(200.0, 180.0)]);
    assert!(h.selected(150, 140) && !h.selected(250, 140) && !h.selected(150, 60));
    h.use_tool(Tool::EllipseSelect);
    h.drag(&[v(100.0, 100.0), v(300.0, 200.0)]);
    assert!(h.selected(200, 150) && !h.selected(105, 105));
}

#[test]
fn lasso_and_polygon_select() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::LassoSelect);
    h.drag(&[
        v(100.0, 100.0),
        v(300.0, 100.0),
        v(200.0, 250.0),
        v(100.0, 100.0),
    ]);
    assert!(h.selected(200, 150) && !h.selected(120, 230));
    h.app.change_selection(|selection, _, _| selection.clear());
    h.use_tool(Tool::PolygonSelect);
    for p in [
        v(50.0, 50.0),
        v(150.0, 50.0),
        v(150.0, 150.0),
        v(50.0, 150.0),
    ] {
        h.click(p);
    }
    h.key(egui::Key::Enter, true);
    h.key(egui::Key::Enter, false);
    assert!(h.selected(100, 100), "polygon not selected");
    assert!(!h.selected(250, 100));
}

#[test]
fn magic_wand_and_color_range() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(50, 50, 120, 120, [200, 0, 0, 255]);
    h.fill_rect(250, 50, 320, 120, [200, 0, 0, 255]);
    h.use_tool(Tool::MagicWand);
    h.click(v(80.0, 80.0));
    assert!(h.selected(80, 80) && !h.selected(280, 80) && !h.selected(180, 80));
    h.use_tool(Tool::ColorRange);
    h.click(v(80.0, 80.0));
    assert!(h.selected(80, 80) && h.selected(280, 80) && !h.selected(180, 80));
}

#[test]
fn selection_brush_paints_selection() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::SelectionBrush);
    h.app.size = 20.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.selected(200, 150) && !h.selected(200, 60));
}

#[test]
fn rulers_draw_straight_lines_ellipses_and_curves() {
    let mut h = Harness::new(400, 300);
    h.app.size = 6.0;
    h.use_tool(Tool::Line);
    // A wobbly drag still gives a straight line from start to end.
    h.drag(&[v(50.0, 50.0), v(120.0, 90.0), v(350.0, 50.0)]);
    assert!(h.pixel(200, 50)[3] > 100, "line missing");
    assert_eq!(h.pixel(120, 90)[3], 0, "line followed the pointer");
    h.use_tool(Tool::EllipseRuler);
    h.drag(&[v(100.0, 150.0), v(300.0, 250.0)]);
    assert!(
        h.pixel(200, 150)[3] > 100 || h.pixel(200, 151)[3] > 100,
        "ellipse top missing"
    );
    assert_eq!(h.pixel(200, 200)[3], 0, "ellipse filled");
    h.use_tool(Tool::BezierRuler);
    for p in [
        v(20.0, 280.0),
        v(20.0, 200.0),
        v(380.0, 200.0),
        v(380.0, 280.0),
    ] {
        h.click(p);
    }
    let painted: u32 = (0..300).map(|y| h.pixel(200, y)[3] as u32).sum();
    assert!(painted > 200, "bezier curve missing");
}

#[test]
fn perspective_ruler_uses_the_vanishing_point() {
    let mut h = Harness::new(400, 300);
    h.app.size = 6.0;
    h.use_tool(Tool::PerspectiveRuler);
    h.app.setting_vanishing_point = true;
    h.click(v(200.0, 20.0));
    assert_eq!(h.app.perspective_points, vec![(200, 20)]);
    assert_eq!(h.pixel(200, 20)[3], 0, "setting the point painted a dot");
    h.drag(&[v(200.0, 280.0), v(200.0, 200.0)]);
    assert!(
        h.pixel(200, 100)[3] > 100,
        "line to vanishing point missing"
    );
}

#[test]
fn tool_keys_tap_to_switch_and_hold_to_borrow() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    // Tap: switch for good.
    h.key(egui::Key::Num3, true);
    h.key(egui::Key::Num3, false);
    h.frames(2);
    assert!(h.app.tool == Tool::Fill);
    h.key(egui::Key::Num1, true);
    h.key(egui::Key::Num1, false);
    assert!(h.app.tool == Tool::Brush);
    // Hold Space: hand while held, then back.
    h.key(egui::Key::Space, true);
    assert!(h.app.tool == Tool::Pan);
    h.key(egui::Key::Space, false);
    h.frames(2);
    assert!(h.app.tool == Tool::Brush);
}

#[test]
fn undo_and_redo_buttons_work() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.pixel(200, 150)[3] > 150);
    let [undo, redo] = h.app.history_buttons;
    h.click_screen(undo.center());
    assert_eq!(h.pixel(200, 150)[3], 0, "undo button did nothing");
    h.click_screen(redo.center());
    assert!(h.pixel(200, 150)[3] > 150, "redo button did nothing");
}

#[test]
fn new_document_replaces_the_picture() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [200, 30, 30, 255]);
    h.app.canvas_width_input = 640;
    h.app.canvas_height_input = 480;
    h.app.history = History::default();
    assert!(h.app.new_document());
    h.frames(2);
    assert_eq!((h.app.doc.width, h.app.doc.height), (640, 480));
    assert!(h.app.doc.layers[0].pixels.tile_keys().is_empty());
}

#[test]
fn every_edit_can_be_undone() {
    let mut h = Harness::new(400, 300);
    h.app.size = 8.0;
    // Brush.
    h.use_tool(Tool::Brush);
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    h.app.undo();
    h.frames(1);
    assert_eq!(h.pixel(200, 150)[3], 0, "brush undo");
    // Fill.
    h.use_tool(Tool::Fill);
    h.click(v(100.0, 100.0));
    assert_eq!(h.pixel(100, 100)[3], 255);
    h.app.undo();
    h.frames(1);
    assert_eq!(h.pixel(100, 100)[3], 0, "fill undo");
    // Line ruler.
    h.use_tool(Tool::Line);
    h.drag(&[v(50.0, 50.0), v(350.0, 50.0)]);
    assert!(h.pixel(200, 50)[3] > 100);
    h.app.undo();
    h.frames(1);
    assert_eq!(h.pixel(200, 50)[3], 0, "line undo");
    // Move.
    h.fill_rect(90, 90, 110, 110, [0, 0, 255, 255]);
    h.app.history = History::default();
    h.use_tool(Tool::Move);
    h.drag(&[v(100.0, 100.0), v(150.0, 130.0)]);
    h.app.undo();
    h.frames(1);
    assert_eq!(h.pixel(100, 100)[3], 255, "move undo");
    assert_eq!(h.pixel(150, 130)[3], 0, "move undo");
}

#[test]
fn selection_limits_painting() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(100.0, 100.0), v(200.0, 200.0)]);
    h.use_tool(Tool::Brush);
    h.app.size = 10.0;
    h.drag(&[v(40.0, 150.0), v(360.0, 150.0)]);
    assert!(h.pixel(150, 150)[3] > 150, "inside not painted");
    assert_eq!(h.pixel(300, 150)[3], 0, "painted outside the selection");
}

#[test]
fn holding_the_eraser_key_erases_then_returns() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [200, 30, 30, 255]);
    h.use_tool(Tool::Brush);
    h.app.size = 16.0;
    h.key(egui::Key::E, true);
    assert!(h.app.tool == Tool::Eraser);
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    h.key(egui::Key::E, false);
    h.frames(2);
    assert!(h.app.tool == Tool::Brush, "did not return to the brush");
    assert!(h.pixel(200, 150)[3] < 40, "did not erase");
}

#[test]
fn move_with_a_selection_moves_only_the_selection() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(50, 50, 100, 100, [0, 0, 255, 255]);
    h.fill_rect(250, 50, 300, 100, [0, 200, 0, 255]);
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(40.0, 40.0), v(110.0, 110.0)]);
    h.use_tool(Tool::Move);
    h.drag(&[v(75.0, 75.0), v(75.0, 175.0)]);
    assert_eq!(h.pixel(75, 175)[2], 255, "selection did not move");
    assert_eq!(h.pixel(75, 75)[3], 0, "selection left a copy");
    assert_eq!(
        h.pixel(275, 75),
        [0, 200, 0, 255],
        "unselected pixels moved"
    );
}

#[test]
fn copy_and_paste_places_a_copy_where_clicked() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(50, 50, 100, 100, [0, 0, 255, 255]);
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(50.0, 50.0), v(99.0, 99.0)]);
    h.app.copy_selection(false);
    let ctx = h.ctx.clone();
    h.app.paste_clipboard(&ctx);
    assert!(h.app.paste_preview.is_some());
    h.drag(&[v(200.0, 150.0), v(300.0, 200.0)]);
    h.click(v(300.0, 200.0));
    assert!(h.app.paste_preview.is_none(), "paste was not committed");
    assert_eq!(h.pixel(75, 75)[2], 255, "original lost");
    let copied: u32 = (0..300)
        .flat_map(|y| (150..400).map(move |x| (x, y)))
        .filter(|&(x, y)| h.pixel(x, y)[2] == 255)
        .count() as u32;
    assert!(copied > 1500, "copy not placed ({copied} px)");
}

#[test]
fn symmetry_mirrors_strokes() {
    let mut h = Harness::new(400, 300);
    h.app.symmetry_x = true;
    h.app.symmetry_center = Vec2::new(199.5, 149.5);
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    h.drag(&[v(50.0, 100.0), v(150.0, 100.0)]);
    assert!(h.pixel(100, 100)[3] > 150 && h.pixel(299, 100)[3] > 150);
}

#[test]
fn locked_layers_are_not_painted() {
    let mut h = Harness::new(400, 300);
    h.app.doc.layers[0].locked = true;
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert_eq!(h.pixel(200, 150)[3], 0);
    h.use_tool(Tool::Fill);
    h.click(v(100.0, 100.0));
    assert_eq!(h.pixel(100, 100)[3], 0);
}

#[test]
fn wheel_zoom_keeps_the_point_under_the_pointer() {
    let mut h = Harness::new(400, 300);
    let target = v(100.0, 80.0);
    let pos = h.screen(target);
    h.move_to(pos);
    for _ in 0..6 {
        h.frame_with(vec![egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, 60.0),
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    h.frames(20);
    assert!(h.app.zoom > 1.2, "no zoom: {}", h.app.zoom);
    assert!(
        (h.screen(target) - pos).length() < 3.0,
        "point drifted {:?} {:?}",
        h.screen(target),
        pos
    );
}

#[test]
fn stays_dark_with_a_light_os_theme() {
    let mut h = Harness::new(100, 100);
    for _ in 0..3 {
        h.time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
            system_theme: Some(egui::Theme::Light),
            time: Some(h.time),
            ..Default::default()
        };
        let app = &mut h.app;
        let _ = h.ctx.run(input, |ctx| app.update_ui(ctx));
    }
    let style = h.ctx.style();
    assert!(style.visuals.dark_mode);
    assert!(style.visuals.text_color().r() > 220);
}

#[test]
fn delete_key_clears_the_selection_and_escape_cancels_points() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [200, 30, 30, 255]);
    h.app.history = History::default();
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(100.0, 100.0), v(200.0, 200.0)]);
    h.key(egui::Key::Delete, true);
    h.key(egui::Key::Delete, false);
    assert_eq!(h.pixel(150, 150)[3], 0, "selection not cleared");
    assert_eq!(h.pixel(300, 150)[3], 255, "cleared outside the selection");
    h.app.undo();
    assert_eq!(h.pixel(150, 150)[3], 255, "delete not undoable");
    h.use_tool(Tool::PolygonSelect);
    h.click(v(50.0, 50.0));
    h.click(v(150.0, 50.0));
    h.key(egui::Key::Escape, true);
    h.key(egui::Key::Escape, false);
    assert!(h.app.selection_points.is_empty());
}

#[test]
fn quick_mask_paints_a_selection() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::QuickMask);
    h.app.size = 20.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.selected(200, 150) && !h.selected(200, 60));
}

#[test]
fn rotated_view_turns_the_canvas_and_input_follows() {
    let mut h = Harness::new(400, 300);
    h.app.view_rotation = 90.0;
    h.frames(2);
    // The long side of the picture is now vertical on screen.
    let (top_left, top_right) = (h.screen(v(0.0, 0.0)), h.screen(v(400.0, 0.0)));
    assert!((top_right.x - top_left.x).abs() < 1.0 && (top_right.y - top_left.y).abs() > 100.0);
    // A horizontal drag on screen paints a vertical line in the picture.
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    let (rect, _) = h.app.canvas_screen.unwrap();
    let path: Vec<Pos2> = (0..=30)
        .map(|i| rect.center() + Vec2::new(-80.0 + 160.0 * i as f32 / 30.0, 0.0))
        .collect();
    h.drag_screen(&path, egui::PointerButton::Primary);
    let column: u32 = (0..300).map(|y| h.pixel(200, y)[3] as u32).sum();
    let row: u32 = (0..400).map(|x| h.pixel(x, 150)[3] as u32).sum();
    assert!(column > row * 3, "column {column} row {row}");
    // Panning follows the pointer on the rotated view.
    h.use_tool(Tool::Pan);
    let grip = v(100.0, 100.0);
    let start = h.screen(grip);
    let path: Vec<Pos2> = (0..=10)
        .map(|i| start + Vec2::new(6.0 * i as f32, 3.0 * i as f32))
        .collect();
    h.drag_screen(&path, egui::PointerButton::Primary);
    assert!((h.screen(grip) - (start + Vec2::new(60.0, 30.0))).length() < 2.0);
}

#[test]
fn flipped_view_mirrors_the_canvas() {
    let mut h = Harness::new(400, 300);
    h.app.flip_x = true;
    h.frames(2);
    assert!(h.screen(v(0.0, 0.0)).x > h.screen(v(400.0, 0.0)).x);
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    h.drag(&[v(50.0, 60.0), v(150.0, 60.0)]);
    assert!(h.pixel(100, 60)[3] > 150 && h.pixel(300, 60)[3] == 0);
}

// ---- Manga ----------------------------------------------------------------

/// A small manga page (100 dpi) so the tests stay fast.
fn manga_harness() -> Harness {
    let mut h = Harness::new(200, 200);
    let spec = efude_comic::PageSpec {
        trim_mm: [100.0, 140.0],
        bleed_mm: 3.0,
        inner_margins_mm: [15.0, 15.0, 12.0, 10.0],
        dpi: 100.0,
        binding: efude_comic::Binding::Right,
        right_page: false,
    };
    h.app.history = History::default();
    assert!(h.app.new_comic_document(&spec));
    h.frames(3);
    h
}

fn composite_alpha(h: &Harness, x: u32, y: u32) -> [u8; 4] {
    let pixels = efude_canvas::composite(&h.app.doc);
    let i = ((y * h.app.doc.width + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

#[test]
fn manga_page_panels_split_by_dragging_and_undo() {
    let mut h = manga_harness();
    let comic = h.app.comic_doc().expect("comic data stored");
    let g = comic.page.geometry();
    assert_eq!(
        (h.app.doc.width, h.app.doc.height),
        (g.canvas_width, g.canvas_height)
    );
    h.app.create_panel(None);
    h.frames(2);
    assert_eq!(h.app.comic_doc().unwrap().layout.panels.len(), 1);
    let [min, max] = g.inner;
    let mid = (min + max) * 0.5;
    // Split with the tool: a level drag across the panel.
    h.use_tool(Tool::PanelSplit);
    h.drag(&[v(min.x - 20.0, mid.y), v(max.x + 20.0, mid.y + 1.0)]);
    let comic = h.app.comic_doc().unwrap();
    assert_eq!(comic.layout.panels.len(), 2, "{}", h.app.status);
    // The gap between the rows is the vertical gutter.
    let bottom_of_top = comic.layout.panels[0]
        .polygon
        .iter()
        .map(|p| p.y)
        .fold(f32::MIN, f32::max);
    let top_of_bottom = comic.layout.panels[1]
        .polygon
        .iter()
        .map(|p| p.y)
        .fold(f32::MAX, f32::min);
    assert!((top_of_bottom - bottom_of_top - comic.layout.gutter_vertical).abs() < 1.5);
    // Drawing in the first panel is clipped at its edge.
    let first = &comic.layout.panels[0];
    let drawing = h
        .app
        .doc
        .layers
        .iter()
        .position(|l| l.parent_id == Some(first.folder_id) && !l.locked)
        .unwrap();
    h.app.selected_layer = drawing;
    h.use_tool(Tool::Brush);
    h.app.size = 6.0;
    h.drag(&[v(mid.x - 30.0, mid.y - 60.0), v(mid.x - 30.0, mid.y + 60.0)]);
    let above = composite_alpha(&h, (mid.x - 30.0) as u32, (mid.y - 40.0) as u32);
    let gap = composite_alpha(&h, (mid.x - 30.0) as u32, mid.y as u32);
    assert!(above[0] < 80, "stroke inside the panel: {above:?}");
    assert!(gap[0] > 200, "stroke must not show in the gutter: {gap:?}");
    // The border is drawn inside the panel edge.
    let edge = composite_alpha(&h, (min.x + 1.0) as u32, (min.y + 20.0) as u32);
    assert!(edge[0] < 80, "border: {edge:?}");
    // Undo the stroke and the split.
    let [undo, _] = h.app.history_buttons;
    h.click_screen(undo.center());
    h.click_screen(undo.center());
    assert_eq!(h.app.comic_doc().unwrap().layout.panels.len(), 1);
    let folder = h
        .app
        .doc
        .layers
        .iter()
        .find(|l| l.id == first.folder_id)
        .unwrap();
    let mask = folder.mask.as_ref().unwrap();
    assert_eq!(
        mask.pixel((mid.x - 30.0) as u32, mid.y as u32)[0],
        255,
        "undo restores the panel shape"
    );
}

#[test]
fn manga_grid_split_reads_from_the_binding() {
    let mut h = manga_harness();
    h.app.create_panel(None);
    assert!(h.app.grid_split_panel(2, 2));
    let comic = h.app.comic_doc().unwrap();
    assert_eq!(comic.layout.panels.len(), 4);
    // Right-bound: the panel kept by the split (read first) is top right.
    let centre = |i: usize| {
        let p = &comic.layout.panels[i].polygon;
        p.iter().fold(glam::Vec2::ZERO, |a, &b| a + b) / p.len() as f32
    };
    assert!(centre(0).x > centre(1).x && centre(0).y < centre(2).y);
    // Folders: one per panel, each with a drawing and a locked border layer.
    let folders = h
        .app
        .doc
        .layers
        .iter()
        .filter(|l| l.kind == LayerKind::Folder)
        .count();
    assert_eq!(folders, 4);
}

#[test]
fn manga_tone_shows_dots_and_lines_leave_the_centre_clear() {
    let mut h = manga_harness();
    h.app.create_panel(None);
    let [min, max] = h.app.comic_doc().unwrap().page.geometry().inner;
    let settings = ToneSettings {
        lines_per_inch: 20.0,
        ..ToneSettings::default()
    };
    h.app.tone_fill(settings, 0.4);
    h.frames(2);
    assert!(h.app.doc.layers[h.app.selected_layer].tone.is_some());
    let pixels = efude_canvas::composite(&h.app.doc);
    let w = h.app.doc.width;
    let (mut dark, mut light) = (0, 0);
    for y in (min.y as u32 + 10)..(min.y as u32 + 60) {
        for x in (min.x as u32 + 10)..(min.x as u32 + 60) {
            let r = pixels[((y * w + x) * 4) as usize];
            if r < 60 {
                dark += 1;
            } else if r > 200 {
                light += 1;
            }
        }
    }
    assert!(
        dark > 300 && light > 300,
        "halftone: {dark} dark, {light} light"
    );
    // Outside the panel there is no tone.
    let outside = composite_alpha(&h, 3, 3);
    assert!(outside[0] > 240);

    // Focus lines on their own layer.
    h.app.open_effect_dialog(comic::EffectKind::Focus);
    h.app.comic_ui.focus.length_jitter = 0.0;
    h.app.draw_effect_lines(comic::EffectKind::Focus);
    let layer = &h.app.doc.layers[h.app.selected_layer];
    assert!(layer.name.contains("集中線"));
    let centre = (min + max) * 0.5;
    assert_eq!(layer.pixels.pixel(centre.x as u32, centre.y as u32)[3], 0);
    let edge: u32 = (min.x as u32..max.x as u32)
        .map(|x| layer.pixels.pixel(x, min.y as u32 + 3)[3] as u32)
        .sum();
    assert!(edge > 255 * 10, "lines reach the edge: {edge}");
    // Redrawing replaces the lines instead of adding a layer.
    let layers = h.app.doc.layers.len();
    h.app.comic_ui.focus.seed += 1;
    h.app.draw_effect_lines(comic::EffectKind::Focus);
    assert_eq!(h.app.doc.layers.len(), layers);
}

#[test]
fn manga_menu_opens_page_setup() {
    let mut h = Harness::new(200, 200);
    h.app.comic_ui.setup_open = true;
    h.frames(2);
    assert!(h.app.comic_ui.setup_open);
    // Without a manga page the split tool does nothing and says why.
    h.use_tool(Tool::PanelSplit);
    h.drag(&[v(20.0, 100.0), v(180.0, 100.0)]);
    assert!(h.app.comic_doc().is_none());
}

/// Frame times of large strokes on a B5 350 dpi page (run with
/// `cargo test --release -p efude-ui perf_ -- --ignored --nocapture`).
#[test]
#[ignore]
fn perf_large_strokes() {
    let mut h = Harness::new(2598, 3661);
    // Something to mix with: a coloured band under the strokes.
    for y in 1400..2300 {
        for x in 0..2598 {
            let c = if (x / 200) % 2 == 0 {
                [200, 40, 40, 255]
            } else {
                [40, 60, 210, 255]
            };
            h.app.doc.layers[0].pixels.set_pixel(x, y, c);
        }
    }
    let only = std::env::var("EFUDE_PERF_ONLY").ok();
    let cases = [
        ("pen", Tool::Brush, 0usize),
        ("watercolor", Tool::Brush, 3),
        ("airbrush", Tool::Brush, 4),
        ("blender", Tool::Brush, 8),
        ("blur", Tool::Blur, 5),
        ("smudge", Tool::Smudge, 6),
    ];
    for (name, tool, brush) in cases {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        h.use_tool(tool);
        h.app.selected_brush = brush;
        for size in [40.0, 150.0] {
            h.app.size = size;
            let path: Vec<Pos2> = (0..40)
                .map(|i| {
                    h.screen(v(
                        300.0 + i as f32 * 30.0,
                        1800.0 + (i as f32 * 0.2).sin() * 300.0,
                    ))
                })
                .collect();
            h.move_to(path[0]);
            h.button(true, egui::PointerButton::Primary);
            let start = std::time::Instant::now();
            let mut worst = std::time::Duration::ZERO;
            for &p in &path[1..] {
                let t = std::time::Instant::now();
                h.move_to(p);
                worst = worst.max(t.elapsed());
            }
            let total = start.elapsed();
            h.button(false, egui::PointerButton::Primary);
            eprintln!(
                "{name} size {size}: {:.1} ms/frame avg, worst {:.1} ms",
                total.as_secs_f64() * 1000.0 / (path.len() - 1) as f64,
                worst.as_secs_f64() * 1000.0
            );
        }
    }
}

#[test]
fn new_canvases_open_in_tabs() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    h.app.size = 8.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.pixel(200, 150)[3] > 150);
    // A new canvas opens next to the painted one.
    h.app.canvas_width_input = 640;
    h.app.canvas_height_input = 480;
    assert!(h.app.new_document());
    h.frames(3);
    assert_eq!(h.app.tabs.slots.len(), 2);
    assert_eq!((h.app.doc.width, h.app.doc.height), (640, 480));
    assert_eq!(h.pixel(200, 150)[3], 0);
    h.drag(&[v(100.0, 100.0), v(500.0, 100.0)]);
    assert!(h.pixel(300, 100)[3] > 150);
    // Back to the first tab: its picture, size and undo history.
    h.app.switch_tab(0);
    h.frames(3);
    assert_eq!((h.app.doc.width, h.app.doc.height), (400, 300));
    assert!(h.pixel(200, 150)[3] > 150);
    let [undo, _] = h.app.history_buttons;
    h.click_screen(undo.center());
    assert_eq!(h.pixel(200, 150)[3], 0, "undo works per tab");
    h.app.switch_tab(1);
    h.frames(2);
    assert!(h.pixel(300, 100)[3] > 150, "second tab kept its stroke");
    // Closing a tab with changes asks first.
    h.app.tabs.confirm_close = None;
    assert_eq!(h.app.dirty_tabs().len(), 2);
    h.app.close_tab_now(1);
    h.frames(2);
    assert_eq!(h.app.tabs.slots.len(), 1);
    assert_eq!((h.app.doc.width, h.app.doc.height), (400, 300));
    // An untouched canvas is reused instead of piling up tabs.
    h.app.close_tab_now(0);
    h.frames(1);
    assert!(h.app.new_document());
    assert_eq!(h.app.tabs.slots.len(), 1);
}

#[test]
fn balloons_are_drawn_moved_and_undone() {
    let mut h = Harness::new(600, 500);
    h.use_tool(Tool::Balloon);
    h.drag(&[v(150.0, 150.0), v(350.0, 300.0)]);
    let balloons = h.app.balloons();
    assert_eq!(balloons.len(), 1, "{}", h.app.status);
    let b = balloons[0].clone();
    assert!((b.center - glam::Vec2::new(250.0, 225.0)).length() < 3.0);
    let layer = h
        .app
        .doc
        .layers
        .iter()
        .position(|l| l.id == b.layer_id)
        .unwrap();
    let pixel = |h: &Harness, x, y| h.app.doc.layers[layer].pixels.pixel(x, y);
    assert_eq!(pixel(&h, 250, 225), [255, 255, 255, 255], "white fill");
    assert!(
        pixel(&h, 250, 151)[3] > 200 && pixel(&h, 250, 151)[0] < 80,
        "outline"
    );
    // Text goes in through the editor.
    let mut edited = b.clone();
    edited.text = "テスト".into();
    edited.style.vertical = true;
    h.app.update_balloon_for_test(edited);
    h.frames(2);
    // Drag the balloon to the right.
    h.drag(&[v(250.0, 225.0), v(330.0, 225.0)]);
    let moved = h.app.balloons()[0].clone();
    assert!((moved.center.x - 330.0).abs() < 3.0, "{:?}", moved.center);
    assert_eq!(pixel(&h, 190, 225)[3], 0, "old place cleared");
    assert_eq!(pixel(&h, 330, 225)[3], 255);
    // Undo the move.
    let [undo, _] = h.app.history_buttons;
    h.click_screen(undo.center());
    assert!((h.app.balloons()[0].center.x - 250.0).abs() < 3.0);
    assert_eq!(pixel(&h, 250, 225)[3], 255);
    // Plain text with the text tool.
    h.use_tool(Tool::Text);
    h.click(v(80.0, 420.0));
    assert_eq!(h.app.balloons().len(), 2);
    assert_eq!(h.app.balloons()[1].shape, efude_comic::BalloonShape::None);
}

#[test]
fn books_create_pages_and_export_for_print() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(200, 200);
    let spec = efude_comic::PageSpec {
        trim_mm: [100.0, 140.0],
        bleed_mm: 3.0,
        inner_margins_mm: [15.0, 15.0, 12.0, 10.0],
        dpi: 100.0,
        binding: efude_comic::Binding::Right,
        right_page: false,
    };
    h.app
        .create_book(dir.path(), "テスト", 3, spec.clone())
        .unwrap();
    let book_path = dir.path().join("テスト.efudebook");
    assert!(book_path.exists());
    for n in 1..=3 {
        assert!(dir.path().join(format!("{n:03}.efude")).exists());
    }
    // Pages alternate sides: the binding margin moves.
    let page2 = efude_io::load(&dir.path().join("002.efude")).unwrap();
    let comic: comic::ComicDoc = serde_json::from_str(&page2.metadata["comic"]).unwrap();
    assert!(comic.page.right_page);
    // Mark page 1 so the export can be checked.
    let mut page1 = efude_io::load(&dir.path().join("001.efude")).unwrap();
    for y in 100..140 {
        for x in 100..140 {
            page1.layers[0].pixels.set_pixel(x, y, [0, 0, 0, 255]);
        }
    }
    efude_io::save(&dir.path().join("001.efude"), &page1).unwrap();
    let out = dir.path().join("out");
    let options = efude_comic::ExportOptions {
        area: efude_comic::book::ExportArea::Trim,
        color: efude_comic::book::ExportColor::Monochrome,
        threshold: 128,
        spreads: false,
    };
    book::export_for_test(&book_path, &out, options).unwrap();
    let first = image::open(out.join("001.png")).unwrap().to_rgba8();
    let g = spec.geometry();
    assert_eq!(
        first.width(),
        (g.trim[1].x.round() - g.trim[0].x.round()) as u32
    );
    let trim_x = g.trim[0].x.round() as u32;
    let trim_y = g.trim[0].y.round() as u32;
    assert_eq!(
        first.get_pixel(120 - trim_x, 120 - trim_y).0,
        [0, 0, 0, 255]
    );
    // Only black and white, and a page number somewhere in the bottom margin.
    assert!(first.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));
    let bottom = (g.inner[1].y.round() as u32 - trim_y)..first.height();
    let ink = bottom
        .flat_map(|y| (0..first.width()).map(move |x| (x, y)))
        .filter(|&(x, y)| first.get_pixel(x, y).0[0] == 0)
        .count();
    if !efude_comic::text::system_fonts().is_empty() {
        assert!(ink > 5, "page number drawn");
    }
    // Spreads: page 1 alone, then pages 2–3 side by side.
    let options = efude_comic::ExportOptions {
        spreads: true,
        area: efude_comic::book::ExportArea::WithMarks,
        ..options
    };
    book::export_for_test(&book_path, &out, options).unwrap();
    let spread = image::open(out.join("spread_002.png")).unwrap();
    let single = image::open(out.join("spread_001.png")).unwrap();
    assert_eq!(spread.width(), single.width());
    assert!(spread.width() > 2 * g.canvas_width);
}

#[test]
fn grain_images_paint_black_and_skip_white_and_transparent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("grain.png");
    let mut image = image::RgbaImage::new(3, 1);
    image.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
    image.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
    image.put_pixel(2, 0, image::Rgba([0, 0, 0, 0]));
    image.save(&path).unwrap();
    let grain = load_grain_texture(&path).unwrap();
    assert_eq!(grain.coverage, vec![255, 0, 0]);
}

#[test]
fn grain_templates_load_and_tile() {
    for index in 0..GRAIN_TEMPLATES.len() {
        let grain = grain_template(index).expect("template decodes");
        assert_eq!((grain.width, grain.height), (512, 512));
        // Seamless: opposite edges are close to each other.
        let at = |x: u32, y: u32| grain.coverage[(y * 512 + x) as usize] as i32;
        let seam: i32 = (0..512).map(|y| (at(0, y) - at(511, y)).abs()).sum::<i32>() / 512;
        let inner: i32 = (0..512)
            .map(|y| (at(255, y) - at(256, y)).abs())
            .sum::<i32>()
            / 512;
        assert!(
            seam <= inner * 3 + 12,
            "template {index}: seam {seam}, inner {inner}"
        );
    }
}

#[test]
fn each_brush_tool_keeps_its_own_brush_and_settings() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    let pen = h.app.selected_brush;
    assert!(!matches!(
        h.app.brushes[pen].kind,
        BrushKind::Eraser | BrushKind::Blur | BrushKind::Smudge
    ));
    h.app.size = 12.0;
    h.frames(1);
    h.use_tool(Tool::Eraser);
    let eraser = h.app.selected_brush;
    assert!(matches!(h.app.brushes[eraser].kind, BrushKind::Eraser));
    // Eraser settings do not touch the pen.
    h.app.size = 40.0;
    h.app.brushes[eraser].antialias = 0;
    h.frames(1);
    h.use_tool(Tool::Brush);
    assert_eq!(h.app.selected_brush, pen);
    assert_eq!(h.app.size, 12.0);
    assert_eq!(h.app.brushes[pen].antialias, 2);
    h.use_tool(Tool::Eraser);
    assert_eq!(h.app.selected_brush, eraser);
    assert_eq!(h.app.size, 40.0);
    // Picking an eraser preset while the pen is active switches tools.
    h.use_tool(Tool::Brush);
    h.app.select_brush_preset(eraser);
    h.frames(1);
    assert_eq!(h.app.tool, Tool::Eraser);
    assert_eq!(h.app.selected_brush, eraser);
}

#[test]
fn the_eraser_ignores_pressure_by_default() {
    let brushes = efude_brush::defaults();
    let eraser = brushes
        .iter()
        .find(|brush| matches!(brush.kind, BrushKind::Eraser))
        .unwrap();
    assert_eq!(eraser.opacity_source, DynamicSource::None);
}

#[test]
fn selecting_a_tool_on_the_rail_opens_its_properties() {
    let mut h = Harness::new(400, 300);
    // Start with the tool panel closed.
    let location = h.app.workspace.find_tab(&layout::Pane::Tool).unwrap();
    h.app.workspace.remove_tab(location);
    h.frames(1);
    // The rail's buttons are the first column of icons; find the eraser's.
    let mut opened = false;
    for y in (40..800).step_by(6) {
        h.click_screen(Pos2::new(20.0, y as f32));
        if h.app.tool == Tool::Eraser {
            opened = h.app.workspace.find_tab(&layout::Pane::Tool).is_some();
            break;
        }
    }
    assert!(opened, "tool {:?}", h.app.tool);
}

#[test]
fn settling_is_a_brush_setting() {
    let mut h = Harness::new(400, 300);
    h.use_tool(Tool::Brush);
    let pen = h.app.selected_brush;
    h.app.brushes[pen].settle = false;
    h.app.size = 10.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.app.pending_provisional.is_empty());
    assert!(h.pixel(200, 150)[3] > 150);
    h.use_tool(Tool::Eraser);
    assert!(h.app.brushes[h.app.selected_brush].settle);
}

#[test]
fn moving_a_selection_over_paint_neither_drags_nor_erases_it() {
    let mut h = Harness::new(400, 300);
    // A red square, a green bar in the way, and the move passes over it.
    h.fill_rect(40, 120, 80, 160, [220, 20, 20, 255]);
    h.fill_rect(150, 100, 170, 180, [0, 200, 0, 255]);
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(35.0, 115.0), v(85.0, 165.0)]);
    h.use_tool(Tool::Move);
    h.drag(&[v(60.0, 140.0), v(160.0, 140.0), v(300.0, 140.0)]);
    // The green bar is where it was, whole.
    for y in [105, 140, 175] {
        assert_eq!(h.pixel(160, y), [0, 200, 0, 255], "bar at y {y}");
    }
    // The red square arrived without green in it, and left nothing behind.
    assert_eq!(h.pixel(280, 140), [220, 20, 20, 255]);
    assert_eq!(h.pixel(60, 140)[3], 0);
    // One undo restores the start.
    h.app.history.undo_document(&mut h.app.doc);
    h.frames(1);
    assert_eq!(h.pixel(60, 140), [220, 20, 20, 255]);
    assert_eq!(h.pixel(280, 140)[3], 0);
}

#[test]
fn selection_outline_is_four_runs_around_a_rectangle() {
    let (w, h) = (10usize, 10usize);
    let mut mask = vec![0u8; w * h];
    for y in 3..7 {
        for x in 2..6 {
            mask[y * w + x] = 255;
        }
    }
    let mut runs = crate::canvas_view::selection_outline(&mask, (w, h), (0..w, 0..h), 1);
    runs.sort_by(|a, b| {
        (a.0.x, a.0.y, a.1.x)
            .partial_cmp(&(b.0.x, b.0.y, b.1.x))
            .unwrap()
    });
    let expected = vec![
        (v(2.0, 3.0), v(2.0, 7.0)),
        (v(2.0, 3.0), v(6.0, 3.0)),
        (v(2.0, 7.0), v(6.0, 7.0)),
        (v(6.0, 3.0), v(6.0, 7.0)),
    ];
    assert_eq!(runs, expected);
    // A whole-canvas selection is outlined along the canvas border.
    let all = vec![255u8; w * h];
    let runs = crate::canvas_view::selection_outline(&all, (w, h), (0..w, 0..h), 3);
    assert_eq!(runs.len(), 4, "{runs:?}");
    assert!(
        runs.iter()
            .all(|(a, b)| a.x.max(b.x) <= 10.0 && a.y.max(b.y) <= 10.0)
    );
}

fn ctrl_key(h: &mut Harness, key: egui::Key, shift: bool) {
    let modifiers = if shift {
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
    } else {
        egui::Modifiers::COMMAND
    };
    h.frame_with(vec![egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]);
}

#[test]
fn common_shortcuts_select_copy_paste_and_undo() {
    let mut h = Harness::new(200, 100);
    h.fill_rect(10, 10, 40, 40, [0, 0, 255, 255]);
    ctrl_key(&mut h, egui::Key::A, false);
    assert!(h.app.selection.active);
    assert!(h.selected(150, 90));
    ctrl_key(&mut h, egui::Key::D, false);
    assert!(!h.app.selection.active);
    // Ctrl+C / Ctrl+V arrive as copy and paste events.
    h.use_tool(Tool::RectangleSelect);
    h.drag(&[v(5.0, 5.0), v(45.0, 45.0)]);
    ctrl_key(&mut h, egui::Key::I, true);
    assert!(!h.selected(20, 20) && h.selected(150, 90), "inverted");
    ctrl_key(&mut h, egui::Key::I, true);
    h.frame_with(vec![egui::Event::Copy]);
    assert!(h.app.clipboard.is_some(), "copied");
    h.frame_with(vec![egui::Event::Paste(String::new())]);
    assert!(h.app.paste_preview.is_some(), "pasting");
    h.app.paste_preview = None;
    // Undo and redo, also Ctrl+Shift+Z.
    h.frame_with(vec![egui::Event::Cut]);
    h.frames(1);
    assert_eq!(h.pixel(20, 20)[3], 0, "cut");
    ctrl_key(&mut h, egui::Key::Z, false);
    assert_eq!(h.pixel(20, 20)[3], 255, "undone");
    ctrl_key(&mut h, egui::Key::Z, true);
    assert_eq!(h.pixel(20, 20)[3], 0, "redone");
    // New layer, duplicate layer.
    let layers = h.app.doc.layers.len();
    ctrl_key(&mut h, egui::Key::N, true);
    assert_eq!(h.app.doc.layers.len(), layers + 1);
    ctrl_key(&mut h, egui::Key::J, false);
    assert_eq!(h.app.doc.layers.len(), layers + 2);
    ctrl_key(&mut h, egui::Key::N, false);
    assert!(h.app.show_new_document);
}

#[test]
fn ctrl_plus_and_minus_zoom_the_canvas_on_any_layout() {
    let mut h = Harness::new(200, 100);
    let start = h.app.zoom;
    ctrl_key(&mut h, egui::Key::Plus, true);
    assert!(h.app.zoom > start);
    // Japanese keyboards: Ctrl and the ";+" key.
    let before = h.app.zoom;
    ctrl_key(&mut h, egui::Key::Semicolon, false);
    assert!(h.app.zoom > before);
    let before = h.app.zoom;
    ctrl_key(&mut h, egui::Key::Minus, false);
    assert!(h.app.zoom < before);
    ctrl_key(&mut h, egui::Key::Num0, false);
    assert_eq!(h.app.zoom, 1.0);
}

#[test]
fn delete_all_layers_leaves_one_empty_layer_and_undoes_in_one_step() {
    let mut h = Harness::new(200, 100);
    h.fill_rect(10, 10, 40, 40, [0, 0, 255, 255]);
    h.app.add_raster_layer();
    h.app.add_raster_layer();
    let layers = h.app.doc.layers.len();
    h.app.clear_all_layers();
    h.frames(1);
    assert_eq!(h.app.doc.layers.len(), 1);
    assert_eq!(h.app.selected_layer, 0);
    assert_eq!(h.pixel(20, 20)[3], 0);
    h.app.undo();
    h.frames(1);
    assert_eq!(h.app.doc.layers.len(), layers);
    assert_eq!(h.app.doc.layers[0].pixels.pixel(20, 20)[3], 255);
}

#[test]
fn the_shipped_presets_load() {
    let shipped = efude_brush::set_from_bytes(crate::DEFAULT_PRESETS).expect("preset set loads");
    assert_eq!(shipped.len(), 30);
    assert!(shipped.iter().all(|brush| !brush.name.is_empty()));
}

#[test]
fn new_installations_get_three_columns_of_presets() {
    let presets = crate::default_presets();
    let one = efude_brush::defaults();
    assert_eq!(presets.len(), one.len() * 3);
    for (index, brush) in presets.iter().enumerate() {
        assert_eq!(brush.name, one[index % one.len()].name);
    }
    assert_eq!(EfudeApp::default().brushes.len(), presets.len());
}

#[test]
fn presets_can_be_renamed_one_by_one() {
    let mut h = Harness::new(200, 100);
    h.app.brush_rename = Some((10, "Gペン".into()));
    h.frames(1);
    // Confirm with Enter.
    h.frame_with(vec![egui::Event::Key {
        key: egui::Key::Enter,
        physical_key: Some(egui::Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.frames(1);
    assert!(h.app.brush_rename.is_none());
    assert_eq!(h.app.brushes[10].name, "Gペン");
    assert_eq!(h.app.brushes[0].name, "ペン");
}

#[test]
fn filter_windows_preview_and_apply_on_the_worker() {
    let mut h = Harness::new(120, 80);
    h.fill_rect(0, 0, 60, 80, [0, 0, 0, 255]);
    h.fill_rect(60, 0, 120, 80, [255, 255, 255, 255]);
    h.app.filter_dialog = Some(crate::filters_ui::FilterDialog::open(
        crate::filters_ui::FilterKind::Blur,
    ));
    h.frames(2);
    assert!(
        h.app.filter_dialog.as_ref().unwrap().has_preview(),
        "preview made"
    );
    // Apply, then wait for the worker.
    let operation = h
        .app
        .filter_operation_for_tests(crate::filters_ui::FilterKind::Blur);
    h.app.queue_filter(operation, &h.ctx.clone());
    assert!(h.app.io_task_sender.busy.get() > 0);
    for _ in 0..500 {
        h.frames(1);
        if !h.app.filter_pending {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.app.filter_pending);
    assert_eq!(h.app.io_task_sender.busy.get(), 0);
    let edge = h.pixel(59, 40)[0];
    assert!(edge > 20 && edge < 235, "blurred edge {edge}");
    h.app.undo();
    h.frames(1);
    assert_eq!(h.pixel(59, 40)[0], 0);
}

#[test]
fn the_loading_animation_decodes() {
    assert!(crate::filters_ui::loading_animation_frames() > 100);
}

#[test]
fn zoom_levels_show_pixels_at_that_size() {
    let mut h = Harness::new(400, 300);
    for level in [10.0, 12.5, 50.0, 100.0, 300.0, 500.0] {
        h.app.set_zoom_percent(level);
        h.frames(1);
        let (_, scale) = h.app.canvas_screen.unwrap();
        let physical = scale * h.ctx.pixels_per_point() * 100.0;
        assert!((physical - level).abs() < 0.01, "{level}: {physical}");
        assert!((h.app.zoom_percent() - level).abs() < 0.01);
    }
}

#[test]
fn vector_layers_keep_lines_that_can_be_erased_moved_and_undone() {
    let mut h = Harness::new(400, 300);
    h.app.add_vector_layer();
    h.frames(1);
    assert!(h.app.doc.layers[h.app.selected_layer].is_vector());
    h.use_tool(Tool::Brush);
    h.app.selected_brush = 0;
    h.app.size = 8.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    let strokes = |h: &Harness| {
        h.app.doc.layers[h.app.selected_layer]
            .vector
            .clone()
            .unwrap()
    };
    assert_eq!(strokes(&h).len(), 1);
    for x in [80, 200, 320] {
        assert!(h.pixel(x, 150)[3] > 200, "x {x}: {:?}", h.pixel(x, 150));
    }
    // The pixels are exactly the drawing of the kept line.
    let mut redrawn = h.app.doc.layers[h.app.selected_layer].clone();
    efude_canvas::vector::render_all(&mut redrawn, 400, 300, None);
    assert_eq!(
        redrawn.pixels.to_dense(),
        h.app.doc.layers[h.app.selected_layer].pixels.to_dense()
    );
    // The eraser cuts the line in two.
    h.use_tool(Tool::Eraser);
    h.app.size = 20.0;
    h.drag(&[v(200.0, 100.0), v(200.0, 200.0)]);
    assert_eq!(strokes(&h).len(), 2);
    assert_eq!(h.pixel(200, 150)[3], 0);
    assert!(h.pixel(100, 150)[3] > 200);
    // Whole-line erasing removes a line it touches.
    h.app.vector_erase_whole = true;
    h.drag(&[v(100.0, 130.0), v(100.0, 170.0)]);
    assert_eq!(strokes(&h).len(), 1);
    assert_eq!(h.pixel(100, 150)[3], 0);
    assert!(h.pixel(300, 150)[3] > 200);
    // Moving moves the line itself.
    h.use_tool(Tool::Move);
    h.drag(&[v(300.0, 150.0), v(300.0, 200.0)]);
    assert!(strokes(&h)[0].points.iter().all(|p| p.y > 190.0));
    assert!(h.pixel(300, 200)[3] > 200);
    assert_eq!(h.pixel(300, 150)[3], 0);
    // Raster-only tools leave it alone.
    h.use_tool(Tool::Fill);
    h.click(v(20.0, 20.0));
    assert_eq!(h.pixel(20, 20)[3], 0);
    // Undo walks back through the move and both erasures.
    for _ in 0..3 {
        h.app.undo();
    }
    h.frames(1);
    assert_eq!(strokes(&h).len(), 1);
    assert!(h.pixel(200, 150)[3] > 200);
    // Rasterizing keeps the picture and drops the lines.
    let index = h.app.selected_layer;
    h.app.rasterize_layer(index);
    assert!(!h.app.doc.layers[index].is_vector());
    assert!(h.pixel(200, 150)[3] > 200);
    h.app.undo();
    assert!(h.app.doc.layers[index].is_vector());
}

#[test]
fn rulers_draw_vector_lines() {
    let mut h = Harness::new(400, 300);
    h.app.add_vector_layer();
    h.use_tool(Tool::Line);
    h.app.size = 6.0;
    h.drag(&[v(50.0, 50.0), v(350.0, 250.0)]);
    let layer = &h.app.doc.layers[h.app.selected_layer];
    assert_eq!(layer.vector.as_ref().unwrap().len(), 1);
    assert!(h.pixel(200, 150)[3] > 200);
}

#[test]
fn dragging_a_layer_onto_a_folder_puts_it_inside_and_the_picture_follows() {
    let mut h = Harness::new(200, 100);
    // Bottom to top: layer 1 (blue), folder, layer 3 (red).
    h.fill_rect(0, 0, 200, 100, [0, 0, 255, 255]);
    h.app.doc.layers.push({
        let mut folder = efude_canvas::Layer::new(2, "folder", 200, 100);
        folder.kind = LayerKind::Folder;
        folder
    });
    h.app.doc.layers.push({
        let mut top = efude_canvas::Layer::new(3, "top", 200, 100);
        for y in 0..100 {
            for x in 0..200 {
                top.pixels.set_pixel(x, y, [255, 0, 0, 255]);
            }
        }
        top
    });
    // Show the Layers tab.
    if let Some(tab) = h.app.workspace.find_tab(&layout::Pane::Layers) {
        h.app.workspace.set_active_tab(tab);
    }
    h.frames(2);
    let row = |h: &Harness, id: u64| {
        h.app
            .layer_rows
            .iter()
            .find(|(row_id, _)| *row_id == id)
            .map(|(_, rect)| *rect)
            .expect("row drawn")
    };
    // Drag the bottom layer onto the middle of the folder row.
    let from = row(&h, 1).center();
    let to = row(&h, 2).center();
    let path = (0..=10)
        .map(|i| from + (to - from) * (i as f32 / 10.0))
        .collect::<Vec<_>>();
    h.drag_screen(&path, egui::PointerButton::Primary);
    let ids = h.app.doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(h.app.doc.layers[0].parent_id, Some(2));
    // Hide the folder: its contents disappear, the top layer stays.
    h.app.doc.layers[1].visible = false;
    h.app.doc.layers[2].visible = false;
    assert_eq!(efude_canvas::composite_transparent(&h.app.doc)[3], 0);
    h.app.doc.layers[1].visible = true;
    h.app.doc.layers[2].visible = true;
    // Drag it above the top layer: out of the folder, now on top.
    h.frames(1);
    let from = row(&h, 1).center();
    let top = row(&h, 3);
    let to = Pos2::new(top.center().x, top.top() + 3.0);
    let path = (0..=10)
        .map(|i| from + (to - from) * (i as f32 / 10.0))
        .collect::<Vec<_>>();
    h.drag_screen(&path, egui::PointerButton::Primary);
    let ids = h.app.doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![2, 3, 1]);
    assert_eq!(h.app.doc.layers[2].parent_id, None);
    assert_eq!(&efude_canvas::composite(&h.app.doc)[..4], &[0, 0, 255, 255]);
    h.app.undo();
    h.frames(1);
    let ids = h.app.doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn new_child_layers_stack_inside_their_folder() {
    let mut h = Harness::new(100, 100);
    h.app.doc.layers.push({
        let mut folder = efude_canvas::Layer::new(2, "folder", 100, 100);
        folder.kind = LayerKind::Folder;
        folder
    });
    h.app
        .doc
        .layers
        .push(efude_canvas::Layer::new(3, "top", 100, 100));
    // A child added last would sit above "top" in the stack; it is put
    // inside the folder, directly below it.
    let mut child = efude_canvas::Layer::new(4, "child", 100, 100);
    child.parent_id = Some(2);
    h.app.history.insert_layer(&mut h.app.doc.layers, 3, child);
    h.app.selected_layer = 3;
    h.frames(1);
    let ids = h.app.doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 4, 2, 3]);
    assert_eq!(h.app.doc.layers[h.app.selected_layer].id, 4);
    h.app.undo();
    h.frames(1);
    let ids = h.app.doc.layers.iter().map(|l| l.id).collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 2, 3]);
}

#[test]
fn the_transparent_colour_erases_with_brushes_and_fills() {
    let mut h = Harness::new(400, 300);
    h.fill_rect(0, 0, 400, 300, [200, 30, 30, 255]);
    h.app.transparent_color = true;
    h.use_tool(Tool::Brush);
    h.app.selected_brush = 0;
    h.app.size = 16.0;
    h.drag(&[v(60.0, 150.0), v(340.0, 150.0)]);
    assert!(h.pixel(200, 150)[3] < 40, "{:?}", h.pixel(200, 150));
    assert_eq!(h.pixel(200, 60)[3], 255);
    // A fill clears the region it floods.
    h.use_tool(Tool::Fill);
    h.click(v(200.0, 40.0));
    assert_eq!(h.pixel(200, 40)[3], 0);
    // Picking a colour ends it.
    h.fill_rect(0, 0, 400, 300, [10, 200, 90, 255]);
    h.use_tool(Tool::Eyedropper);
    h.click(v(100.0, 100.0));
    assert!(!h.app.transparent_color);
}

#[test]
fn transparent_parts_show_a_checkerboard_on_screen_only() {
    let doc = Document::new(640, 640);
    let size = efude_canvas::checker_size(640, 640);
    let shown = efude_canvas::composite_display(&doc, size);
    let at = |x: u32, y: u32| shown[((y * 640 + x) * 4) as usize];
    assert_eq!(at(0, 0), 255);
    assert_eq!(at(size, 0), efude_canvas::CHECKER_GREY);
    assert_eq!(at(size, size), 255);
    // Tiles agree with the whole picture, and exports stay white.
    let (_, _, tile) = efude_canvas::composite_tile_display(&doc, 1, 0, size).unwrap();
    assert_eq!(tile[0], at(256, 0));
    assert!(efude_canvas::composite(&doc).iter().all(|&v| v == 255));
}

#[test]
fn control_points_of_vector_lines_can_be_dragged() {
    let mut h = Harness::new(400, 300);
    h.app.add_vector_layer();
    h.use_tool(Tool::Line);
    h.app.size = 6.0;
    h.drag(&[v(50.0, 150.0), v(350.0, 150.0)]);
    let line = |h: &Harness| {
        h.app.doc.layers[h.app.selected_layer]
            .vector
            .clone()
            .unwrap()[0]
            .clone()
    };
    // A drawn line is kept as a curve: a straight one has two anchors.
    assert_eq!(line(&h).anchors.len(), 2);
    let end = line(&h).anchors[1];
    h.use_tool(Tool::VectorEdit);
    // Select the line, then drag its end anchor down.
    h.click(v(200.0, 150.0));
    assert_eq!(h.app.vector_edit.stroke, Some(0));
    h.drag(&[v(end.x, end.y), v(end.x, end.y + 80.0)]);
    let moved = line(&h);
    assert!(
        (moved.anchors[1].y - (end.y + 80.0)).abs() < 1.5,
        "{:?}",
        moved.anchors[1]
    );
    assert!(h.pixel(end.x as u32 - 2, end.y as u32 + 79)[3] > 150);
    assert_eq!(h.pixel(end.x as u32 - 2, end.y as u32)[3], 0);
    // Dragging a handle bends the line between the anchors.
    let start = moved.anchors[0];
    let handle = v(start.x + start.out_x, start.y + start.out_y);
    h.drag(&[handle, handle + v(0.0, -120.0)]);
    let bent = line(&h);
    let top = bent.points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    assert!(top < 140.0, "the curve did not bend: {top}");
    // Dragging the line itself moves all of it.
    let mid = bent.points[bent.points.len() / 2];
    h.drag(&[v(mid.x, mid.y), v(mid.x - 20.0, mid.y)]);
    assert!((line(&h).anchors[0].x - (start.x - 20.0)).abs() < 1.5);
    // Each drag is one Undo step.
    for _ in 0..3 {
        h.app.undo();
    }
    h.frames(1);
    assert_eq!(line(&h).anchors, vec![line(&h).anchors[0], end]);
    assert!(h.pixel(200, 150)[3] > 150);
    // Delete removes the selected line.
    h.click(v(200.0, 150.0));
    h.key(egui::Key::Delete, true);
    h.key(egui::Key::Delete, false);
    assert!(
        h.app.doc.layers[h.app.selected_layer]
            .vector
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert_eq!(h.pixel(200, 150)[3], 0);
}
