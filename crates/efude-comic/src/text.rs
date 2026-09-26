// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Text: horizontal lines, or vertical columns read right to left, rendered
//! with a font installed on the system (no fonts are bundled).

use crate::raster::Coverage;
use ab_glyph::{Font, FontRef, GlyphId, ScaleFont, point};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How text is set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    /// Em size in canvas pixels.
    pub size: f32,
    pub vertical: bool,
    /// Distance between lines (or columns) in ems.
    pub line_spacing: f32,
    /// Extra space between characters in ems (may be negative).
    #[serde(default)]
    pub letter_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 40.0,
            vertical: true,
            line_spacing: 1.3,
            letter_spacing: 0.0,
        }
    }
}

/// Points to pixels at `dpi`.
pub fn points_to_pixels(points: f32, dpi: f32) -> f32 {
    points / 72.0 * dpi
}

/// How a character is drawn in vertical text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vertical {
    /// Drawn as is.
    Upright(char),
    /// Turned 90° clockwise (long vowel mark, dashes, Latin brackets).
    Rotated(char),
    /// Moved to the upper right of its cell (comma, full stop).
    UpperRight(char),
    /// Nudged up and right a little (small kana).
    Small(char),
}

/// The vertical presentation form of `c`, if Unicode has one.
fn vertical_form(c: char) -> Option<char> {
    Some(match c {
        '、' => '\u{FE11}',
        '。' => '\u{FE12}',
        '，' => '\u{FE10}',
        '：' => '\u{FE13}',
        '；' => '\u{FE14}',
        '！' => '\u{FE15}',
        '？' => '\u{FE16}',
        '「' => '\u{FE41}',
        '」' => '\u{FE42}',
        '『' => '\u{FE43}',
        '』' => '\u{FE44}',
        '（' => '\u{FE35}',
        '）' => '\u{FE36}',
        '｛' => '\u{FE37}',
        '｝' => '\u{FE38}',
        '〔' => '\u{FE39}',
        '〕' => '\u{FE3A}',
        '【' => '\u{FE3B}',
        '】' => '\u{FE3C}',
        '《' => '\u{FE3D}',
        '》' => '\u{FE3E}',
        '〈' => '\u{FE3F}',
        '〉' => '\u{FE40}',
        '［' => '\u{FE47}',
        '］' => '\u{FE48}',
        '…' => '\u{FE19}',
        '‥' => '\u{FE30}',
        '—' | '―' => '\u{FE31}',
        _ => return None,
    })
}

fn classify_vertical(font: &FontRef, c: char) -> Vertical {
    if let Some(form) = vertical_form(c)
        && font.glyph_id(form) != GlyphId(0)
    {
        return Vertical::Upright(form);
    }
    match c {
        'ー' | '〜' | '～' | '~' | '-' | '－' | '—' | '―' | '…' | '‥' | '(' | ')' | '[' | ']'
        | '「' | '」' | '『' | '』' | '（' | '）' | '【' | '】' | '〈' | '〉' | '《' | '》' => {
            Vertical::Rotated(c)
        }
        '、' | '。' | '，' | '．' | ',' | '.' => Vertical::UpperRight(c),
        'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'っ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ァ' | 'ィ'
        | 'ゥ' | 'ェ' | 'ォ' | 'ッ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ' | 'ヵ' | 'ヶ' => {
            Vertical::Small(c)
        }
        _ => Vertical::Upright(c),
    }
}

