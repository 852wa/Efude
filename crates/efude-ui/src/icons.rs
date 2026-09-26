// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Tool icons, drawn with simple vector shapes (no image or font assets).

use super::*;

/// Draws the icon of `tool` inside `rect` (about 18×18 points).
pub(crate) fn paint_tool_icon(painter: &egui::Painter, rect: Rect, tool: Tool, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let thin = Stroke::new(1.2, color);
    // Coordinates below are in a 0..1 box mapped onto `rect`.
    let at = |x: f32, y: f32| {
        Pos2::new(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let s = rect.width();
    let line = |points: &[(f32, f32)], stroke: Stroke| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let closed = |points: &[(f32, f32)], stroke: Stroke| {
        painter.add(egui::Shape::closed_line(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            stroke,
        ));
    };
    let filled = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::convex_polygon(
            points.iter().map(|&(x, y)| at(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let dashed = |points: &[(f32, f32)]| {
        let mut path: Vec<Pos2> = points.iter().map(|&(x, y)| at(x, y)).collect();
        path.push(path[0]);
        painter.extend(egui::Shape::dashed_line(&path, thin, 2.5, 2.0));
    };
    match tool {
        Tool::Brush => {
            // Handle, ferrule and a pointed tip.
            line(&[(0.95, 0.05), (0.52, 0.48)], stroke);
            filled(&[
                (0.46, 0.44),
                (0.56, 0.54),
                (0.30, 0.86),
                (0.08, 0.95),
                (0.16, 0.72),
            ]);
        }
        Tool::Eraser => {
            closed(
                &[(0.10, 0.62), (0.52, 0.20), (0.90, 0.58), (0.48, 0.98)],
                stroke,
            );
            filled(&[(0.10, 0.62), (0.30, 0.42), (0.68, 0.80), (0.48, 0.98)]);
            line(&[(0.48, 0.98), (0.98, 0.98)], thin);
        }
        Tool::Blur => {
            // A drop.
            painter.circle_stroke(at(0.5, 0.64), s * 0.30, stroke);
            line(&[(0.24, 0.50), (0.5, 0.05), (0.76, 0.50)], stroke);
            painter.circle_filled(at(0.40, 0.66), s * 0.06, color);
        }
        Tool::Smudge => {
            // A fingertip dragging a trail.
            line(
                &[(0.05, 0.85), (0.28, 0.70), (0.50, 0.72), (0.72, 0.55)],
                thin,
            );
            closed(
                &[
                    (0.55, 0.52),
                    (0.80, 0.20),
                    (0.95, 0.30),
                    (0.75, 0.68),
                    (0.60, 0.70),
                ],
                stroke,
            );
        }
        Tool::Fill => {
            // A tipped bucket with a drop.
            closed(
                &[(0.12, 0.45), (0.45, 0.12), (0.80, 0.47), (0.47, 0.80)],
                stroke,
            );
            line(&[(0.12, 0.45), (0.80, 0.47)], thin);
            painter.circle_filled(at(0.88, 0.80), s * 0.09, color);
        }
        Tool::Eyedropper => {
            line(&[(0.10, 0.90), (0.55, 0.45)], stroke);
            filled(&[(0.50, 0.38), (0.62, 0.50), (0.78, 0.34), (0.66, 0.22)]);
            painter.circle_filled(at(0.80, 0.20), s * 0.14, color);
        }
        Tool::VectorEdit => {
            let curve: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    (
                        u * u * u * 0.08
                            + 3.0 * u * u * t * 0.20
                            + 3.0 * u * t * t * 0.62
                            + t * t * t * 0.92,
                        u * u * u * 0.85
                            + 3.0 * u * u * t * 0.10
                            + 3.0 * u * t * t * 0.95
                            + t * t * t * 0.25,
                    )
                })
                .collect();
            line(&curve, stroke);
            line(&[(0.20, 0.10), (0.50, 0.55), (0.80, 1.0)], thin);
            filled(&[(0.42, 0.47), (0.58, 0.47), (0.58, 0.63), (0.42, 0.63)]);
            painter.circle_filled(at(0.20, 0.10), s * 0.07, color);
            painter.circle_filled(at(0.80, 0.98), s * 0.07, color);
        }
        Tool::Move => {
            line(&[(0.5, 0.02), (0.5, 0.98)], stroke);
            line(&[(0.02, 0.5), (0.98, 0.5)], stroke);
            for (tip, a, b) in [
                ((0.5, 0.0), (0.36, 0.16), (0.64, 0.16)),
                ((0.5, 1.0), (0.36, 0.84), (0.64, 0.84)),
                ((0.0, 0.5), (0.16, 0.36), (0.16, 0.64)),
                ((1.0, 0.5), (0.84, 0.36), (0.84, 0.64)),
            ] {
                filled(&[tip, a, b]);
            }
        }
        Tool::Pan => {
            // Open hand: palm and four fingers.
            closed(
                &[
                    (0.22, 0.52),
                    (0.80, 0.52),
                    (0.76, 0.84),
                    (0.50, 0.98),
                    (0.30, 0.90),
                ],
                stroke,
            );
            for (x, top) in [(0.30, 0.18), (0.46, 0.06), (0.62, 0.10), (0.78, 0.26)] {
                line(&[(x, 0.52), (x, top)], stroke);
            }
            line(&[(0.22, 0.60), (0.06, 0.40)], stroke);
        }
        Tool::RectangleSelect => dashed(&[(0.05, 0.12), (0.95, 0.12), (0.95, 0.88), (0.05, 0.88)]),
        Tool::EllipseSelect => {
            let points: Vec<(f32, f32)> = (0..24)
                .map(|i| {
                    let a = i as f32 / 24.0 * std::f32::consts::TAU;
                    (0.5 + 0.46 * a.cos(), 0.5 + 0.38 * a.sin())
                })
                .collect();
            dashed(&points);
        }
        Tool::LassoSelect => {
            line(
                &[
                    (0.30, 0.80),
                    (0.10, 0.55),
                    (0.18, 0.22),
                    (0.50, 0.08),
                    (0.85, 0.20),
                    (0.92, 0.50),
                    (0.65, 0.70),
                    (0.36, 0.70),
                    (0.30, 0.80),
                    (0.38, 0.98),
                ],
                thin,
            );
        }
        Tool::PolygonSelect => dashed(&[
            (0.10, 0.25),
            (0.62, 0.05),
            (0.95, 0.55),
            (0.55, 0.95),
            (0.08, 0.72),
        ]),
        Tool::MagicWand => {
            line(&[(0.08, 0.95), (0.62, 0.40)], stroke);
            let (cx, cy, r) = (0.74, 0.26, 0.22);
            let star: Vec<(f32, f32)> = (0..8)
                .map(|i| {
                    let a = i as f32 / 8.0 * std::f32::consts::TAU;
                    let radius = if i % 2 == 0 { r } else { r * 0.4 };
                    (cx + radius * a.cos(), cy + radius * a.sin())
                })
                .collect();
            closed(&star, thin);
        }
        Tool::ColorRange => {
            for (x, y) in [(0.05, 0.05), (0.55, 0.05), (0.05, 0.55)] {
                filled(&[(x, y), (x + 0.38, y), (x + 0.38, y + 0.38), (x, y + 0.38)]);
            }
            closed(
                &[(0.57, 0.57), (0.95, 0.57), (0.95, 0.95), (0.57, 0.95)],
                thin,
            );
        }
        Tool::SelectionBrush => {
            line(&[(0.95, 0.05), (0.60, 0.40)], stroke);
            filled(&[(0.55, 0.36), (0.64, 0.45), (0.42, 0.70), (0.30, 0.62)]);
            dashed(&[(0.05, 0.55), (0.45, 0.80), (0.40, 0.98), (0.05, 0.98)]);
        }
        Tool::QuickMask => {
            closed(
                &[(0.05, 0.12), (0.95, 0.12), (0.95, 0.88), (0.05, 0.88)],
                stroke,
            );
            painter.circle_filled(at(0.5, 0.5), s * 0.24, color);
        }
        Tool::Line => {
            // A ruler with ticks.
            closed(
                &[(0.05, 0.70), (0.70, 0.05), (0.95, 0.30), (0.30, 0.95)],
                stroke,
            );
            for i in 1..5 {
                let t = i as f32 / 5.0;
                let (x, y) = (0.05 + 0.65 * t, 0.70 - 0.65 * t);
                line(&[(x, y), (x + 0.10, y + 0.10)], thin);
            }
        }
        Tool::EllipseRuler => {
            let points: Vec<(f32, f32)> = (0..28)
                .map(|i| {
                    let a = i as f32 / 28.0 * std::f32::consts::TAU;
                    (0.5 + 0.46 * a.cos(), 0.5 + 0.30 * a.sin())
                })
                .collect();
            closed(&points, stroke);
            painter.circle_filled(at(0.5, 0.5), s * 0.06, color);
        }
        Tool::BezierRuler => {
            let curve: Vec<(f32, f32)> = (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    let x = u * u * u * 0.05
                        + 3.0 * u * u * t * 0.25
                        + 3.0 * u * t * t * 0.75
                        + t * t * t * 0.95;
                    let y = u * u * u * 0.85
                        + 3.0 * u * u * t * 0.05
                        + 3.0 * u * t * t * 0.95
                        + t * t * t * 0.20;
                    (x, y)
                })
                .collect();
            line(&curve, stroke);
            line(&[(0.05, 0.85), (0.25, 0.05)], thin);
            line(&[(0.95, 0.20), (0.75, 0.95)], thin);
            painter.circle_filled(at(0.25, 0.05), s * 0.07, color);
            painter.circle_filled(at(0.75, 0.95), s * 0.07, color);
        }
        Tool::PerspectiveRuler => {
            painter.circle_filled(at(0.5, 0.18), s * 0.07, color);
            for x in [0.0, 0.33, 0.67, 1.0] {
                line(&[(0.5, 0.18), (x, 0.98)], thin);
            }
            line(&[(0.0, 0.98), (1.0, 0.98)], stroke);
        }
        Tool::Balloon => {
            let points: Vec<(f32, f32)> = (0..28)
                .map(|i| {
                    let a = i as f32 / 28.0 * std::f32::consts::TAU;
                    (0.5 + 0.44 * a.cos(), 0.4 + 0.32 * a.sin())
                })
                .collect();
            closed(&points, stroke);
            line(&[(0.36, 0.66), (0.22, 0.97), (0.52, 0.71)], stroke);
        }
        Tool::Text => {
            line(&[(0.12, 0.12), (0.88, 0.12)], stroke);
            line(&[(0.5, 0.12), (0.5, 0.92)], stroke);
            line(&[(0.12, 0.12), (0.12, 0.24)], thin);
            line(&[(0.88, 0.12), (0.88, 0.24)], thin);
            line(&[(0.36, 0.92), (0.64, 0.92)], thin);
        }
        Tool::PanelSplit => {
            closed(
                &[(0.05, 0.05), (0.95, 0.05), (0.95, 0.95), (0.05, 0.95)],
                thin,
            );
            line(&[(0.05, 0.62), (0.95, 0.38)], stroke);
            line(&[(0.05, 0.72), (0.95, 0.48)], stroke);
        }
    }
}

/// Curved arrow for undo (pointing left) or redo (`mirrored`).
pub(crate) fn paint_history_icon(
    painter: &egui::Painter,
    rect: Rect,
    mirrored: bool,
    color: Color32,
) {
    let stroke = Stroke::new(1.6, color);
    let flip = |x: f32| if mirrored { 1.0 - x } else { x };
    let at = |x: f32, y: f32| {
        Pos2::new(
            rect.left() + flip(x) * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let arc: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * (1.15 - i as f32 / 12.0 * 1.15);
            at(0.55 + 0.38 * a.cos(), 0.62 - 0.38 * a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(arc, stroke));
    painter.add(egui::Shape::convex_polygon(
        vec![at(0.02, 0.50), at(0.34, 0.44), at(0.16, 0.80)],
        color,
        Stroke::NONE,
    ));
}
