// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Books in the UI: a book file lists page `.efude` files in reading order.
//! Pages open in canvas tabs; the book window shows spreads and exports
//! every page for print (see `docs/spec/comic.md`).

use super::*;
use efude_comic::book::{
    self, Book, BookPage, ExportArea, ExportColor, ExportOptions, Image, NombrePosition,
};
use efude_comic::text::{self, FontInfo, TextStyle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A running export.
struct ExportJob {
    done: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
}

/// State of the book window.
#[derive(Default)]
pub(crate) struct BookUi {
    pub open: bool,
    /// Book file and its contents.
    book: Option<(PathBuf, Book)>,
    new_title: String,
    new_pages: u32,
    thumbnails: std::collections::HashMap<String, Option<egui::TextureHandle>>,
    export: ExportOptions,
    job: Option<ExportJob>,
}

impl BookUi {
    pub fn new() -> Self {
        Self {
            new_title: String::new(),
            new_pages: 8,
            ..Self::default()
        }
    }
}

/// A blank page document laid out for page `index` of `book`.
fn blank_page(book: &Book, index: usize) -> Document {
    let spec = book.page_spec(index);
    let g = spec.geometry();
    let mut doc = Document::new(g.canvas_width, g.canvas_height);
    doc.dpi = spec.dpi;
    let comic = comic::ComicDoc {
        page: spec.clone(),
        layout: efude_comic::PanelLayout::for_dpi(spec.dpi),
    };
    doc.metadata.insert(
        "comic".into(),
        serde_json::to_string(&comic).unwrap_or_default(),
    );
    doc
}

fn page_path(book_path: &Path, page: &BookPage) -> PathBuf {
    book_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&page.file)
}

/// Renders page `index` for print: composite with tones, page number,
/// cropped to the export area, colour converted.
fn render_page(
    book: &Book,
    book_path: &Path,
    index: usize,
    options: &ExportOptions,
    font: Option<&(Arc<Vec<u8>>, u32)>,
) -> Result<Image, String> {
    let path = page_path(book_path, &book.pages[index]);
    let doc = efude_io::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let spec = doc
        .metadata
        .get("comic")
        .and_then(|text| serde_json::from_str::<comic::ComicDoc>(text).ok())
        .map(|comic| comic.page)
        .unwrap_or_else(|| book.page_spec(index));
    let geometry = spec.geometry();
    let mut image = Image {
        width: doc.width,
        height: doc.height,
        rgba: efude_canvas::composite(&doc),
    };
    if let (Some(number), Some((data, face))) = (book.nombre_of(index), font) {
        let style = TextStyle {
            size: text::points_to_pixels(book.nombre.size_points, spec.dpi),
            vertical: false,
            line_spacing: 1.0,
            letter_spacing: 0.0,
        };
        if let Some(coverage) = text::render_with(data, *face, &number.to_string(), &style) {
            let origin = book::nombre_origin(
                &geometry,
                book.nombre.position,
                spec.right_page,
                (coverage.width, coverage.height),
            );
            image.draw(&coverage, origin, [0, 0, 0]);
        }
    }
    let mut image = if geometry.canvas_width == doc.width && geometry.canvas_height == doc.height {
        book::page_area(&image, &geometry, options.area, spec.dpi)
    } else {
        image
    };
    book::convert_color(&mut image, options.color, options.threshold);
    Ok(image)
}

fn write_png(path: &Path, image: &Image) -> Result<(), String> {
    image::save_buffer(
        path,
        &image.rgba,
        image.width,
        image.height,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| format!("{}: {e}", path.display()))
}

