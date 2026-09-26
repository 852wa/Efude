// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Image filters that work on a whole RGBA picture (straight alpha, 8 bits
//! per channel, rows top to bottom): blurs, sharpening, noise, mosaic,
//! chromatic aberration, colour grading and line width. Neighbourhood
//! filters work on premultiplied colour so transparent pixels never bleed
//! black into what is painted.

use serde::{Deserialize, Serialize};

/// The kinds of blur (see [`Filter::Blur`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlurKind {
    /// Soft Gaussian blur.
    Gaussian,
    /// Out-of-focus lens: a round kernel that makes bright spots bloom.
    Lens,
    /// Smooths surfaces but keeps edges (for skin, gradients, noise).
    Smooth,
    /// Motion along an angle.
    Motion,
}

/// Direction of a chromatic aberration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Horizontal,
    Vertical,
    /// Top-left to bottom-right.
    DiagonalDown,
    /// Bottom-left to top-right.
    DiagonalUp,
}

/// Colour grading looks (see [`GRADE_NAMES`]).
pub const GRADE_COUNT: usize = 15;

/// Names of the colour grading looks: (Japanese, English).
pub const GRADE_NAMES: [(&str, &str); GRADE_COUNT] = [
    ("ウォーム", "Warm"),
    ("クール", "Cool"),
    ("セピア", "Sepia"),
    ("モノクロ", "Monochrome"),
    ("ビンテージ", "Vintage"),
    ("フィルム", "Film"),
    ("ティール＆オレンジ", "Teal & Orange"),
    ("パステル", "Pastel"),
    ("夕焼け", "Sunset"),
    ("夜", "Night"),
    ("ブリーチバイパス", "Bleach Bypass"),
    ("クロスプロセス", "Cross Process"),
    ("フェード", "Faded"),
    ("ハイコントラスト", "High Contrast"),
    ("シネマ", "Cinema"),
];

/// A filter and its settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Filter {
    /// `amount` is the radius in pixels.
    Blur {
        kind: BlurKind,
        amount: f32,
        /// Motion blur direction in degrees (0 = to the right).
        angle: f32,
    },
    /// Unsharp mask: `amount` 0..=5, `radius` in pixels.
    Sharpen { amount: f32, radius: f32 },
    /// `strength` 1..=3 (window radius of a median).
    Denoise { strength: u32 },
    /// Look `preset` (index into [`GRADE_NAMES`]) mixed in by `strength`.
    Grade { preset: usize, strength: f32 },
    /// Red and blue moved `amount` pixels apart along `direction`.
    ChromaticAberration { direction: Direction, amount: f32 },
    /// Cells of `size` pixels.
    Mosaic { size: u32 },
    /// Grain of `amount` 0..=1; `mono` makes it grey.
    Noise { amount: f32, mono: bool, seed: u32 },
    /// Lines made `amount` pixels thicker (negative: thinner).
    LineWidth { amount: i32 },
}

impl Filter {
    /// How far (in pixels) a result pixel looks around itself.
    pub fn reach(&self) -> u32 {
        match *self {
            Filter::Blur { kind, amount, .. } => {
                let r = amount.max(0.0);
                match kind {
                    BlurKind::Gaussian => (r * 1.6).ceil() as u32 + 2,
                    BlurKind::Motion => r.ceil() as u32 + 1,
                    _ => r.ceil() as u32 + 1,
                }
            }
            Filter::Sharpen { radius, .. } => (radius * 1.6).ceil() as u32 + 2,
            Filter::Denoise { strength } => strength,
            Filter::ChromaticAberration { amount, .. } => amount.abs().ceil() as u32 + 1,
            Filter::Mosaic { size } => size,
            Filter::LineWidth { amount } => amount.unsigned_abs(),
            Filter::Grade { .. } | Filter::Noise { .. } => 0,
        }
    }

    /// Applies the filter to `rgba` (`width` × `height`). `origin` is where
    /// the picture sits in the document, so patterns (noise, mosaic cells)
    /// line up when a part is filtered on its own (for previews).
    pub fn apply(&self, rgba: &mut [u8], width: usize, height: usize, origin: (i64, i64)) {
        if width == 0 || height == 0 || rgba.len() < width * height * 4 {
            return;
        }
        match *self {
            Filter::Blur {
                kind,
                amount,
                angle,
            } => {
                let amount = amount.clamp(0.0, 200.0);
                if amount <= 0.0 {
                    return;
                }
                let mut image = Premul::from_rgba(rgba, width, height);
                image = match kind {
                    BlurKind::Gaussian => gaussian(&image, amount / 2.0),
                    BlurKind::Lens => lens(&image, amount.min(64.0)),
                    BlurKind::Smooth => smooth(&image, amount.min(64.0)),
                    BlurKind::Motion => motion(&image, angle, amount.min(200.0)),
                };
                image.write_rgba(rgba);
            }
            Filter::Sharpen { amount, radius } => sharpen(rgba, width, height, amount, radius),
            Filter::Denoise { strength } => median(rgba, width, height, strength.clamp(1, 3)),
            Filter::Grade { preset, strength } => grade(rgba, preset, strength),
            Filter::ChromaticAberration { direction, amount } => {
                chromatic_aberration(rgba, width, height, direction, amount)
            }
            Filter::Mosaic { size } => mosaic(rgba, width, height, size.max(1), origin),
            Filter::Noise { amount, mono, seed } => noise(rgba, width, amount, mono, seed, origin),
            Filter::LineWidth { amount } => line_width(rgba, width, height, amount),
        }
    }
}

