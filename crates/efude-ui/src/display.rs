// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! How the canvas reaches the screen without changing its pixels: exact
//! pixels when zoomed in (no smoothing), 100% meaning one document pixel
//! per physical screen pixel, the picture aligned to the pixel grid, and
//! mip levels (averaged in linear light) when zoomed out so thin lines and
//! tones do not break up. The GPU display builds its levels on the GPU;
//! this module keeps them on the CPU for the fallback display.

use super::*;

/// Texture options of the full-size canvas: nearest pixel when magnified.
pub(crate) const CANVAS_TEXTURE: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: None,
};

/// A reduced level: each is half the size of the one before.
struct Level {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    texture: Option<egui::TextureHandle>,
    /// Region changed since the last upload (`[x0, y0, x1, y1)`).
    dirty: Option<[u32; 4]>,
}

/// Reduced copies of the composite for the CPU display.
#[derive(Default)]
pub(crate) struct CpuMips {
    size: (u32, u32),
    levels: Vec<Level>,
}

impl CpuMips {
    fn ensure(&mut self, width: u32, height: u32) {
        if self.size == (width, height) {
            return;
        }
        self.size = (width, height);
        self.levels.clear();
        let (mut w, mut h) = (width, height);
        while w.max(h) > 32 && self.levels.len() < 12 {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            self.levels.push(Level {
                width: w,
                height: h,
                pixels: vec![255; (w * h * 4) as usize],
                texture: None,
                dirty: Some([0, 0, w, h]),
            });
        }
    }

    /// Recomputes the levels under a block of the full-size composite.
    pub fn update_block(
        &mut self,
        block: &[u8],
        width: u32,
        height: u32,
        x0: u32,
        y0: u32,
        size: (u32, u32),
    ) {
        self.ensure(size.0, size.1);
        let (mut src, mut w, mut h, mut x, mut y) = (block.to_vec(), width, height, x0, y0);
        for level in &mut self.levels {
            efude_canvas::downsample_half_into(
                &src,
                w,
                h,
                x,
                y,
                &mut level.pixels,
                level.width,
                level.height,
            );
            let (tx0, ty0) = (x / 2, y / 2);
            let tx1 = (x + w).div_ceil(2).min(level.width);
            let ty1 = (y + h).div_ceil(2).min(level.height);
            if tx1 <= tx0 || ty1 <= ty0 {
                break;
            }
            level.dirty = Some(match level.dirty {
                Some([a, b, c, d]) => [a.min(tx0), b.min(ty0), c.max(tx1), d.max(ty1)],
                None => [tx0, ty0, tx1, ty1],
            });
            // The next level reads this level's updated block, widened to
            // even coordinates so every 2×2 group is complete.
            let (ex0, ey0) = (tx0 & !1, ty0 & !1);
            let (ex1, ey1) = (
                (tx1 + (tx1 & 1)).min(level.width),
                (ty1 + (ty1 & 1)).min(level.height),
            );
            let (nw, nh) = (ex1 - ex0, ey1 - ey0);
            let mut next = vec![0u8; (nw * nh * 4) as usize];
            for row in 0..nh {
                let from = (((ey0 + row) * level.width + ex0) * 4) as usize;
                let to = (row * nw * 4) as usize;
                next[to..to + (nw * 4) as usize]
                    .copy_from_slice(&level.pixels[from..from + (nw * 4) as usize]);
            }
            (src, w, h, x, y) = (next, nw, nh, ex0, ey0);
        }
    }