/// A growing coverage image in local pixels.
struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(width: f32, height: f32) -> Self {
        let (width, height) = (
            width.ceil().max(1.0) as u32 + 2,
            height.ceil().max(1.0) as u32 + 2,
        );
        Self {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    fn put(&mut self, x: i32, y: i32, value: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = (y as u32 * self.width + x as u32) as usize;
        let v = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.data[i] = self.data[i].max(v);
    }
}

struct Metrics {
    em: f32,
    ascent: f32,
}

fn metrics(font: &FontRef, em: f32) -> (ab_glyph::PxScale, Metrics) {
    // `PxScale` is the ascent-to-descent height; convert from the em size.
    let units = font.units_per_em().unwrap_or(1000.0);
    let scale = ab_glyph::PxScale::from(em * font.height_unscaled() / units);
    // The ideographic em box: the baseline sits 0.88 em below its top.
    (
        scale,
        Metrics {
            em,
            ascent: em * 0.88,
        },
    )
}

#[allow(clippy::too_many_arguments)]
/// Draws glyph `id` with its advance box's top-left at (`x`, `top`),
/// optionally turned 90° clockwise around the em box.
fn draw_glyph(
    font: &FontRef,
    id: GlyphId,
    scale: ab_glyph::PxScale,
    m: &Metrics,
    x: f32,
    top: f32,
    rotate: bool,
    out: &mut Canvas,
) {
    let glyph = id.with_scale_and_position(scale, point(0.0, m.ascent));
    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    let advance = font.as_scaled(scale).h_advance(id);
    outlined.draw(|gx, gy, c| {
        // Position inside the advance box (0..advance, 0..em).
        let (bx, by) = (bounds.min.x + gx as f32, bounds.min.y + gy as f32);
        let (px, py) = if rotate {
            // Turned clockwise: the box becomes em wide, advance tall, and
            // is centred in the cell.
            (x + (m.em - by), top + bx + (m.em - advance) * 0.5)
        } else {
            (x + bx, top + by)
        };
        out.put(px.round() as i32, py.round() as i32, c);
    });
}

/// Renders `text` (lines separated by `\n`) with its layout box's top-left
/// at local (1, 1). Horizontal lines are centred on each other; vertical
/// columns run right to left and start at the top.
pub fn render(font: &FontRef, text: &str, style: &TextStyle) -> Option<Coverage> {
    let em = style.size.clamp(2.0, 2000.0);
    let (scale, m) = metrics(font, em);
    let scaled = font.as_scaled(scale);
    let lines: Vec<&str> = text.split('\n').collect();
    if text.trim().is_empty() {
        return None;
    }
    let pitch = em * style.line_spacing.clamp(0.5, 4.0);
    let spacing = em * style.letter_spacing.clamp(-0.5, 2.0);
    let mut canvas;
    if style.vertical {
        let column_length = |line: &str| line.chars().count() as f32 * (em + spacing) - spacing;
        let height = lines
            .iter()
            .map(|l| column_length(l))
            .fold(0.0f32, f32::max);
        let width = pitch * (lines.len() - 1) as f32 + em;
        canvas = Canvas::new(width, height);
        for (column, line) in lines.iter().enumerate() {
            // Rightmost column first.
            let x = 1.0 + width - em - column as f32 * pitch;
            for (row, c) in line.chars().enumerate() {
                let top = 1.0 + row as f32 * (em + spacing);
                let (shown, rotate, dx, dy) = match classify_vertical(font, c) {
                    Vertical::Upright(c) => (c, false, 0.0, 0.0),
                    Vertical::Rotated(c) => (c, true, 0.0, 0.0),
                    Vertical::UpperRight(c) => (c, false, em * 0.6, -em * 0.6),
                    Vertical::Small(c) => (c, false, em * 0.1, -em * 0.1),
                };
                let id = font.glyph_id(shown);
                let advance = scaled.h_advance(id);
                // Upright glyphs are centred across the column.
                let cx = if rotate { x } else { x + (em - advance) * 0.5 };
                draw_glyph(font, id, scale, &m, cx + dx, top + dy, rotate, &mut canvas);
            }
        }
    } else {
        let line_width = |line: &str| {
            let mut width = 0.0;
            let mut previous: Option<GlyphId> = None;
            for c in line.chars() {
                let id = font.glyph_id(c);
                if let Some(p) = previous {
                    width += scaled.kern(p, id);
                }
                width += scaled.h_advance(id) + spacing;
                previous = Some(id);
            }
            (width - spacing).max(0.0)
        };
        let width = lines.iter().map(|l| line_width(l)).fold(0.0f32, f32::max);
        let height = pitch * (lines.len() - 1) as f32 + em;
        canvas = Canvas::new(width, height);
        for (row, line) in lines.iter().enumerate() {
            let mut x = 1.0 + (width - line_width(line)) * 0.5;
            let top = 1.0 + row as f32 * pitch;
            let mut previous: Option<GlyphId> = None;
            for c in line.chars() {
                let id = font.glyph_id(c);
                if let Some(p) = previous {
                    x += scaled.kern(p, id);
                }
                draw_glyph(font, id, scale, &m, x, top, false, &mut canvas);
                x += scaled.h_advance(id) + spacing;
                previous = Some(id);
            }
        }
    }
    Some(Coverage {
        x0: 0,
        y0: 0,
        width: canvas.width,
        height: canvas.height,
        data: canvas.data,
    })
}

/// Renders `text` with face `index` of the font file `data`.
pub fn render_with(data: &[u8], index: u32, text: &str, style: &TextStyle) -> Option<Coverage> {
    let font = FontRef::try_from_slice_and_index(data, index).ok()?;
    render(&font, text, style)
}

/// A font face installed on the system.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FontInfo {
    /// Family and style, preferring the Japanese name.
    pub name: String,
    pub path: PathBuf,
    /// Face index inside a collection (.ttc).
    pub index: u32,
}

