// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Books: pages in reading order, spreads, page numbers, and the pixel
//! steps of print export (cropping, crop marks, monochrome, spreads).

use crate::page::{Binding, PageGeometry, PageSpec};
use crate::raster::Coverage;
use serde::{Deserialize, Serialize};

/// File extension of a book file.
pub const EXTENSION: &str = "efudebook";

/// One page: its `.efude` file, relative to the book file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BookPage {
    pub file: String,
}

/// Where page numbers go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NombrePosition {
    /// Bottom corner on the outer side (away from the binding).
    #[default]
    BottomOuter,
    BottomCenter,
    /// Near the binding, low on the page (hidden once bound).
    BottomInner,
}

/// Page number settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nombre {
    pub enabled: bool,
    /// Number of the first page.
    pub start: u32,
    pub size_points: f32,
    pub position: NombrePosition,
    /// Leave the first page (the cover or title) unnumbered.
    pub skip_first: bool,
}

impl Default for Nombre {
    fn default() -> Self {
        Self {
            enabled: true,
            start: 1,
            size_points: 8.0,
            position: NombrePosition::BottomOuter,
            skip_first: false,
        }
    }
}

/// A book: page setup shared by every page, and the pages in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Book {
    pub version: u32,
    pub title: String,
    pub page: PageSpec,
    pub pages: Vec<BookPage>,
    #[serde(default)]
    pub nombre: Nombre,
}

impl Book {
    pub fn new(title: &str, page: PageSpec) -> Self {
        Self {
            version: 1,
            title: title.into(),
            page,
            pages: Vec::new(),
            nombre: Nombre::default(),
        }
    }

    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Whether page `index` (0-based) is a right-hand page. The first page
    /// faces the inside of the front cover: on the left in a right-bound
    /// book, on the right in a left-bound one; pages then alternate.
    pub fn is_right_page(&self, index: usize) -> bool {
        let first_on_right = self.page.binding == Binding::Left;
        (index & 1 == 0) == first_on_right
    }

    /// The page spec of page `index` (with its side).
    pub fn page_spec(&self, index: usize) -> PageSpec {
        PageSpec {
            right_page: self.is_right_page(index),
            ..self.page.clone()
        }
    }

    /// Pages as they face each other when the book is open, each spread as
    /// (left page, right page). The first page stands alone.
    pub fn spreads(&self) -> Vec<(Option<usize>, Option<usize>)> {
        let mut spreads = Vec::new();
        let count = self.pages.len();
        let mut index = 0;
        while index < count {
            let (a, b) = if index == 0 {
                (Some(0), None)
            } else {
                (Some(index), (index + 1 < count).then_some(index + 1))
            };
            // Reading order a then b; place by side.
            let mut left = None;
            let mut right = None;
            for page in [a, b].into_iter().flatten() {
                if self.is_right_page(page) {
                    right = Some(page);
                } else {
                    left = Some(page);
                }
            }
            spreads.push((left, right));
            index += if index == 0 { 1 } else { 2 };
        }
        spreads
    }

    /// The page number printed on page `index`, if any.
    pub fn nombre_of(&self, index: usize) -> Option<u32> {
        (self.nombre.enabled && !(self.nombre.skip_first && index == 0))
            .then(|| self.nombre.start + index as u32)
    }

    /// File name for page `number` (1-based) of a new book.
    pub fn page_file_name(number: usize) -> String {
        format!("{number:03}.efude")
    }
}

