// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use efude_core::InkPoint;

mod builder;
pub use builder::{StrokeBuilder, StrokeParams, StrokeUpdate};

pub fn process(points: &[InkPoint], stabilization: usize, spacing: f32) -> Vec<InkPoint> {
    process_with_options(points, stabilization, spacing, 0.0, 0.0)
}

pub fn process_with_options(
    points: &[InkPoint],
    stabilization: usize,
    spacing: f32,
    pull_distance: f32,
    speed_adaptation: f32,
) -> Vec<InkPoint> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut smoothed = Vec::with_capacity(points.len());
    for i in 0..points.len() {
        let speed = if i == 0 {
            0.0
        } else {
            let elapsed = points[i]
                .time_ms
                .saturating_sub(points[i - 1].time_ms)
                .max(1) as f32;
            (points[i].position.distance(points[i - 1].position) / elapsed / 0.5).clamp(0.0, 1.0)
        };
        let local_stabilization = (stabilization as f32
            * (1.0 - speed_adaptation.clamp(0.0, 1.0) * speed))
            .round() as usize;
        let from = i.saturating_sub(local_stabilization);
        let slice = &points[from..=i];
        let (mut p, mut q) = (glam::Vec2::ZERO, 0.);
        for (j, item) in slice.iter().enumerate() {
            let w = (j + 1) as f32;
            p += item.position * w;
            q += item.pressure * w;
        }
        let weight = (slice.len() * (slice.len() + 1)) as f32 / 2.;
        let mut item = points[i];
        item.position = p / weight;
        item.pressure = q / weight;
        smoothed.push(item);
    }
    if pull_distance > 0.0 {
        let distance = pull_distance.clamp(0.0, 256.0);
        let mut pulled = Vec::with_capacity(smoothed.len() + 1);
        pulled.push(smoothed[0]);
        for point in smoothed.iter().copied().skip(1) {
            let previous = pulled.last().unwrap().position;
            let delta = point.position - previous;
            let length = delta.length();
            let mut constrained = point;
            if length > distance {
                constrained.position = previous + delta * ((length - distance) / length);
            }
            pulled.push(constrained);
        }
        smoothed = pulled;
    }
    let endpoint = *points.last().unwrap();
    let filtered_endpoint = *smoothed.last().unwrap();
    if endpoint.position.distance(filtered_endpoint.position) > f32::EPSILON
        || (endpoint.pressure - filtered_endpoint.pressure).abs() > f32::EPSILON
        || endpoint.tilt.distance(filtered_endpoint.tilt) > f32::EPSILON
        || (endpoint.rotation - filtered_endpoint.rotation).abs() > f32::EPSILON
    {
        smoothed.push(endpoint);
    }
    let mut out = vec![smoothed[0]];
    for i in 0..smoothed.len() - 1 {
        let (a, b) = (smoothed[i], smoothed[i + 1]);
        let before = smoothed[i.saturating_sub(1)];
        let after = smoothed[(i + 2).min(smoothed.len() - 1)];
        let delta = b.position - a.position;
        let steps = (delta.length() / spacing.max(0.25)).ceil() as usize;
        for s in 1..=steps {
            let t = s as f32 / steps.max(1) as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let catmull_rom = |p0: f32, p1: f32, p2: f32, p3: f32| {
                0.5 * ((2.0 * p1)
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
            };
            let rotation_delta = (b.rotation - a.rotation + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            out.push(InkPoint {
                position: glam::Vec2::new(
                    catmull_rom(
                        before.position.x,
                        a.position.x,
                        b.position.x,
                        after.position.x,
                    ),
                    catmull_rom(
                        before.position.y,
                        a.position.y,
                        b.position.y,
                        after.position.y,
                    ),
                ),
                pressure: a.pressure + (b.pressure - a.pressure) * t,
                taper: a.taper + (b.taper - a.taper) * t,
                tilt: a.tilt.lerp(b.tilt, t),
                rotation: a.rotation + rotation_delta * t,
                time_ms: a.time_ms + ((b.time_ms.saturating_sub(a.time_ms)) as f32 * t) as u64,
            });
        }
    }
    out
}

pub fn taper(points: &mut [InkPoint], start_fraction: f32, end_fraction: f32, minimum: f32) {
    taper_lengths(points, start_fraction, end_fraction, minimum, false)
}

pub fn taper_pixels(points: &mut [InkPoint], start_px: f32, end_px: f32, minimum: f32) {
    taper_lengths(points, start_px, end_px, minimum, true)
}

fn taper_lengths(points: &mut [InkPoint], start: f32, end: f32, minimum: f32, pixels: bool) {
    if points.len() < 2 {
        return;
    }
    let mut distance = vec![0.0; points.len()];
    for i in 1..points.len() {
        distance[i] = distance[i - 1] + points[i].position.distance(points[i - 1].position);
    }
    let total = distance[points.len() - 1].max(1.0);
    let start_distance = if pixels {
        start.clamp(0., total)
    } else {
        start.clamp(0., 1.) * total
    };
    let end_distance = if pixels {
        end.clamp(0., total)
    } else {
        end.clamp(0., 1.) * total
    };
    let min = minimum.clamp(0.01, 1.);
    for (i, p) in points.iter_mut().enumerate() {
        let a = if start > 0. {
            min + (1. - min) * (distance[i] / start_distance.max(0.01)).clamp(0., 1.)
        } else {
            1.
        };
        let b = if end > 0. {
            min + (1. - min) * ((total - distance[i]) / end_distance.max(0.01)).clamp(0., 1.)
        } else {
            1.
        };
        p.taper *= a.min(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_taper_scales_size_opacity_channel_without_changing_pressure() {
        let mut points = [
            InkPoint::new(0.0, 0.0, 0.6, 0),
            InkPoint::new(5.0, 0.0, 0.6, 1),
            InkPoint::new(10.0, 0.0, 0.6, 2),
        ];
        taper_pixels(&mut points, 5.0, 5.0, 0.2);
        assert!((points[0].pressure - 0.6).abs() < f32::EPSILON);
        assert!((points[0].taper - 0.2).abs() < f32::EPSILON);
        assert!((points[1].taper - 1.0).abs() < f32::EPSILON);
        assert!((points[2].taper - 0.2).abs() < f32::EPSILON);
    }
}
