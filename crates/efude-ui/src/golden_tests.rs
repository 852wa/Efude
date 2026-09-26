// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Brush feel regression test: replays input logs through the real stroke
//! path with every built-in brush and compares the result with reference
//! images in `tests/golden/`.
//!
//! - Input logs: `tests/golden/logs/*.json` (the format saved by the
//!   "Save Input Log" button). Any log dropped there is picked up.
//! - Reference images: `tests/golden/<log name>.png`, one row per brush.
//! - After an intended change, regenerate them with
//!   `EFUDE_BLESS=1 cargo test -p efude-ui golden` and review the images.
//! - On a mismatch the actual image and a difference map are written to
//!   `target/golden-out/`.

use super::*;
use std::path::{Path, PathBuf};

const CELL_WIDTH: u32 = 256;
const CELL_HEIGHT: u32 = 160;
/// Channel difference ignored (floating-point libraries differ slightly
/// between platforms).
const TOLERANCE: u8 = 3;
/// Share of pixels allowed to exceed `TOLERANCE`.
const MAX_DIFFERING_SHARE: f64 = 0.002;
/// Any pixel off by more than this fails the test.
const MAX_DIFFERENCE: u8 = 48;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn blessing() -> bool {
    std::env::var_os("EFUDE_BLESS").is_some_and(|value| value != "0")
}

/// The four standard strokes: a slow curve, fast hatching, a pressure
/// swell with tapered ends, and small handwriting-like loops. Coordinates
/// fit a `CELL_WIDTH`×`CELL_HEIGHT` canvas; time is in milliseconds.
fn standard_logs() -> Vec<(&'static str, efude_input::StrokeLog)> {
    fn log(strokes: Vec<Vec<InkPoint>>) -> efude_input::StrokeLog {
        let mut log = efude_input::StrokeLog::default();
        for stroke in strokes {
            log.push(&stroke);
        }
        log
    }
    let point = |x: f32, y: f32, pressure: f32, time: u64| InkPoint::new(x, y, pressure, time);

    // 1.5 s along an S-curve, 4 ms samples, steady medium pressure.
    let slow_curve: Vec<InkPoint> = (0..=375)
        .map(|i| {
            let t = i as f32 / 375.0;
            point(
                20.0 + 216.0 * t,
                80.0 + 45.0 * (t * std::f32::consts::TAU).sin(),
                0.55 + 0.1 * (t * 9.0).sin(),
                i * 4,
            )
        })
        .collect();

    // Eight quick diagonal strokes, 60 ms each, pressure rising then falling.
    let hatching = (0..8)
        .map(|n| {
            let x0 = 30.0 + n as f32 * 26.0;
            (0..=15)
                .map(|i| {
                    let t = i as f32 / 15.0;
                    point(
                        x0 + 30.0 * t,
                        130.0 - 100.0 * t,
                        0.2 + 0.7 * (t * std::f32::consts::PI).sin(),
                        n as u64 * 400 + i * 4,
                    )
                })
                .collect()
        })
        .collect();

    // One long stroke: pressure 0 → 1 → 0, for tapers and pressure curves.
    let pressure_swell = vec![
        (0..=250)
            .map(|i| {
                let t = i as f32 / 250.0;
                point(
                    24.0 + 208.0 * t,
                    90.0 - 30.0 * t,
                    (t * std::f32::consts::PI).sin().max(0.0).powf(1.5),
                    i * 4,
                )
            })
            .collect(),
    ];

    // Small loops, like fast handwriting (thin-line quality at speed).
    let small_text = (0..4)
        .map(|n| {
            let cx = 45.0 + n as f32 * 55.0;
            (0..=90)
                .map(|i| {
                    let t = i as f32 / 90.0;
                    let angle = t * std::f32::consts::TAU * 2.0;
                    point(
                        cx - 18.0 + 36.0 * t + 8.0 * angle.cos(),
                        80.0 + 12.0 * angle.sin(),
                        0.35 + 0.4 * (t * 5.0).sin().abs(),
                        n as u64 * 1000 + i * 3,
                    )
                })
                .collect()
        })
        .collect();

    vec![
        ("slow_curve", log(vec![slow_curve])),
        ("hatching", log(hatching)),
        ("pressure_swell", log(pressure_swell)),
        ("small_text", log(small_text)),
    ]
}