/// An RGBA image (straight alpha).
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32, color: [u8; 4]) -> Self {
        Self {
            width,
            height,
            rgba: color.repeat((width * height) as usize),
        }
    }

    fn at(&self, x: u32, y: u32) -> usize {
        ((y * self.width + x) * 4) as usize
    }

    /// The part inside `[x0, y0, x1, y1)`.
    pub fn crop(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Image {
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        let (x0, y0) = (x0.min(x1), y0.min(y1));
        let mut out = Image::new(x1 - x0, y1 - y0, [0; 4]);
        for y in 0..out.height {
            let src = self.at(x0, y0 + y);
            let dst = out.at(0, y);
            let len = (out.width * 4) as usize;
            out.rgba[dst..dst + len].copy_from_slice(&self.rgba[src..src + len]);
        }
        out
    }

    /// Copies `other` with its top-left at (`x`, `y`).
    pub fn paste(&mut self, other: &Image, x: u32, y: u32) {
        for row in 0..other.height.min(self.height.saturating_sub(y)) {
            let len = (other.width.min(self.width.saturating_sub(x)) * 4) as usize;
            let src = other.at(0, row);
            let dst = self.at(x, y + row);
            self.rgba[dst..dst + len].copy_from_slice(&other.rgba[src..src + len]);
        }
    }

    /// Draws `coverage` (placed at its x0/y0 plus `offset`) in `color`.
    pub fn draw(&mut self, coverage: &Coverage, offset: (i64, i64), color: [u8; 3]) {
        for y in 0..coverage.height {
            for x in 0..coverage.width {
                let value = coverage.data[(y * coverage.width + x) as usize];
                if value == 0 {
                    continue;
                }
                let (px, py) = (
                    coverage.x0 as i64 + x as i64 + offset.0,
                    coverage.y0 as i64 + y as i64 + offset.1,
                );
                if px < 0 || py < 0 || px >= self.width as i64 || py >= self.height as i64 {
                    continue;
                }
                let i = self.at(px as u32, py as u32);
                let a = value as f32 / 255.0;
                for (c, &channel) in color.iter().enumerate() {
                    self.rgba[i + c] =
                        (channel as f32 * a + self.rgba[i + c] as f32 * (1.0 - a)).round() as u8;
                }
                self.rgba[i + 3] = self.rgba[i + 3].max(value);
            }
        }
    }

    /// Solid rectangle, used for crop mark strokes.
    fn fill_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
        let (x0, y0) = (x0.floor().max(0.0) as u32, y0.floor().max(0.0) as u32);
        let (x1, y1) = (
            (x1.ceil().max(0.0) as u32).min(self.width),
            (y1.ceil().max(0.0) as u32).min(self.height),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                let i = self.at(x, y);
                self.rgba[i..i + 4].copy_from_slice(&color);
            }
        }
    }
}

/// How much of the page is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportArea {
    /// The finished size.
    Trim,
    /// Trim plus bleed (the whole canvas).
    #[default]
    Bleed,
    /// The canvas on a larger sheet with crop marks around it.
    WithMarks,
}

/// Colour of the written pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportColor {
    #[default]
    Color,
    Grayscale,
    /// Pure black and white at a threshold (0..255).
    Monochrome,
}

/// Print export settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportOptions {
    pub area: ExportArea,
    pub color: ExportColor,
    pub threshold: u8,
    /// Write facing pages side by side.
    pub spreads: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            area: ExportArea::Bleed,
            color: ExportColor::Color,
            threshold: 128,
            spreads: false,
        }
    }
}

/// Flattens `image` on white paper and converts its colour.
pub fn convert_color(image: &mut Image, color: ExportColor, threshold: u8) {
    for pixel in image.rgba.chunks_exact_mut(4) {
        let a = pixel[3] as f32 / 255.0;
        let rgb = [0, 1, 2].map(|c| pixel[c] as f32 * a + 255.0 * (1.0 - a));
        let luma = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
        let out = match color {
            ExportColor::Color => rgb,
            ExportColor::Grayscale => [luma; 3],
            ExportColor::Monochrome => [if luma >= threshold as f32 { 255.0 } else { 0.0 }; 3],
        };
        for (target, value) in pixel.iter_mut().zip(out) {
            *target = value.round() as u8;
        }
        pixel[3] = 255;
    }
}

/// Crops a page (a canvas of `geometry`) to `area`; with marks, the canvas
/// is placed on a larger sheet and crop marks are drawn around the trim.
pub fn page_area(image: &Image, geometry: &PageGeometry, area: ExportArea, dpi: f32) -> Image {
    match area {
        ExportArea::Bleed => image.clone(),
        ExportArea::Trim => {
            let [min, max] = geometry.trim;
            image.crop(
                min.x.round() as u32,
                min.y.round() as u32,
                max.x.round() as u32,
                max.y.round() as u32,
            )
        }
        ExportArea::WithMarks => {
            let margin = (geometry.mark_length + dpi / 25.4 * 3.0).ceil() as u32;
            let mut sheet = Image::new(
                image.width + 2 * margin,
                image.height + 2 * margin,
                [255, 255, 255, 255],
            );
            sheet.paste(image, margin, margin);
            let stroke = (dpi / 25.4 * 0.1).max(1.0);
            let offset = margin as f32;
            for [a, b] in geometry.crop_marks() {
                let (a, b) = (a + offset, b + offset);
                let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
                let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
                sheet.fill_rect(
                    x0 - stroke * 0.5,
                    y0 - stroke * 0.5,
                    x1 + stroke * 0.5,
                    y1 + stroke * 0.5,
                    [0, 0, 0, 255],
                );
            }
            sheet
        }
    }
}