/// Premultiplied linear-in-storage RGBA as floats (0..=1).
#[derive(Clone)]
struct Premul {
    width: usize,
    height: usize,
    data: Vec<[f32; 4]>,
}

impl Premul {
    fn from_rgba(rgba: &[u8], width: usize, height: usize) -> Self {
        let data = rgba
            .chunks_exact(4)
            .take(width * height)
            .map(|p| {
                let a = p[3] as f32 / 255.0;
                [
                    p[0] as f32 / 255.0 * a,
                    p[1] as f32 / 255.0 * a,
                    p[2] as f32 / 255.0 * a,
                    a,
                ]
            })
            .collect();
        Self {
            width,
            height,
            data,
        }
    }

    fn write_rgba(&self, rgba: &mut [u8]) {
        for (p, v) in rgba.chunks_exact_mut(4).zip(&self.data) {
            p.copy_from_slice(&unpremultiply(*v));
        }
    }

    fn at(&self, x: isize, y: isize) -> [f32; 4] {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.data[y * self.width + x]
    }

    /// Bilinear sample with edges extended.
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.at(x0, y0);
        let b = self.at(x0 + 1, y0);
        let c = self.at(x0, y0 + 1);
        let d = self.at(x0 + 1, y0 + 1);
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

fn unpremultiply(v: [f32; 4]) -> [u8; 4] {
    let a = v[3].clamp(0.0, 1.0);
    if a <= 0.5 / 255.0 {
        return [0; 4];
    }
    let c = |x: f32| ((x / a).clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(v[0]), c(v[1]), c(v[2]), (a * 255.0).round() as u8]
}

/// Runs `f(y, row)` for every output row, spread over the processor's
/// threads.
fn par_rows<T: Send, F: Fn(usize, &mut [T]) + Sync>(out: &mut [T], width: usize, f: F) {
    let height = out.len() / width.max(1);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 16);
    let rows_per_chunk = height.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in out.chunks_mut(rows_per_chunk * width).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (row_index, row) in chunk.chunks_mut(width).enumerate() {
                    f(chunk_index * rows_per_chunk + row_index, row);
                }
            });
        }
    });
}

/// Box blur of radius `rx` along rows and `ry` along columns (edges
/// extended), in place. Columns are blurred as rows of a transposed copy,
/// so only one extra buffer is needed.
fn box_blur(image: &mut Premul, rx: usize, ry: usize) {
    let (w, h) = (image.width, image.height);
    blur_rows(&mut image.data, w, rx);
    if ry == 0 || h < 2 {
        return;
    }
    let mut transposed = vec![[0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            transposed[x * h + y] = image.data[y * w + x];
        }
    }
    blur_rows(&mut transposed, h, ry);
    for x in 0..w {
        for y in 0..h {
            image.data[y * w + x] = transposed[x * h + y];
        }
    }
}

/// Running mean along each row of `data` (rows of `width`), in place.
fn blur_rows(data: &mut [[f32; 4]], width: usize, r: usize) {
    if r == 0 || width == 0 {
        return;
    }
    par_rows(data, width, |_, row| {
        let line = row.to_vec();
        running_mean(&line, row, r);
    });
}

/// Mean of a window of radius `r` around each element (edges extended).
fn running_mean(input: &[[f32; 4]], output: &mut [[f32; 4]], r: usize) {
    let n = input.len();
    if n == 0 {
        return;
    }
    if r == 0 {
        output.copy_from_slice(input);
        return;
    }
    let at = |i: isize| input[i.clamp(0, n as isize - 1) as usize];
    let mut sum = [0f32; 4];
    let r = r as isize;
    for i in -r..=r {
        let v = at(i);
        for c in 0..4 {
            sum[c] += v[c];
        }
    }
    let scale = 1.0 / (2 * r + 1) as f32;
    for i in 0..n as isize {
        output[i as usize] = sum.map(|s| s * scale);
        let (add, remove) = (at(i + r + 1), at(i - r));
        for c in 0..4 {
            sum[c] += add[c] - remove[c];
        }
    }
}