    /// Uploads changed levels and returns the texture to show at
    /// `physical_scale` screen pixels per document pixel (`None`: the full
    /// size texture).
    pub fn texture_for(
        &mut self,
        ctx: &egui::Context,
        physical_scale: f32,
    ) -> Option<egui::TextureId> {
        if physical_scale >= 1.0 || self.levels.is_empty() {
            return None;
        }
        let wanted = ((1.0 / physical_scale).log2().floor() as usize).clamp(1, self.levels.len());
        let level = &mut self.levels[wanted - 1];
        let options = egui::TextureOptions::LINEAR;
        match (&mut level.texture, level.dirty.take()) {
            (None, _) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [level.width as usize, level.height as usize],
                    &level.pixels,
                );
                level.texture =
                    Some(ctx.load_texture(format!("canvas-mip-{wanted}"), image, options));
            }
            (Some(texture), Some([x0, y0, x1, y1])) => {
                let (w, h) = (x1 - x0, y1 - y0);
                let mut part = Vec::with_capacity((w * h * 4) as usize);
                for row in y0..y1 {
                    let from = ((row * level.width + x0) * 4) as usize;
                    part.extend_from_slice(&level.pixels[from..from + (w * 4) as usize]);
                }
                texture.set_partial(
                    [x0 as usize, y0 as usize],
                    egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &part),
                    options,
                );
            }
            (Some(_), None) => {}
        }
        level.texture.as_ref().map(|t| t.id())
    }
}

/// Screen points per document pixel for `zoom`, where 100% is one document
/// pixel per physical screen pixel and "fit" never enlarges past 100%.
pub(crate) fn view_scale(
    available: Vec2,
    document: (u32, u32),
    zoom: f32,
    pixels_per_point: f32,
) -> (f32, f32) {
    let ppp = pixels_per_point.max(0.1);
    let fit = (available.x / document.0 as f32)
        .min(available.y / document.1 as f32)
        .min(1.0 / ppp);
    (fit.max(0.01 / ppp), fit.max(0.01 / ppp) * zoom)
}

/// Moves `rect` (the unrotated canvas) by less than a pixel so its corner
/// sits on the physical pixel grid, which keeps pixels crisp at whole-number
/// zoom levels.
pub(crate) fn snap_to_pixels(rect: Rect, pixels_per_point: f32) -> Rect {
    let ppp = pixels_per_point.max(0.1);
    let corner = rect.min.to_vec2() * ppp;
    let snapped = Vec2::new(corner.x.round(), corner.y.round());
    rect.translate((snapped - corner) / ppp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_hundred_percent_is_one_physical_pixel() {
        // 150% UI scale: one document pixel is 1/1.5 point.
        let (_, scale) = view_scale(Vec2::new(4000.0, 4000.0), (100, 100), 1.0, 1.5);
        assert!((scale * 1.5 - 1.0).abs() < 1e-6);
        let (_, scale) = view_scale(Vec2::new(400.0, 400.0), (1000, 1000), 2.0, 1.0);
        assert!((scale - 0.8).abs() < 1e-6);
    }

    #[test]
    fn snapping_moves_less_than_a_pixel() {
        let rect = Rect::from_min_size(Pos2::new(10.3, 20.7), Vec2::splat(50.0));
        let snapped = snap_to_pixels(rect, 1.25);
        let corner = snapped.min.to_vec2() * 1.25;
        assert!((corner.x - corner.x.round()).abs() < 1e-4);
        assert!((corner.y - corner.y.round()).abs() < 1e-4);
        assert!((snapped.min - rect.min).length() < 1.0);
    }

    #[test]
    fn mips_follow_block_updates() {
        let mut mips = CpuMips::default();
        let size = (512, 512);
        let white = [255u8; 4].repeat(512 * 512);
        mips.update_block(&white, 512, 512, 0, 0, size);
        let black = [0u8, 0, 0, 255].repeat(256 * 256);
        mips.update_block(&black, 256, 256, 256, 256, size);
        let level = &mips.levels[0];
        let at = |x: u32, y: u32| level.pixels[((y * level.width + x) * 4) as usize];
        assert_eq!(at(10, 10), 255);
        assert_eq!(at(200, 200), 0);
        let deepest = mips.levels.last().unwrap();
        assert!(deepest.width <= 32);
    }
}