/// Where page number text of size `text` goes on a page canvas: inside the
/// trim, between the trim edge and the inner frame.
pub fn nombre_origin(
    geometry: &PageGeometry,
    position: NombrePosition,
    right_page: bool,
    text: (u32, u32),
) -> (i64, i64) {
    let [trim_min, trim_max] = geometry.trim;
    let [inner_min, inner_max] = geometry.inner;
    let (w, h) = (text.0 as f32, text.1 as f32);
    // Centred in the bottom margin.
    let y = (inner_max.y + trim_max.y) * 0.5 - h * 0.5;
    // The outer side is away from the binding: left on a left-hand page.
    let outer_right = right_page;
    let x = match position {
        NombrePosition::BottomCenter => (trim_min.x + trim_max.x) * 0.5 - w * 0.5,
        NombrePosition::BottomOuter | NombrePosition::BottomInner => {
            let on_right = match position {
                NombrePosition::BottomOuter => outer_right,
                _ => !outer_right,
            };
            if on_right {
                (inner_max.x + trim_max.x) * 0.5 - w * 0.5
            } else {
                (trim_min.x + inner_min.x) * 0.5 - w * 0.5
            }
        }
    };
    (x.round() as i64, y.round() as i64)
}

/// Two pages side by side (either may be missing: blank paper).
pub fn spread(left: Option<&Image>, right: Option<&Image>) -> Option<Image> {
    let sample = left.or(right)?;
    let (w, h) = (sample.width, sample.height);
    let mut out = Image::new(w * 2, h, [255, 255, 255, 255]);
    if let Some(left) = left {
        out.paste(left, 0, 0);
    }
    if let Some(right) = right {
        out.paste(right, w, 0);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(pages: usize, binding: Binding) -> Book {
        let mut spec = PageSpec::presets()[1].2.clone();
        spec.binding = binding;
        spec.dpi = 100.0;
        let mut book = Book::new("test", spec);
        for n in 1..=pages {
            book.pages.push(BookPage {
                file: Book::page_file_name(n),
            });
        }
        book
    }

    #[test]
    fn right_bound_books_start_on_the_left_and_read_right_to_left() {
        let b = book(5, Binding::Right);
        assert!(!b.is_right_page(0) && b.is_right_page(1) && !b.is_right_page(2));
        // Spreads: [1], then 2 on the right with 3 on the left, ...
        assert_eq!(
            b.spreads(),
            vec![(Some(0), None), (Some(2), Some(1)), (Some(4), Some(3))]
        );
        let l = book(4, Binding::Left);
        assert!(l.is_right_page(0));
        assert_eq!(
            l.spreads(),
            vec![(None, Some(0)), (Some(1), Some(2)), (Some(3), None)]
        );
        // The page spec follows the side (the binding margin moves).
        assert!(!b.page_spec(0).right_page && b.page_spec(1).right_page);
    }

    #[test]
    fn nombre_numbers_and_skips() {
        let mut b = book(3, Binding::Right);
        assert_eq!(b.nombre_of(0), Some(1));
        b.nombre.skip_first = true;
        b.nombre.start = 3;
        assert_eq!(b.nombre_of(0), None);
        assert_eq!(b.nombre_of(2), Some(5));
        let json = b.to_json();
        assert_eq!(Book::from_json(&json).unwrap(), b);
    }

    #[test]
    fn export_areas_crop_and_mark() {
        let b = book(1, Binding::Right);
        let g = b.page.geometry();
        let image = Image::new(g.canvas_width, g.canvas_height, [200, 10, 10, 255]);
        let trim = page_area(&image, &g, ExportArea::Trim, b.page.dpi);
        assert_eq!(
            (trim.width, trim.height),
            (
                (g.trim[1].x.round() - g.trim[0].x.round()) as u32,
                (g.trim[1].y.round() - g.trim[0].y.round()) as u32
            )
        );
        let marked = page_area(&image, &g, ExportArea::WithMarks, b.page.dpi);
        assert!(marked.width > image.width && marked.height > image.height);
        let black = marked
            .rgba
            .chunks_exact(4)
            .filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
            .count();
        assert!(black > 50, "crop marks drawn: {black}");
    }

    #[test]
    fn monochrome_thresholds_on_white_paper() {
        let mut image = Image::new(2, 1, [0, 0, 0, 0]);
        image.rgba[0..4].copy_from_slice(&[30, 30, 30, 255]);
        convert_color(&mut image, ExportColor::Monochrome, 128);
        assert_eq!(&image.rgba[..], &[0, 0, 0, 255, 255, 255, 255, 255]);
    }

    #[test]
    fn nombre_sits_in_the_bottom_margin_on_the_outer_side() {
        let b = book(2, Binding::Right);
        let g = b.page_spec(0).geometry();
        let (x, y) = nombre_origin(&g, NombrePosition::BottomOuter, false, (20, 10));
        assert!(y as f32 > g.inner[1].y && (y as f32 + 10.0) < g.trim[1].y);
        // Left-hand page: the outer side is the left.
        assert!((x as f32) < g.inner[0].x);
        let g = b.page_spec(1).geometry();
        let (x, _) = nombre_origin(&g, NombrePosition::BottomOuter, true, (20, 10));
        assert!(x as f32 > g.inner[1].x);
    }
}