/// Gaussian blur of standard deviation `sigma` (three box blurs).
fn gaussian(image: &Premul, sigma: f32) -> Premul {
    if sigma < 0.3 {
        return image.clone();
    }
    // Box sizes whose three passes approximate the Gaussian.
    let n = 3.0;
    let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut lower = ideal.floor() as i32;
    if lower % 2 == 0 {
        lower -= 1;
    }
    let upper = lower + 2;
    let m = ((12.0 * sigma * sigma - n * (lower * lower) as f32 - 4.0 * n * lower as f32 - 3.0 * n)
        / (-4.0 * lower as f32 - 4.0))
        .round() as i32;
    let mut out = image.clone();
    for pass in 0..3 {
        let size = if pass < m { lower } else { upper };
        let r = ((size - 1) / 2).max(0) as usize;
        box_blur(&mut out, r, r);
    }
    out
}

/// Round out-of-focus blur. Colours are averaged in light (squared) so
/// highlights bloom as they do through a lens.
fn lens(image: &Premul, radius: f32) -> Premul {
    let (w, h) = (image.width, image.height);
    let r = radius.round().max(1.0) as isize;
    // Squared (roughly linear light) premultiplied values, and per-row
    // prefix sums for quick spans.
    let light: Vec<[f32; 4]> = image
        .data
        .iter()
        .map(|v| {
            let a = v[3];
            if a <= 0.0 {
                return [0.0; 4];
            }
            [
                (v[0] / a).powi(2) * a,
                (v[1] / a).powi(2) * a,
                (v[2] / a).powi(2) * a,
                a,
            ]
        })
        .collect();
    let mut prefix = vec![[0f32; 4]; (w + 1) * h];
    for y in 0..h {
        for x in 0..w {
            let (before, value) = (prefix[y * (w + 1) + x], light[y * w + x]);
            prefix[y * (w + 1) + x + 1] = std::array::from_fn(|c| before[c] + value[c]);
        }
    }
    let spans: Vec<isize> = (-r..=r)
        .map(|dy| (((r * r - dy * dy) as f32).sqrt()).floor() as isize)
        .collect();
    let mut out = vec![[0f32; 4]; w * h];
    par_rows(&mut out, w, |y, row| {
        for (x, pixel) in row.iter_mut().enumerate() {
            let mut sum = [0f32; 4];
            let mut count = 0.0;
            for (k, &half) in spans.iter().enumerate() {
                let sy = (y as isize + k as isize - r).clamp(0, h as isize - 1) as usize;
                let x0 = (x as isize - half).max(0) as usize;
                let x1 = ((x as isize + half + 1).min(w as isize)) as usize;
                let (a, b) = (prefix[sy * (w + 1) + x1], prefix[sy * (w + 1) + x0]);
                for c in 0..4 {
                    sum[c] += a[c] - b[c];
                }
                count += (x1 - x0) as f32;
            }
            let mean = sum.map(|s| s / count.max(1.0));
            let a = mean[3];
            *pixel = if a <= 0.0 {
                [0.0; 4]
            } else {
                [
                    (mean[0] / a).sqrt() * a,
                    (mean[1] / a).sqrt() * a,
                    (mean[2] / a).sqrt() * a,
                    a,
                ]
            };
        }
    });
    Premul {
        width: w,
        height: h,
        data: out,
    }
}

/// Edge-keeping smoothing (a guided filter guided by the picture itself):
/// flat areas and gentle gradients are smoothed, strong edges are kept.
fn smooth(image: &Premul, radius: f32) -> Premul {
    let r = radius.round().max(1.0) as usize;
    let eps = 0.004f32;
    // `mean` becomes the per-pixel offset b and `mean_sq` the gain a
    // (each blurred afterwards), to keep few full-size buffers.
    let mut mean_sq = Premul {
        width: image.width,
        height: image.height,
        data: image.data.iter().map(|v| v.map(|c| c * c)).collect(),
    };
    let mut mean = image.clone();
    box_blur(&mut mean, r, r);
    box_blur(&mut mean_sq, r, r);
    for (m, sq) in mean.data.iter_mut().zip(mean_sq.data.iter_mut()) {
        for c in 0..4 {
            let variance = (sq[c] - m[c] * m[c]).max(0.0);
            let k = variance / (variance + eps);
            sq[c] = k;
            m[c] *= 1.0 - k;
        }
    }
    let (mut mean_a, mut mean_b) = (mean_sq, mean);
    box_blur(&mut mean_a, r, r);
    box_blur(&mut mean_b, r, r);
    let data = image
        .data
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let mut out: [f32; 4] =
                std::array::from_fn(|c| mean_a.data[i][c] * v[c] + mean_b.data[i][c]);
            out[3] = out[3].clamp(0.0, 1.0);
            for c in 0..3 {
                out[c] = out[c].clamp(0.0, out[3]);
            }
            out
        })
        .collect();
    Premul {
        width: image.width,
        height: image.height,
        data,
    }
}