/// Moves and scales recorded strokes to fit one cell, keeping their shape.
/// Scaling changes pen speed, so it is uniform and deterministic.
fn fit_to_cell(strokes: &mut [Vec<InkPoint>]) {
    let points = strokes.iter().flatten();
    let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    for p in points {
        min = min.min(p.position);
        max = max.max(p.position);
    }
    if min.x > max.x {
        return;
    }
    let margin = 16.0;
    let available = glam::Vec2::new(CELL_WIDTH as f32, CELL_HEIGHT as f32) - 2.0 * margin;
    let extent = (max - min).max(glam::Vec2::ONE);
    let scale = (available / extent).min_element().min(1.0);
    let offset = glam::Vec2::splat(margin) + (available - extent * scale) * 0.5;
    for p in strokes.iter_mut().flatten() {
        p.position = (p.position - min) * scale + offset;
    }
}

/// Canvas under the strokes: colour bands with transparent gaps, so wet
/// brushes, blur, smudge and the eraser all have something to act on.
fn background(doc: &mut Document) {
    let bands: [(u32, u32, [u8; 4]); 3] = [
        (40, 90, [210, 40, 40, 255]),
        (120, 170, [40, 80, 210, 255]),
        (200, 240, [240, 200, 40, 160]),
    ];
    for (x0, x1, color) in bands {
        for y in 0..doc.height {
            for x in x0..x1.min(doc.width) {
                doc.layers[0].pixels.set_pixel(x, y, color);
            }
        }
    }
}

fn render(brush_index: usize, strokes: &[Vec<InkPoint>]) -> Vec<u8> {
    let mut app = EfudeApp::default();
    app.doc = Document::new(CELL_WIDTH, CELL_HEIGHT);
    app.selected_layer = 0;
    app.tool = Tool::Brush;
    app.selected_brush = brush_index;
    app.size = app.brushes[brush_index].size;
    app.color = Color32::from_rgb(30, 110, 60);
    background(&mut app.doc);
    for stroke in strokes {
        app.reset_stroke_buffers();
        app.history.begin();
        app.render_replay(stroke);
        app.history.commit();
    }
    app.doc.layers[0].pixels.to_dense()
}

fn sheet(strokes: &[Vec<InkPoint>]) -> image::RgbaImage {
    let brushes = efude_brush::defaults().len();
    let mut sheet = image::RgbaImage::new(CELL_WIDTH, CELL_HEIGHT * brushes as u32);
    for index in 0..brushes {
        let pixels = render(index, strokes);
        let cell = image::RgbaImage::from_raw(CELL_WIDTH, CELL_HEIGHT, pixels).unwrap();
        image::imageops::replace(&mut sheet, &cell, 0, (index as u32 * CELL_HEIGHT) as i64);
    }
    sheet
}