/// Exports every page (or spread) of `book` to `folder`.
fn export_book(
    book: Book,
    book_path: PathBuf,
    folder: PathBuf,
    options: ExportOptions,
    font: Option<(Arc<Vec<u8>>, u32)>,
    done: Arc<AtomicUsize>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    if options.spreads {
        for (n, (left, right)) in book.spreads().into_iter().enumerate() {
            let left = left
                .map(|i| render_page(&book, &book_path, i, &options, font.as_ref()))
                .transpose()?;
            let right = right
                .map(|i| render_page(&book, &book_path, i, &options, font.as_ref()))
                .transpose()?;
            if let Some(image) = book::spread(left.as_ref(), right.as_ref()) {
                write_png(&folder.join(format!("spread_{:03}.png", n + 1)), &image)?;
            }
            done.fetch_add(
                usize::from(left.is_some()) + usize::from(right.is_some()),
                Ordering::Relaxed,
            );
        }
    } else {
        for index in 0..book.pages.len() {
            let image = render_page(&book, &book_path, index, &options, font.as_ref())?;
            write_png(&folder.join(format!("{:03}.png", index + 1)), &image)?;
            done.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(folder)
}

impl EfudeApp {
    fn save_book(&mut self) {
        if let Some((path, book)) = &self.book_ui.book
            && let Err(error) = std::fs::write(path, book.to_json())
        {
            self.status = format!("{}: {error}", path.display());
        }
    }

    /// Creates a book of `pages` blank pages in `folder`.
    pub(crate) fn create_book(
        &mut self,
        folder: &Path,
        title: &str,
        pages: u32,
        spec: efude_comic::PageSpec,
    ) -> Result<(), String> {
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let mut book = Book::new(title, spec);
        for n in 1..=pages.max(1) as usize {
            let file = Book::page_file_name(n);
            book.pages.push(BookPage { file: file.clone() });
            let path = folder.join(&file);
            if !path.exists() {
                let doc = blank_page(&book, n - 1);
                efude_io::save(&path, &doc).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        let name = if title.trim().is_empty() {
            "book"
        } else {
            title.trim()
        };
        let path = folder.join(format!("{name}.{}", book::EXTENSION));
        std::fs::write(&path, book.to_json()).map_err(|e| e.to_string())?;
        self.book_ui.book = Some((path, book));
        self.book_ui.thumbnails.clear();
        Ok(())
    }

    pub(crate) fn open_book(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let book = Book::from_json(&text).map_err(|e| e.to_string())?;
        self.book_ui.book = Some((path.to_path_buf(), book));
        self.book_ui.thumbnails.clear();
        Ok(())
    }

    fn add_book_page(&mut self) -> Result<(), String> {
        let Some((path, book)) = &mut self.book_ui.book else {
            return Ok(());
        };
        let folder = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut n = book.pages.len() + 1;
        while folder.join(Book::page_file_name(n)).exists() {
            n += 1;
        }
        let file = Book::page_file_name(n);
        let doc = blank_page(book, book.pages.len());
        efude_io::save(&folder.join(&file), &doc).map_err(|e| e.to_string())?;
        book.pages.push(BookPage { file });
        self.save_book();
        Ok(())
    }

    /// Book pages open in tabs with unsaved changes.
    fn unsaved_book_pages(&self) -> Vec<String> {
        let Some((path, book)) = &self.book_ui.book else {
            return Vec::new();
        };
        let pages: Vec<PathBuf> = book.pages.iter().map(|p| page_path(path, p)).collect();
        self.dirty_tab_paths()
            .into_iter()
            .filter(|p| pages.contains(p))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }

    fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        let key = path.to_string_lossy().into_owned();
        if let Some(texture) = self.book_ui.thumbnails.get(&key) {
            return texture.clone();
        }
        let texture = efude_io::load_thumbnail(path).ok().map(|(w, h, rgba)| {
            ctx.load_texture(
                format!("book-thumb-{key}"),
                egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba),
                egui::TextureOptions::LINEAR,
            )
        });
        self.book_ui.thumbnails.insert(key, texture.clone());
        texture
    }

    fn start_book_export(&mut self, folder: PathBuf) {
        let Some((path, book)) = self.book_ui.book.clone() else {
            return;
        };
        let font = self.nombre_font();
        let done = Arc::new(AtomicUsize::new(0));
        let result = Arc::new(Mutex::new(None));
        let options = self.book_ui.export;
        let total = book.pages.len();
        let (job_done, job_result) = (done.clone(), result.clone());
        std::thread::spawn(move || {
            let outcome = export_book(book, path, folder, options, font, job_done);
            if let Ok(mut slot) = job_result.lock() {
                *slot = Some(outcome);
            }
        });
        self.book_ui.job = Some(ExportJob {
            done,
            total,
            result,
        });
    }

    fn nombre_font(&mut self) -> Option<(Arc<Vec<u8>>, u32)> {
        let fonts: Vec<FontInfo> = text::system_fonts();
        let font = [
            "Noto Sans CJK JP",
            "游ゴシック",
            "Yu Gothic",
            "Meiryo",
            "メイリオ",
            "Arial",
            "DejaVu Sans",
        ]
        .iter()
        .find_map(|name| fonts.iter().find(|f| f.name.contains(name)))
        .or(fonts.first())?;
        let data = std::fs::read(&font.path).ok()?;
        Some((Arc::new(data), font.index))
    }

    pub(crate) fn book_window(&mut self, ctx: &egui::Context) {
        if !self.book_ui.open {
            return;
        }
        let english = self.language_english;
        let t = |ja: &'static str, en: &'static str| if english { en } else { ja };
        let mut open = true;
        let mut action: Option<BookAction> = None;
        // Finish a completed export.
        if let Some(job) = &self.book_ui.job {
            let finished = job.result.lock().ok().and_then(|mut slot| slot.take());
            if let Some(outcome) = finished {
                self.status = match outcome {
                    Ok(folder) => {
                        if english {
                            format!("Exported the book: {}", folder.display())
                        } else {
                            format!("作品を書き出しました: {}", folder.display())
                        }
                    }
                    Err(error) => {
                        if english {
                            format!("Export failed: {error}")
                        } else {
                            format!("書き出しに失敗しました: {error}")
                        }
                    }
                };
                self.book_ui.job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        let book = self.book_ui.book.clone();
        let unsaved = self.unsaved_book_pages();
        egui::Window::new(t("作品（複数ページ）", "Book (Pages)"))
            .id(egui::Id::new("book-window"))
            .open(&mut open)
            .resizable(true)
            .default_width(560.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                let Some((path, book)) = book else {
                    ui.label(t(
                        "作品は、ページごとの .efude ファイルをまとめたものです。フォルダーを選ぶと、原稿の設定（漫画 → 原稿の設定）でページを作ります。",
                        "A book collects one .efude file per page. Choose a folder to create the pages with the current page setup (Manga → Page Setup).",
                    ));
                    egui::Grid::new("book-new").num_columns(2).show(ui, |ui| {
                        ui.label(t("作品名", "Title"));
                        ui.text_edit_singleline(&mut self.book_ui.new_title);
                        ui.end_row();
                        ui.label(t("ページ数", "Pages"));
                        ui.add(egui::DragValue::new(&mut self.book_ui.new_pages).range(1..=400));
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button(t("フォルダーを選んで作成…", "Create in Folder…")).clicked() {
                            action = Some(BookAction::Create);
                        }
                        if ui.button(t("作品を開く…", "Open Book…")).clicked() {
                            action = Some(BookAction::Open);
                        }
                    });
                    return;
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&book.title).strong().size(16.0));
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {} · {}",
                            book.pages.len(),
                            t("ページ", "pages"),
                            match book.page.binding {
                                efude_comic::Binding::Right => t("右綴じ", "right-bound"),
                                efude_comic::Binding::Left => t("左綴じ", "left-bound"),
                            }
                        ))
                        .color(layout::MUTED_TEXT),
                    );
                    if ui.small_button(t("閉じる", "Close Book")).clicked() {
                        action = Some(BookAction::CloseBook);
                    }
                });
                ui.separator();
                // Spreads with thumbnails; click a page to open it.
                egui::ScrollArea::vertical()
                    .id_salt("book-spreads")
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (left, right) in book.spreads() {
                            ui.horizontal(|ui| {
                                for side in [left, right] {
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(120.0, 170.0),
                                        egui::Sense::click(),
                                    );
                                    let painter = ui.painter();
                                    let Some(index) = side else {
                                        painter.rect_filled(rect.shrink(4.0), 2.0, Color32::from_gray(40));
                                        continue;
                                    };
                                    let page = page_path(&path, &book.pages[index]);
                                    painter.rect_filled(rect.shrink(4.0), 2.0, Color32::WHITE);
                                    if let Some(texture) = self.thumbnail(ctx, &page) {
                                        let size = texture.size_vec2();
                                        let fit = (rect.shrink(6.0).size() / size).min_elem();
                                        let image_rect = Rect::from_center_size(rect.center(), size * fit);
                                        ui.painter().image(
                                            texture.id(),
                                            image_rect,
                                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                            Color32::WHITE,
                                        );
                                    }
                                    let label = format!(
                                        "{}{}",
                                        index + 1,
                                        if unsaved.iter().any(|u| u == &book.pages[index].file) {
                                            " ●"
                                        } else {
                                            ""
                                        }
                                    );
                                    ui.painter().text(
                                        rect.center_bottom() + Vec2::new(0.0, -12.0),
                                        egui::Align2::CENTER_CENTER,
                                        label,
                                        egui::FontId::proportional(13.0),
                                        Color32::from_rgb(40, 40, 60),
                                    );
                                    if response
                                        .on_hover_text(t("クリックでタブに開く", "Click to open in a tab"))
                                        .clicked()
                                    {
                                        action = Some(BookAction::OpenPage(index));
                                    }
                                }
                            });
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button(t("ページを追加", "Add Page")).clicked() {
                        action = Some(BookAction::AddPage);
                    }
                    if ui.button(t("縮小画像を更新", "Refresh Thumbnails")).clicked() {
                        action = Some(BookAction::Refresh);
                    }
                });
                egui::CollapsingHeader::new(t("ページの順番", "Page Order"))
                    .id_salt("book-order")
                    .show(ui, |ui| {
                        for (index, page) in book.pages.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "{:>3}  {}  {}",
                                    index + 1,
                                    if book.is_right_page(index) { t("右", "R") } else { t("左", "L") },
                                    page.file
                                ));
                                if ui.add_enabled(index > 0, egui::Button::new("↑").small()).clicked() {
                                    action = Some(BookAction::Move(index, index - 1));
                                }
                                if ui
                                    .add_enabled(index + 1 < book.pages.len(), egui::Button::new("↓").small())
                                    .clicked()
                                {
                                    action = Some(BookAction::Move(index, index + 1));
                                }
                                if ui
                                    .small_button(t("外す", "Remove"))
                                    .on_hover_text(t(
                                        "作品から外します（ファイルは残ります）",
                                        "Removes it from the book (the file stays)",
                                    ))
                                    .clicked()
                                {
                                    action = Some(BookAction::Remove(index));
                                }
                            });
                        }
                    });
                let mut nombre = book.nombre.clone();
                egui::CollapsingHeader::new(t("ノンブル（ページ番号）", "Page Numbers"))
                    .id_salt("book-nombre")
                    .show(ui, |ui| {
                        ui.checkbox(&mut nombre.enabled, t("書き出しに入れる", "Print page numbers"));
                        ui.horizontal(|ui| {
                            ui.label(t("開始番号", "Start at"));
                            ui.add(egui::DragValue::new(&mut nombre.start).range(0..=9999));
                            ui.label(t("大きさ", "Size"));
                            ui.add(egui::DragValue::new(&mut nombre.size_points).range(4.0..=24.0).suffix(" pt"));
                        });
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomOuter, t("下・小口側", "Bottom outer"));
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomCenter, t("下・中央", "Bottom centre"));
                            ui.selectable_value(&mut nombre.position, NombrePosition::BottomInner, t("下・ノド側", "Bottom inner"));
                        });
                        ui.checkbox(&mut nombre.skip_first, t("最初のページには入れない", "Skip the first page"));
                    });
                if nombre != book.nombre {
                    action = Some(BookAction::Nombre(nombre));
                }
                egui::CollapsingHeader::new(t("書き出し（印刷用）", "Export for Print"))
                    .id_salt("book-export")
                    .default_open(true)
                    .show(ui, |ui| {
                        let export = &mut self.book_ui.export;
                        ui.horizontal(|ui| {
                            ui.label(t("範囲", "Area"));
                            ui.selectable_value(&mut export.area, ExportArea::Trim, t("仕上がり", "Trim"));
                            ui.selectable_value(&mut export.area, ExportArea::Bleed, t("裁ち落としまで", "With bleed"));
                            ui.selectable_value(&mut export.area, ExportArea::WithMarks, t("トンボ付き", "With crop marks"));
                        });
                        ui.horizontal(|ui| {
                            ui.label(t("色", "Colour"));
                            ui.selectable_value(&mut export.color, ExportColor::Color, t("カラー", "Colour"));
                            ui.selectable_value(&mut export.color, ExportColor::Grayscale, t("グレー", "Grey"));
                            ui.selectable_value(&mut export.color, ExportColor::Monochrome, t("モノクロ2階調", "Black and white"));
                        });
                        if export.color == ExportColor::Monochrome {
                            ui.add(egui::Slider::new(&mut export.threshold, 1..=254).text(t("しきい値", "Threshold")));
                        }
                        ui.checkbox(&mut export.spreads, t("見開きで書き出す", "Export spreads"));
                        if !unsaved.is_empty() {
                            ui.colored_label(
                                Color32::from_rgb(255, 205, 110),
                                if english {
                                    format!("Unsaved pages are exported as last saved: {}", unsaved.join(", "))
                                } else {
                                    format!("保存していないページは、最後に保存した状態で書き出されます: {}", unsaved.join("、"))
                                },
                            );
                        }
                        match &self.book_ui.job {
                            Some(job) => {
                                let done = job.done.load(Ordering::Relaxed);
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / job.total.max(1) as f32)
                                        .text(format!("{done} / {}", job.total)),
                                );
                            }
                            None => {
                                if ui.button(t("PNGで書き出す…", "Export PNG…")).clicked() {
                                    action = Some(BookAction::Export);
                                }
                            }
                        }
                    });
            });
        match action {
            Some(BookAction::Create) => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    let title = if self.book_ui.new_title.trim().is_empty() {
                        folder
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "book".into())
                    } else {
                        self.book_ui.new_title.clone()
                    };
                    let spec = self
                        .comic_doc()
                        .map(|c| c.page)
                        .unwrap_or_else(|| self.comic_ui.setup.clone());
                    if let Err(error) =
                        self.create_book(&folder, &title, self.book_ui.new_pages, spec)
                    {
                        self.status = error;
                    }
                }
            }
            Some(BookAction::Open) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Efude Book", &[book::EXTENSION])
                    .pick_file()
                    && let Err(error) = self.open_book(&path)
                {
                    self.status = error;
                }
            }
            Some(BookAction::CloseBook) => self.book_ui.book = None,
            Some(BookAction::OpenPage(index)) => {
                if let Some((path, book)) = &self.book_ui.book {
                    let page = page_path(path, &book.pages[index]);
                    if let Some(tab) = self.tab_with_path(&page) {
                        self.switch_tab(tab);
                    } else if let Err(error) = self.queue_document_load(page, false, ctx) {
                        self.status = error;
                    }
                }
            }
            Some(BookAction::AddPage) => {
                if let Err(error) = self.add_book_page() {
                    self.status = error;
                }
            }
            Some(BookAction::Refresh) => self.book_ui.thumbnails.clear(),
            Some(BookAction::Move(from, to)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.pages.swap(from, to);
                }
                self.save_book();
            }
            Some(BookAction::Remove(index)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.pages.remove(index);
                }
                self.save_book();
            }
            Some(BookAction::Nombre(nombre)) => {
                if let Some((_, book)) = &mut self.book_ui.book {
                    book.nombre = nombre;
                }
                self.save_book();
            }
            Some(BookAction::Export) => {
                if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                    self.start_book_export(folder);
                }
            }
            None => {}
        }
        self.book_ui.open = open;
    }
}

enum BookAction {
    Create,
    Open,
    CloseBook,
    OpenPage(usize),
    AddPage,
    Refresh,
    Move(usize, usize),
    Remove(usize),
    Nombre(efude_comic::book::Nombre),
    Export,
}

#[cfg(test)]
pub(crate) fn export_for_test(
    book_path: &Path,
    folder: &Path,
    options: ExportOptions,
) -> Result<PathBuf, String> {
    let book = Book::from_json(&std::fs::read_to_string(book_path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let fonts = text::system_fonts();
    let font = fonts
        .first()
        .and_then(|f| std::fs::read(&f.path).ok().map(|d| (Arc::new(d), f.index)));
    export_book(
        book,
        book_path.to_path_buf(),
        folder.to_path_buf(),
        options,
        font,
        Arc::new(AtomicUsize::new(0)),
    )
}
