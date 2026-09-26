// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
pub mod filters;
pub mod tone;
pub mod vector;

pub use tone::{DotShape, ToneSettings};
pub use vector::{VectorAnchor, VectorPoint, VectorStroke};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::{Index, IndexMut, Range};
use std::sync::Arc;

pub const TILE_SIZE: u32 = 256;
pub const MAX_DOCUMENT_DIMENSION: u32 = 30_000;
pub const MAX_DOCUMENT_PIXELS: u64 = 100_000_000;
pub fn valid_document_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= MAX_DOCUMENT_DIMENSION
        && height <= MAX_DOCUMENT_DIMENSION
        && (width as u64) * (height as u64) <= MAX_DOCUMENT_PIXELS
}
const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 4) as usize;
static ZERO: u8 = 0;
static ZERO_TILE: [u8; TILE_BYTES] = [0; TILE_BYTES];
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TilePixels {
    width: u32,
    height: u32,
    tiles: HashMap<(u32, u32), Arc<Vec<u8>>>,
}
impl TilePixels {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            tiles: HashMap::new(),
        }
    }
    pub fn len(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn address(&self, index: usize) -> Option<((u32, u32), usize)> {
        if index >= self.len() {
            return None;
        }
        let p = index / 4;
        let x = p as u32 % self.width;
        let y = p as u32 / self.width;
        Some((
            (x / TILE_SIZE, y / TILE_SIZE),
            (((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) * 4) as usize + index % 4,
        ))
    }
    pub fn get(&self, index: usize) -> Option<&u8> {
        let (key, offset) = self.address(index)?;
        Some(
            self.tiles
                .get(&key)
                .and_then(|t| t.get(offset))
                .unwrap_or(&ZERO),
        )
    }
    pub fn get_or_tile_default(&self, index: usize, default: u8) -> Option<u8> {
        let (key, offset) = self.address(index)?;
        Some(
            self.tiles
                .get(&key)
                .and_then(|t| t.get(offset))
                .copied()
                .unwrap_or(default),
        )
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut u8> {
        let (key, offset) = self.address(index)?;
        let tile = self
            .tiles
            .entry(key)
            .or_insert_with(|| Arc::new(vec![0; TILE_BYTES]));
        Some(&mut Arc::make_mut(tile)[offset])
    }
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let Some(tile) = self.tiles.get(&(x / TILE_SIZE, y / TILE_SIZE)) else {
            return [0; 4];
        };
        let offset = (((y % TILE_SIZE) * TILE_SIZE + x % TILE_SIZE) * 4) as usize;
        tile.get(offset..offset + 4)
            .and_then(|pixel| pixel.try_into().ok())
            .unwrap_or([0; 4])
    }
    pub fn pixel_or_tile_default(&self, x: u32, y: u32, default: [u8; 4]) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return default;
        }
        let key = (x / TILE_SIZE, y / TILE_SIZE);
        if !self.tiles.contains_key(&key) {
            return default;
        }
        self.pixel(x, y)
    }
    pub fn set_pixel(&mut self, x: u32, y: u32, px: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        self[i..i + 4].copy_from_slice(&px)
    }
    pub fn tiles(&self) -> impl Iterator<Item = ((u32, u32), &[u8])> {
        self.tiles.iter().map(|(&key, data)| (key, data.as_slice()))
    }
    pub fn tile_data(&self, tile_x: u32, tile_y: u32) -> Option<&[u8]> {
        self.tiles
            .get(&(tile_x, tile_y))
            .map(|data| data.as_slice())
    }
    /// Tiles whose storage differs from `before`, a clone taken earlier.
    /// Cheap: tiles are shared `Arc`s, so unchanged tiles compare by pointer.
    pub fn tiles_changed_since(&self, before: &TilePixels) -> Vec<(u32, u32)> {
        let mut changed: Vec<(u32, u32)> = self
            .tiles
            .iter()
            .filter(|(key, tile)| {
                before
                    .tiles
                    .get(key)
                    .is_none_or(|old| !Arc::ptr_eq(old, tile))
            })
            .map(|(key, _)| *key)
            .collect();
        changed.extend(
            before
                .tiles
                .keys()
                .filter(|key| !self.tiles.contains_key(key)),
        );
        changed
    }
    pub fn tile_keys(&self) -> Vec<(u32, u32)> {
        let mut keys = self.tiles.keys().copied().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }
    pub fn ensure_tile_filled(&mut self, x: u32, y: u32, value: [u8; 4]) {
        let tile = self
            .tiles
            .entry((x / TILE_SIZE, y / TILE_SIZE))
            .or_insert_with(|| Arc::new(vec![0; TILE_BYTES]));
        for px in Arc::make_mut(tile).chunks_exact_mut(4) {
            px.copy_from_slice(&value);
        }
    }
    pub fn allocated_bytes(&self) -> usize {
        self.tiles.len() * TILE_BYTES
    }
    pub fn has_allocated_tiles(&self) -> bool {
        !self.tiles.is_empty()
    }
    pub fn has_tile(&self, x: u32, y: u32) -> bool {
        self.tiles.contains_key(&(x / TILE_SIZE, y / TILE_SIZE))
    }
    pub fn clear_tiles(&mut self) {
        self.tiles.clear();
    }
    /// Removes one tile (it becomes transparent).
    pub fn remove_tile(&mut self, tile_x: u32, tile_y: u32) {
        self.tiles.remove(&(tile_x, tile_y));
    }
    /// Mutable pixels of one tile (`TILE_SIZE`² RGBA, row-major), created
    /// transparent if missing and unshared if shared.
    pub fn tile_mut(&mut self, tile_x: u32, tile_y: u32) -> &mut [u8] {
        let tile = self
            .tiles
            .entry((tile_x, tile_y))
            .or_insert_with(|| Arc::new(vec![0; TILE_BYTES]));
        Arc::make_mut(tile).as_mut_slice()
    }
    /// Stores a tile that may be shared with other tiles or layers.
    pub fn insert_shared_tile(&mut self, tile_x: u32, tile_y: u32, tile: Arc<Vec<u8>>) {
        if tile.len() == (TILE_SIZE * TILE_SIZE * 4) as usize {
            self.tiles.insert((tile_x, tile_y), tile);
        }
    }
    /// Sets every pixel to `value`, with all tiles sharing one allocation
    /// (a tile is copied only when written). Used for masks that hide
    /// everything outside a shape.
    pub fn fill_shared(&mut self, value: [u8; 4]) {
        let tile: Arc<Vec<u8>> = Arc::new(value.repeat((TILE_SIZE * TILE_SIZE) as usize));
        self.tiles.clear();
        for ty in 0..self.height.div_ceil(TILE_SIZE) {
            for tx in 0..self.width.div_ceil(TILE_SIZE) {
                self.tiles.insert((tx, ty), tile.clone());
            }
        }
    }
    pub fn prune_empty_tiles(&mut self) {
        self.tiles.retain(|_, tile| tile.iter().any(|&b| b != 0));
    }
    pub fn from_dense(width: u32, height: u32, dense: &[u8]) -> Self {
        let mut p = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let i = ((y * width + x) * 4) as usize;
                if dense.get(i + 3).copied().unwrap_or(0) != 0 {
                    p.set_pixel(x, y, [dense[i], dense[i + 1], dense[i + 2], dense[i + 3]]);
                }
            }
        }
        p
    }
    pub fn to_dense(&self) -> Vec<u8> {
        let mut out = vec![0; self.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                let px = self.pixel(x, y);
                let i = ((y * self.width + x) * 4) as usize;
                out[i..i + 4].copy_from_slice(&px);
            }
        }
        out
    }
    pub fn chunks_exact(&self, size: usize) -> PixelChunks<'_> {
        PixelChunks {
            pixels: self,
            index: 0,
            size,
        }
    }
}
impl Index<usize> for TilePixels {
    type Output = u8;
    fn index(&self, index: usize) -> &u8 {
        self.get(index).expect("pixel index out of bounds")
    }
}
impl IndexMut<usize> for TilePixels {
    fn index_mut(&mut self, index: usize) -> &mut u8 {
        self.get_mut(index).expect("pixel index out of bounds")
    }
}
impl Index<Range<usize>> for TilePixels {
    type Output = [u8];
    fn index(&self, r: Range<usize>) -> &[u8] {
        assert!(r.end <= self.len() && r.start <= r.end);
        if r.is_empty() {
            return &[];
        }
        let (a, oa) = self.address(r.start).expect("pixel range");
        let (b, ob) = self.address(r.end - 1).expect("pixel range");
        assert_eq!(a, b, "pixel range crosses tile boundary");
        assert!(ob - oa + 1 == r.len(), "non-contiguous pixel range");
        self.tiles
            .get(&a)
            .map(|tile| &tile[oa..ob + 1])
            .unwrap_or(&ZERO_TILE[oa..ob + 1])
    }
}
impl IndexMut<Range<usize>> for TilePixels {
    fn index_mut(&mut self, r: Range<usize>) -> &mut [u8] {
        assert!(r.end <= self.len() && r.start <= r.end);
        if r.is_empty() {
            return &mut [];
        }
        let (a, oa) = self.address(r.start).expect("pixel range");
        let (b, ob) = self.address(r.end - 1).expect("pixel range");
        assert_eq!(a, b, "pixel range crosses tile boundary");
        assert!(ob - oa + 1 == r.len(), "non-contiguous pixel range");
        let tile = self
            .tiles
            .entry(a)
            .or_insert_with(|| Arc::new(vec![0; TILE_BYTES]));
        &mut Arc::make_mut(tile)[oa..ob + 1]
    }
}
pub struct PixelChunks<'a> {
    pixels: &'a TilePixels,
    index: usize,
    size: usize,
}
impl<'a> Iterator for PixelChunks<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        if self.size == 0 || self.index + self.size > self.pixels.len() {
            return None;
        }
        let start = self.index;
        self.index += self.size;
        Some(&self.pixels[start..start + self.size])
    }
}
pub fn sparse_tile_keys(layer: &Layer, width: u32, height: u32) -> Vec<(u32, u32)> {
    let mut keys = layer
        .pixels
        .tiles()
        .filter_map(|(key, data)| {
            let (tx, ty) = key;
            if tx * TILE_SIZE >= width || ty * TILE_SIZE >= height {
                return None;
            }
            let tw = TILE_SIZE.min(width - tx * TILE_SIZE) as usize;
            let th = TILE_SIZE.min(height - ty * TILE_SIZE) as usize;
            for y in 0..th {
                for x in 0..tw {
                    if data[(y * TILE_SIZE as usize + x) * 4 + 3] != 0 {
                        return Some(key);
                    }
                }
            }
            None
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Add,
    Subtract,
}
fn blend_channel(mode: BlendMode, source: f32, destination: f32) -> f32 {
    let (s, d) = (source.clamp(0., 1.), destination.clamp(0., 1.));
    match mode {
        BlendMode::Normal => s,
        BlendMode::Multiply => s * d,
        BlendMode::Screen => 1. - (1. - s) * (1. - d),
        BlendMode::Overlay => {
            if d < 0.5 {
                2. * s * d
            } else {
                1. - 2. * (1. - s) * (1. - d)
            }
        }
        BlendMode::Darken => s.min(d),
        BlendMode::Lighten => s.max(d),
        BlendMode::ColorDodge => {
            if s >= 1. {
                1.
            } else {
                (d / (1. - s)).min(1.)
            }
        }
        BlendMode::ColorBurn => {
            if s <= 0. {
                0.
            } else {
                1. - ((1. - d) / s).min(1.)
            }
        }
        BlendMode::HardLight => {
            if s < 0.5 {
                2. * s * d
            } else {
                1. - 2. * (1. - s) * (1. - d)
            }
        }
        BlendMode::SoftLight => {
            if s <= 0.5 {
                d - (1. - 2. * s) * d * (1. - d)
            } else {
                let g = if d <= 0.25 {
                    ((16. * d - 12.) * d + 4.) * d
                } else {
                    d.sqrt()
                };
                d + (2. * s - 1.) * (g - d)
            }
        }
        BlendMode::Difference => (d - s).abs(),
        BlendMode::Exclusion => s + d - 2. * s * d,
        BlendMode::Add => (s + d).min(1.),
        BlendMode::Subtract => (d - s).max(0.),
    }
    .clamp(0., 1.)
}
/// Halves a block of an RGBA image for a display mip level. `src` is a
/// `width`×`height` block whose top-left pixel is at (`x0`, `y0`) of the
/// finer level (`x0`, `y0` even); the pixels it covers in the coarser
/// `target_width`×`target_height` level `target` are replaced by the mean
/// of their 2×2 source pixels, averaged in linear light.
#[allow(clippy::too_many_arguments)]
pub fn downsample_half_into(
    src: &[u8],
    width: u32,
    height: u32,
    x0: u32,
    y0: u32,
    target: &mut [u8],
    target_width: u32,
    target_height: u32,
) {
    static LINEAR: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    let linear = LINEAR.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)));
    let (tx0, ty0) = (x0 / 2, y0 / 2);
    let tx1 = (x0 + width).div_ceil(2).min(target_width);
    let ty1 = (y0 + height).div_ceil(2).min(target_height);
    for ty in ty0..ty1 {
        for tx in tx0..tx1 {
            let mut sum = [0.0f32; 4];
            for dy in 0..2 {
                for dx in 0..2 {
                    let sx = (tx * 2 + dx).saturating_sub(x0).min(width - 1);
                    let sy = (ty * 2 + dy).saturating_sub(y0).min(height - 1);
                    let i = ((sy * width + sx) * 4) as usize;
                    for c in 0..3 {
                        sum[c] += linear[src[i + c] as usize];
                    }
                    sum[3] += src[i + 3] as f32;
                }
            }
            let o = ((ty * target_width + tx) * 4) as usize;
            for c in 0..3 {
                target[o + c] = (linear_to_srgb(sum[c] * 0.25) * 255.0)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            target[o + 3] = (sum[3] * 0.25).round() as u8;
        }
    }
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LayerKind {
    Raster,
    Folder,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Layer {
    pub id: u64,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub locked: bool,
    pub clipping: bool,
    pub sketch: bool,
    pub reference: bool,
    pub blend: BlendMode,
    #[serde(default)]
    pub linear_blend: bool,
    pub kind: LayerKind,
    pub parent_id: Option<u64>,
    pub expanded: bool,
    pub pixels: TilePixels,
    pub mask: Option<TilePixels>,
    /// Shown as a halftone of its pixels' density (see `tone`).
    #[serde(default)]
    pub tone: Option<ToneSettings>,
    /// The strokes of a vector layer (`None` for other layers). The pixels
    /// are their drawing (see `vector`).
    #[serde(default)]
    pub vector: Option<Vec<VectorStroke>>,
}
impl Layer {
    pub fn new(id: u64, name: impl Into<String>, w: u32, h: u32) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            opacity: 1.,
            locked: false,
            clipping: false,
            sketch: false,
            reference: false,
            blend: BlendMode::Normal,
            linear_blend: false,
            kind: LayerKind::Raster,
            parent_id: None,
            expanded: true,
            pixels: TilePixels::new(w, h),
            mask: None,
            tone: None,
            vector: None,
        }
    }
    /// Whether this is a vector layer.
    pub fn is_vector(&self) -> bool {
        self.vector.is_some()
    }
    pub fn property_state(&self) -> LayerPropertyState {
        LayerPropertyState {
            name: self.name.clone(),
            visible: self.visible,
            opacity: self.opacity,
            locked: self.locked,
            clipping: self.clipping,
            sketch: self.sketch,
            reference: self.reference,
            blend: self.blend,
            linear_blend: self.linear_blend,
            kind: self.kind,
            parent_id: self.parent_id,
            expanded: self.expanded,
            tone: self.tone,
        }
    }
    fn restore_property_state(&mut self, state: &LayerPropertyState) {
        self.name = state.name.clone();
        self.visible = state.visible;
        self.opacity = state.opacity;
        self.locked = state.locked;
        self.clipping = state.clipping;
        self.sketch = state.sketch;
        self.reference = state.reference;
        self.blend = state.blend;
        self.linear_blend = state.linear_blend;
        self.kind = state.kind;
        self.parent_id = state.parent_id;
        self.expanded = state.expanded;
        self.tone = state.tone;
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerPropertyState {
    name: String,
    visible: bool,
    opacity: f32,
    locked: bool,
    clipping: bool,
    sketch: bool,
    reference: bool,
    blend: BlendMode,
    #[serde(default)]
    linear_blend: bool,
    kind: LayerKind,
    parent_id: Option<u64>,
    expanded: bool,
    #[serde(default)]
    tone: Option<ToneSettings>,
}
#[derive(Clone)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub dpi: f32,
    pub layers: Vec<Layer>,
    /// Document-level data of other components (for example comic page
    /// setup and panels), by key, as JSON text. Saved with the document.
    pub metadata: std::collections::BTreeMap<String, String>,
}
/// Where a dragged layer lands, next to a layer of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerPlacement {
    Above,
    Below,
    Into,
}
/// The layer and everything inside it (a folder's contents, at any depth).
pub fn subtree_ids(layers: &[Layer], id: u64) -> Vec<u64> {
    let mut ids = vec![id];
    let mut cursor = 0;
    while cursor < ids.len() {
        let parent = ids[cursor];
        for child in layers
            .iter()
            .filter(|layer| layer.parent_id == Some(parent))
        {
            if !ids.contains(&child.id) {
                ids.push(child.id);
            }
        }
        cursor += 1;
    }
    ids
}
/// The stacking order (bottom first) the layer panel shows: each folder's
/// contents directly below it, siblings keeping their order. Layers whose
/// parent is missing (or part of a loop) count as top level.
pub fn layer_tree_order(layers: &[Layer]) -> Vec<u64> {
    let parents = layers
        .iter()
        .map(|layer| (layer.id, layer.parent_id))
        .collect::<HashMap<_, _>>();
    // Top level when the parent chain does not end at a top-level layer
    // (a missing parent or a loop).
    let parent_of = |layer: &Layer| -> Option<u64> {
        let mut seen = std::collections::HashSet::new();
        let mut current = layer.parent_id;
        while let Some(parent) = current {
            if parent == layer.id || !seen.insert(parent) {
                return None;
            }
            match parents.get(&parent) {
                Some(next) => current = *next,
                None => return None,
            }
        }
        layer.parent_id
    };
    let mut children = HashMap::<Option<u64>, Vec<usize>>::new();
    for (index, layer) in layers.iter().enumerate() {
        children.entry(parent_of(layer)).or_default().push(index);
    }
    let mut order = Vec::with_capacity(layers.len());
    let mut placed = std::collections::HashSet::with_capacity(layers.len());
    // Depth first, without recursion: a folder follows its contents.
    let mut stack: Vec<(usize, bool)> = children
        .get(&None)
        .map(|roots| roots.iter().rev().map(|&i| (i, false)).collect())
        .unwrap_or_default();
    while let Some((index, expanded)) = stack.pop() {
        let id = layers[index].id;
        if expanded {
            if placed.insert(id) {
                order.push(id);
            }
            continue;
        }
        stack.push((index, true));
        if let Some(inside) = children.get(&Some(id)) {
            stack.extend(inside.iter().rev().map(|&i| (i, false)));
        }
    }
    for layer in layers {
        if placed.insert(layer.id) {
            order.push(layer.id);
        }
    }
    order
}
/// Puts the layers in `layer_tree_order` (for documents just opened).
pub fn tidy_layer_order(layers: &mut Vec<Layer>) -> bool {
    let order = layer_tree_order(layers);
    if layers
        .iter()
        .map(|layer| layer.id)
        .eq(order.iter().copied())
    {
        return false;
    }
    reorder_by_ids(layers, &order);
    true
}
fn reorder_by_ids(layers: &mut Vec<Layer>, ids: &[u64]) {
    let mut by_id = layers
        .drain(..)
        .map(|layer| (layer.id, layer))
        .collect::<HashMap<_, _>>();
    for id in ids {
        if let Some(layer) = by_id.remove(id) {
            layers.push(layer);
        }
    }
    layers.extend(by_id.into_values());
}
#[allow(clippy::too_many_arguments)] // Keeps color-plane and mask translation in one shared path.
fn translated_pixels(
    source: &TilePixels,
    old_width: u32,
    old_height: u32,
    new_width: u32,
    new_height: u32,
    offset_x: i32,
    offset_y: i32,
    mask: bool,
) -> TilePixels {
    let mut result = TilePixels::new(new_width, new_height);
    for ((tile_x, tile_y), data) in source.tiles() {
        for local_y in 0..TILE_SIZE {
            let source_y = tile_y * TILE_SIZE + local_y;
            if source_y >= old_height {
                break;
            }
            let target_y = source_y as i64 + offset_y as i64;
            if !(0..new_height as i64).contains(&target_y) {
                continue;
            }
            for local_x in 0..TILE_SIZE {
                let source_x = tile_x * TILE_SIZE + local_x;
                if source_x >= old_width {
                    break;
                }
                let target_x = source_x as i64 + offset_x as i64;
                if !(0..new_width as i64).contains(&target_x) {
                    continue;
                }
                let si = ((local_y * TILE_SIZE + local_x) * 4) as usize;
                let pixel = [data[si], data[si + 1], data[si + 2], data[si + 3]];
                if !mask && pixel[3] == 0 {
                    continue;
                }
                let tx = target_x as u32;
                let ty = target_y as u32;
                if mask && !result.has_tile(tx, ty) {
                    result.ensure_tile_filled(tx, ty, [255; 4]);
                }
                result.set_pixel(tx, ty, pixel);
            }
        }
    }
    if !mask {
        result.prune_empty_tiles();
    }
    result
}
impl Document {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            dpi: 300.,
            layers: vec![Layer::new(1, "レイヤー 1", width, height)],
            metadata: Default::default(),
        }
    }
}