fn be_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
        *data.get(at + 2)?,
        *data.get(at + 3)?,
    ]))
}

/// Reads `length` bytes at `offset` of an open file.
fn read_at(file: &mut std::fs::File, offset: u64, length: usize) -> Option<Vec<u8>> {
    if length > 4 * 1024 * 1024 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0; length];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

/// Name of the face whose table directory is at `offset`, reading only the
/// directory and the naming table (fonts can be large).
fn face_name(file: &mut std::fs::File, offset: u64) -> Option<String> {
    let header = read_at(file, offset, 12)?;
    let tables = be_u16(&header, 4)? as usize;
    let directory = read_at(file, offset + 12, tables * 16)?;
    let mut has_cjk_or_latin = false;
    let mut name = None;
    for record in directory.chunks_exact(16) {
        let tag = &record[..4];
        if tag == b"cmap" {
            has_cjk_or_latin = true;
        }
        if tag == b"name" {
            let (table_offset, length) = (be_u32(record, 8)?, be_u32(record, 12)?);
            let data = read_at(file, table_offset as u64, length as usize)?;
            let table = ttf_parser::name::Table::parse(&data)?;
            let pick = |ids: &[u16]| {
                let mut best: Option<(u8, String)> = None;
                for record in table.names {
                    if !ids.contains(&record.name_id) {
                        continue;
                    }
                    let Some(text) = record.to_string() else {
                        continue;
                    };
                    // Japanese first, then English, then anything.
                    let rank = match record.language_id {
                        0x0411 => 0,
                        0x0409 => 1,
                        _ => 2,
                    };
                    if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                        best = Some((rank, text));
                    }
                }
                best.map(|(_, text)| text)
            };
            let family = pick(&[16]).or_else(|| pick(&[1]))?;
            let style = pick(&[17]).or_else(|| pick(&[2])).unwrap_or_default();
            name = Some(
                if style.is_empty()
                    || ["Regular", "標準", "Normal", "Book"].contains(&style.as_str())
                {
                    family
                } else {
                    format!("{family} {style}")
                },
            );
        }
    }
    name.filter(|_| has_cjk_or_latin)
}

/// Faces in one font file.
pub fn faces_in(path: &Path) -> Vec<FontInfo> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Some(header) = read_at(&mut file, 0, 12) else {
        return Vec::new();
    };
    let offsets: Vec<u64> = if &header[..4] == b"ttcf" {
        let count = be_u32(&header, 8).unwrap_or(0).min(64) as usize;
        read_at(&mut file, 12, count * 4)
            .map(|data| {
                data.chunks_exact(4)
                    .filter_map(|c| be_u32(c, 0).map(u64::from))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![0]
    };
    offsets
        .into_iter()
        .enumerate()
        .filter_map(|(index, offset)| {
            face_name(&mut file, offset).map(|name| FontInfo {
                name,
                path: path.to_path_buf(),
                index: index as u32,
            })
        })
        .collect()
}

/// Folders where the system keeps fonts.
pub fn font_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if cfg!(target_os = "windows") {
        let root = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        folders.push(root.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            folders.push(
                PathBuf::from(local)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Fonts"),
            );
        }
    } else if cfg!(target_os = "macos") {
        folders.push("/System/Library/Fonts".into());
        folders.push("/Library/Fonts".into());
        if let Some(home) = std::env::var_os("HOME") {
            folders.push(PathBuf::from(home).join("Library/Fonts"));
        }
    } else {
        folders.push("/usr/share/fonts".into());
        folders.push("/usr/local/share/fonts".into());
        if let Some(home) = std::env::var_os("HOME") {
            folders.push(PathBuf::from(home).join(".local/share/fonts"));
        }
    }
    folders
}

