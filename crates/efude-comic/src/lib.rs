// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! Comic features: page setup and guides, panels, effect lines, text and
//! balloons, and books of pages.
//!
//! Everything here is plain geometry in canvas pixels; the caller decides
//! which layers the results are written to. See `docs/spec/comic.md`.

pub mod balloon;
pub mod book;
pub mod lines;
pub mod page;
pub mod panel;
pub mod raster;
pub mod text;

pub use balloon::{Balloon, BalloonShape, Tail};
pub use book::{Book, BookPage, ExportOptions, Nombre};
pub use lines::{FocusLines, SpeedLines};
pub use page::{Binding, PageGeometry, PageSpec};
pub use panel::{Panel, PanelLayout};
pub use raster::Coverage;
pub use text::{FontInfo, TextStyle};