/// Tiles saved before the current action changed them: (layer id, 0 for
/// pixels or 1 for the mask, tile x, tile y) to the tile (none if empty).
type SavedTiles = HashMap<(u64, u8, u32, u32), Option<Arc<Vec<u8>>>>;

pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    current: SavedTiles,
    changed_tiles: std::collections::HashSet<(u32, u32)>,
    full_redraw_pending: bool,
    pending: Vec<HistoryEntry>,
    active: bool,
    undo_bytes: usize,
    redo_bytes: usize,
    undo_spills: Vec<std::path::PathBuf>,
    redo_spills: Vec<std::path::PathBuf>,
    current_state: u64,
    saved_state: u64,
    undo_states: Vec<u64>,
    redo_states: Vec<u64>,
    document_id: u64,
    selection_update: Option<(bool, Vec<u8>)>,
}
static HISTORY_STATE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
impl Default for History {
    fn default() -> Self {
        let initial_state = HISTORY_STATE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let document_id = HISTORY_STATE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            current: HashMap::new(),
            changed_tiles: std::collections::HashSet::new(),
            full_redraw_pending: false,
            pending: Vec::new(),
            active: false,
            undo_bytes: 0,
            redo_bytes: 0,
            undo_spills: Vec::new(),
            redo_spills: Vec::new(),
            current_state: initial_state,
            saved_state: initial_state,
            undo_states: Vec::new(),
            redo_states: Vec::new(),
            document_id,
            selection_update: None,
        }
    }
}
const HISTORY_MEMORY_LIMIT: usize = 2 * 1024 * 1024 * 1024;
static HISTORY_FILE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
const HISTORY_MAGIC: &[u8] = b"EFUDE-HISTORY-4\n";
#[derive(Serialize, Deserialize)]
enum HistoryEntry {
    Batch(Vec<HistoryEntry>),
    Tiles(Vec<TileSnapshot>),
    Insert {
        index: usize,
        layer: Layer,
    },
    Delete {
        index: usize,
        layer: Layer,
    },
    Move {
        from: usize,
        to: usize,
    },
    Order {
        before: Vec<u64>,
        after: Vec<u64>,
    },
    DocumentState {
        width: u32,
        height: u32,
        dpi: f32,
        layers: Vec<Layer>,
    },
    Properties {
        id: u64,
        before: LayerPropertyState,
        after: LayerPropertyState,
    },
    Mask {
        id: u64,
        before: Option<TilePixels>,
        after: Option<TilePixels>,
    },
    /// The strokes of a vector layer from `start` on (`None`: not a vector
    /// layer).
    Vector {
        id: u64,
        start: usize,
        before: Option<Vec<VectorStroke>>,
        after: Option<Vec<VectorStroke>>,
    },
    Selection {
        before_active: bool,
        before: Vec<u8>,
        after_active: bool,
        after: Vec<u8>,
    },
    Metadata {
        key: String,
        before: Option<String>,
        after: Option<String>,
    },
}
type TileSnapshot = (u64, u8, u32, u32, Option<Arc<Vec<u8>>>);
impl History {
    pub fn begin(&mut self) {
        self.current.clear();
        self.pending.clear();
        self.active = true;
    }
    /// Records a whole storage tile of `layer` before it changes.
    pub fn record_tile(&mut self, layer: &Layer, tile_x: u32, tile_y: u32) {
        let key = (tile_x, tile_y);
        self.changed_tiles.insert(key);
        self.current
            .entry((layer.id, 0, key.0, key.1))
            .or_insert_with(|| layer.pixels.tiles.get(&key).cloned());
    }
    pub fn record_pixel(&mut self, layer: &Layer, index: usize) {
        if let Some((key, _)) = layer.pixels.address(index) {
            self.changed_tiles.insert(key);
            self.current
                .entry((layer.id, 0, key.0, key.1))
                .or_insert_with(|| layer.pixels.tiles.get(&key).cloned());
        }
    }
    /// Capture every in-bounds raster tile, including absent tiles that a
    /// full-layer operation may allocate while expanding blur or sharpen.
    pub fn record_all_layer_tiles(&mut self, layer: &Layer, width: u32, height: u32) {
        for ty in 0..height.div_ceil(TILE_SIZE) {
            for tx in 0..width.div_ceil(TILE_SIZE) {
                self.changed_tiles.insert((tx, ty));
                self.current
                    .entry((layer.id, 0, tx, ty))
                    .or_insert_with(|| layer.pixels.tiles.get(&(tx, ty)).cloned());
            }
        }
    }
    /// Capture every in-bounds layer-mask tile, including absent white tiles.
    pub fn record_all_mask_tiles(&mut self, layer: &Layer, width: u32, height: u32) {
        let Some(mask) = &layer.mask else {
            return;
        };
        for ty in 0..height.div_ceil(TILE_SIZE) {
            for tx in 0..width.div_ceil(TILE_SIZE) {
                self.changed_tiles.insert((tx, ty));
                self.current
                    .entry((layer.id, 1, tx, ty))
                    .or_insert_with(|| mask.tiles.get(&(tx, ty)).cloned());
            }
        }
    }
    pub fn record_mask(&mut self, layer: &Layer, index: usize) {
        if let Some(mask) = &layer.mask
            && let Some((key, _)) = mask.address(index)
        {
            self.changed_tiles.insert(key);
            self.current
                .entry((layer.id, 1, key.0, key.1))
                .or_insert_with(|| mask.tiles.get(&key).cloned());
        }
    }
    /// Return the original tile captured before the active history operation
    /// first modified it. `Some(None)` means the tile was originally sparse.
    pub fn recorded_tile(
        &self,
        layer_id: u64,
        plane: u8,
        tile_x: u32,
        tile_y: u32,
    ) -> Option<Option<&[u8]>> {
        self.current
            .get(&(layer_id, plane, tile_x, tile_y))
            .map(|tile| tile.as_ref().map(|bytes| bytes.as_slice()))
    }
    pub fn take_changed_tiles(&mut self) -> Vec<(u32, u32)> {
        self.changed_tiles.drain().collect()
    }
    pub fn take_full_redraw(&mut self) -> bool {
        std::mem::take(&mut self.full_redraw_pending)
    }
    /// Restore the document pixels captured by the active history transaction.
    /// This supports discarding a live stroke preview before applying its
    /// stabilized final samples exactly once.
    pub fn rollback_active(&mut self, doc: &mut Document) {
        for ((layer_id, plane, tx, ty), snapshot) in self.current.drain() {
            if let Some(layer) = doc.layers.iter_mut().find(|layer| layer.id == layer_id) {
                let target = if plane == 0 {
                    Some(&mut layer.pixels)
                } else {
                    layer.mask.as_mut()
                };
                if let Some(target) = target {
                    if let Some(tile) = snapshot {
                        target.tiles.insert((tx, ty), tile);
                    } else {
                        target.tiles.remove(&(tx, ty));
                    }
                }
            }
        }
    }
    pub fn commit(&mut self) {
        if !self.current.is_empty() {
            self.pending.push(HistoryEntry::Tiles(
                self.current
                    .drain()
                    .map(|((id, plane, tx, ty), tile)| (id, plane, tx, ty, tile))
                    .collect(),
            ));
        }
        self.active = false;
        if !self.pending.is_empty() {
            let entries = std::mem::take(&mut self.pending);
            self.push_undo_entry(HistoryEntry::Batch(entries));
        }
    }
    /// Sets (or removes) a document metadata entry, undoably.
    pub fn set_metadata(&mut self, doc: &mut Document, key: &str, value: Option<String>) {
        let before = doc.metadata.get(key).cloned();
        if before == value {
            return;
        }
        match &value {
            Some(text) => doc.metadata.insert(key.to_owned(), text.clone()),
            None => doc.metadata.remove(key),
        };
        self.push_entry(HistoryEntry::Metadata {
            key: key.to_owned(),
            before,
            after: value,
        });
    }
    pub fn insert_layer(&mut self, layers: &mut Vec<Layer>, index: usize, layer: Layer) {
        let index = index.min(layers.len());
        layers.insert(index, layer.clone());
        self.push_entry(HistoryEntry::Insert { index, layer });
    }
    pub fn delete_layer(&mut self, layers: &mut Vec<Layer>, index: usize) {
        if index >= layers.len() {
            return;
        }
        let mut ids = vec![layers[index].id];
        let mut cursor = 0;
        while cursor < ids.len() {
            let parent_id = ids[cursor];
            for child in layers
                .iter()
                .filter(|layer| layer.parent_id == Some(parent_id))
            {
                if !ids.contains(&child.id) {
                    ids.push(child.id);
                }
            }
            cursor += 1;
        }
        let mut indices = layers
            .iter()
            .enumerate()
            .filter_map(|(i, layer)| ids.contains(&layer.id).then_some(i))
            .collect::<Vec<_>>();
        if indices.len() >= layers.len() {
            return;
        }
        let was_active = self.active;
        if !was_active {
            self.begin();
        }
        indices.sort_unstable_by(|a, b| b.cmp(a));
        for at in indices {
            let layer = layers.remove(at);
            self.push_entry(HistoryEntry::Delete { index: at, layer });
        }
        if !was_active {
            self.commit();
        }
    }
    pub fn move_layer(&mut self, layers: &mut Vec<Layer>, from: usize, to: usize) {
        if from >= layers.len() || to >= layers.len() || from == to {
            return;
        }
        let layer = layers.remove(from);
        layers.insert(to, layer);
        self.push_entry(HistoryEntry::Move { from, to });
    }
    pub fn move_layer_subtree(
        &mut self,
        layers: &mut Vec<Layer>,
        from: usize,
        to: usize,
    ) -> Option<usize> {
        if from >= layers.len()
            || to >= layers.len()
            || from == to
            || layers[from].parent_id != layers[to].parent_id
        {
            return None;
        }
        let root_id = layers[from].id;
        let mut subtree = vec![root_id];
        let mut cursor = 0;
        while cursor < subtree.len() {
            let id = subtree[cursor];
            for child in layers.iter().filter(|layer| layer.parent_id == Some(id)) {
                if !subtree.contains(&child.id) {
                    subtree.push(child.id);
                }
            }
            cursor += 1;
        }
        let before = layers.iter().map(|layer| layer.id).collect::<Vec<_>>();
        let block = layers
            .iter()
            .filter(|layer| subtree.contains(&layer.id))
            .map(|layer| layer.id)
            .collect::<Vec<_>>();
        let target_id = layers[to].id;
        let target_subtree = {
            let mut ids = vec![target_id];
            let mut cursor = 0;
            while cursor < ids.len() {
                let id = ids[cursor];
                for child in layers.iter().filter(|layer| layer.parent_id == Some(id)) {
                    if !ids.contains(&child.id) {
                        ids.push(child.id);
                    }
                }
                cursor += 1;
            }
            ids
        };
        let mut after = before
            .iter()
            .copied()
            .filter(|id| !subtree.contains(id))
            .collect::<Vec<_>>();
        let target_positions = after
            .iter()
            .enumerate()
            .filter_map(|(i, id)| target_subtree.contains(id).then_some(i))
            .collect::<Vec<_>>();
        if target_positions.is_empty() {
            return None;
        }
        let insert_at = if to > from {
            target_positions.into_iter().max().unwrap() + 1
        } else {
            target_positions.into_iter().min().unwrap()
        };
        after.splice(insert_at..insert_at, block.iter().copied());
        if before == after {
            return None;
        }
        let new_index = after.iter().position(|id| *id == root_id)?;
        let was_active = self.active;
        if !was_active {
            self.begin();
        }
        reorder_by_ids(layers, &after);
        self.push_entry(HistoryEntry::Order { before, after });
        if !was_active {
            self.commit();
        }
        Some(new_index)
    }
    /// Records a change of a vector layer's strokes (`None`: not a vector
    /// layer). Only the part from the first difference on is kept.
    pub fn record_vector(
        &mut self,
        id: u64,
        before: Option<&[VectorStroke]>,
        after: Option<&[VectorStroke]>,
    ) {
        if before == after {
            return;
        }
        let start = match (before, after) {
            (Some(a), Some(b)) => a.iter().zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        };
        self.push_entry(HistoryEntry::Vector {
            id,
            start,
            before: before.map(|s| s[start..].to_vec()),
            after: after.map(|s| s[start..].to_vec()),
        });
    }
    /// Keeps every folder's contents directly below the folder, as the
    /// layer panel shows them, so the picture stacks like the panel. The
    /// change joins the last undo step (the one that moved the layers).
    pub fn tidy_layer_tree(&mut self, layers: &mut Vec<Layer>) -> bool {
        if self.active {
            return false;
        }
        let before = layers.iter().map(|layer| layer.id).collect::<Vec<_>>();
        let after = layer_tree_order(layers);
        if before == after {
            return false;
        }
        reorder_by_ids(layers, &after);
        let entry = HistoryEntry::Order { before, after };
        Self::collect_history_changes(
            &entry,
            &mut self.changed_tiles,
            &mut self.full_redraw_pending,
        );
        if let Some(last) = self.undo.pop() {
            let batch = match last {
                HistoryEntry::Batch(mut entries) => {
                    entries.push(entry);
                    HistoryEntry::Batch(entries)
                }
                other => HistoryEntry::Batch(vec![other, entry]),
            };
            self.undo_bytes = self.undo_bytes.saturating_add(64);
            self.undo.push(batch);
        }
        true
    }
    /// Moves a layer (with its contents) next to another in the panel:
    /// above or below it, or into it when it is a folder (on top of its
    /// contents). One undo step. Returns false if nothing changed.
    pub fn place_layer(
        &mut self,
        layers: &mut Vec<Layer>,
        id: u64,
        target: u64,
        placement: LayerPlacement,
    ) -> bool {
        let subtree = subtree_ids(layers, id);
        if subtree.contains(&target) {
            return false;
        }
        let Some(target_layer) = layers.iter().find(|layer| layer.id == target) else {
            return false;
        };
        let new_parent = match placement {
            LayerPlacement::Into => {
                if target_layer.kind != LayerKind::Folder {
                    return false;
                }
                Some(target)
            }
            _ => target_layer.parent_id,
        };
        let Some(index) = layers.iter().position(|layer| layer.id == id) else {
            return false;
        };
        let property_before = layers[index].property_state();
        let before = layers.iter().map(|layer| layer.id).collect::<Vec<_>>();
        let block = before
            .iter()
            .copied()
            .filter(|layer| subtree.contains(layer))
            .collect::<Vec<_>>();
        let mut after = before
            .iter()
            .copied()
            .filter(|layer| !subtree.contains(layer))
            .collect::<Vec<_>>();
        let target_block = subtree_ids(layers, target);
        let Some(target_at) = after.iter().position(|layer| *layer == target) else {
            return false;
        };
        let insert_at = match placement {
            // Above the folder or layer: after it in the stack (a folder
            // comes after its contents).
            LayerPlacement::Above => target_at + 1,
            // On top of the folder's contents: just before the folder.
            LayerPlacement::Into => target_at,
            // Below the target and everything in it.
            LayerPlacement::Below => after
                .iter()
                .position(|layer| target_block.contains(layer))
                .unwrap_or(target_at),
        };
        after.splice(insert_at..insert_at, block);
        layers[index].parent_id = new_parent;
        let property_after = layers[index].property_state();
        if before == after && property_before == property_after {
            return false;
        }
        let was_active = self.active;
        if !was_active {
            self.begin();
        }
        self.record_layer_properties(id, property_before, property_after);
        reorder_by_ids(layers, &after);
        let tidy = layer_tree_order(layers);
        reorder_by_ids(layers, &tidy);
        if before != tidy {
            self.push_entry(HistoryEntry::Order {
                before,
                after: tidy,
            });
        }
        if !was_active {
            self.commit();
        }
        true
    }
    pub fn record_layer_properties(
        &mut self,
        id: u64,
        before: LayerPropertyState,
        after: LayerPropertyState,
    ) {
        if before != after {
            self.push_entry(HistoryEntry::Properties { id, before, after });
        }
    }
    pub fn record_selection_change(
        &mut self,
        before_active: bool,
        before: Vec<u8>,
        after_active: bool,
        after: Vec<u8>,
    ) {
        if before_active != after_active || before != after {
            self.push_entry(HistoryEntry::Selection {
                before_active,
                before,
                after_active,
                after,
            });
        }
    }
    pub fn set_layer_mask(&mut self, layers: &mut [Layer], index: usize, mask: Option<TilePixels>) {
        let Some(layer) = layers.get_mut(index) else {
            return;
        };
        if layer.mask.is_some() == mask.is_some() {
            return;
        }
        let id = layer.id;
        let after = mask.clone();
        let before = std::mem::replace(&mut layer.mask, mask);
        self.push_entry(HistoryEntry::Mask { id, before, after });
    }
    /// Replaces a layer's mask with another one (undoable).
    pub fn replace_layer_mask(
        &mut self,
        layers: &mut [Layer],
        index: usize,
        mask: Option<TilePixels>,
    ) {
        let Some(layer) = layers.get_mut(index) else {
            return;
        };
        let id = layer.id;
        let after = mask.clone();
        let before = std::mem::replace(&mut layer.mask, mask);
        self.push_entry(HistoryEntry::Mask { id, before, after });
    }
    fn entry_size(entry: &HistoryEntry) -> usize {
        match entry {
            HistoryEntry::Tiles(snapshots) => snapshots.iter().fold(16usize, |size, snapshot| {
                size.saturating_add(64)
                    .saturating_add(snapshot.4.as_ref().map_or(0, |tile| tile.len()))
            }),
            HistoryEntry::Batch(entries) => entries.iter().fold(16usize, |size, entry| {
                size.saturating_add(Self::entry_size(entry))
            }),
            HistoryEntry::Order { before, after } => 32usize
                .saturating_add(before.len().saturating_mul(8))
                .saturating_add(after.len().saturating_mul(8)),
            HistoryEntry::Move { .. } => 32,
            HistoryEntry::Properties { .. } => 256,
            HistoryEntry::Selection { before, after, .. } => 2usize
                .saturating_add(before.len())
                .saturating_add(after.len()),
            _ => postcard::to_allocvec(entry)
                .map(|serialized| serialized.len())
                .unwrap_or(usize::MAX),
        }
    }
    fn spill_path(entry: &HistoryEntry) -> Option<std::path::PathBuf> {
        let id = HISTORY_FILE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("efude-history-{}-{id}.bin", std::process::id()));
        let mut file = std::fs::File::create(&path).ok()?;
        use std::io::Write;
        if file.write_all(HISTORY_MAGIC).is_err() || postcard::to_io(entry, &mut file).is_err() {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        if file.sync_all().is_err() {
            let _ = std::fs::remove_file(&path);
            return None;
        }
        Some(path)
    }
    fn load_spill(path: &std::path::Path) -> Option<HistoryEntry> {
        let bytes = std::fs::read(path).ok()?;
        let payload = bytes.strip_prefix(HISTORY_MAGIC)?;
        postcard::from_bytes(payload).ok()
    }
    fn clear_redo(&mut self) {
        self.redo.clear();
        self.redo_bytes = 0;
        for path in self.redo_spills.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
    fn push_undo_entry(&mut self, entry: HistoryEntry) {
        self.clear_redo();
        let changes_document = Self::changes_document(&entry);
        self.store_undo_entry(entry);
        if changes_document {
            self.current_state =
                HISTORY_STATE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.undo_states.push(self.current_state);
    }
    fn changes_document(entry: &HistoryEntry) -> bool {
        match entry {
            HistoryEntry::Selection { .. } => false,
            HistoryEntry::Batch(entries) => entries.iter().any(Self::changes_document),
            _ => true,
        }
    }
    fn store_undo_entry(&mut self, entry: HistoryEntry) {
        self.undo_bytes = self.undo_bytes.saturating_add(Self::entry_size(&entry));
        self.undo.push(entry);
        while self.undo_bytes > HISTORY_MEMORY_LIMIT && !self.undo.is_empty() {
            let oldest = self.undo.remove(0);
            self.undo_bytes = self.undo_bytes.saturating_sub(Self::entry_size(&oldest));
            if let Some(path) = Self::spill_path(&oldest) {
                self.undo_spills.push(path);
            } else {
                self.undo_bytes = self.undo_bytes.saturating_add(Self::entry_size(&oldest));
                self.undo.insert(0, oldest);
                break;
            }
        }
    }
    fn push_redo_entry(&mut self, entry: HistoryEntry) {
        self.redo_bytes = self.redo_bytes.saturating_add(Self::entry_size(&entry));
        self.redo.push(entry);
        while self.redo_bytes > HISTORY_MEMORY_LIMIT && !self.redo.is_empty() {
            let oldest = self.redo.remove(0);
            self.redo_bytes = self.redo_bytes.saturating_sub(Self::entry_size(&oldest));
            if let Some(path) = Self::spill_path(&oldest) {
                self.redo_spills.push(path);
            } else {
                self.redo_bytes = self.redo_bytes.saturating_add(Self::entry_size(&oldest));
                self.redo.insert(0, oldest);
                break;
            }
        }
    }
    fn push_entry(&mut self, entry: HistoryEntry) {
        Self::collect_history_changes(
            &entry,
            &mut self.changed_tiles,
            &mut self.full_redraw_pending,
        );
        if self.active {
            self.pending.push(entry);
        } else {
            self.push_undo_entry(HistoryEntry::Batch(vec![entry]));
        }
    }
    fn pop_undo_entry(&mut self) -> Option<HistoryEntry> {
        if let Some(entry) = self.undo.pop() {
            self.undo_bytes = self.undo_bytes.saturating_sub(Self::entry_size(&entry));
            return Some(entry);
        }
        let path = self.undo_spills.pop()?;
        let entry = Self::load_spill(&path);
        let _ = std::fs::remove_file(path);
        entry
    }
    fn pop_redo_entry(&mut self) -> Option<HistoryEntry> {
        if let Some(entry) = self.redo.pop() {
            self.redo_bytes = self.redo_bytes.saturating_sub(Self::entry_size(&entry));
            return Some(entry);
        }
        let path = self.redo_spills.pop()?;
        let entry = Self::load_spill(&path);
        let _ = std::fs::remove_file(path);
        entry
    }
    pub fn checkpoint(&mut self, layer: &Layer) {
        self.current.clear();
        for (&(tx, ty), tile) in &layer.pixels.tiles {
            self.current
                .insert((layer.id, 0, tx, ty), Some(tile.clone()));
        }
        self.commit();
    }
    pub fn resize_document(
        &mut self,
        doc: &mut Document,
        width: u32,
        height: u32,
        dpi: f32,
    ) -> Result<bool, &'static str> {
        if !valid_document_dimensions(width, height) {
            return Err("canvas dimensions exceed safety limits");
        }
        let dpi = dpi.clamp(10.0, 2400.0);
        if doc.width == width && doc.height == height && (doc.dpi - dpi).abs() < f32::EPSILON {
            return Ok(false);
        }
        let before = HistoryEntry::DocumentState {
            width: doc.width,
            height: doc.height,
            dpi: doc.dpi,
            layers: doc.layers.clone(),
        };
        let offset_x = (width as i64 - doc.width as i64) as i32 / 2;
        let offset_y = (height as i64 - doc.height as i64) as i32 / 2;
        for layer in &mut doc.layers {
            layer.pixels = translated_pixels(
                &layer.pixels,
                doc.width,
                doc.height,
                width,
                height,
                offset_x,
                offset_y,
                false,
            );
            if let Some(mask) = &layer.mask {
                layer.mask = Some(translated_pixels(
                    mask, doc.width, doc.height, width, height, offset_x, offset_y, true,
                ));
            }
        }
        doc.width = width;
        doc.height = height;
        doc.dpi = dpi;
        self.push_entry(before);
        Ok(true)
    }
    pub fn undo(&mut self, layer: &mut Layer) {
        let mut layers = vec![layer.clone()];
        self.undo_layers(&mut layers);
        if let Some(value) = layers.pop() {
            *layer = value;
        }
    }
    pub fn undo_layers(&mut self, layers: &mut Vec<Layer>) {
        let width = layers.first().map_or(1, |layer| layer.pixels.width);
        let height = layers.first().map_or(1, |layer| layer.pixels.height);
        let mut doc = Document {
            width,
            height,
            dpi: 300.0,
            layers: std::mem::take(layers),
            metadata: Default::default(),
        };
        self.undo_document(&mut doc);
        *layers = doc.layers;
    }
    pub fn undo_document(&mut self, doc: &mut Document) {
        self.selection_update = None;
        if let Some(entry) = self.pop_undo_entry() {
            let reverse = self.apply_history_entry(entry, doc, true);
            Self::collect_history_changes(
                &reverse,
                &mut self.changed_tiles,
                &mut self.full_redraw_pending,
            );
            self.push_redo_entry(reverse);
            self.redo_states.push(self.current_state);
            self.current_state = self.undo_states.pop().unwrap_or(0);
        }
    }
    pub fn redo(&mut self, layer: &mut Layer) {
        let mut layers = vec![layer.clone()];
        self.redo_layers(&mut layers);
        if let Some(value) = layers.pop() {
            *layer = value;
        }
    }
    pub fn redo_layers(&mut self, layers: &mut Vec<Layer>) {
        let width = layers.first().map_or(1, |layer| layer.pixels.width);
        let height = layers.first().map_or(1, |layer| layer.pixels.height);
        let mut doc = Document {
            width,
            height,
            dpi: 300.0,
            layers: std::mem::take(layers),
            metadata: Default::default(),
        };
        self.redo_document(&mut doc);
        *layers = doc.layers;
    }
    pub fn redo_document(&mut self, doc: &mut Document) {
        self.selection_update = None;
        if let Some(entry) = self.pop_redo_entry() {
            let reverse = self.apply_history_entry(entry, doc, false);
            Self::collect_history_changes(
                &reverse,
                &mut self.changed_tiles,
                &mut self.full_redraw_pending,
            );
            self.store_undo_entry(reverse);
            self.current_state = self.redo_states.pop().unwrap_or(self.current_state);
            self.undo_states.push(self.current_state);
        }
    }
    pub fn take_selection_update(&mut self) -> Option<(bool, Vec<u8>)> {
        self.selection_update.take()
    }
    fn collect_history_changes(
        entry: &HistoryEntry,
        tiles: &mut std::collections::HashSet<(u32, u32)>,
        full_redraw: &mut bool,
    ) {
        match entry {
            HistoryEntry::Tiles(snapshots) => {
                tiles.extend(snapshots.iter().map(|snapshot| (snapshot.2, snapshot.3)));
            }
            HistoryEntry::Batch(entries) => {
                for item in entries {
                    Self::collect_history_changes(item, tiles, full_redraw);
                }
            }
            HistoryEntry::Selection { .. } | HistoryEntry::Metadata { .. } => {}
            _ => *full_redraw = true,
        }
    }
    /// Returns the state token captured for an asynchronous save request.
    pub fn state_token(&self) -> u64 {
        self.current_state
    }
    pub fn document_id(&self) -> u64 {
        self.document_id
    }
    /// Marks a previously captured state as saved. A later edit remains dirty.
    pub fn mark_saved(&mut self, state_token: u64) {
        self.saved_state = state_token;
    }
    pub fn is_dirty(&self) -> bool {
        self.current_state != self.saved_state
    }
    fn apply_history_entry(
        &mut self,
        entry: HistoryEntry,
        doc: &mut Document,
        undo: bool,
    ) -> HistoryEntry {
        match entry {
            HistoryEntry::Batch(entries) => {
                let mut result = if undo {
                    entries
                        .into_iter()
                        .rev()
                        .map(|e| self.apply_history_entry(e, doc, true))
                        .collect::<Vec<_>>()
                } else {
                    entries
                        .into_iter()
                        .map(|e| self.apply_history_entry(e, doc, false))
                        .collect::<Vec<_>>()
                };
                if undo {
                    result.reverse();
                }
                HistoryEntry::Batch(result)
            }
            HistoryEntry::DocumentState {
                width,
                height,
                dpi,
                layers,
            } => {
                let old_width = std::mem::replace(&mut doc.width, width);
                let old_height = std::mem::replace(&mut doc.height, height);
                let old_dpi = std::mem::replace(&mut doc.dpi, dpi);
                let old_layers = std::mem::replace(&mut doc.layers, layers);
                HistoryEntry::DocumentState {
                    width: old_width,
                    height: old_height,
                    dpi: old_dpi,
                    layers: old_layers,
                }
            }
            HistoryEntry::Tiles(delta) => {
                let layers = &mut doc.layers;
                let mut reverse = Vec::with_capacity(delta.len());
                for (id, plane, tx, ty, target_tile) in delta {
                    if let Some(layer) = layers.iter_mut().find(|l| l.id == id) {
                        let target = if plane == 0 {
                            Some(&mut layer.pixels)
                        } else {
                            layer.mask.as_mut()
                        };
                        if let Some(target) = target {
                            let key = (tx, ty);
                            reverse.push((id, plane, tx, ty, target.tiles.get(&key).cloned()));
                            if let Some(tile) = target_tile {
                                target.tiles.insert(key, tile);
                            } else {
                                target.tiles.remove(&key);
                            }
                        }
                    }
                }
                HistoryEntry::Tiles(reverse)
            }
            HistoryEntry::Insert { index, layer } => {
                let layers = &mut doc.layers;
                if undo {
                    if index < layers.len() {
                        let removed = layers.remove(index);
                        HistoryEntry::Insert {
                            index,
                            layer: removed,
                        }
                    } else {
                        HistoryEntry::Insert { index, layer }
                    }
                } else {
                    let at = index.min(layers.len());
                    layers.insert(at, layer.clone());
                    HistoryEntry::Insert { index: at, layer }
                }
            }
            HistoryEntry::Delete { index, layer } => {
                let layers = &mut doc.layers;
                if undo {
                    let at = index.min(layers.len());
                    layers.insert(at, layer.clone());
                    HistoryEntry::Delete { index: at, layer }
                } else if index < layers.len() {
                    let removed = layers.remove(index);
                    HistoryEntry::Delete {
                        index,
                        layer: removed,
                    }
                } else {
                    HistoryEntry::Delete { index, layer }
                }
            }
            HistoryEntry::Move { from, to } => {
                let layers = &mut doc.layers;
                let (source, destination) = if undo { (to, from) } else { (from, to) };
                if source < layers.len() {
                    let layer = layers.remove(source);
                    let at = destination.min(layers.len());
                    layers.insert(at, layer);
                    HistoryEntry::Move { from, to }
                } else {
                    HistoryEntry::Move { from, to }
                }
            }
            HistoryEntry::Order { before, after } => {
                let layers = &mut doc.layers;
                reorder_by_ids(layers, if undo { &before } else { &after });
                HistoryEntry::Order { before, after }
            }
            HistoryEntry::Vector {
                id,
                start,
                before,
                after,
            } => {
                let state = if undo { &before } else { &after };
                if let Some(layer) = doc.layers.iter_mut().find(|l| l.id == id) {
                    match state {
                        None => layer.vector = None,
                        Some(tail) => {
                            let strokes = layer.vector.get_or_insert_with(Vec::new);
                            strokes.truncate(start);
                            strokes.extend(tail.iter().cloned());
                        }
                    }
                }
                HistoryEntry::Vector {
                    id,
                    start,
                    before,
                    after,
                }
            }
            HistoryEntry::Properties { id, before, after } => {
                let layers = &mut doc.layers;
                let state = if undo { &before } else { &after };
                if let Some(layer) = layers.iter_mut().find(|l| l.id == id) {
                    layer.restore_property_state(state);
                }
                HistoryEntry::Properties { id, before, after }
            }
            HistoryEntry::Mask { id, before, after } => {
                if let Some(layer) = doc.layers.iter_mut().find(|layer| layer.id == id) {
                    layer.mask = if undo { before.clone() } else { after.clone() };
                }
                HistoryEntry::Mask { id, before, after }
            }
            HistoryEntry::Metadata { key, before, after } => {
                match if undo { &before } else { &after } {
                    Some(text) => doc.metadata.insert(key.clone(), text.clone()),
                    None => doc.metadata.remove(&key),
                };
                HistoryEntry::Metadata { key, before, after }
            }
            HistoryEntry::Selection {
                before_active,
                before,
                after_active,
                after,
            } => {
                self.selection_update = Some(if undo {
                    (before_active, before.clone())
                } else {
                    (after_active, after.clone())
                });
                HistoryEntry::Selection {
                    before_active,
                    before,
                    after_active,
                    after,
                }
            }
        }
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || !self.undo_spills.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty() || !self.redo_spills.is_empty()
    }
}
impl Drop for History {
    fn drop(&mut self) {
        for path in self.undo_spills.iter().chain(self.redo_spills.iter()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn composite(doc: &Document) -> Vec<u8> {
    composite_at(doc, (0, 0), 0)
}

/// The light square colour of the transparency checkerboard (the other
/// squares are white).
pub const CHECKER_GREY: u8 = 214;

/// Size in document pixels of the transparency checkerboard's squares for a
/// canvas: small enough to read as a pattern, large enough not to blur into
/// grey when the whole canvas is shown.
pub fn checker_size(width: u32, height: u32) -> u32 {
    (width.min(height) / 64).clamp(8, 64)
}

/// The backdrop of the display at a document pixel: white, or the
/// checkerboard that shows transparency (`checker` is its square size; 0
/// for white).
pub fn backdrop(checker: u32, x: u32, y: u32) -> u8 {
    if checker == 0 || (x / checker + y / checker).is_multiple_of(2) {
        255
    } else {
        CHECKER_GREY
    }
}

/// `composite` for the screen: transparent parts show a checkerboard with
/// squares of `checker` pixels (0: white, as exported).
pub fn composite_display(doc: &Document, checker: u32) -> Vec<u8> {
    composite_at(doc, (0, 0), checker)
}

/// A layer's storage tile as displayed: tone layers become halftone.
/// `origin` offsets the tile into document coordinates (for sub-documents).
pub fn display_tile<'a>(
    layer: &Layer,
    dpi: f32,
    tile_x: u32,
    tile_y: u32,
    origin: (u32, u32),
    tile: &'a [u8],
) -> std::borrow::Cow<'a, [u8]> {
    match &layer.tone {
        Some(settings) => std::borrow::Cow::Owned(tone::tone_tile(
            settings,
            dpi,
            (origin.0 + tile_x * TILE_SIZE, origin.1 + tile_y * TILE_SIZE),
            TILE_SIZE,
            tile,
        )),
        None => std::borrow::Cow::Borrowed(tile),
    }
}

/// A layer pixel as displayed (tone layers become halftone).
pub fn display_pixel(layer: &Layer, dpi: f32, x: u32, y: u32, pixel: [u8; 4]) -> [u8; 4] {
    match &layer.tone {
        Some(settings) => tone::tone_pixel(settings, dpi, x, y, pixel),
        None => pixel,
    }
}

fn composite_at(doc: &Document, origin: (u32, u32), checker: u32) -> Vec<u8> {
    let mut out = vec![255; (doc.width * doc.height * 4) as usize];
    if checker > 0 {
        for y in 0..doc.height {
            for x in 0..doc.width {
                let value = backdrop(checker, origin.0 + x, origin.1 + y);
                let i = ((y * doc.width + x) * 4) as usize;
                out[i..i + 3].fill(value);
            }
        }
    }
    let layer_indices: HashMap<_, _> = doc
        .layers
        .iter()
        .enumerate()
        .map(|(i, layer)| (layer.id, i))
        .collect();
    let pixel_count = (doc.width * doc.height) as usize;
    let needs_base_alpha = doc.layers.iter().any(|layer| layer.clipping);
    let mut base_alpha: Option<Vec<f32>> = None;
    for layer in doc.layers.iter().filter(|l| l.kind == LayerKind::Raster) {
        let mut visible = layer.visible;
        let mut opacity = layer.opacity.clamp(0., 1.);
        let mut ancestor_masks = Vec::new();
        let mut parent = layer.parent_id;
        let mut depth = 0;
        while let Some(id) = parent {
            if depth > doc.layers.len() {
                break;
            }
            depth += 1;
            if let Some(group) = layer_indices.get(&id).map(|index| &doc.layers[*index]) {
                visible &= group.visible;
                opacity *= group.opacity.clamp(0., 1.);
                if let Some(mask) = &group.mask {
                    ancestor_masks.push(mask);
                }
                parent = group.parent_id;
            } else {
                break;
            }
        }
        if !visible {
            continue;
        }
        if needs_base_alpha
            && !layer.clipping
            && base_alpha.is_none()
            && layer.pixels.has_allocated_tiles()
        {
            base_alpha = Some(vec![0.0; pixel_count]);
        }
        for ((tile_x, tile_y), tile) in layer.pixels.tiles() {
            let origin_x = tile_x * TILE_SIZE;
            let origin_y = tile_y * TILE_SIZE;
            if origin_x >= doc.width || origin_y >= doc.height {
                continue;
            }
            let tile = display_tile(layer, doc.dpi, tile_x, tile_y, origin, tile);
            let tile_width = TILE_SIZE.min(doc.width - origin_x);
            let tile_height = TILE_SIZE.min(doc.height - origin_y);
            let ancestor_tiles = ancestor_masks
                .iter()
                .map(|mask| mask.tile_data(tile_x, tile_y))
                .collect::<Vec<_>>();
            let layer_mask_tile = layer
                .mask
                .as_ref()
                .and_then(|mask| mask.tile_data(tile_x, tile_y));
            for local_y in 0..tile_height {
                for local_x in 0..tile_width {
                    let tile_index = ((local_y * TILE_SIZE + local_x) * 4) as usize;
                    let src = &tile[tile_index..tile_index + 4];
                    let x = origin_x + local_x;
                    let y = origin_y + local_y;
                    let pixel_index = (y * doc.width + x) as usize;
                    let mut a = src[3] as f32 / 255. * opacity;
                    for mask_tile in ancestor_tiles.iter().flatten() {
                        a *= mask_tile[tile_index] as f32 / 255.;
                    }
                    if let Some(mask_tile) = layer_mask_tile {
                        a *= mask_tile[tile_index] as f32 / 255.;
                    }
                    if layer.clipping {
                        a *= base_alpha.as_ref().map_or(0., |values| values[pixel_index]);
                    }
                    if a == 0. {
                        continue;
                    }
                    let dst_index = pixel_index * 4;
                    for c in 0..3 {
                        let mut s = src[c] as f32 / 255.;
                        let mut d = out[dst_index + c] as f32 / 255.;
                        if layer.linear_blend {
                            s = srgb_to_linear(s);
                            d = srgb_to_linear(d);
                        }
                        let mixed = blend_channel(layer.blend, s, d);
                        let result = mixed * a + d * (1. - a);
                        out[dst_index + c] = ((if layer.linear_blend {
                            linear_to_srgb(result)
                        } else {
                            result
                        }) * 255.)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                    }
                    if !layer.clipping
                        && let Some(values) = &mut base_alpha
                    {
                        values[pixel_index] = a + values[pixel_index] * (1. - a);
                    }
                }
            }
        }
    }
    out
}

/// Recompose one sparse storage tile while preserving the original layer stack semantics.
pub fn composite_tile(doc: &Document, tile_x: u32, tile_y: u32) -> Option<(u32, u32, Vec<u8>)> {
    composite_tile_display(doc, tile_x, tile_y, 0)
}

/// `composite_tile` over the display backdrop (see `composite_display`).
pub fn composite_tile_display(
    doc: &Document,
    tile_x: u32,
    tile_y: u32,
    checker: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    let origin_x = tile_x.checked_mul(TILE_SIZE)?;
    let origin_y = tile_y.checked_mul(TILE_SIZE)?;
    if origin_x >= doc.width || origin_y >= doc.height {
        return None;
    }
    let tile_width = TILE_SIZE.min(doc.width - origin_x);
    let tile_height = TILE_SIZE.min(doc.height - origin_y);
    let mut tile_doc = Document {
        width: tile_width,
        height: tile_height,
        dpi: doc.dpi,
        layers: Vec::with_capacity(doc.layers.len()),
        metadata: Default::default(),
    };
    for source_layer in &doc.layers {
        let mut layer = Layer::new(
            source_layer.id,
            source_layer.name.clone(),
            tile_width,
            tile_height,
        );
        layer.restore_property_state(&source_layer.property_state());
        let mut pixels = TilePixels::new(tile_width, tile_height);
        if let Some(source) = source_layer.pixels.tiles.get(&(tile_x, tile_y)) {
            pixels.tiles.insert((0, 0), source.clone());
        }
        layer.pixels = pixels;
        if let Some(source_mask) = &source_layer.mask {
            let mut mask = TilePixels::new(tile_width, tile_height);
            if let Some(source) = source_mask.tiles.get(&(tile_x, tile_y)) {
                mask.tiles.insert((0, 0), source.clone());
            }
            layer.mask = Some(mask);
        }
        tile_doc.layers.push(layer);
    }
    Some((
        tile_width,
        tile_height,
        composite_at(&tile_doc, (origin_x, origin_y), checker),
    ))
}

pub fn composite_transparent(doc: &Document) -> Vec<u8> {
    let mut out = vec![0; (doc.width * doc.height * 4) as usize];
    let layer_indices: HashMap<_, _> = doc
        .layers
        .iter()
        .enumerate()
        .map(|(i, layer)| (layer.id, i))
        .collect();
    let pixel_count = (doc.width * doc.height) as usize;
    let needs_base_alpha = doc.layers.iter().any(|layer| layer.clipping);
    let mut base_alpha: Option<Vec<f32>> = None;
    for layer in doc.layers.iter().filter(|l| l.kind == LayerKind::Raster) {
        let mut visible = layer.visible;
        let mut opacity = layer.opacity.clamp(0.0, 1.0);
        let mut ancestor_masks = Vec::new();
        let mut parent = layer.parent_id;
        let mut depth = 0;
        while let Some(id) = parent {
            if depth >= doc.layers.len() {
                break;
            }
            depth += 1;
            if let Some(group) = layer_indices.get(&id).map(|index| &doc.layers[*index]) {
                visible &= group.visible;
                opacity *= group.opacity.clamp(0.0, 1.0);
                if let Some(mask) = &group.mask {
                    ancestor_masks.push(mask);
                }
                parent = group.parent_id;
            } else {
                break;
            }
        }
        if !visible {
            continue;
        }
        if needs_base_alpha
            && !layer.clipping
            && base_alpha.is_none()
            && layer.pixels.has_allocated_tiles()
        {
            base_alpha = Some(vec![0.0; pixel_count]);
        }
        for ((tile_x, tile_y), tile) in layer.pixels.tiles() {
            let origin_x = tile_x * TILE_SIZE;
            let origin_y = tile_y * TILE_SIZE;
            if origin_x >= doc.width || origin_y >= doc.height {
                continue;
            }
            let tile = display_tile(layer, doc.dpi, tile_x, tile_y, (0, 0), tile);
            let tile_width = TILE_SIZE.min(doc.width - origin_x);
            let tile_height = TILE_SIZE.min(doc.height - origin_y);
            let ancestor_tiles = ancestor_masks
                .iter()
                .map(|mask| mask.tile_data(tile_x, tile_y))
                .collect::<Vec<_>>();
            let layer_mask_tile = layer
                .mask
                .as_ref()
                .and_then(|mask| mask.tile_data(tile_x, tile_y));
            for local_y in 0..tile_height {
                for local_x in 0..tile_width {
                    let tile_index = ((local_y * TILE_SIZE + local_x) * 4) as usize;
                    let src = &tile[tile_index..tile_index + 4];
                    let x = origin_x + local_x;
                    let y = origin_y + local_y;
                    let pixel_index = (y * doc.width + x) as usize;
                    let mut sa = src[3] as f32 / 255.0 * opacity;
                    for mask_tile in ancestor_tiles.iter().flatten() {
                        sa *= mask_tile[tile_index] as f32 / 255.0;
                    }
                    if let Some(mask_tile) = layer_mask_tile {
                        sa *= mask_tile[tile_index] as f32 / 255.0;
                    }
                    if layer.clipping {
                        sa *= base_alpha.as_ref().map_or(0., |values| values[pixel_index]);
                    }
                    if sa <= 0.0 {
                        continue;
                    }
                    let di = pixel_index * 4;
                    let da = out[di + 3] as f32 / 255.0;
                    let oa = sa + da * (1.0 - sa);
                    for c in 0..3 {
                        let mut s = src[c] as f32 / 255.0;
                        let mut d = out[di + c] as f32 / 255.0;
                        if layer.linear_blend {
                            s = srgb_to_linear(s);
                            d = srgb_to_linear(d);
                        }
                        let blend = blend_channel(layer.blend, s, d);
                        if oa > 0.0 {
                            let premultiplied =
                                blend * sa * da + s * sa * (1.0 - da) + d * da * (1.0 - sa);
                            let result = premultiplied / oa;
                            out[di + c] = ((if layer.linear_blend {
                                linear_to_srgb(result)
                            } else {
                                result
                            }) * 255.0)
                                .round()
                                .clamp(0.0, 255.0) as u8;
                        }
                    }
                    out[di + 3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
                    if !layer.clipping
                        && let Some(values) = &mut base_alpha
                    {
                        values[pixel_index] = oa;
                    }
                }
            }
        }
    }
    out
}

pub fn sample_composite_pixel(doc: &Document, x: u32, y: u32) -> [u8; 4] {
    CompositeSampler::new(doc).sample(doc, x, y)
}

/// Reusable layer-ID index for many pixel samples from an unchanged layer
/// structure, such as the dabs of one wet-mixing stroke.
#[derive(Clone)]
pub struct CompositeSampler {
    layer_indices: HashMap<u64, usize>,
}
impl CompositeSampler {
    pub fn new(doc: &Document) -> Self {
        Self {
            layer_indices: doc
                .layers
                .iter()
                .enumerate()
                .map(|(index, layer)| (layer.id, index))
                .collect(),
        }
    }
    pub fn sample(&self, doc: &Document, x: u32, y: u32) -> [u8; 4] {
        sample_composite_pixel_from_indices(doc, doc.layers.len(), x, y, &self.layer_indices)
    }
    pub fn sample_below(&self, doc: &Document, layer_index: usize, x: u32, y: u32) -> [u8; 4] {
        sample_composite_pixel_from_indices(
            doc,
            layer_index.min(doc.layers.len()),
            x,
            y,
            &self.layer_indices,
        )
    }
}

#[derive(Clone)]
pub struct LayerSampler {
    layer_index: usize,
    ancestor_indices: Vec<usize>,
}
impl LayerSampler {
    pub fn new(doc: &Document, layer_id: u64) -> Option<Self> {
        let indices: HashMap<_, _> = doc
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| (layer.id, index))
            .collect();
        let layer_index = *indices.get(&layer_id)?;
        let layer = &doc.layers[layer_index];
        if layer.kind != LayerKind::Raster {
            return None;
        }
        let mut ancestor_indices = Vec::new();
        let mut parent = layer.parent_id;
        while let Some(parent_id) = parent {
            if ancestor_indices.len() >= doc.layers.len() {
                return None;
            }
            let index = *indices.get(&parent_id)?;
            let group = &doc.layers[index];
            if group.kind != LayerKind::Folder {
                return None;
            }
            ancestor_indices.push(index);
            parent = group.parent_id;
        }
        Some(Self {
            layer_index,
            ancestor_indices,
        })
    }

    pub fn sample(&self, doc: &Document, x: u32, y: u32) -> [u8; 4] {
        if x >= doc.width || y >= doc.height {
            return [0; 4];
        }
        let layer = &doc.layers[self.layer_index];
        if !layer.visible {
            return [0; 4];
        }
        let mut alpha = layer.opacity.clamp(0.0, 1.0);
        if let Some(mask) = &layer.mask {
            alpha *= mask.pixel_or_tile_default(x, y, [255; 4])[0] as f32 / 255.0;
        }
        for &index in &self.ancestor_indices {
            let group = &doc.layers[index];
            if !group.visible {
                return [0; 4];
            }
            alpha *= group.opacity.clamp(0.0, 1.0);
            if let Some(mask) = &group.mask {
                alpha *= mask.pixel_or_tile_default(x, y, [255; 4])[0] as f32 / 255.0;
            }
        }
        let mut pixel = display_pixel(layer, doc.dpi, x, y, layer.pixels.pixel(x, y));
        pixel[3] = (pixel[3] as f32 * alpha).round().clamp(0.0, 255.0) as u8;
        pixel
    }
}

pub fn sample_composite_pixel_below(doc: &Document, layer_index: usize, x: u32, y: u32) -> [u8; 4] {
    sample_composite_pixel_from(doc, layer_index.min(doc.layers.len()), x, y)
}

fn sample_composite_pixel_from(doc: &Document, layer_count: usize, x: u32, y: u32) -> [u8; 4] {
    let layer_indices: HashMap<_, _> = doc
        .layers
        .iter()
        .enumerate()
        .map(|(index, layer)| (layer.id, index))
        .collect();
    sample_composite_pixel_from_indices(doc, layer_count, x, y, &layer_indices)
}

fn sample_composite_pixel_from_indices(
    doc: &Document,
    layer_count: usize,
    x: u32,
    y: u32,
    layer_indices: &HashMap<u64, usize>,
) -> [u8; 4] {
    if x >= doc.width || y >= doc.height {
        return [0; 4];
    }
    let mut dst = [255u8; 4];
    let mut base_alpha = 0.0f32;
    for layer in doc
        .layers
        .iter()
        .take(layer_count)
        .filter(|l| l.kind == LayerKind::Raster)
    {
        let mut visible = layer.visible;
        let mut opacity = layer.opacity.clamp(0., 1.);
        let mut ancestor_masks = Vec::new();
        let mut parent = layer.parent_id;
        let mut depth = 0;
        while let Some(id) = parent {
            if depth > doc.layers.len() {
                break;
            }
            depth += 1;
            if let Some(group) = layer_indices.get(&id).map(|index| &doc.layers[*index]) {
                visible &= group.visible;
                opacity *= group.opacity.clamp(0., 1.);
                if let Some(mask) = &group.mask {
                    ancestor_masks.push(mask);
                }
                parent = group.parent_id;
            } else {
                break;
            }
        }
        if !visible {
            continue;
        }
        let src = display_pixel(layer, doc.dpi, x, y, layer.pixels.pixel(x, y));
        let mut a = src[3] as f32 / 255. * opacity;
        for mask in &ancestor_masks {
            if mask.has_tile(x, y) {
                a *= mask[((y * doc.width + x) * 4) as usize] as f32 / 255.;
            }
        }
        if let Some(mask) = &layer.mask {
            let m = if mask.has_tile(x, y) {
                mask[((y * doc.width + x) * 4) as usize] as f32 / 255.
            } else {
                1.
            };
            a *= m;
        }
        if layer.clipping {
            a *= base_alpha;
        }
        if a == 0. {
            continue;
        }
        for c in 0..3 {
            let s = src[c] as f32 / 255.;
            let d = dst[c] as f32 / 255.;
            let mixed = blend_channel(layer.blend, s, d);
            dst[c] = ((mixed * a + d * (1. - a)) * 255.)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        if !layer.clipping {
            base_alpha = a + base_alpha * (1. - a);
        }
    }
    dst
}

#[derive(Default)]
pub struct Selection {
    pub mask: Vec<u8>,
    pub active: bool,
}
impl Selection {
    pub fn clear(&mut self) {
        self.active = false;
        self.mask.clear()
    }
    pub fn invert(&mut self, width: u32, height: u32) {
        if !self.active {
            self.mask = vec![255; (width * height) as usize];
            self.active = true;
            return;
        }
        self.mask.resize((width * height) as usize, 0);
        for value in &mut self.mask {
            *value = if *value == 0 { 255 } else { 0 };
        }
    }
    pub fn expand(&mut self, width: u32, height: u32, radius: u32) {
        self.morph(width, height, radius, true);
    }
    pub fn shrink(&mut self, width: u32, height: u32, radius: u32) {
        self.morph(width, height, radius, false);
    }
    pub fn feather(&mut self, width: u32, height: u32, radius: u32) {
        if !self.active || width == 0 || height == 0 || radius == 0 {
            return;
        }
        let len = (width as usize).saturating_mul(height as usize);
        self.mask.resize(len, 0);
        let radius = radius.min(64) as i32;
        let sigma = (radius as f32 * 0.5).max(0.5);
        let mut kernel = (-radius..=radius)
            .map(|offset| (-(offset * offset) as f32 / (2.0 * sigma * sigma)).exp())
            .collect::<Vec<_>>();
        let total = kernel.iter().sum::<f32>().max(f32::EPSILON);
        kernel.iter_mut().for_each(|weight| *weight /= total);
        let (w, h) = (width as usize, height as usize);
        let window = radius as usize * 2 + 1;
        let mut rows = std::collections::VecDeque::<Vec<u8>>::with_capacity(window);
        let mut horizontal = vec![0u8; w];
        for source_y in 0..h + radius as usize {
            if source_y < h {
                for (x, output) in horizontal.iter_mut().enumerate() {
                    let mut value = 0.0;
                    for (k, &weight) in (-radius..=radius).zip(&kernel) {
                        let sx = (x as i32 + k).clamp(0, w as i32 - 1) as usize;
                        value += self.mask[source_y * w + sx] as f32 * weight;
                    }
                    *output = value.round().clamp(0.0, 255.0) as u8;
                }
            }
            if rows.len() == window {
                rows.pop_front();
            }
            rows.push_back(horizontal.clone());
            if source_y >= radius as usize {
                let y = source_y - radius as usize;
                let first_row = source_y + 1 - rows.len();
                let row = &mut self.mask[y * w..(y + 1) * w];
                for (x, output) in row.iter_mut().enumerate() {
                    let mut value = 0.0;
                    for (offset, &weight) in (-radius..=radius).zip(&kernel) {
                        let sy = (y as i32 + offset).clamp(0, h as i32 - 1) as usize;
                        value += rows[sy - first_row][x] as f32 * weight;
                    }
                    *output = value.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    fn morph(&mut self, width: u32, height: u32, radius: u32, dilate: bool) {
        if !self.active || width == 0 || height == 0 || radius == 0 {
            return;
        }
        self.mask.resize((width * height) as usize, 0);
        let r = radius.min(128) as usize;
        let (w, h) = (width as usize, height as usize);
        let full_window = 2 * r + 1;
        let mut prefix = vec![0u32; w + 1];
        let mut horizontal_row = vec![0u8; w];
        let mut vertical_counts = vec![0u32; w];
        let mut rows = std::collections::VecDeque::<Vec<u8>>::with_capacity(full_window);
        for source_y in 0..h + r {
            if source_y < h {
                prefix[0] = 0;
                for x in 0..w {
                    prefix[x + 1] = prefix[x] + u32::from(self.mask[source_y * w + x] != 0);
                }
                for (x, output) in horizontal_row.iter_mut().enumerate() {
                    let lo = x.saturating_sub(r);
                    let hi = (x + r + 1).min(w);
                    let count = prefix[hi] - prefix[lo];
                    *output = if (dilate && count > 0) || (!dilate && count == full_window as u32) {
                        255
                    } else {
                        0
                    };
                }
            } else {
                horizontal_row.fill(0);
            }
            if rows.len() == full_window {
                let expired = rows.pop_front().expect("full morphology row window");
                for (count, value) in vertical_counts.iter_mut().zip(expired) {
                    *count -= u32::from(value != 0);
                }
            }
            for (count, value) in vertical_counts.iter_mut().zip(&horizontal_row) {
                *count += u32::from(*value != 0);
            }
            rows.push_back(horizontal_row.clone());
            if source_y >= r {
                let y = source_y - r;
                let edge = y < r || y + r >= h;
                let row = &mut self.mask[y * w..(y + 1) * w];
                for (x, output) in row.iter_mut().enumerate() {
                    let selected = if dilate {
                        vertical_counts[x] > 0
                    } else {
                        !edge && vertical_counts[x] == full_window as u32
                    };
                    *output = if selected { 255 } else { 0 };
                }
            }
        }
    }
    pub fn rectangle(&mut self, w: u32, h: u32, a: (i32, i32), b: (i32, i32)) {
        self.mask = vec![0; (w * h) as usize];
        let (x0, x1) = (a.0.min(b.0).max(0), a.0.max(b.0).min(w as i32 - 1));
        let (y0, y1) = (a.1.min(b.1).max(0), a.1.max(b.1).min(h as i32 - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.mask[(y as u32 * w + x as u32) as usize] = 255;
            }
        }
        self.active = true
    }
    pub fn ellipse(&mut self, w: u32, h: u32, a: (i32, i32), b: (i32, i32)) {
        self.mask = vec![0; (w * h) as usize];
        if w == 0 || h == 0 {
            self.active = false;
            return;
        }
        let cx = (a.0 + b.0) as f32 / 2.;
        let cy = (a.1 + b.1) as f32 / 2.;
        let rx = ((a.0 - b.0).abs() as f32 / 2.).max(0.5);
        let ry = ((a.1 - b.1).abs() as f32 / 2.).max(0.5);
        let x0 = ((cx - rx).floor() as i32).clamp(0, w as i32 - 1) as u32;
        let x1 = ((cx + rx).ceil() as i32).clamp(0, w as i32 - 1) as u32;
        let y0 = ((cy - ry).floor() as i32).clamp(0, h as i32 - 1) as u32;
        let y1 = ((cy + ry).ceil() as i32).clamp(0, h as i32 - 1) as u32;
        if cx + rx < 0.0 || cy + ry < 0.0 || cx - rx >= w as f32 || cy - ry >= h as f32 {
            self.active = true;
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = (x as f32 - cx) / rx;
                let dy = (y as f32 - cy) / ry;
                if dx * dx + dy * dy <= 1. {
                    self.mask[(y * w + x) as usize] = 255;
                }
            }
        }
        self.active = true
    }
    pub fn polygon(&mut self, w: u32, h: u32, points: &[(i32, i32)]) {
        self.mask = vec![0; (w * h) as usize];
        if points.len() < 3 {
            self.active = false;
            return;
        }
        if w == 0 || h == 0 {
            self.active = false;
            return;
        }
        let left = points
            .iter()
            .map(|p| p.0)
            .min()
            .unwrap_or(0)
            .clamp(0, w as i32 - 1);
        let right = points
            .iter()
            .map(|p| p.0)
            .max()
            .unwrap_or(-1)
            .clamp(0, w as i32 - 1);
        let top = points
            .iter()
            .map(|p| p.1)
            .min()
            .unwrap_or(0)
            .clamp(0, h as i32 - 1);
        let bottom = points
            .iter()
            .map(|p| p.1)
            .max()
            .unwrap_or(-1)
            .clamp(0, h as i32 - 1);
        if left > right || top > bottom {
            self.active = true;
            return;
        }
        for y in top..=bottom {
            for x in left..=right {
                let mut inside = false;
                let mut j = points.len() - 1;
                for i in 0..points.len() {
                    let (xi, yi) = points[i];
                    let (xj, yj) = points[j];
                    if (yi > y) != (yj > y)
                        && (x as f32)
                            < (xj - xi) as f32 * (y - yi) as f32 / (yj - yi) as f32 + xi as f32
                    {
                        inside = !inside;
                    }
                    j = i;
                }
                if inside {
                    self.mask[(y as u32 * w + x as u32) as usize] = 255;
                }
            }
        }
        self.active = true;
    }
    pub fn color_range(
        &mut self,
        layer: &Layer,
        width: u32,
        height: u32,
        color: [u8; 4],
        tolerance: u8,
    ) {
        self.mask = vec![0; (width * height) as usize];
        let threshold = (tolerance as u32).pow(2) * 4;
        for y in 0..height {
            for x in 0..width {
                let pixel = layer.pixels.pixel(x, y);
                let distance = (0..4)
                    .map(|channel| {
                        (pixel[channel] as i32 - color[channel] as i32)
                            .unsigned_abs()
                            .pow(2)
                    })
                    .sum::<u32>();
                if distance <= threshold {
                    self.mask[(y * width + x) as usize] = 255;
                }
            }
        }
        self.active = true;
    }
    pub fn color_range_pixels(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        color: [u8; 4],
        tolerance: u8,
    ) {
        self.mask = vec![0; (width * height) as usize];
        let threshold = (tolerance as u32).pow(2) * 4;
        for i in 0..(width * height) as usize {
            let Some(px) = pixels.get(i * 4..i * 4 + 4) else {
                break;
            };
            let distance = (0..4)
                .map(|c| (px[c] as i32 - color[c] as i32).unsigned_abs().pow(2))
                .sum::<u32>();
            if distance <= threshold {
                self.mask[i] = 255;
            }
        }
        self.active = true;
        let _ = (width, height);
    }
    pub fn contiguous_color(
        &mut self,
        layer: &Layer,
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        tolerance: u8,
    ) {
        self.mask = vec![0; (width * height) as usize];
        if x >= width || y >= height {
            self.active = false;
            return;
        }
        let target = layer.pixels.pixel(x, y);
        let threshold = (tolerance as u32).pow(2) * 4;
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            let index = (cy * width + cx) as usize;
            if self.mask[index] != 0 {
                continue;
            }
            let pixel = layer.pixels.pixel(cx, cy);
            let distance = (0..4)
                .map(|channel| {
                    (pixel[channel] as i32 - target[channel] as i32)
                        .unsigned_abs()
                        .pow(2)
                })
                .sum::<u32>();
            if distance > threshold {
                continue;
            }
            self.mask[index] = 255;
            if cx > 0 {
                stack.push((cx - 1, cy));
            }
            if cx + 1 < width {
                stack.push((cx + 1, cy));
            }
            if cy > 0 {
                stack.push((cx, cy - 1));
            }
            if cy + 1 < height {
                stack.push((cx, cy + 1));
            }
        }
        self.active = true;
    }
    pub fn contiguous_color_pixels(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        x: u32,
        y: u32,
        tolerance: u8,
    ) {
        self.mask = vec![0; (width * height) as usize];
        if x >= width || y >= height {
            self.active = false;
            return;
        }
        let start = ((y * width + x) * 4) as usize;
        let Some(target) = pixels.get(start..start + 4) else {
            self.active = false;
            return;
        };
        let threshold = (tolerance as u32).pow(2) * 4;
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            let index = (cy * width + cx) as usize;
            if self.mask[index] != 0 {
                continue;
            }
            let i = index * 4;
            let Some(pixel) = pixels.get(i..i + 4) else {
                continue;
            };
            let distance = (0..4)
                .map(|c| (pixel[c] as i32 - target[c] as i32).unsigned_abs().pow(2))
                .sum::<u32>();
            if distance > threshold {
                continue;
            }
            self.mask[index] = 255;
            if cx > 0 {
                stack.push((cx - 1, cy));
            }
            if cx + 1 < width {
                stack.push((cx + 1, cy));
            }
            if cy > 0 {
                stack.push((cx, cy - 1));
            }
            if cy + 1 < height {
                stack.push((cx, cy + 1));
            }
        }
        self.active = true;
    }
}
pub fn translate_selection(
    layer: &mut Layer,
    mask: &mut Selection,
    width: u32,
    height: u32,
    dx: i32,
    dy: i32,
) {
    if !mask.active {
        layer.pixels =
            translated_pixels(&layer.pixels, width, height, width, height, dx, dy, false);
        if let Some(layer_mask) = &layer.mask {
            layer.mask = Some(translated_pixels(
                layer_mask, width, height, width, height, dx, dy, true,
            ));
        }
        return;
    }
    if dx == 0 && dy == 0 {
        return;
    }
    let original = layer.pixels.clone();
    let original_layer_mask = layer.mask.clone();
    let mut next = original.clone();
    let mut next_layer_mask = original_layer_mask.clone();
    let mut next_mask = vec![0; mask.mask.len()];
    let mut moves = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let src = (y * width + x) as usize;
            let coverage = mask.mask.get(src).copied().unwrap_or(0);
            if coverage == 0 {
                continue;
            }
            let tx = x as i32 + dx;
            let ty = y as i32 + dy;
            let dst = (tx >= 0 && ty >= 0 && tx < width as i32 && ty < height as i32)
                .then(|| (ty as u32 * width + tx as u32) as usize);
            moves.push((src, dst, coverage));
        }
    }
    for &(src, _, coverage) in &moves {
        let (x, y) = (src as u32 % width, src as u32 / width);
        let mut pixel = original.pixel(x, y);
        pixel[3] = (pixel[3] as f32 * (1.0 - coverage as f32 / 255.0)).round() as u8;
        if pixel[3] == 0 {
            pixel[..3].fill(0);
        }
        next.set_pixel(x, y, pixel);
        if let Some(layer_mask) = &mut next_layer_mask {
            layer_mask.set_pixel(x, y, [255; 4]);
        }
    }
    for (src, dst, coverage) in moves {
        let Some(dst) = dst else {
            continue;
        };
        let src_px = original.pixel(src as u32 % width, src as u32 / width);
        let mut dst_px = next.pixel(dst as u32 % width, dst as u32 / width);
        let source_alpha = src_px[3] as f32 / 255.0 * coverage as f32 / 255.0;
        let destination_alpha = dst_px[3] as f32 / 255.0;
        let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
        if output_alpha > 0.0 {
            for channel in 0..3 {
                dst_px[channel] = ((src_px[channel] as f32 * source_alpha
                    + dst_px[channel] as f32 * destination_alpha * (1.0 - source_alpha))
                    / output_alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            dst_px[3] = (output_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        next.set_pixel(dst as u32 % width, dst as u32 / width, dst_px);
        next_mask[dst] = next_mask[dst].max(coverage);
        if let Some(layer_mask) = &mut next_layer_mask {
            layer_mask.set_pixel(
                dst as u32 % width,
                dst as u32 / width,
                original_layer_mask
                    .as_ref()
                    .map(|source| {
                        source.pixel_or_tile_default(
                            src as u32 % width,
                            src as u32 / width,
                            [255; 4],
                        )
                    })
                    .unwrap_or([255; 4]),
            );
        }
    }
    layer.pixels = next;
    layer.mask = next_layer_mask;
    mask.mask = next_mask;
    layer.pixels.prune_empty_tiles();
    if let Some(layer_mask) = &mut layer.mask {
        layer_mask.prune_empty_tiles();
    }
}

#[allow(clippy::too_many_arguments)] // The transform is defined by an explicit matrix-like parameter set.
pub fn transform_selection(
    layer: &mut Layer,
    selection: &mut Selection,
    width: u32,
    height: u32,
    scale_x: f32,
    scale_y: f32,
    angle: f32,
    history: &mut History,
) {
    let selected = selection.active;
    let mut bounds = (width as i32, height as i32, -1i32, -1i32);
    if selected {
        for (i, &v) in selection.mask.iter().enumerate() {
            if v != 0 {
                let x = (i % width as usize) as i32;
                let y = (i / width as usize) as i32;
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x);
                bounds.3 = bounds.3.max(y);
            }
        }
        if bounds.2 < 0 {
            return;
        }
    } else {
        bounds = (0, 0, width as i32 - 1, height as i32 - 1);
    }
    let cx = (bounds.0 + bounds.2) as f32 / 2.;
    let cy = (bounds.1 + bounds.3) as f32 / 2.;
    let (c, s) = (angle.cos(), angle.sin());
    let mut moves = Vec::new();
    let mut sources = Vec::new();
    for y in bounds.1..=bounds.3 {
        for x in bounds.0..=bounds.2 {
            let src = (y as u32 * width + x as u32) as usize;
            let coverage = if selected {
                selection.mask.get(src).copied().unwrap_or(0)
            } else {
                255
            };
            if coverage == 0 {
                continue;
            }
            sources.push((src, coverage));
            let dx = (x as f32 - cx) * scale_x;
            let dy = (y as f32 - cy) * scale_y;
            let tx = cx + dx * c - dy * s;
            let ty = cy + dx * s + dy * c;
            let left = tx.floor() as i32;
            let top = ty.floor() as i32;
            let fx = tx - left as f32;
            let fy = ty - top as f32;
            for (offset_x, offset_y, weight) in [
                (0, 0, (1.0 - fx) * (1.0 - fy)),
                (1, 0, fx * (1.0 - fy)),
                (0, 1, (1.0 - fx) * fy),
                (1, 1, fx * fy),
            ] {
                let dest_x = left + offset_x;
                let dest_y = top + offset_y;
                if weight > 0.0
                    && dest_x >= 0
                    && dest_y >= 0
                    && dest_x < width as i32
                    && dest_y < height as i32
                {
                    moves.push((
                        src,
                        (dest_y as u32 * width + dest_x as u32) as usize,
                        coverage as f32 / 255.0 * weight,
                    ));
                }
            }
        }
    }
    let mut affected = std::collections::HashSet::new();
    for &(src, _) in &sources {
        affected.insert(src);
    }
    for &(_, dst, _) in &moves {
        affected.insert(dst);
    }
    for &pi in &affected {
        for ch in 0..4 {
            history.record_pixel(layer, pi * 4 + ch);
            if layer.mask.is_some() {
                history.record_mask(layer, pi * 4 + ch);
            }
        }
    }
    let original = layer.pixels.clone();
    let original_mask = layer.mask.clone();
    let mut next = original.clone();
    let mut next_layer_mask = original_mask.clone();
    let mut next_mask = selection.mask.clone();
    let mut moved_mask = HashMap::<usize, (f32, f32)>::new();
    for &(src, coverage) in &sources {
        let (x, y) = (src as u32 % width, src as u32 / width);
        let mut pixel = original.pixel(x, y);
        pixel[3] = (pixel[3] as f32 * (1.0 - coverage as f32 / 255.0)).round() as u8;
        if pixel[3] == 0 {
            pixel[..3].fill(0);
        }
        next.set_pixel(x, y, pixel);
        if selected {
            next_mask[src] = 0;
        }
        if let Some(next_layer_mask) = next_layer_mask.as_mut() {
            let old = original_mask
                .as_ref()
                .map(|source| source.pixel_or_tile_default(x, y, [255; 4])[0])
                .unwrap_or(255) as f32;
            let cleared = old + (255.0 - old) * (coverage as f32 / 255.0);
            next_layer_mask.set_pixel(x, y, [cleared.round() as u8; 4]);
        }
    }
    for (src, dst, coverage) in moves {
        let src_px = original.pixel(src as u32 % width, src as u32 / width);
        let mut dst_px = next.pixel(dst as u32 % width, dst as u32 / width);
        let source_alpha = src_px[3] as f32 / 255.0 * coverage;
        let destination_alpha = dst_px[3] as f32 / 255.0;
        let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
        if output_alpha > 0.0 {
            for channel in 0..3 {
                dst_px[channel] = ((src_px[channel] as f32 * source_alpha
                    + dst_px[channel] as f32 * destination_alpha * (1.0 - source_alpha))
                    / output_alpha)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            dst_px[3] = (output_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        next.set_pixel(dst as u32 % width, dst as u32 / width, dst_px);
        if selected {
            next_mask[dst] = (next_mask[dst] as f32 + coverage * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
        if next_layer_mask.is_some() {
            let value = original_mask
                .as_ref()
                .map(|source| {
                    source.pixel_or_tile_default(src as u32 % width, src as u32 / width, [255; 4])
                        [0] as f32
                })
                .unwrap_or(255.0);
            let accumulated = moved_mask.entry(dst).or_default();
            accumulated.0 += value * coverage;
            accumulated.1 += coverage;
        }
    }
    if let Some(mask) = &mut next_layer_mask {
        for (pixel, (weighted_value, total_weight)) in moved_mask {
            if total_weight > 0.0 {
                let value = (weighted_value / total_weight).round().clamp(0.0, 255.0) as u8;
                mask.set_pixel(pixel as u32 % width, pixel as u32 / width, [value; 4]);
            }
        }
    }
    layer.pixels = next;
    layer.mask = next_layer_mask;
    if selected {
        selection.mask = next_mask;
    }
    layer.pixels.prune_empty_tiles();
    if let Some(mask) = &mut layer.mask {
        mask.prune_empty_tiles();
    }
}

/// Bilinear four-corner mesh warp. Offsets are ordered top-left, top-right,
/// bottom-left, bottom-right in canvas pixels.
pub fn mesh_warp(
    layer: &mut Layer,
    selection: &mut Selection,
    width: u32,
    height: u32,
    offsets: [(f32, f32); 4],
    history: &mut History,
) {
    let grid = offsets.map(|(x, y)| [x, y]);
    mesh_warp_grid(layer, selection, width, height, 2, 2, &grid, history);
}

fn bilinear_layer_pixel(pixels: &TilePixels, width: u32, height: u32, x: f32, y: f32) -> [u8; 4] {
    let x0 = x.floor().clamp(0.0, width.saturating_sub(1) as f32) as u32;
    let y0 = y.floor().clamp(0.0, height.saturating_sub(1) as f32) as u32;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = (x - x.floor()).clamp(0.0, 1.0);
    let fy = (y - y.floor()).clamp(0.0, 1.0);
    let samples = [
        (
            pixels.pixel_or_tile_default(x0, y0, [0; 4]),
            (1.0 - fx) * (1.0 - fy),
        ),
        (
            pixels.pixel_or_tile_default(x1, y0, [0; 4]),
            fx * (1.0 - fy),
        ),
        (
            pixels.pixel_or_tile_default(x0, y1, [0; 4]),
            (1.0 - fx) * fy,
        ),
        (pixels.pixel_or_tile_default(x1, y1, [0; 4]), fx * fy),
    ];
    let mut alpha = 0.0;
    let mut premultiplied = [0.0; 3];
    for (pixel, weight) in samples {
        let sample_alpha = pixel[3] as f32 / 255.0;
        alpha += sample_alpha * weight;
        for channel in 0..3 {
            premultiplied[channel] += pixel[channel] as f32 * sample_alpha * weight;
        }
    }
    if alpha <= f32::EPSILON {
        return [0; 4];
    }
    [
        (premultiplied[0] / alpha).round().clamp(0.0, 255.0) as u8,
        (premultiplied[1] / alpha).round().clamp(0.0, 255.0) as u8,
        (premultiplied[2] / alpha).round().clamp(0.0, 255.0) as u8,
        (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn bilinear_mask_value(mask: &[u8], width: u32, height: u32, x: f32, y: f32) -> u8 {
    let x0 = x.floor().clamp(0.0, width.saturating_sub(1) as f32) as u32;
    let y0 = y.floor().clamp(0.0, height.saturating_sub(1) as f32) as u32;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = (x - x.floor()).clamp(0.0, 1.0);
    let fy = (y - y.floor()).clamp(0.0, 1.0);
    let value =
        |sx: u32, sy: u32| mask.get((sy * width + sx) as usize).copied().unwrap_or(0) as f32;
    let top = value(x0, y0) * (1.0 - fx) + value(x1, y0) * fx;
    let bottom = value(x0, y1) * (1.0 - fx) + value(x1, y1) * fx;
    (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
}

/// Warp a regular grid of displacement controls. Offsets are row-major and
/// define a piecewise-bilinear displacement field over the selected bounds.
#[allow(clippy::too_many_arguments)] // Includes document, selection, grid dimensions, points, and history.
pub fn mesh_warp_grid(
    layer: &mut Layer,
    selection: &mut Selection,
    width: u32,
    height: u32,
    columns: usize,
    rows: usize,
    offsets: &[[f32; 2]],
    history: &mut History,
) {
    if width == 0 || height == 0 || columns < 2 || rows < 2 || offsets.len() != columns * rows {
        return;
    }
    if offsets
        .iter()
        .all(|offset| offset[0].abs() < 0.01 && offset[1].abs() < 0.01)
    {
        return;
    }
    let selected = selection.active;
    let mut bounds = (width as i32, height as i32, -1i32, -1i32);
    if selected {
        for (i, &v) in selection.mask.iter().enumerate() {
            if v != 0 {
                let x = (i % width as usize) as i32;
                let y = (i / width as usize) as i32;
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x);
                bounds.3 = bounds.3.max(y);
            }
        }
        if bounds.2 < 0 {
            return;
        }
    } else {
        bounds = (0, 0, width as i32 - 1, height as i32 - 1);
    }
    let max_offset = offsets
        .iter()
        .map(|offset| offset[0].abs().max(offset[1].abs()))
        .fold(0.0f32, f32::max)
        .ceil() as i32;
    let out = (
        (bounds.0 - max_offset).max(0),
        (bounds.1 - max_offset).max(0),
        (bounds.2 + max_offset).min(width as i32 - 1),
        (bounds.3 + max_offset).min(height as i32 - 1),
    );
    let span_x = (bounds.2 - bounds.0).max(1) as f32;
    let span_y = (bounds.3 - bounds.1).max(1) as f32;
    let displacement = |x: f32, y: f32| -> [f32; 2] {
        let u = ((x - bounds.0 as f32) / span_x).clamp(0., 1.);
        let v = ((y - bounds.1 as f32) / span_y).clamp(0., 1.);
        let grid_x = u * (columns - 1) as f32;
        let grid_y = v * (rows - 1) as f32;
        let cell_x = (grid_x.floor() as usize).min(columns - 2);
        let cell_y = (grid_y.floor() as usize).min(rows - 2);
        let local_x = (grid_x - cell_x as f32).clamp(0., 1.);
        let local_y = (grid_y - cell_y as f32).clamp(0., 1.);
        let top_left = offsets[cell_y * columns + cell_x];
        let top_right = offsets[cell_y * columns + cell_x + 1];
        let bottom_left = offsets[(cell_y + 1) * columns + cell_x];
        let bottom_right = offsets[(cell_y + 1) * columns + cell_x + 1];
        std::array::from_fn(|axis| {
            let top = top_left[axis] * (1. - local_x) + top_right[axis] * local_x;
            let bottom = bottom_left[axis] * (1. - local_x) + bottom_right[axis] * local_x;
            top * (1. - local_y) + bottom * local_y
        })
    };
    let original = layer.pixels.clone();
    let original_mask = layer.mask.clone();
    let mut next = original.clone();
    let mut next_layer_mask = original_mask.clone();
    let mut next_mask = selection.mask.clone();
    let mut affected = std::collections::HashSet::new();
    for y in bounds.1..=bounds.3 {
        for x in bounds.0..=bounds.2 {
            let i = (y as u32 * width + x as u32) as usize;
            let coverage = if selected {
                selection.mask.get(i).copied().unwrap_or(0)
            } else {
                255
            };
            if coverage != 0 {
                affected.insert(i);
                let mut pixel = original.pixel(x as u32, y as u32);
                pixel[3] = (pixel[3] as f32 * (1.0 - coverage as f32 / 255.0)).round() as u8;
                if pixel[3] == 0 {
                    pixel[..3].fill(0);
                }
                next.set_pixel(x as u32, y as u32, pixel);
                if selected {
                    next_mask[i] = 0;
                }
                if let Some(mask) = &mut next_layer_mask {
                    let old = original_mask
                        .as_ref()
                        .map(|source| source.pixel_or_tile_default(x as u32, y as u32, [255; 4])[0])
                        .unwrap_or(255) as f32;
                    let cleared = old + (255.0 - old) * (coverage as f32 / 255.0);
                    mask.set_pixel(x as u32, y as u32, [cleared.round() as u8; 4]);
                }
            }
        }
    }
    for y in out.1..=out.3 {
        for x in out.0..=out.2 {
            let (mut sx, mut sy) = (x as f32, y as f32);
            for _ in 0..8 {
                let displacement = displacement(sx, sy);
                sx = x as f32 - displacement[0];
                sy = y as f32 - displacement[1];
            }
            if sx < bounds.0 as f32
                || sy < bounds.1 as f32
                || sx > bounds.2 as f32
                || sy > bounds.3 as f32
            {
                continue;
            }
            let coverage = if selected {
                bilinear_mask_value(&selection.mask, width, height, sx, sy)
            } else {
                255
            };
            if coverage == 0 {
                continue;
            }
            let dst = (y as u32 * width + x as u32) as usize;
            affected.insert(dst);
            let src_px = bilinear_layer_pixel(&original, width, height, sx, sy);
            let mut dst_px = next.pixel(x as u32, y as u32);
            let source_alpha = src_px[3] as f32 / 255.0 * coverage as f32 / 255.0;
            let destination_alpha = dst_px[3] as f32 / 255.0;
            let output_alpha = source_alpha + destination_alpha * (1.0 - source_alpha);
            if output_alpha > 0.0 {
                for channel in 0..3 {
                    dst_px[channel] = ((src_px[channel] as f32 * source_alpha
                        + dst_px[channel] as f32 * destination_alpha * (1.0 - source_alpha))
                        / output_alpha)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                dst_px[3] = (output_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            next.set_pixel(x as u32, y as u32, dst_px);
            if selected {
                next_mask[dst] = next_mask[dst].max(coverage);
            }
            if let Some(mask) = &mut next_layer_mask {
                let value = original_mask
                    .as_ref()
                    .map(|source| bilinear_layer_pixel(source, width, height, sx, sy))
                    .unwrap_or([255; 4]);
                mask.set_pixel(x as u32, y as u32, value);
            }
        }
    }
    for &pixel in &affected {
        for channel in 0..4 {
            history.record_pixel(layer, pixel * 4 + channel);
            if layer.mask.is_some() {
                history.record_mask(layer, pixel * 4 + channel);
            }
        }
    }
    layer.pixels = next;
    layer.mask = next_layer_mask;
    if selected {
        selection.mask = next_mask;
    }
    layer.pixels.prune_empty_tiles();
    if let Some(mask) = &mut layer.mask {
        mask.prune_empty_tiles();
    }
}

pub fn gaussian_blur(layer: &mut Layer, width: u32, height: u32, radius: u32) {
    if radius == 0 || width == 0 || height == 0 {
        return;
    }
    let src = layer.pixels.clone();
    let mut tmp = src.clone();
    let r = radius.min(32) as i32;
    let sigma = (r as f32 / 2.0).max(0.5);
    let weights: Vec<f32> = (-r..=r)
        .map(|offset| (-0.5 * (offset as f32 / sigma).powi(2)).exp())
        .collect();
    let weight_sum: f32 = weights.iter().sum();
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            let i = ((y as u32 * width + x as u32) * 4) as usize;
            let mut sum = [0.0f32; 4];
            for (index, k) in (-r..=r).enumerate() {
                let xx = (x + k).clamp(0, width as i32 - 1) as u32;
                let j = ((y as u32 * width + xx) * 4) as usize;
                let alpha = src[j + 3] as f32 / 255.0;
                let weight = weights[index];
                for c in 0..3 {
                    sum[c] += src[j + c] as f32 * alpha * weight;
                }
                sum[3] += src[j + 3] as f32 * weight;
            }
            for c in 0..4 {
                tmp[i + c] = (sum[c] / weight_sum).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;

            let mut sum = [0.0f32; 4];
            for (index, k) in (-r..=r).enumerate() {
                let yy = (y as i32 + k).clamp(0, height as i32 - 1) as u32;
                let j = ((yy * width + x) * 4) as usize;
                for c in 0..4 {
                    sum[c] += tmp[j + c] as f32 * weights[index];
                }
            }
            let alpha = (sum[3] / weight_sum).clamp(0.0, 255.0);
            layer.pixels[i + 3] = alpha.round() as u8;
            if alpha > 0.0 {
                for (c, sum_channel) in sum.iter().take(3).enumerate() {
                    let premultiplied = sum_channel / weight_sum;
                    layer.pixels[i + c] =
                        (premultiplied * 255.0 / alpha).round().clamp(0.0, 255.0) as u8;
                }
            } else {
                layer.pixels[i..i + 3].fill(0);
            }
        }
    }
    layer.pixels.prune_empty_tiles();
}
pub fn sharpen(layer: &mut Layer, width: u32, height: u32, amount: f32) {
    let src = layer.pixels.clone();
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let i = ((y * width + x) * 4) as usize;
            for c in 0..3 {
                let center = src[i + c] as f32;
                let neighbors = src[i - 4 + c] as f32
                    + src[i + 4 + c] as f32
                    + src[i - width as usize * 4 + c] as f32
                    + src[i + width as usize * 4 + c] as f32;
                layer.pixels[i + c] = (center + (center * 4. - neighbors) * amount)
                    .round()
                    .clamp(0., 255.) as u8;
            }
        }
    }
    layer.pixels.prune_empty_tiles();
}
pub fn auto_levels(layer: &mut Layer) {
    let mut low = [255u8; 3];
    let mut high = [0u8; 3];
    for px in layer.pixels.chunks_exact(4) {
        if px[3] == 0 {
            continue;
        }
        for c in 0..3 {
            low[c] = low[c].min(px[c]);
            high[c] = high[c].max(px[c]);
        }
    }
    for i in (0..layer.pixels.len()).step_by(4) {
        if layer.pixels[i + 3] == 0 {
            continue;
        }
        let old = [layer.pixels[i], layer.pixels[i + 1], layer.pixels[i + 2]];
        for c in 0..3 {
            let range = high[c].saturating_sub(low[c]).max(1);
            layer.pixels[i + c] =
                ((old[c].saturating_sub(low[c]) as u16 * 255) / range as u16) as u8;
        }
    }
    layer.pixels.prune_empty_tiles();
}
pub fn levels(
    layer: &mut Layer,
    input_black: f32,
    input_white: f32,
    gamma: f32,
    output_black: f32,
    output_white: f32,
) {
    let black = input_black.clamp(0.0, 1.0 - 1.0 / 255.0);
    let white = input_white.clamp(black + 1.0 / 255.0, 1.0);
    let range = (white - black).max(1.0 / 255.0);
    let gamma = gamma.clamp(0.1, 4.0);
    let out_black = output_black.clamp(0.0, 1.0);
    let out_white = output_white.clamp(out_black, 1.0);
    for i in (0..layer.pixels.len()).step_by(4) {
        if layer.pixels[i + 3] == 0 {
            continue;
        }
        for channel in 0..3 {
            let input = (layer.pixels[i + channel] as f32 / 255.0 - black) / range;
            let normalized = input.clamp(0.0, 1.0).powf(1.0 / gamma);
            layer.pixels[i + channel] = ((out_black + normalized * (out_white - out_black)) * 255.0)
                .round()
                .clamp(0.0, 255.0) as u8;
        }
    }
    layer.pixels.prune_empty_tiles();
}
pub fn hue_saturation(layer: &mut Layer, hue_degrees: f32, saturation_scale: f32) {
    let shift = hue_degrees.rem_euclid(360.) / 60.;
    for i in (0..layer.pixels.len()).step_by(4) {
        if layer.pixels[i + 3] == 0 {
            continue;
        }
        let (r, g, b) = (
            layer.pixels[i] as f32 / 255.,
            layer.pixels[i + 1] as f32 / 255.,
            layer.pixels[i + 2] as f32 / 255.,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let mut h = if delta == 0. {
            0.
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.)
        } else if max == g {
            (b - r) / delta + 2.
        } else {
            (r - g) / delta + 4.
        };
        h = (h + shift).rem_euclid(6.);
        let s = if max == 0. {
            0.
        } else {
            (delta / max * saturation_scale.max(0.)).clamp(0.0, 1.0)
        };
        let c = max * s;
        let x = c * (1. - ((h.rem_euclid(2.) - 1.).abs()));
        let m = max - c;
        let (r1, g1, b1) = match h as u8 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        layer.pixels[i] = ((r1 + m) * 255.).round().clamp(0., 255.) as u8;
        layer.pixels[i + 1] = ((g1 + m) * 255.).round().clamp(0., 255.) as u8;
        layer.pixels[i + 2] = ((b1 + m) * 255.).round().clamp(0., 255.) as u8;
    }
    layer.pixels.prune_empty_tiles();
}
pub fn color_adjust(
    layer: &mut Layer,
    brightness: f32,
    contrast: f32,
    saturation: f32,
    gamma: f32,
) {
    let g = gamma.clamp(0.1, 4.0);
    for i in (0..layer.pixels.len()).step_by(4) {
        if layer.pixels[i + 3] == 0 {
            continue;
        }
        let mut rgb = [0f32; 3];
        for (c, channel) in rgb.iter_mut().enumerate() {
            let v = (layer.pixels[i + c] as f32 / 255.).clamp(0., 1.);
            let curved = v.powf(1. / g);
            *channel = ((curved - 0.5) * (1. + contrast.clamp(-0.95, 2.)) + 0.5 + brightness)
                .clamp(0., 1.);
        }
        let l = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        for (c, channel) in rgb.iter().enumerate() {
            layer.pixels[i + c] = ((l + (channel - l) * saturation.clamp(0., 3.)) * 255.)
                .round()
                .clamp(0., 255.) as u8;
        }
    }
    layer.pixels.prune_empty_tiles();
}
/// Apply an RGB tone curve described by five evenly spaced control values.
/// Applies the smooth tone curve through `points` (see
/// [`filters::tone_curve_value`]) to the colour of every painted pixel.
pub fn apply_tone_curve(layer: &mut Layer, points: [f32; 5]) {
    let lut: Vec<f32> = (0..256)
        .map(|i| filters::tone_curve_value(points, i as f32 / 255.0))
        .collect();
    for i in (0..layer.pixels.len()).step_by(4) {
        if layer.pixels[i + 3] == 0 {
            continue;
        }
        for channel in 0..3 {
            let output = lut[layer.pixels[i + channel] as usize];
            layer.pixels[i + channel] = (output * 255.).round() as u8;
        }
    }
    layer.pixels.prune_empty_tiles();
}

#[cfg(test)]
mod tile_tests {
    use super::*;

    #[test]
    fn tiles_changed_since_reports_only_written_tiles() {
        let mut pixels = TilePixels::new(600, 600);
        pixels.set_pixel(10, 10, [1, 2, 3, 255]);
        pixels.set_pixel(300, 10, [1, 2, 3, 255]);
        let before = pixels.clone();
        pixels.set_pixel(310, 20, [9, 9, 9, 255]);
        pixels.set_pixel(10, 300, [9, 9, 9, 255]);
        let mut changed = pixels.tiles_changed_since(&before);
        changed.sort_unstable();
        assert_eq!(changed, vec![(0, 1), (1, 0)]);
        assert!(before.tiles_changed_since(&before).is_empty());
    }
}

#[cfg(test)]
mod tone_composite_tests {
    use super::*;

    #[test]
    fn tone_layers_composite_the_same_whole_or_by_tile() {
        let mut doc = Document::new(600, 400);
        doc.dpi = 600.0;
        for y in 0..400 {
            for x in 0..600 {
                doc.layers[0].pixels.set_pixel(x, y, [0, 0, 0, 100]);
            }
        }
        doc.layers[0].tone = Some(ToneSettings::default());
        let whole = composite(&doc);
        // Neither flat grey nor white: dots.
        let values: std::collections::HashSet<u8> = whole.chunks_exact(4).map(|p| p[0]).collect();
        assert!(values.contains(&255) && values.contains(&0), "no halftone");
        let (w, h, tile) = composite_tile(&doc, 1, 1).unwrap();
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (TILE_SIZE + x, TILE_SIZE + y);
                let a = &whole[((dy * 600 + dx) * 4) as usize..][..4];
                let b = &tile[((y * w + x) * 4) as usize..][..4];
                assert_eq!(a, b, "at {dx},{dy}");
            }
        }
        assert_eq!(
            sample_composite_pixel(&doc, 300, 200),
            [
                whole[(200 * 600 + 300) * 4],
                whole[(200 * 600 + 300) * 4 + 1],
                whole[(200 * 600 + 300) * 4 + 2],
                255
            ]
        );
    }
}

/// A copy of `doc` with every tone layer converted to its displayed pixels
/// (for formats that cannot describe tones).
pub fn bake_tone_layers(doc: &Document) -> Document {
    let mut baked = doc.clone();
    for layer in &mut baked.layers {
        let Some(settings) = layer.tone.take() else {
            continue;
        };
        let keys: Vec<(u32, u32)> = layer.pixels.tile_keys();
        for (tile_x, tile_y) in keys {
            let Some(tile) = layer.pixels.tile_data(tile_x, tile_y) else {
                continue;
            };
            let toned = tone::tone_tile(
                &settings,
                doc.dpi,
                (tile_x * TILE_SIZE, tile_y * TILE_SIZE),
                TILE_SIZE,
                tile,
            );
            layer.pixels.tiles.insert((tile_x, tile_y), Arc::new(toned));
        }
    }
    baked
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;

    #[test]
    fn halving_keeps_flat_colours_and_averages_in_linear_light() {
        // A flat colour stays exactly the same at every level.
        let flat = [123u8, 45, 200, 255].repeat(16);
        let mut half = vec![0u8; 4 * 4];
        downsample_half_into(&flat, 4, 4, 0, 0, &mut half, 2, 2);
        assert_eq!(half, [123u8, 45, 200, 255].repeat(4));
        // Black and white stripes: half the light, not mid-grey paint.
        let mut stripes = Vec::new();
        for _ in 0..2 {
            stripes.extend([0, 0, 0, 255, 255, 255, 255, 255]);
        }
        let mut one = vec![0u8; 4];
        downsample_half_into(&stripes, 2, 2, 0, 0, &mut one, 1, 1);
        assert!((186..=189).contains(&one[0]), "{}", one[0]);
    }

    /// A 256×4 layer holding every 8-bit value in each channel.
    fn ramp(alpha: u8) -> Document {
        let mut doc = Document::new(256, 4);
        for x in 0..256u32 {
            let v = x as u8;
            doc.layers[0].pixels.set_pixel(x, 0, [v, 0, 0, alpha]);
            doc.layers[0].pixels.set_pixel(x, 1, [0, v, 0, alpha]);
            doc.layers[0].pixels.set_pixel(x, 2, [0, 0, v, alpha]);
            doc.layers[0]
                .pixels
                .set_pixel(x, 3, [v, 255 - v, v / 2, alpha]);
        }
        doc
    }

    #[test]
    fn opaque_pixels_composite_to_exactly_themselves() {
        let doc = ramp(255);
        let whole = composite(&doc);
        let (_, _, tile) = composite_tile(&doc, 0, 0).unwrap();
        for y in 0..4u32 {
            for x in 0..256u32 {
                let expected = doc.layers[0].pixels.pixel(x, y);
                let i = ((y * 256 + x) * 4) as usize;
                assert_eq!(&whole[i..i + 4], &expected, "whole at {x},{y}");
                assert_eq!(&tile[i..i + 4], &expected, "tile at {x},{y}");
            }
        }
    }

    #[test]
    fn translucent_pixels_round_to_nearest() {
        let doc = ramp(128);
        let out = composite(&doc);
        for x in 0..256u32 {
            let a = 128.0 / 255.0;
            let expected = (x as f32 / 255.0 * a + (1.0 - a)) * 255.0;
            let got = out[(x * 4) as usize] as f32;
            assert!(
                (got - expected).abs() <= 0.5 + 1e-3,
                "{x}: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn neutral_filters_leave_colours_unchanged() {
        let doc = ramp(255);
        let mut layer = doc.layers[0].clone();
        hue_saturation(&mut layer, 0.0, 1.0);
        let mut adjusted = doc.layers[0].clone();
        color_adjust(&mut adjusted, 0.0, 0.0, 1.0, 1.0);
        for y in 0..4 {
            for x in 0..256 {
                let original = doc.layers[0].pixels.pixel(x, y);
                assert_eq!(layer.pixels.pixel(x, y), original, "hue at {x},{y}");
                assert_eq!(adjusted.pixels.pixel(x, y), original, "adjust at {x},{y}");
            }
        }
    }
}

#[cfg(test)]
mod layer_tree_tests {
    use super::*;

    fn layer(id: u64, parent: Option<u64>, folder: bool) -> Layer {
        let mut layer = Layer::new(id, format!("{id}"), 8, 8);
        layer.parent_id = parent;
        if folder {
            layer.kind = LayerKind::Folder;
        }
        layer
    }

    fn ids(layers: &[Layer]) -> Vec<u64> {
        layers.iter().map(|layer| layer.id).collect()
    }

    #[test]
    fn folder_contents_sit_directly_below_the_folder() {
        // 3 is a child of folder 2 but was added on top of everything.
        let layers = vec![
            layer(1, None, false),
            layer(2, None, true),
            layer(4, None, false),
            layer(3, Some(2), false),
        ];
        assert_eq!(layer_tree_order(&layers), vec![1, 3, 2, 4]);
        // Nested folders and a missing parent.
        let layers = vec![
            layer(5, Some(6), false),
            layer(1, Some(99), false),
            layer(6, Some(2), true),
            layer(2, None, true),
            layer(7, Some(2), false),
        ];
        assert_eq!(layer_tree_order(&layers), vec![1, 5, 6, 7, 2]);
    }

    #[test]
    fn tidying_joins_the_last_undo_step() {
        let mut doc = Document::new(8, 8);
        doc.layers = vec![layer(1, None, false), layer(2, None, true)];
        let mut history = History::default();
        history.insert_layer(&mut doc.layers, 2, layer(3, Some(2), false));
        assert!(history.tidy_layer_tree(&mut doc.layers));
        assert_eq!(ids(&doc.layers), vec![1, 3, 2]);
        history.undo_document(&mut doc);
        assert_eq!(ids(&doc.layers), vec![1, 2]);
        history.redo_document(&mut doc);
        assert_eq!(ids(&doc.layers), vec![1, 3, 2]);
        assert!(!history.tidy_layer_tree(&mut doc.layers));
    }

    #[test]
    fn dropping_a_layer_on_a_folder_puts_it_inside() {
        let mut doc = Document::new(8, 8);
        doc.layers = vec![
            layer(1, None, false),
            layer(3, Some(2), false),
            layer(2, None, true),
            layer(4, None, false),
        ];
        let mut history = History::default();
        assert!(history.place_layer(&mut doc.layers, 4, 2, LayerPlacement::Into));
        assert_eq!(ids(&doc.layers), vec![1, 3, 4, 2]);
        assert_eq!(doc.layers[2].parent_id, Some(2));
        // Out again, below the folder.
        assert!(history.place_layer(&mut doc.layers, 4, 2, LayerPlacement::Below));
        assert_eq!(ids(&doc.layers), vec![1, 4, 3, 2]);
        assert_eq!(doc.layers[1].parent_id, None);
        // The folder moves with its contents, and cannot go into itself.
        assert!(history.place_layer(&mut doc.layers, 2, 1, LayerPlacement::Below));
        assert_eq!(ids(&doc.layers), vec![3, 2, 1, 4]);
        assert!(!history.place_layer(&mut doc.layers, 2, 3, LayerPlacement::Above));
        history.undo_document(&mut doc);
        assert_eq!(ids(&doc.layers), vec![1, 4, 3, 2]);
        history.undo_document(&mut doc);
        assert_eq!(ids(&doc.layers), vec![1, 3, 4, 2]);
        assert_eq!(doc.layers[2].parent_id, Some(2));
        history.undo_document(&mut doc);
        assert_eq!(ids(&doc.layers), vec![1, 3, 2, 4]);
        assert_eq!(doc.layers[3].parent_id, None);
    }
}
