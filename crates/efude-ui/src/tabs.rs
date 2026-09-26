// SPDX-License-Identifier: MPL-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Several open documents, one per canvas tab. The active document lives in
//! the app's own fields (so every tool works on it unchanged); the others
//! are parked here and swapped in when their tab is chosen.

use super::*;

/// Everything that belongs to one open document.
pub(crate) struct DocumentTab {
    doc: Document,
    history: History,
    selection: Selection,
    selected_layer: usize,
    editing_mask: bool,
    zoom: f32,
    navigator_center: Vec2,
    view_rotation: f32,
    flip_x: bool,
    flip_y: bool,
    doc_path: Option<std::path::PathBuf>,
    symmetry_center: Vec2,
    perspective_points: Vec<(i32, i32)>,
    perspective_selected: usize,
    fill_reference_layer: u64,
    selection_reference_layer: u64,
    effect_layer: Option<u64>,
}

/// A tab: its number for untitled documents, and the parked document
/// (`None` for the active tab, whose document is in the app).
pub(crate) struct TabSlot {
    pub number: u32,
    pub parked: Option<DocumentTab>,
}

/// State of the tab bar.
#[derive(Default)]
pub(crate) struct Tabs {
    pub slots: Vec<TabSlot>,
    pub active: usize,
    pub next_number: u32,
    /// Tab waiting for "close without saving?" confirmation.
    pub confirm_close: Option<usize>,
}

impl Tabs {
    pub fn with_one() -> Self {
        Self {
            slots: vec![TabSlot {
                number: 1,
                parked: None,
            }],
            active: 0,
            next_number: 2,
            confirm_close: None,
        }
    }
}

impl EfudeApp {
    fn park_active(&mut self) -> DocumentTab {
        let size = (self.doc.width, self.doc.height);
        DocumentTab {
            doc: std::mem::replace(&mut self.doc, Document::new(size.0, size.1)),
            history: std::mem::take(&mut self.history),
            selection: std::mem::take(&mut self.selection),
            selected_layer: self.selected_layer,
            editing_mask: self.editing_mask,
            zoom: self.zoom,
            navigator_center: self.navigator_center,
            view_rotation: self.view_rotation,
            flip_x: self.flip_x,
            flip_y: self.flip_y,
            doc_path: self.doc_path.take(),
            symmetry_center: self.symmetry_center,
            perspective_points: std::mem::take(&mut self.perspective_points),
            perspective_selected: self.perspective_selected,
            fill_reference_layer: self.fill_reference_layer,
            selection_reference_layer: self.selection_reference_layer,
            effect_layer: self.comic_ui.effect_layer.take(),
        }
    }

    fn unpark(&mut self, tab: DocumentTab) {
        self.doc = tab.doc;
        self.history = tab.history;
        self.selection = tab.selection;
        self.selected_layer = tab.selected_layer;
        self.editing_mask = tab.editing_mask;
        self.zoom = tab.zoom;
        self.navigator_center = tab.navigator_center;
        self.view_rotation = tab.view_rotation;
        self.flip_x = tab.flip_x;
        self.flip_y = tab.flip_y;
        self.doc_path = tab.doc_path;
        self.symmetry_center = tab.symmetry_center;
        self.perspective_points = tab.perspective_points;
        self.perspective_selected = tab.perspective_selected;
        self.fill_reference_layer = tab.fill_reference_layer;
        self.selection_reference_layer = tab.selection_reference_layer;
        self.comic_ui.effect_layer = tab.effect_layer;
        self.canvas_width_input = self.doc.width;
        self.canvas_height_input = self.doc.height;
        self.canvas_dpi_input = self.doc.dpi;
    }

    /// Drops in-progress gestures and redraws everything: called whenever
    /// the active document changes.
    fn reset_transient_state(&mut self) {
        self.active.clear();
        self.stroke_builder = None;
        self.provisional = None;
        self.pending_provisional.clear();
        self.vector_live = None;
        self.layer_drag = None;
        self.paste_preview = None;
        self.paste_texture = None;
        self.selection_start = None;
        self.selection_before_gesture = None;
        self.selection_points.clear();
        self.bezier_points.clear();
        self.gesture_end = None;
        self.pan_start = None;
        self.setting_vanishing_point = false;
        self.comic_ui.split_start = None;
        self.comic_ui.split_end = None;
        self.reset_stroke_buffers();
        self.dirty_canvas_tiles.clear();
        self.canvas_texture_dirty = true;
        self.navigator_texture_dirty = true;
    }