/// Differences between two sheets, per brush row, or `None` if they match.
fn compare(expected: &image::RgbaImage, actual: &image::RgbaImage) -> Option<String> {
    if expected.dimensions() != actual.dimensions() {
        return Some(format!(
            "size {:?} differs from reference {:?}",
            actual.dimensions(),
            expected.dimensions()
        ));
    }
    let names: Vec<String> = efude_brush::defaults()
        .into_iter()
        .map(|b| b.name)
        .collect();
    let mut report = Vec::new();
    for (row, name) in names.iter().enumerate() {
        let (mut differing, mut worst) = (0usize, 0u8);
        for y in row as u32 * CELL_HEIGHT..(row as u32 + 1) * CELL_HEIGHT {
            for x in 0..CELL_WIDTH {
                let (a, b) = (expected.get_pixel(x, y).0, actual.get_pixel(x, y).0);
                let difference = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                worst = worst.max(difference);
                if difference > TOLERANCE {
                    differing += 1;
                }
            }
        }
        let share = differing as f64 / (CELL_WIDTH * CELL_HEIGHT) as f64;
        if share > MAX_DIFFERING_SHARE || worst > MAX_DIFFERENCE {
            report.push(format!("{name}: {differing} px differ (worst {worst})"));
        }
    }
    (!report.is_empty()).then(|| report.join(", "))
}

fn difference_map(expected: &image::RgbaImage, actual: &image::RgbaImage) -> image::RgbaImage {
    image::RgbaImage::from_fn(actual.width(), actual.height(), |x, y| {
        let (a, b) = (expected.get_pixel(x, y).0, actual.get_pixel(x, y).0);
        let difference = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
        let level = (difference as u32 * 4).min(255) as u8;
        image::Rgba([255, 255 - level, 255 - level, 255])
    })
}

#[test]
fn golden_brush_strokes_match_reference_images() {
    let dir = golden_dir();
    let logs_dir = dir.join("logs");
    if blessing() {
        std::fs::create_dir_all(&logs_dir).unwrap();
        for (name, log) in standard_logs() {
            let path = logs_dir.join(format!("{name}.json"));
            if !path.exists() {
                log.save(&path).unwrap();
            }
        }
    }
    let mut logs: Vec<PathBuf> = std::fs::read_dir(&logs_dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default();
    logs.sort();
    assert!(
        !logs.is_empty(),
        "no input logs in {}; run with EFUDE_BLESS=1 to create the standard set",
        logs_dir.display()
    );
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/golden-out");
    let mut failures = Vec::new();
    for log_path in logs {
        let name = log_path.file_stem().unwrap().to_string_lossy().into_owned();
        let log = efude_input::StrokeLog::load(&log_path).unwrap();
        let mut strokes: Vec<Vec<InkPoint>> = (0..log.strokes.len())
            .filter_map(|index| log.replay(index))
            .filter(|stroke| !stroke.is_empty())
            .collect();
        fit_to_cell(&mut strokes);
        let actual = sheet(&strokes);
        let reference_path = dir.join(format!("{name}.png"));
        if blessing() {
            actual.save(&reference_path).unwrap();
            continue;
        }
        let Ok(expected) = image::open(&reference_path) else {
            failures.push(format!(
                "{name}: no reference image (run with EFUDE_BLESS=1)"
            ));
            continue;
        };
        let expected = expected.to_rgba8();
        if let Some(report) = compare(&expected, &actual) {
            std::fs::create_dir_all(&out_dir).unwrap();
            actual.save(out_dir.join(format!("{name}.png"))).unwrap();
            difference_map(&expected, &actual)
                .save(out_dir.join(format!("{name}-diff.png")))
                .unwrap();
            failures.push(format!("{name}: {report}"));
        }
    }
    assert!(
        failures.is_empty(),
        "brush output changed (images in target/golden-out; if intended, re-run with EFUDE_BLESS=1):\n{}",
        failures.join("\n")
    );
}

#[test]
fn recorded_logs_fit_the_test_canvas() {
    let mut strokes = vec![vec![
        InkPoint::new(1000.0, 2000.0, 1.0, 0),
        InkPoint::new(3000.0, 2500.0, 1.0, 4),
    ]];
    fit_to_cell(&mut strokes);
    for p in &strokes[0] {
        assert!(p.position.x >= 16.0 && p.position.x <= CELL_WIDTH as f32 - 16.0);
        assert!(p.position.y >= 16.0 && p.position.y <= CELL_HEIGHT as f32 - 16.0);
    }
}