/// Motion blur: the picture averaged along a line `distance` long.
fn motion(image: &Premul, angle: f32, distance: f32) -> Premul {
    let (w, h) = (image.width, image.height);
    let steps = distance.round().max(1.0) as usize;
    let (dx, dy) = (angle.to_radians().cos(), -angle.to_radians().sin());
    let offsets: Vec<(f32, f32)> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32 - 0.5;
            (dx * t * distance, dy * t * distance)
        })
        .collect();
    let mut out = vec![[0f32; 4]; w * h];
    par_rows(&mut out, w, |y, row| {
        for (x, pixel) in row.iter_mut().enumerate() {
            let mut sum = [0f32; 4];
            for &(ox, oy) in &offsets {
                let v = image.sample(x as f32 + ox, y as f32 + oy);
                for c in 0..4 {
                    sum[c] += v[c];
                }
            }
            *pixel = sum.map(|s| s / offsets.len() as f32);
        }
    });
    Premul {
        width: w,
        height: h,
        data: out,
    }
}

/// Unsharp mask: colour pushed away from its blurred surroundings.
fn sharpen(rgba: &mut [u8], width: usize, height: usize, amount: f32, radius: f32) {
    let amount = amount.clamp(0.0, 5.0);
    if amount <= 0.0 {
        return;
    }
    let image = Premul::from_rgba(rgba, width, height);
    let blurred = gaussian(&image, radius.clamp(0.3, 50.0));
    for (i, p) in rgba.chunks_exact_mut(4).enumerate().take(width * height) {
        if p[3] == 0 {
            continue;
        }
        let b = blurred.data[i];
        if b[3] <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let value = p[c] as f32 / 255.0;
            let around = (b[c] / b[3]).clamp(0.0, 1.0);
            p[c] = ((value + (value - around) * amount).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

/// Median of each channel over a (2r+1)² window: removes speckles and
/// grain but keeps edges.
fn median(rgba: &mut [u8], width: usize, height: usize, r: u32) {
    let source = rgba[..width * height * 4].to_vec();
    let r = r as isize;
    let mut out = vec![[0u8; 4]; width * height];
    par_rows(&mut out, width, |y, row| {
        let mut window: Vec<u8> = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
        for (x, pixel) in row.iter_mut().enumerate() {
            for c in 0..4 {
                window.clear();
                for dy in -r..=r {
                    let sy = (y as isize + dy).clamp(0, height as isize - 1) as usize;
                    for dx in -r..=r {
                        let sx = (x as isize + dx).clamp(0, width as isize - 1) as usize;
                        window.push(source[(sy * width + sx) * 4 + c]);
                    }
                }
                let middle = window.len() / 2;
                pixel[c] = *window.select_nth_unstable(middle).1;
            }
        }
    });
    for (p, v) in rgba.chunks_exact_mut(4).zip(out) {
        p.copy_from_slice(&v);
    }
}

fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn s_curve(x: f32, strength: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let s = x * x * (3.0 - 2.0 * x);
    x + (s - x) * strength
}

/// One colour graded by look `preset` (sRGB values 0..=1).
pub fn grade_color(preset: usize, [r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = luma(r, g, b);
    let desaturate = |amount: f32, [r, g, b]: [f32; 3]| {
        let l = luma(r, g, b);
        [
            l + (r - l) * amount,
            l + (g - l) * amount,
            l + (b - l) * amount,
        ]
    };
    let out = match preset {
        // Warm
        0 => [r * 1.08 + 0.02, g * 1.01, b * 0.88],
        // Cool
        1 => [r * 0.9, g * 1.0, b * 1.08 + 0.03],
        // Sepia
        2 => [
            0.393 * r + 0.769 * g + 0.189 * b,
            0.349 * r + 0.686 * g + 0.168 * b,
            0.272 * r + 0.534 * g + 0.131 * b,
        ],
        // Monochrome
        3 => [l, l, l],
        // Vintage: faded blacks, warm, softer colours.
        4 => {
            let [r, g, b] = desaturate(0.75, [r, g, b]);
            [0.1 + r * 0.85, 0.08 + g * 0.8, 0.06 + b * 0.7]
        }
        // Film: gentle S-curve, greenish shadows.
        5 => {
            let shadow = (1.0 - l).powi(2) * 0.05;
            [
                s_curve(r, 0.35),
                s_curve(g, 0.35) + shadow,
                s_curve(b, 0.3) + shadow * 0.5,
            ]
        }
        // Teal and orange: shadows teal, highlights orange.
        6 => {
            let (shadow, light) = ((1.0 - l).powi(2), l * l);
            [
                r - 0.12 * shadow + 0.12 * light,
                g + 0.03 * shadow + 0.03 * light,
                b + 0.12 * shadow - 0.12 * light,
            ]
        }
        // Pastel
        7 => {
            let [r, g, b] = desaturate(0.55, [r, g, b]);
            [0.18 + r * 0.82, 0.18 + g * 0.82, 0.2 + b * 0.8]
        }
        // Sunset: warm pink-orange.
        8 => [r * 1.1 + 0.05, g * 0.95 + 0.01, b * 0.85 + 0.06 * (1.0 - l)],
        // Night: dark, blue, less colour.
        9 => {
            let [r, g, b] = desaturate(0.5, [r, g, b]);
            [r * 0.6, g * 0.7, b * 0.9 + 0.05]
        }
        // Bleach bypass: silver, contrasty, little colour.
        10 => {
            let [r, g, b] = desaturate(0.4, [r, g, b]);
            [s_curve(r, 0.6), s_curve(g, 0.6), s_curve(b, 0.6)]
        }
        // Cross process: yellow-green highlights, blue shadows.
        11 => [s_curve(r, 0.6), s_curve(g, 0.3) * 1.04, 0.15 + b * 0.7],
        // Faded
        12 => {
            let [r, g, b] = desaturate(0.8, [r, g, b]);
            [0.12 + r * 0.8, 0.12 + g * 0.8, 0.14 + b * 0.78]
        }
        // High contrast
        13 => {
            let [r, g, b] = desaturate(1.2, [r, g, b]);
            [s_curve(r, 1.0), s_curve(g, 1.0), s_curve(b, 1.0)]
        }
        // Cinema: cool green shadows, muted colour, crisp contrast.
        14 => {
            let [r, g, b] = desaturate(0.8, [r, g, b]);
            let shadow = (1.0 - l).powi(2);
            [
                s_curve(r, 0.4) - 0.04 * shadow,
                s_curve(g, 0.4) + 0.03 * shadow,
                s_curve(b, 0.4) + 0.05 * shadow,
            ]
        }
        _ => [r, g, b],
    };
    out.map(|v| v.clamp(0.0, 1.0))
}

fn grade(rgba: &mut [u8], preset: usize, strength: f32) {
    let strength = strength.clamp(0.0, 1.0);
    for p in rgba.chunks_exact_mut(4) {
        if p[3] == 0 {
            continue;
        }
        let source = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let graded = grade_color(preset, source);
        for c in 0..3 {
            let v = source[c] + (graded[c] - source[c]) * strength;
            p[c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

fn chromatic_aberration(
    rgba: &mut [u8],
    width: usize,
    height: usize,
    direction: Direction,
    amount: f32,
) {
    let amount = amount.clamp(-100.0, 100.0);
    if amount == 0.0 {
        return;
    }
    let image = Premul::from_rgba(rgba, width, height);
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    let (ux, uy) = match direction {
        Direction::Horizontal => (1.0, 0.0),
        Direction::Vertical => (0.0, 1.0),
        Direction::DiagonalDown => (diagonal, diagonal),
        Direction::DiagonalUp => (diagonal, -diagonal),
    };
    let (dx, dy) = (ux * amount, uy * amount);
    let mut out = vec![[0f32; 4]; width * height];
    par_rows(&mut out, width, |y, row| {
        for (x, pixel) in row.iter_mut().enumerate() {
            let (fx, fy) = (x as f32, y as f32);
            let red = image.sample(fx - dx, fy - dy);
            let green = image.data[y * width + x];
            let blue = image.sample(fx + dx, fy + dy);
            let alpha = red[3].max(green[3]).max(blue[3]);
            *pixel = [red[0], green[1], blue[2], alpha];
        }
    });
    Premul {
        width,
        height,
        data: out,
    }
    .write_rgba(rgba);
}

fn mosaic(rgba: &mut [u8], width: usize, height: usize, size: u32, origin: (i64, i64)) {
    let size = size as i64;
    if size <= 1 {
        return;
    }
    let image = Premul::from_rgba(rgba, width, height);
    let cell = |v: i64| v.div_euclid(size);
    let (cx0, cy0) = (cell(origin.0), cell(origin.1));
    let (cx1, cy1) = (
        cell(origin.0 + width as i64 - 1),
        cell(origin.1 + height as i64 - 1),
    );
    let mut out = image.data.clone();
    for cy in cy0..=cy1 {
        for cx in cx0..=cx1 {
            let x0 = (cx * size - origin.0).max(0) as usize;
            let y0 = (cy * size - origin.1).max(0) as usize;
            let x1 = ((cx + 1) * size - origin.0).min(width as i64) as usize;
            let y1 = ((cy + 1) * size - origin.1).min(height as i64) as usize;
            let mut sum = [0f32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let v = image.data[y * width + x];
                    for c in 0..4 {
                        sum[c] += v[c];
                    }
                }
            }
            let count = ((x1 - x0) * (y1 - y0)).max(1) as f32;
            let mean = sum.map(|s| s / count);
            for y in y0..y1 {
                for x in x0..x1 {
                    out[y * width + x] = mean;
                }
            }
        }
    }
    Premul {
        width,
        height,
        data: out,
    }
    .write_rgba(rgba);
}

fn hash(x: i64, y: i64, seed: u32, channel: u32) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ ((seed as u64) << 32 | channel as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn noise(rgba: &mut [u8], width: usize, amount: f32, mono: bool, seed: u32, origin: (i64, i64)) {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return;
    }
    for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
        if p[3] == 0 {
            continue;
        }
        let (x, y) = (origin.0 + (i % width) as i64, origin.1 + (i / width) as i64);
        for (c, value) in p.iter_mut().take(3).enumerate() {
            let channel = if mono { 0 } else { c as u32 };
            // Triangular noise (sum of two), -1..=1.
            let n = hash(x, y, seed, channel * 2) + hash(x, y, seed, channel * 2 + 1) - 1.0;
            let v = *value as f32 / 255.0 + n * amount * 0.5;
            *value = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

/// Makes lines thicker (positive `amount`) or thinner. On a transparent
/// layer the painted shapes grow or shrink; on an opaque picture dark lines
/// on a light ground do.
fn line_width(rgba: &mut [u8], width: usize, height: usize, amount: i32) {
    if amount == 0 {
        return;
    }
    let transparent = rgba.chunks_exact(4).any(|p| p[3] < 255);
    let grow = amount > 0;
    let mut current = rgba[..width * height * 4].to_vec();
    for step in 0..amount.unsigned_abs() {
        // Square and cross steps in turn grow an octagon (close to round).
        let offsets: &[(isize, isize)] = if step % 2 == 0 {
            &[(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)]
        } else {
            &[
                (0, 0),
                (-1, 0),
                (1, 0),
                (0, -1),
                (0, 1),
                (-1, -1),
                (1, -1),
                (-1, 1),
                (1, 1),
            ]
        };
        let source = current.clone();
        let mut out = vec![[0u8; 4]; width * height];
        par_rows(&mut out, width, |y, row| {
            for (x, pixel) in row.iter_mut().enumerate() {
                let at = |dx: isize, dy: isize| {
                    let sx = (x as isize + dx).clamp(0, width as isize - 1) as usize;
                    let sy = (y as isize + dy).clamp(0, height as isize - 1) as usize;
                    let i = (sy * width + sx) * 4;
                    [source[i], source[i + 1], source[i + 2], source[i + 3]]
                };
                let own = at(0, 0);
                *pixel = if transparent {
                    if grow {
                        // The most opaque neighbour, with its colour.
                        offsets
                            .iter()
                            .map(|&(dx, dy)| at(dx, dy))
                            .max_by_key(|p| p[3])
                            .unwrap_or(own)
                    } else {
                        let alpha = offsets.iter().map(|&(dx, dy)| at(dx, dy)[3]).min();
                        [own[0], own[1], own[2], alpha.unwrap_or(own[3])]
                    }
                } else {
                    let mut value = own;
                    for &(dx, dy) in offsets {
                        let p = at(dx, dy);
                        for c in 0..3 {
                            value[c] = if grow {
                                value[c].min(p[c])
                            } else {
                                value[c].max(p[c])
                            };
                        }
                    }
                    value
                };
            }
        });
        for (p, v) in current.chunks_exact_mut(4).zip(out) {
            p.copy_from_slice(&v);
        }
    }
    rgba[..width * height * 4].copy_from_slice(&current);
}

/// A smooth tone curve through five points at inputs 0, ¼, ½, ¾ and 1
/// (monotone cubic: no overshoot between points).
pub fn tone_curve_value(points: [f32; 5], x: f32) -> f32 {
    let y = points.map(|v| v.clamp(0.0, 1.0));
    let h = 0.25f32;
    let delta: [f32; 4] = std::array::from_fn(|i| (y[i + 1] - y[i]) / h);
    let mut slope = [0f32; 5];
    slope[0] = delta[0];
    slope[4] = delta[3];
    for i in 1..4 {
        slope[i] = if delta[i - 1] * delta[i] <= 0.0 {
            0.0
        } else {
            // Harmonic mean keeps the curve monotone between points.
            2.0 / (1.0 / delta[i - 1] + 1.0 / delta[i])
        };
    }
    let x = x.clamp(0.0, 1.0);
    let i = ((x / h).floor() as usize).min(3);
    let t = (x - i as f32 * h) / h;
    let (t2, t3) = (t * t, t * t * t);
    let value = (2.0 * t3 - 3.0 * t2 + 1.0) * y[i]
        + (t3 - 2.0 * t2 + t) * h * slope[i]
        + (-2.0 * t3 + 3.0 * t2) * y[i + 1]
        + (t3 - t2) * h * slope[i + 1];
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: usize, height: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut out = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                out.extend_from_slice(&f(x, y));
            }
        }
        out
    }

    fn at(rgba: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * width + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    /// Black left half, white right half.
    fn edge() -> Vec<u8> {
        picture(40, 20, |x, _| {
            if x < 20 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        })
    }

    #[test]
    fn every_blur_softens_an_edge_and_keeps_flat_colour() {
        for kind in [BlurKind::Gaussian, BlurKind::Lens, BlurKind::Motion] {
            let mut rgba = edge();
            Filter::Blur {
                kind,
                amount: 6.0,
                angle: 0.0,
            }
            .apply(&mut rgba, 40, 20, (0, 0));
            let near = at(&rgba, 40, 19, 10)[0];
            assert!(near > 20 && near < 235, "{kind:?}: {near}");
            assert_eq!(at(&rgba, 40, 2, 10)[0], 0, "{kind:?}");
            assert_eq!(at(&rgba, 40, 38, 10)[0], 255, "{kind:?}");
        }
    }

    #[test]
    fn blur_does_not_darken_the_edge_of_paint() {
        // Red paint on transparency: the blurred rim fades, it does not
        // turn dark.
        let mut rgba = picture(30, 10, |x, _| {
            if x < 15 {
                [230, 20, 20, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        Filter::Blur {
            kind: BlurKind::Gaussian,
            amount: 8.0,
            angle: 0.0,
        }
        .apply(&mut rgba, 30, 10, (0, 0));
        let rim = at(&rgba, 30, 17, 5);
        assert!(rim[3] > 0 && rim[3] < 255, "{rim:?}");
        assert!(rim[0] > 200, "{rim:?}");
    }

    #[test]
    fn smoothing_keeps_a_strong_edge() {
        let mut rgba = edge();
        Filter::Blur {
            kind: BlurKind::Smooth,
            amount: 5.0,
            angle: 0.0,
        }
        .apply(&mut rgba, 40, 20, (0, 0));
        assert!(at(&rgba, 40, 18, 10)[0] < 40);
        assert!(at(&rgba, 40, 21, 10)[0] > 215);
    }

    #[test]
    fn sharpening_increases_contrast_at_an_edge() {
        let mut rgba = picture(40, 10, |x, _| {
            let v = if x < 20 { 100 } else { 160 };
            [v, v, v, 255]
        });
        Filter::Sharpen {
            amount: 2.0,
            radius: 2.0,
        }
        .apply(&mut rgba, 40, 10, (0, 0));
        assert!(at(&rgba, 40, 19, 5)[0] < 100);
        assert!(at(&rgba, 40, 20, 5)[0] > 160);
    }

    #[test]
    fn denoise_removes_a_speck() {
        let mut rgba = picture(9, 9, |x, y| {
            if (x, y) == (4, 4) {
                [255, 255, 255, 255]
            } else {
                [50, 50, 50, 255]
            }
        });
        Filter::Denoise { strength: 1 }.apply(&mut rgba, 9, 9, (0, 0));
        assert_eq!(at(&rgba, 9, 4, 4), [50, 50, 50, 255]);
    }

    #[test]
    fn chromatic_aberration_splits_red_and_blue() {
        let mut rgba = picture(20, 5, |x, _| {
            if x == 10 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        Filter::ChromaticAberration {
            direction: Direction::Horizontal,
            amount: 2.0,
        }
        .apply(&mut rgba, 20, 5, (0, 0));
        assert_eq!(at(&rgba, 20, 12, 2), [255, 0, 0, 255]);
        assert_eq!(at(&rgba, 20, 8, 2), [0, 0, 255, 255]);
        assert_eq!(at(&rgba, 20, 10, 2), [0, 255, 0, 255]);
    }

    #[test]
    fn mosaic_cells_are_flat_and_line_up_with_the_document() {
        let mut rgba = picture(16, 16, |x, y| [(x * 16) as u8, (y * 16) as u8, 0, 255]);
        Filter::Mosaic { size: 4 }.apply(&mut rgba, 16, 16, (2, 0));
        // Cells start at document x = 0, 4, 8…: here at local x = 2, 6….
        assert_eq!(at(&rgba, 16, 2, 0), at(&rgba, 16, 5, 3));
        assert_ne!(at(&rgba, 16, 1, 0), at(&rgba, 16, 2, 0));
    }

    #[test]
    fn noise_changes_only_painted_pixels_and_mono_stays_grey() {
        let mut rgba = picture(8, 8, |x, _| {
            if x < 4 {
                [128, 128, 128, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        Filter::Noise {
            amount: 0.5,
            mono: true,
            seed: 7,
        }
        .apply(&mut rgba, 8, 8, (0, 0));
        assert!((0..4).any(|x| at(&rgba, 8, x, 3)[0] != 128));
        for x in 0..4 {
            let p = at(&rgba, 8, x, 3);
            assert!(p[0] == p[1] && p[1] == p[2]);
        }
        assert_eq!(at(&rgba, 8, 6, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn line_width_grows_and_shrinks_lines() {
        // A 3 px transparent-background line.
        let line = picture(20, 20, |x, _| {
            if (9..12).contains(&x) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let mut thick = line.clone();
        Filter::LineWidth { amount: 2 }.apply(&mut thick, 20, 20, (0, 0));
        assert_eq!(at(&thick, 20, 7, 10)[3], 255);
        let mut thin = line.clone();
        Filter::LineWidth { amount: -1 }.apply(&mut thin, 20, 20, (0, 0));
        assert_eq!(at(&thin, 20, 9, 10)[3], 0);
        assert_eq!(at(&thin, 20, 10, 10)[3], 255);
        // Dark lines on white paper.
        let mut paper = picture(20, 20, |x, _| {
            if x == 10 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        Filter::LineWidth { amount: 1 }.apply(&mut paper, 20, 20, (0, 0));
        assert_eq!(at(&paper, 20, 11, 10), [0, 0, 0, 255]);
    }

    #[test]
    fn every_grade_changes_colour_and_strength_zero_does_not() {
        for (preset, name) in GRADE_NAMES.iter().enumerate() {
            let original = picture(4, 4, |x, y| {
                [60 + x as u8 * 40, 120, 40 + y as u8 * 50, 255]
            });
            let mut graded = original.clone();
            Filter::Grade {
                preset,
                strength: 1.0,
            }
            .apply(&mut graded, 4, 4, (0, 0));
            assert_ne!(graded, original, "{}", name.1);
            let mut none = original.clone();
            Filter::Grade {
                preset,
                strength: 0.0,
            }
            .apply(&mut none, 4, 4, (0, 0));
            assert_eq!(none, original);
        }
    }

    #[test]
    fn tone_curve_is_smooth_and_passes_its_points() {
        let points = [0.0, 0.1, 0.5, 0.9, 1.0];
        for (i, &p) in points.iter().enumerate() {
            assert!((tone_curve_value(points, i as f32 / 4.0) - p).abs() < 1e-5);
        }
        // Smooth: no kink at the middle point (slopes match on both sides).
        let e = 1e-3;
        let left = (tone_curve_value(points, 0.5) - tone_curve_value(points, 0.5 - e)) / e;
        let right = (tone_curve_value(points, 0.5 + e) - tone_curve_value(points, 0.5)) / e;
        assert!((left - right).abs() < 0.05, "{left} {right}");
        // Monotone data gives a monotone curve.
        let mut last = 0.0;
        for i in 0..=100 {
            let v = tone_curve_value(points, i as f32 / 100.0);
            assert!(v + 1e-6 >= last);
            last = v;
        }
    }

    /// Timing on an A4 350 dpi page: `cargo test --release -p efude-canvas
    /// filter_speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn filter_speed() {
        let (w, h) = (2894, 4093);
        let base = picture(w, h, |x, y| [(x % 256) as u8, (y % 256) as u8, 90, 255]);
        let filters = [
            Filter::Blur {
                kind: BlurKind::Gaussian,
                amount: 50.0,
                angle: 0.0,
            },
            Filter::Blur {
                kind: BlurKind::Lens,
                amount: 30.0,
                angle: 0.0,
            },
            Filter::Blur {
                kind: BlurKind::Smooth,
                amount: 10.0,
                angle: 0.0,
            },
            Filter::Blur {
                kind: BlurKind::Motion,
                amount: 60.0,
                angle: 30.0,
            },
            Filter::Sharpen {
                amount: 1.0,
                radius: 3.0,
            },
            Filter::Denoise { strength: 2 },
            Filter::Grade {
                preset: 6,
                strength: 1.0,
            },
            Filter::ChromaticAberration {
                direction: Direction::DiagonalDown,
                amount: 5.0,
            },
            Filter::Mosaic { size: 20 },
            Filter::Noise {
                amount: 0.2,
                mono: false,
                seed: 1,
            },
            Filter::LineWidth { amount: 4 },
        ];
        for filter in filters {
            let mut rgba = base.clone();
            let start = std::time::Instant::now();
            filter.apply(&mut rgba, w, h, (0, 0));
            eprintln!("{filter:?}: {:?}", start.elapsed());
        }
    }
}
