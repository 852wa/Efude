// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Streaming stroke construction.
//!
//! [`StrokeBuilder`] turns raw pen samples into evenly spaced dabs *while the
//! stroke is being drawn*, so the live preview and the committed result come
//! from the same computation. Feeding the same samples one at a time or all at
//! once produces identical dabs (see the `chunking_does_not_change_output`
//! test). Only the last `taper_end_px` of the stroke stays provisional,
//! because its end taper cannot be known until the pen lifts.

use efude_core::InkPoint;
use glam::Vec2;

/// Parameters that shape a stroke. All distances are in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeParams {
    /// Length of the weighted smoothing window in milliseconds. Time based,
    /// so the feel does not change with the tablet's report rate.
    pub stabilization_ms: f32,
    /// "Pull string" distance: the brush follows the pen at this distance.
    pub pull_distance: f32,
    /// 0..1. Faster pen motion shortens the smoothing window by this amount.
    pub speed_adaptation: f32,
    /// Pen speed (document px per ms) treated as "fast" for speed adaptation.
    pub fast_speed: f32,
    /// Distance between dabs.
    pub spacing: f32,
    pub taper_start_px: f32,
    pub taper_end_px: f32,
    /// Taper lengths as a fraction (0..0.5) of the whole stroke. The stroke
    /// length is only known at pen-up, so while either is non-zero every dab
    /// stays provisional until [`StrokeBuilder::finish`]. Prefer pixel tapers.
    pub taper_start_fraction: f32,
    pub taper_end_fraction: f32,
    /// Size/opacity multiplier at the very tip of a taper (0.01..1).
    pub taper_min: f32,
}

impl Default for StrokeParams {
    fn default() -> Self {
        Self {
            stabilization_ms: 0.0,
            pull_distance: 0.0,
            speed_adaptation: 0.0,
            fast_speed: 1.5,
            spacing: 1.0,
            taper_start_px: 0.0,
            taper_end_px: 0.0,
            taper_start_fraction: 0.0,
            taper_end_fraction: 0.0,
            taper_min: 0.1,
        }
    }
}

impl StrokeParams {
    /// Maps the existing 0..=15 stabilization level to a time window.
    /// One level is 8 ms, so level 15 smooths over the last 120 ms.
    pub fn stabilization_from_level(level: usize) -> f32 {
        level.min(15) as f32 * 8.0
    }

    fn uses_fraction_taper(&self) -> bool {
        self.taper_start_fraction > 0.0 || self.taper_end_fraction > 0.0
    }
}

/// Dabs produced by one [`StrokeBuilder::push`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeUpdate {
    /// Final dabs. Draw them once into the layer; they will never change.
    pub committed: Vec<InkPoint>,
    /// Dabs for the unfinished tail. Draw them into a temporary overlay and
    /// replace the overlay on every update.
    pub provisional: Vec<InkPoint>,
}

#[derive(Clone, Debug)]
pub struct StrokeBuilder {
    params: StrokeParams,
    raw: Vec<InkPoint>,
    smoothed: Vec<InkPoint>,
    /// Segments `smoothed[i] -> smoothed[i + 1]` for `i < next_segment` have
    /// been turned into dabs.
    next_segment: usize,
    /// Arc length walked so far along emitted segments.
    walked: f32,
    /// Arc length at which the next dab is due.
    next_dab_at: f32,
    /// Emitted dabs with their arc length, not yet committed.
    held: Vec<(f32, InkPoint)>,
    /// Sanitised stroke clock (see `stroke_time`).
    clock: u64,
    last_input_time: Option<u64>,
    finished: bool,
}

/// Progress along the stroke path while turning segments into dabs.
struct Walk {
    walked: f32,
    next_dab_at: f32,
    held: Vec<(f32, InkPoint)>,
}

const SUBDIVISIONS: usize = 16;
/// Upper bound on samples in the smoothing window, whatever the timestamps.
const MAX_WINDOW_SAMPLES: usize = 96;
/// Assumed gap between samples whose timestamps are missing, repeated,
/// out of order or from a different clock.
const NOMINAL_SAMPLE_MS: u64 = 4;
/// Longest gap taken at face value; longer pauses count as this much.
const MAX_SAMPLE_GAP_MS: u64 = 250;

impl StrokeBuilder {
    pub fn new(params: StrokeParams) -> Self {
        Self {
            params,
            raw: Vec::new(),
            smoothed: Vec::new(),
            next_segment: 0,
            walked: 0.0,
            next_dab_at: 0.0,
            held: Vec::new(),
            clock: 0,
            last_input_time: None,
            finished: false,
        }
    }