/// Every face in the system font folders, sorted by name.
pub fn system_fonts() -> Vec<FontInfo> {
    let mut found = Vec::new();
    let mut pending = font_folders();
    let mut visited = 0;
    while let Some(folder) = pending.pop() {
        visited += 1;
        if visited > 2000 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase);
            if matches!(extension.as_deref(), Some("ttf" | "otf" | "ttc" | "otc")) {
                found.extend(faces_in(&path));
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.dedup_by(|a, b| a.name == b.name);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An installed font that has the kana the tests draw (preferring
    /// common Japanese fonts), as its file and face index; `None` skips.
    fn test_font() -> Option<(Vec<u8>, u32)> {
        let has_kana = |data: &[u8], index: u32| {
            FontRef::try_from_slice_and_index(data, index)
                .is_ok_and(|font| "あいうえおー".chars().all(|c| font.glyph_id(c).0 != 0))
        };
        let mut fonts = system_fonts();
        let preferred = [
            "Noto Sans CJK JP",
            "Noto Sans JP",
            "Yu Gothic",
            "游ゴシック",
            "Meiryo",
            "メイリオ",
            "MS Gothic",
            "ＭＳ ゴシック",
            "Noto Sans CJK",
        ];
        fonts.sort_by_key(|f| {
            preferred
                .iter()
                .position(|name| f.name.contains(name))
                .unwrap_or(preferred.len())
        });
        fonts.iter().find_map(|info| {
            let data = std::fs::read(&info.path).ok()?;
            has_kana(&data, info.index).then_some((data, info.index))
        })
    }

    #[test]
    fn horizontal_and_vertical_text_have_the_right_shape() {
        let Some((data, index)) = test_font() else {
            eprintln!("no Japanese system font; skipping");
            return;
        };
        let font = FontRef::try_from_slice_and_index(&data, index).unwrap();
        let mut style = TextStyle {
            size: 30.0,
            vertical: false,
            ..TextStyle::default()
        };
        // Rightmost and lowest inked pixel inside a region (fonts differ in
        // width and line spacing, so only the layout is checked).
        fn reach(
            c: &Coverage,
            xs: std::ops::Range<u32>,
            ys: std::ops::Range<u32>,
        ) -> Option<(u32, u32)> {
            let mut found = None;
            for y in ys {
                for x in xs.clone() {
                    if c.data[(y * c.width + x) as usize] > 128 {
                        let (mx, my) = found.unwrap_or((0, 0));
                        found = Some((mx.max(x), my.max(y)));
                    }
                }
            }
            found
        }
        let h = render(&font, "あいう\nえお", &style).unwrap();
        assert!(h.data.iter().any(|&v| v > 200));
        // Two lines: the first (three characters) runs further right.
        let first = reach(&h, 0..h.width, 0..h.height / 2).expect("first line");
        let second = reach(&h, 0..h.width, h.height / 2..h.height).expect("second line");
        assert!(first.0 > second.0, "{first:?} {second:?}");
        style.vertical = true;
        let v = render(&font, "あいう\nえお", &style).unwrap();
        // Two columns read right to left: the right one (three characters)
        // runs further down, and the text is taller than the horizontal one.
        let right = reach(&v, v.width / 2..v.width, 0..v.height).expect("right column");
        let left = reach(&v, 0..v.width / 2, 0..v.height).expect("left column");
        assert!(right.1 > left.1, "{right:?} {left:?}");
        assert!(
            v.height > h.height,
            "{}×{} vs {}×{}",
            v.width,
            v.height,
            h.width,
            h.height
        );
        assert!(render(&font, "  ", &style).is_none());
    }

    #[test]
    fn long_vowel_mark_turns_in_vertical_text() {
        let Some((data, index)) = test_font() else {
            eprintln!("no Japanese system font; skipping");
            return;
        };
        let font = FontRef::try_from_slice_and_index(&data, index).unwrap();
        let style = TextStyle {
            size: 40.0,
            vertical: true,
            ..TextStyle::default()
        };
        let c = render(&font, "ー", &style).unwrap();
        // The mark is a horizontal bar upright; turned it is taller than wide.
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (u32::MAX, 0, u32::MAX, 0);
        for y in 0..c.height {
            for x in 0..c.width {
                if c.data[(y * c.width + x) as usize] > 128 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(
            max_y - min_y > (max_x - min_x) * 3,
            "{min_x}-{max_x} × {min_y}-{max_y}"
        );
    }

    #[test]
    fn points_convert_at_the_page_resolution() {
        assert!((points_to_pixels(9.0, 600.0) - 75.0).abs() < 1e-3);
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Writes sample text to `$EFUDE_TEXT_PREVIEW` (PGM) for a visual check.
    #[test]
    fn write_preview() {
        let Some(path) = std::env::var_os("EFUDE_TEXT_PREVIEW") else {
            return;
        };
        let fonts = system_fonts();
        let info = fonts
            .iter()
            .find(|f| f.name.contains("Noto Sans CJK JP"))
            .unwrap();
        let data = std::fs::read(&info.path).unwrap();
        let font = FontRef::try_from_slice_and_index(&data, info.index).unwrap();
        let style = TextStyle {
            size: 48.0,
            vertical: true,
            ..TextStyle::default()
        };
        let c = render(&font, "えっ、ちょっと\n待ってよー！\n「本当に？」…", &style).unwrap();
        let mut out = format!("P5 {} {} 255\n", c.width, c.height).into_bytes();
        out.extend(c.data.iter().map(|v| 255 - v));
        std::fs::write(path, out).unwrap();
    }
}
