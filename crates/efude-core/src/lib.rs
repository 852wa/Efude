// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use glam::Vec2;
pub const TILE_SIZE: u32 = 256;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InkPoint {
    pub position: Vec2,
    pub pressure: f32,
    /// Per-point start/end taper multiplier for size and opacity.
    pub taper: f32,
    pub tilt: Vec2,
    /// Stylus barrel rotation in radians, when the input device provides it.
    pub rotation: f32,
    pub time_ms: u64,
}
impl InkPoint {
    pub fn new(x: f32, y: f32, pressure: f32, time_ms: u64) -> Self {
        Self {
            position: Vec2::new(x, y),
            pressure: pressure.clamp(0., 1.),
            taper: 1.0,
            tilt: Vec2::ZERO,
            rotation: 0.0,
            time_ms,
        }
    }
}
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.max(0.).powf(1. / 2.4) - 0.055
    }
}