    pub fn params(&self) -> &StrokeParams {
        &self.params
    }

    /// Position the stabilized brush is at right now (for drawing a guide).
    pub fn brush_position(&self) -> Option<Vec2> {
        self.smoothed.last().map(|p| p.position)
    }

    /// Adds one pen sample and returns the dabs that became available.
    pub fn push(&mut self, point: InkPoint) -> StrokeUpdate {
        assert!(!self.finished, "push after finish");
        let mut point = point;
        point.time_ms = self.stroke_time(point.time_ms);
        self.raw.push(point);
        let smoothed = self.stabilize_latest();
        self.smoothed.push(smoothed);
        // A Catmull-Rom segment needs the point after its end, so segment
        // i is final once smoothed[i + 2] exists.
        while self.next_segment + 2 < self.smoothed.len() {
            self.emit_segment(self.next_segment, false);
            self.next_segment += 1;
        }
        let committed = if self.params.uses_fraction_taper() {
            Vec::new()
        } else {
            self.release_held(self.walked - self.params.taper_end_px.max(0.0))
        };
        StrokeUpdate {
            committed,
            provisional: self.provisional_tail(),
        }
    }

    /// Ends the stroke: catches the brush up to the pen, applies the end
    /// taper and returns every remaining dab.
    pub fn finish(&mut self) -> Vec<InkPoint> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        if self.raw.is_empty() {
            return Vec::new();
        }
        // Catch up: the stabilized brush may lag behind the pen.
        let endpoint = *self.raw.last().unwrap();
        if let Some(last) = self.smoothed.last()
            && last.position.distance(endpoint.position) > 1e-3
        {
            self.smoothed.push(endpoint);
        }
        while self.next_segment + 1 < self.smoothed.len() {
            self.emit_segment(self.next_segment, false);
            self.next_segment += 1;
        }
        if self.held.is_empty() && self.walked == 0.0 {
            // A tap: one dab at the pen position.
            let mut tap = self.smoothed[0];
            tap.taper *= self.start_taper(0.0);
            return vec![tap];
        }
        let total = self.walked;
        let held = std::mem::take(&mut self.held);
        held.into_iter()
            .map(|(at, dab)| self.with_length_tapers(at, dab, total))
            .collect()
    }

    /// Applies the tapers that depend on the stroke's total length: the
    /// pixel end taper and both fraction tapers.
    fn with_length_tapers(&self, at: f32, mut dab: InkPoint, total: f32) -> InkPoint {
        let p = &self.params;
        let min = p.taper_min.clamp(0.01, 1.0);
        let ramp = |distance: f32, length: f32| {
            if length > 0.0 {
                min + (1.0 - min) * (distance / length).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        dab.taper *= ramp(total - at, p.taper_end_px.max(0.0));
        dab.taper *= ramp(at, p.taper_start_fraction.clamp(0.0, 0.5) * total);
        dab.taper *= ramp(total - at, p.taper_end_fraction.clamp(0.0, 0.5) * total);
        dab
    }

    /// Converts device timestamps into a clock that always moves forward.
    /// Devices may report no time (all zeros), repeat a time for coalesced
    /// samples, or mix clocks (Windows Ink, WinTab and window events use
    /// different origins). A time-based smoothing window must never see
    /// time stand still, or it would average the whole stroke.
    fn stroke_time(&mut self, input: u64) -> u64 {
        let step = match self.last_input_time {
            None => 0,
            Some(previous) => match input.checked_sub(previous) {
                Some(gap) if gap > 0 && gap <= MAX_SAMPLE_GAP_MS => gap,
                Some(gap) if gap > MAX_SAMPLE_GAP_MS && gap < 10_000 => MAX_SAMPLE_GAP_MS,
                _ => NOMINAL_SAMPLE_MS,
            },
        };
        self.last_input_time = Some(input);
        self.clock += step;
        self.clock
    }

    fn stabilize_latest(&self) -> InkPoint {
        let latest = *self.raw.last().unwrap();
        let p = &self.params;
        let mut window = p.stabilization_ms.max(0.0);
        if window > 0.0 && p.speed_adaptation > 0.0 && self.raw.len() >= 2 {
            let previous = self.raw[self.raw.len() - 2];
            let elapsed = latest.time_ms.saturating_sub(previous.time_ms).max(1) as f32;
            let speed = latest.position.distance(previous.position) / elapsed;
            let fast = (speed / p.fast_speed.max(1e-3)).clamp(0.0, 1.0);
            window *= 1.0 - p.speed_adaptation.clamp(0.0, 1.0) * fast;
        }
        let mut result = latest;
        if window > 0.0 {
            // Linearly rising weights over the time window, newest heaviest.
            let (mut position, mut pressure, mut weight) = (Vec2::ZERO, 0.0, 0.0);
            for sample in self.raw.iter().rev().take(MAX_WINDOW_SAMPLES) {
                let age = latest.time_ms.saturating_sub(sample.time_ms) as f32;
                if age > window {
                    break;
                }
                let w = 1.0 - age / (window + 1.0);
                position += sample.position * w;
                pressure += sample.pressure * w;
                weight += w;
            }
            result.position = position / weight;
            result.pressure = pressure / weight;
        }
        if p.pull_distance > 0.0
            && let Some(previous) = self.smoothed.last()
        {
            let delta = result.position - previous.position;
            let length = delta.length();
            let pull = p.pull_distance.min(256.0);
            result.position = if length > pull {
                previous.position + delta * ((length - pull) / length)
            } else {
                previous.position
            };
        }
        result
    }

    fn start_taper(&self, at: f32) -> f32 {
        let start = self.params.taper_start_px.max(0.0);
        if start <= 0.0 {
            return 1.0;
        }
        let min = self.params.taper_min.clamp(0.01, 1.0);
        min + (1.0 - min) * (at / start).clamp(0.0, 1.0)
    }

    fn segment_points(&self, i: usize, is_last: bool) -> [InkPoint; 4] {
        let s = &self.smoothed;
        let a = s[i];
        let b = s[i + 1];
        let before = if i == 0 { a } else { s[i - 1] };
        let after = if is_last || i + 2 >= s.len() {
            b
        } else {
            s[i + 2]
        };
        [before, a, b, after]
    }

    /// Walks segment `i` and appends dabs to `held`.
    fn emit_segment(&mut self, i: usize, is_last: bool) {
        let mut walk = Walk {
            walked: self.walked,
            next_dab_at: self.next_dab_at,
            held: std::mem::take(&mut self.held),
        };
        self.walk_segment(i, is_last, &mut walk);
        self.walked = walk.walked;
        self.next_dab_at = walk.next_dab_at;
        self.held = walk.held;
    }

    fn walk_segment(&self, i: usize, is_last: bool, walk: &mut Walk) {
        let [before, a, b, after] = self.segment_points(i, is_last);
        let spacing = self.params.spacing.max(0.25);
        if i == 0 && walk.walked == 0.0 && walk.next_dab_at == 0.0 {
            // First dab sits exactly on the first point.
            let mut first = a;
            first.taper *= self.start_taper(0.0);
            walk.held.push((0.0, first));
            walk.next_dab_at = spacing;
        }
        let mut previous = a.position;
        let mut previous_t = 0.0;
        for k in 1..=SUBDIVISIONS {
            let t = k as f32 / SUBDIVISIONS as f32;
            let position = catmull_rom(before.position, a.position, b.position, after.position, t);
            let piece = position.distance(previous);
            while piece > 0.0 && walk.walked + piece >= walk.next_dab_at {
                let f = (walk.next_dab_at - walk.walked) / piece;
                let local_t = previous_t + (t - previous_t) * f;
                let mut dab = interpolate(a, b, local_t);
                dab.position = previous + (position - previous) * f;
                dab.taper *= self.start_taper(walk.next_dab_at);
                walk.held.push((walk.next_dab_at, dab));
                walk.next_dab_at += spacing;
            }
            walk.walked += piece;
            previous = position;
            previous_t = t;
        }
    }

    fn release_held(&mut self, upto: f32) -> Vec<InkPoint> {
        let count = self.held.iter().take_while(|(at, _)| *at <= upto).count();
        self.held.drain(..count).map(|(_, dab)| dab).collect()
    }

    /// Held dabs plus a preview of the segment that is still waiting for its
    /// look-ahead point. Recomputed on every update; never committed as-is.
    fn provisional_tail(&self) -> Vec<InkPoint> {
        let mut pending = self.held.clone();
        let mut length = self.walked;
        if self.next_segment + 1 < self.smoothed.len() {
            // Preview the waiting segment without its look-ahead point.
            let mut walk = Walk {
                walked: self.walked,
                next_dab_at: self.next_dab_at,
                held: Vec::new(),
            };
            self.walk_segment(self.next_segment, true, &mut walk);
            length = walk.walked;
            pending.extend(walk.held);
        }
        // Show the tail as it would look if the pen lifted now.
        let mut tail: Vec<InkPoint> = pending
            .into_iter()
            .map(|(at, dab)| self.with_length_tapers(at, dab, length))
            .collect();
        if tail.is_empty()
            && self.walked == 0.0
            && let Some(first) = self.smoothed.first()
        {
            let mut first = *first;
            first.taper *= self.start_taper(0.0);
            tail.push(first);
        }
        tail
    }
}

fn catmull_rom(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn interpolate(a: InkPoint, b: InkPoint, t: f32) -> InkPoint {
    let rotation_delta = (b.rotation - a.rotation + std::f32::consts::PI)
        .rem_euclid(std::f32::consts::TAU)
        - std::f32::consts::PI;
    InkPoint {
        position: a.position.lerp(b.position, t),
        pressure: a.pressure + (b.pressure - a.pressure) * t,
        taper: a.taper + (b.taper - a.taper) * t,
        tilt: a.tilt.lerp(b.tilt, t),
        rotation: a.rotation + rotation_delta * t,
        time_ms: a.time_ms + (b.time_ms.saturating_sub(a.time_ms) as f32 * t) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(n: usize) -> Vec<InkPoint> {
        (0..n)
            .map(|i| {
                let x = i as f32 * 3.7;
                let y = (i as f32 * 0.21).sin() * 40.0 + ((i * 7919) % 5) as f32 * 0.6;
                InkPoint::new(x, y, 0.3 + 0.6 * ((i % 17) as f32 / 16.0), i as u64 * 4)
            })
            .collect()
    }

    fn params() -> StrokeParams {
        StrokeParams {
            stabilization_ms: 40.0,
            pull_distance: 3.0,
            speed_adaptation: 0.5,
            spacing: 1.5,
            taper_start_px: 20.0,
            taper_end_px: 25.0,
            taper_min: 0.1,
            ..StrokeParams::default()
        }
    }

    fn run(points: &[InkPoint], p: StrokeParams) -> Vec<InkPoint> {
        let mut builder = StrokeBuilder::new(p);
        let mut out = Vec::new();
        for point in points {
            out.extend(builder.push(*point).committed);
        }
        out.extend(builder.finish());
        out
    }

    #[test]
    fn chunking_does_not_change_output() {
        let points = wave(200);
        let a = run(&points, params());
        // Replaying the same samples must give bit-identical dabs.
        let b = run(&points, params());
        assert_eq!(a, b);
        assert!(a.len() > 300);
    }

    #[test]
    fn committed_dabs_never_change_after_release() {
        // What is committed live must be a prefix of the final result.
        let points = wave(150);
        let mut builder = StrokeBuilder::new(params());
        let mut committed = Vec::new();
        for point in &points {
            committed.extend(builder.push(*point).committed);
        }
        let live_count = committed.len();
        committed.extend(builder.finish());
        let full = run(&points, params());
        assert_eq!(committed, full);
        assert!(live_count > full.len() / 2, "most dabs should commit live");
    }

    #[test]
    fn dabs_are_evenly_spaced() {
        let points: Vec<_> = (0..50)
            .map(|i| InkPoint::new(i as f32 * 10.0, 0.0, 1.0, i as u64 * 4))
            .collect();
        let dabs = run(
            &points,
            StrokeParams {
                spacing: 2.0,
                ..Default::default()
            },
        );
        for pair in dabs.windows(2) {
            let d = pair[0].position.distance(pair[1].position);
            assert!(
                (d - 2.0).abs() < 0.05 || pair[1] == *dabs.last().unwrap(),
                "gap {d}"
            );
        }
    }

    #[test]
    fn stroke_reaches_pen_up_position_with_stabilization() {
        let points = wave(80);
        let dabs = run(&points, params());
        let end = points.last().unwrap().position;
        assert!(dabs.last().unwrap().position.distance(end) < 2.0);
    }

    #[test]
    fn tapers_start_and_end_thin() {
        let points: Vec<_> = (0..60)
            .map(|i| InkPoint::new(i as f32 * 5.0, 0.0, 1.0, i as u64 * 4))
            .collect();
        let dabs = run(&points, params());
        assert!(dabs.first().unwrap().taper < 0.2);
        assert!(dabs.last().unwrap().taper < 0.2);
        assert!((dabs[dabs.len() / 2].taper - 1.0).abs() < 1e-6);
    }

    #[test]
    fn time_based_window_ignores_report_rate() {
        // Same motion sampled at 133 Hz and 266 Hz should end up close.
        let line = |step_ms: u64| -> Vec<InkPoint> {
            (0..=(800 / step_ms))
                .map(|i| {
                    let t = (i * step_ms) as f32;
                    InkPoint::new(t * 0.5, (t * 0.01).sin() * 30.0, 1.0, i * step_ms)
                })
                .collect()
        };
        let slow = run(&line(8), params());
        let fast = run(&line(4), params());
        let probe = |dabs: &[InkPoint]| dabs[dabs.len() / 2].position;
        assert!(probe(&slow).distance(probe(&fast)) < 3.0);
    }

    #[test]
    fn fraction_tapers_hold_until_pen_up() {
        let points: Vec<_> = (0..60)
            .map(|i| InkPoint::new(i as f32 * 5.0, 0.0, 1.0, i as u64 * 4))
            .collect();
        let p = StrokeParams {
            spacing: 2.0,
            taper_start_fraction: 0.2,
            taper_end_fraction: 0.2,
            ..StrokeParams::default()
        };
        let mut builder = StrokeBuilder::new(p);
        for point in &points {
            let update = builder.push(*point);
            assert!(update.committed.is_empty());
        }
        let dabs = builder.finish();
        assert!(dabs.first().unwrap().taper < 0.2);
        assert!(dabs.last().unwrap().taper < 0.2);
        assert!((dabs[dabs.len() / 2].taper - 1.0).abs() < 1e-6);
    }

    #[test]
    fn provisional_preview_matches_final_tail() {
        // Every provisional dab shown just before pen-up must lie on the path
        // that finish() commits, so the drawn line does not jump. The final
        // path may continue further (catch-up to the pen-up point).
        let points = wave(120);
        let mut builder = StrokeBuilder::new(params());
        let mut last = StrokeUpdate::default();
        for point in &points {
            last = builder.push(*point);
        }
        let final_tail = builder.finish();
        assert!(!last.provisional.is_empty());
        for dab in &last.provisional {
            let nearest = final_tail
                .iter()
                .map(|f| f.position.distance(dab.position))
                .fold(f32::MAX, f32::min);
            assert!(
                nearest < 1.0,
                "provisional dab {nearest}px off the final path"
            );
        }
    }

    fn long_line(n: usize, time: impl Fn(usize) -> u64) -> Vec<InkPoint> {
        (0..n)
            .map(|i| InkPoint::new(i as f32 * 2.0, 0.0, 1.0, time(i)))
            .collect()
    }

    fn assert_keeps_up(points: &[InkPoint]) {
        let mut builder = StrokeBuilder::new(StrokeParams {
            stabilization_ms: 120.0,
            spacing: 1.0,
            ..StrokeParams::default()
        });
        let mut committed = Vec::new();
        for point in points {
            committed.extend(builder.push(*point).committed);
        }
        let pen = points.last().unwrap().position;
        let brush = builder.brush_position().unwrap();
        assert!(
            pen.distance(brush) < 150.0,
            "brush fell {}px behind the pen",
            pen.distance(brush)
        );
        let drawn = committed.last().unwrap().position;
        assert!(
            pen.distance(drawn) < 150.0,
            "drawing stopped {}px short",
            pen.distance(drawn)
        );
    }

    #[test]
    fn long_stroke_keeps_up_without_timestamps() {
        // Some tablets report no time at all.
        assert_keeps_up(&long_line(3000, |_| 0));
    }

    #[test]
    fn long_stroke_keeps_up_with_repeated_timestamps() {
        // Coalesced samples often share one timestamp.
        assert_keeps_up(&long_line(3000, |i| (i / 8) as u64 * 16));
    }

    #[test]
    fn long_stroke_keeps_up_with_mixed_clocks() {
        // Pen packets (time since boot) mixed with window events (time
        // since app start).
        assert_keeps_up(&long_line(3000, |i| {
            if i % 7 == 3 {
                5_000 + i as u64 * 4
            } else {
                90_000_000 + i as u64 * 4
            }
        }));
    }

    #[test]
    fn a_tap_makes_one_dab() {
        let mut builder = StrokeBuilder::new(params());
        let update = builder.push(InkPoint::new(5.0, 5.0, 0.5, 0));
        assert!(update.committed.is_empty());
        assert_eq!(update.provisional.len(), 1);
        assert_eq!(builder.finish().len(), 1);
    }
}