    /// The active tab holds an untouched, never-saved canvas that a new or
    /// opened document may simply replace.
    fn active_tab_is_pristine(&self) -> bool {
        self.doc_path.is_none()
            && !self.history.is_dirty()
            && !self.history.can_undo()
            && self
                .doc
                .layers
                .iter()
                .all(|layer| !layer.pixels.has_allocated_tiles())
    }

    /// Makes room for a document that is about to be installed in the app
    /// fields: a new tab, unless the active one is pristine.
    pub(crate) fn open_document_tab(&mut self) {
        if self.tabs.slots.is_empty() {
            self.tabs = tabs::Tabs::with_one();
        }
        if self.active_tab_is_pristine() {
            return;
        }
        let parked = self.park_active();
        self.tabs.slots[self.tabs.active].parked = Some(parked);
        let number = self.tabs.next_number;
        self.tabs.next_number += 1;
        self.tabs.slots.push(TabSlot {
            number,
            parked: None,
        });
        self.tabs.active = self.tabs.slots.len() - 1;
        self.reset_transient_state();
    }

    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index == self.tabs.active || index >= self.tabs.slots.len() {
            return;
        }
        let Some(target) = self.tabs.slots[index].parked.take() else {
            return;
        };
        let parked = self.park_active();
        self.tabs.slots[self.tabs.active].parked = Some(parked);
        self.unpark(target);
        self.tabs.active = index;
        self.reset_transient_state();
    }

    fn tab_is_dirty(&self, index: usize) -> bool {
        match &self.tabs.slots[index].parked {
            Some(tab) => tab.history.is_dirty(),
            None => self.history.is_dirty(),
        }
    }

    /// Indices of tabs with unsaved changes.
    pub(crate) fn dirty_tabs(&self) -> Vec<usize> {
        (0..self.tabs.slots.len())
            .filter(|&index| self.tab_is_dirty(index))
            .collect()
    }

    fn tab_path(&self, index: usize) -> Option<&std::path::PathBuf> {
        match &self.tabs.slots[index].parked {
            Some(tab) => tab.doc_path.as_ref(),
            None => self.doc_path.as_ref(),
        }
    }

    /// Files of the tabs with unsaved changes.
    pub(crate) fn dirty_tab_paths(&self) -> Vec<std::path::PathBuf> {
        self.dirty_tabs()
            .into_iter()
            .filter_map(|index| self.tab_path(index).cloned())
            .collect()
    }

    /// The tab showing the file at `path`.
    pub(crate) fn tab_with_path(&self, path: &std::path::Path) -> Option<usize> {
        (0..self.tabs.slots.len()).find(|&index| self.tab_path(index).is_some_and(|p| p == path))
    }

    fn tab_title(&self, index: usize) -> String {
        let path = match &self.tabs.slots[index].parked {
            Some(tab) => tab.doc_path.as_ref(),
            None => self.doc_path.as_ref(),
        };
        path.and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                let number = self.tabs.slots[index].number;
                if self.language_english {
                    format!("Untitled {number}")
                } else {
                    format!("無題 {number}")
                }
            })
    }

    /// Closes a tab without asking. The last tab is replaced by a blank
    /// canvas of the same size.
    pub(crate) fn close_tab_now(&mut self, index: usize) {
        if index >= self.tabs.slots.len() {
            return;
        }
        if self.tabs.slots.len() == 1 {
            let (width, height, dpi) = (self.doc.width, self.doc.height, self.doc.dpi);
            self.replace_document(Document::new(width, height), None);
            self.doc.dpi = dpi;
            self.tabs.slots[0].number = self.tabs.next_number;
            self.tabs.next_number += 1;
            self.reset_transient_state();
            return;
        }
        if index == self.tabs.active {
            // Show a neighbour first, then drop the closed one.
            let neighbour = if index + 1 < self.tabs.slots.len() {
                index + 1
            } else {
                index - 1
            };
            self.switch_tab(neighbour);
        }
        self.tabs.slots.remove(index);
        if self.tabs.active > index {
            self.tabs.active -= 1;
        }
    }

    /// Marks the tab whose document `document_id` belongs to as saved.
    pub(crate) fn mark_tab_saved(
        &mut self,
        document_id: u64,
        state_token: u64,
        path: &std::path::Path,
    ) -> bool {
        for slot in &mut self.tabs.slots {
            if let Some(tab) = &mut slot.parked
                && tab.history.document_id() == document_id
            {
                tab.history.mark_saved(state_token);
                tab.doc_path = Some(path.to_path_buf());
                return true;
            }
        }
        false
    }

    /// The row of document tabs above the canvas.
    pub(crate) fn document_tabs_ui(&mut self, ui: &mut egui::Ui) {
        let english = self.language_english;
        let mut switch_to = None;
        let mut close = None;
        egui::Frame::NONE
            .fill(Color32::from_rgb(24, 25, 31))
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("document_tabs")
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for index in 0..self.tabs.slots.len() {
                                let active = index == self.tabs.active;
                                let dirty = self.tab_is_dirty(index);
                                let title = format!(
                                    "{}{}",
                                    self.tab_title(index),
                                    if dirty { " ●" } else { "" }
                                );
                                let fill = if active {
                                    Color32::from_rgb(52, 50, 74)
                                } else {
                                    Color32::from_rgb(34, 35, 42)
                                };
                                egui::Frame::NONE
                                    .fill(fill)
                                    .corner_radius(6.0)
                                    .inner_margin(egui::Margin::symmetric(8, 3))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let text =
                                                egui::RichText::new(title).color(if active {
                                                    Color32::WHITE
                                                } else {
                                                    layout::MUTED_TEXT
                                                });
                                            if ui
                                                .add(
                                                    egui::Label::new(text)
                                                        .sense(egui::Sense::click()),
                                                )
                                                .clicked()
                                            {
                                                switch_to = Some(index);
                                            }
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("×")
                                                            .color(layout::MUTED_TEXT),
                                                    )
                                                    .frame(false)
                                                    .small(),
                                                )
                                                .on_hover_text(if english {
                                                    "Close"
                                                } else {
                                                    "閉じる"
                                                })
                                                .clicked()
                                            {
                                                close = Some(index);
                                            }
                                        });
                                    });
                            }
                            if ui
                                .add(egui::Button::new("＋").frame(false))
                                .on_hover_text(if english {
                                    "New Canvas…"
                                } else {
                                    "新規キャンバス…"
                                })
                                .clicked()
                            {
                                self.show_new_document = true;
                            }
                        });
                    });
            });
        if let Some(index) = switch_to {
            self.switch_tab(index);
        }
        if let Some(index) = close {
            self.request_close_tab(index);
        }
    }

    /// Closes a tab, asking first when it has unsaved changes.
    pub(crate) fn request_close_tab(&mut self, index: usize) {
        if self.tab_is_dirty(index) {
            self.tabs.confirm_close = Some(index);
        } else {
            self.close_tab_now(index);
        }
    }

    /// Confirmation for closing a tab with unsaved changes.
    pub(crate) fn close_tab_window(&mut self, ctx: &egui::Context) {
        let Some(index) = self.tabs.confirm_close else {
            return;
        };
        if index >= self.tabs.slots.len() {
            self.tabs.confirm_close = None;
            return;
        }
        let english = self.language_english;
        let title = self.tab_title(index);
        let mut decision = None;
        egui::Window::new(if english {
            "Close Tab"
        } else {
            "タブを閉じる"
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(if english {
                format!("\"{title}\" has unsaved changes. Close it without saving?")
            } else {
                format!("「{title}」には保存していない変更があります。保存せずに閉じますか？")
            });
            ui.horizontal(|ui| {
                if ui
                    .button(if english {
                        "Close Without Saving"
                    } else {
                        "保存せずに閉じる"
                    })
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui
                    .button(if english { "Cancel" } else { "キャンセル" })
                    .clicked()
                {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                self.tabs.confirm_close = None;
                self.close_tab_now(index);
            }
            Some(false) => self.tabs.confirm_close = None,
            None => {}
        }
    }
}
