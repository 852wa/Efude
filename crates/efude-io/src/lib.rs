// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
use efude_canvas::{
    BlendMode, Document, Layer, LayerKind, MAX_DOCUMENT_DIMENSION, MAX_DOCUMENT_PIXELS, TILE_SIZE,
    composite, composite_transparent, sparse_tile_keys,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    fs::{self, File},
    io::{Read, Seek, Write},
    path::Path,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};
const MAX_EFUDE_TILE_MEMORY: u64 = 2 * 1024 * 1024 * 1024;
const MAX_INCREMENTAL_ENTRY_COMPARE: usize = 32 * 1024 * 1024;

fn write_or_reuse_entry<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    previous: &mut Option<ZipArchive<File>>,
    name: &str,
    options: SimpleFileOptions,
    data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let can_reuse = if data.len() <= MAX_INCREMENTAL_ENTRY_COMPARE {
        if let Some(archive) = previous.as_mut() {
            if let Ok(mut old_entry) = archive.by_name(name) {
                if old_entry.size() == data.len() as u64 {
                    let mut old_data = Vec::with_capacity(data.len());
                    old_entry.read_to_end(&mut old_data).is_ok() && old_data == data
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    if can_reuse {
        writer.raw_copy_file(previous.as_mut().unwrap().by_name(name)?)?;
        return Ok(());
    }

    writer.start_file(name, options)?;
    writer.write_all(data)?;
    Ok(())
}

fn write_or_reuse_tile<W: Write + Seek>(
    writer: &mut ZipWriter<W>,
    previous: &mut Option<ZipArchive<File>>,
    name: &str,
    options: SimpleFileOptions,
    pixels: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let can_reuse = if let Some(archive) = previous.as_mut() {
        if let Ok(mut old_entry) = archive.by_name(name) {
            if old_entry.size() >= 4 && old_entry.size() <= pixels.len() as u64 + 64 {
                let mut old_tile = Vec::with_capacity(old_entry.size() as usize);
                old_entry.read_to_end(&mut old_tile).is_ok()
                    && old_tile.len() >= 4
                    && u32::from_le_bytes(old_tile[..4].try_into().unwrap()) as usize
                        == pixels.len()
                    && lz4_flex::decompress_size_prepended(&old_tile)
                        .is_ok_and(|decoded| decoded == pixels)
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    if can_reuse {
        writer.raw_copy_file(previous.as_mut().unwrap().by_name(name)?)?;
        return Ok(());
    }

    writer.start_file(name, options)?;
    writer.write_all(&lz4_flex::compress_prepend_size(pixels))?;
    Ok(())
}

fn reserve_efude_tile_memory(used: &mut u64) -> Result<(), Box<dyn std::error::Error>> {
    let tile_bytes = u64::from(TILE_SIZE) * u64::from(TILE_SIZE) * 4;
    *used = used
        .checked_add(tile_bytes)
        .ok_or(".efude decoded tile memory size overflow")?;
    if *used > MAX_EFUDE_TILE_MEMORY {
        return Err(".efude decoded tiles exceed the 2 GiB memory limit".into());
    }
    Ok(())
}

pub fn save(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err(".efude canvas dimensions exceed safety limits".into());
    }
    if !doc.dpi.is_finite() || doc.dpi <= 0.0 || doc.dpi > 10_000.0 {
        return Err(".efude DPI is invalid".into());
    }
    if doc.layers.is_empty() || doc.layers.len() > 2000 {
        return Err(".efude layer count is outside the supported range".into());
    }
    let mut ids = std::collections::HashSet::with_capacity(doc.layers.len());
    for layer in &doc.layers {
        if !ids.insert(layer.id) {
            return Err(".efude contains duplicate layer IDs".into());
        }
        if let Some(parent_id) = layer.parent_id {
            let parent = doc
                .layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .ok_or(".efude layer references a missing parent")?;
            if parent.kind != LayerKind::Folder {
                return Err(".efude layer parent is not a folder".into());
            }
        }
        let mut current = layer.parent_id;
        let mut ancestors = std::collections::HashSet::new();
        while let Some(parent_id) = current {
            if !ancestors.insert(parent_id) || parent_id == layer.id {
                return Err(".efude folder hierarchy contains a cycle".into());
            }
            current = doc
                .layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|parent| parent.parent_id);
        }
    }
    // mimetype, manifest, layers, merged image, thumbnail, optional metadata
    let mut entry_count = 5usize + usize::from(!doc.metadata.is_empty());
    for layer in &doc.layers {
        entry_count = entry_count
            .checked_add(sparse_tile_keys(layer, doc.width, doc.height).len())
            .ok_or(".efude archive entry count overflow")?;
        if let Some(mask) = &layer.mask {
            entry_count = entry_count
                .checked_add(
                    mask.tile_keys()
                        .into_iter()
                        .filter(|&(x, y)| x * TILE_SIZE < doc.width && y * TILE_SIZE < doc.height)
                        .count(),
                )
                .ok_or(".efude archive entry count overflow")?;
        }
    }
    if entry_count > 500_000 {
        return Err(".efude archive would contain too many entries".into());
    }
    let mut previous_archive = File::open(path)
        .ok()
        .and_then(|file| ZipArchive::new(file).ok())
        .filter(|archive| archive.len() <= 500_000);
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let f = temp.as_file().try_clone()?;
    let mut z = ZipWriter::new(f);
    let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let tile_options =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    write_or_reuse_entry(
        &mut z,
        &mut previous_archive,
        "mimetype",
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
        b"application/x-efude",
    )?;
    let manifest = serde_json::to_vec_pretty(
        &serde_json::json!({"format_version":"0.1","width":doc.width,"height":doc.height,"dpi":doc.dpi,"color_space":"sRGB","pixel_format":"RGBA8"}),
    )?;
    write_or_reuse_entry(&mut z, &mut previous_archive, "manifest.json", o, &manifest)?;
    let layers = serde_json::to_vec_pretty(&doc.layers.iter().map(|l|serde_json::json!({"id":l.id,"name":l.name,"visible":l.visible,"opacity":l.opacity,"locked":l.locked,"clipping":l.clipping,"sketch":l.sketch,"reference":l.reference,"blend":l.blend,"linear_blend":l.linear_blend,"kind":l.kind,"parent_id":l.parent_id,"expanded":l.expanded,"has_mask":l.mask.is_some(),"tone":l.tone})).collect::<Vec<_>>())?;
    write_or_reuse_entry(&mut z, &mut previous_archive, "layers.json", o, &layers)?;
    if !doc.metadata.is_empty() {
        let metadata = serde_json::to_vec_pretty(&doc.metadata)?;
        write_or_reuse_entry(&mut z, &mut previous_archive, "metadata.json", o, &metadata)?;
    }
    for l in &doc.layers {
        if let Some(strokes) = &l.vector {
            let bytes = serde_json::to_vec(strokes)?;
            write_or_reuse_entry(
                &mut z,
                &mut previous_archive,
                &format!("vectors/{}.json", l.id),
                o,
                &bytes,
            )?;
        }
        for (tx, ty) in sparse_tile_keys(l, doc.width, doc.height) {
            let tw = TILE_SIZE.min(doc.width - tx * TILE_SIZE);
            let th = TILE_SIZE.min(doc.height - ty * TILE_SIZE);
            let mut tile = Vec::with_capacity((tw * th * 4) as usize);
            for y in ty * TILE_SIZE..ty * TILE_SIZE + th {
                let start = ((y * doc.width + tx * TILE_SIZE) * 4) as usize;
                // Store the in-memory straight-alpha RGBA8 bytes unchanged so a
                // save/load round trip is lossless (see docs/spec/efude-format.md).
                tile.extend_from_slice(&l.pixels[start..start + (tw * 4) as usize]);
            }
            write_or_reuse_tile(
                &mut z,
                &mut previous_archive,
                &format!("tiles/{}/{tx}_{ty}.bin", l.id),
                tile_options,
                &tile,
            )?;
        }
        if let Some(mask) = &l.mask {
            for (tx, ty) in mask.tile_keys() {
                if tx * TILE_SIZE >= doc.width || ty * TILE_SIZE >= doc.height {
                    continue;
                }
                let tw = TILE_SIZE.min(doc.width - tx * TILE_SIZE);
                let th = TILE_SIZE.min(doc.height - ty * TILE_SIZE);
                let mut tile = Vec::with_capacity((tw * th) as usize);
                for y in 0..th {
                    for x in 0..tw {
                        tile.push(mask.pixel(tx * TILE_SIZE + x, ty * TILE_SIZE + y)[0]);
                    }
                }
                write_or_reuse_entry(
                    &mut z,
                    &mut previous_archive,
                    &format!("masks/{}/{tx}_{ty}.gray", l.id),
                    o,
                    &tile,
                )?;
            }
        }
    }
    let merged_pixels = composite(doc);
    let mut png_bytes = Vec::new();
    {
        let mut e = png::Encoder::new(std::io::Cursor::new(&mut png_bytes), doc.width, doc.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(&merged_pixels)?;
    }
    write_or_reuse_entry(&mut z, &mut previous_archive, "merged.png", o, &png_bytes)?;
    let full_preview = image::RgbaImage::from_raw(doc.width, doc.height, merged_pixels)
        .ok_or("could not create document preview")?;
    let thumbnail = image::imageops::thumbnail(&full_preview, 256, 256);
    let mut thumbnail_bytes = Vec::new();
    {
        let mut e = png::Encoder::new(
            std::io::Cursor::new(&mut thumbnail_bytes),
            thumbnail.width(),
            thumbnail.height(),
        );
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(thumbnail.as_raw())?;
    }
    write_or_reuse_entry(
        &mut z,
        &mut previous_archive,
        "thumbnail.png",
        o,
        &thumbnail_bytes,
    )?;
    let file = z.finish()?;
    file.sync_all()?;
    drop(file);
    // Windows cannot replace a file that is still open: close the old
    // archive (read for unchanged entries) first.
    drop(previous_archive);
    temp.persist(path)?;
    Ok(())
}
pub fn load(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
    use std::io::Read;
    let f = File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    if z.len() > 500_000 {
        return Err(".efude archive contains too many entries".into());
    }
    {
        let mut entry = z.by_index(0)?;
        if entry.name() != "mimetype" || entry.compression() != zip::CompressionMethod::Stored {
            return Err(
                "invalid .efude container: mimetype must be the first uncompressed entry".into(),
            );
        }
        let expected = b"application/x-efude";
        if entry.size() != expected.len() as u64 {
            return Err("invalid .efude mimetype length".into());
        }
        let mut mime = vec![0; expected.len()];
        entry.read_exact(&mut mime)?;
        if mime != expected {
            return Err("invalid .efude mimetype".into());
        }
    }
    let mut manifest_entry = z.by_name("manifest.json")?;
    if manifest_entry.size() > 1024 * 1024 {
        return Err(".efude manifest exceeds the size limit".into());
    }
    let mut manifest_bytes = Vec::with_capacity(manifest_entry.size() as usize);
    manifest_entry.read_to_end(&mut manifest_bytes)?;
    drop(manifest_entry);
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)?;
    let version = manifest["format_version"]
        .as_str()
        .ok_or(".efude format_version is missing")?;
    if version != "0.1" {
        return Err(format!("unsupported .efude format version: {version}").into());
    }
    if manifest["color_space"].as_str() != Some("sRGB")
        || !matches!(
            manifest["pixel_format"].as_str(),
            Some("RGBA8") | Some("RGBA8_PREMULTIPLIED")
        )
    {
        return Err("unsupported .efude color space or pixel format".into());
    }
    let width = u32::try_from(
        manifest["width"]
            .as_u64()
            .ok_or(".efude width is missing")?,
    )?;
    let height = u32::try_from(
        manifest["height"]
            .as_u64()
            .ok_or(".efude height is missing")?,
    )?;
    let premultiplied = manifest["pixel_format"].as_str() == Some("RGBA8_PREMULTIPLIED");
    if !efude_canvas::valid_document_dimensions(width, height) {
        return Err(".efude canvas dimensions exceed safety limits".into());
    }
    let dpi = manifest["dpi"].as_f64().unwrap_or(300.) as f32;
    if !dpi.is_finite() || dpi <= 0. || dpi > 10000. {
        return Err(".efude DPI is invalid".into());
    }
    let mut layers_entry = z.by_name("layers.json")?;
    if layers_entry.size() > 16 * 1024 * 1024 {
        return Err(".efude layer metadata exceeds the size limit".into());
    }
    let mut layers_bytes = Vec::with_capacity(layers_entry.size() as usize);
    layers_entry.read_to_end(&mut layers_bytes)?;
    drop(layers_entry);
    let metas: Vec<serde_json::Value> = serde_json::from_slice(&layers_bytes)?;
    if metas.is_empty() || metas.len() > 2000 {
        return Err(".efude layer count is outside the supported range".into());
    }
    let tile_slots =
        width.div_ceil(TILE_SIZE) as u64 * height.div_ceil(TILE_SIZE) as u64 * metas.len() as u64;
    if tile_slots > 2_000_000 {
        return Err(".efude canvas and layer count exceed loading limits".into());
    }
    let mut layers = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let mut allocated_tile_memory = 0u64;
    let mut uniform_mask_tiles = std::collections::HashMap::<u8, std::sync::Arc<Vec<u8>>>::new();
    for meta in metas {
        let id = meta["id"].as_u64().unwrap_or(layers.len() as u64 + 1);
        if !ids.insert(id) {
            return Err(".efude contains duplicate layer IDs".into());
        }
        let name = meta["name"].as_str().unwrap_or("レイヤー").to_string();
        let mut l = Layer::new(id, name, width, height);
        l.visible = meta["visible"].as_bool().unwrap_or(true);
        l.opacity = (meta["opacity"].as_f64().unwrap_or(1.) as f32).clamp(0., 1.);
        l.locked = meta["locked"].as_bool().unwrap_or(false);
        l.clipping = meta["clipping"].as_bool().unwrap_or(false);
        l.sketch = meta["sketch"].as_bool().unwrap_or(false);
        l.reference = meta["reference"].as_bool().unwrap_or(false);
        l.blend = serde_json::from_value(meta["blend"].clone()).unwrap_or(BlendMode::Normal);
        l.linear_blend = meta["linear_blend"].as_bool().unwrap_or(false);
        l.kind = serde_json::from_value(meta["kind"].clone()).unwrap_or(LayerKind::Raster);
        l.parent_id = meta["parent_id"].as_u64();
        l.expanded = meta["expanded"].as_bool().unwrap_or(true);
        l.tone = serde_json::from_value(meta["tone"].clone()).unwrap_or(None);
        if meta["has_mask"].as_bool().unwrap_or(false) {
            l.mask = Some(efude_canvas::TilePixels::new(width, height));
        }
        if let Ok(mut entry) = z.by_name(&format!("vectors/{id}.json")) {
            if entry.size() > 256 * 1024 * 1024 {
                return Err(".efude vector strokes exceed the size limit".into());
            }
            let mut bytes = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut bytes)?;
            let strokes: Vec<efude_canvas::VectorStroke> = serde_json::from_slice(&bytes)?;
            if strokes.iter().any(|stroke| {
                stroke
                    .points
                    .iter()
                    .any(|p| !p.x.is_finite() || !p.y.is_finite() || !p.width.is_finite())
            }) {
                return Err(".efude vector stroke has an invalid point".into());
            }
            l.vector = Some(strokes);
        }
        for ty in 0..height.div_ceil(TILE_SIZE) {
            for tx in 0..width.div_ceil(TILE_SIZE) {
                let tw = TILE_SIZE.min(width - tx * TILE_SIZE);
                let th = TILE_SIZE.min(height - ty * TILE_SIZE);
                let expected = (tw * th * 4) as usize;
                let compressed_name = format!("tiles/{id}/{tx}_{ty}.bin");
                let tile = if let Ok(mut entry) = z.by_name(&compressed_name) {
                    let max_packed = expected.saturating_add(65_536);
                    if entry.size() < 4 || entry.size() > max_packed as u64 {
                        return Err(".efude compressed tile exceeds the size limit".into());
                    }
                    let mut packed = Vec::with_capacity(entry.size() as usize);
                    (&mut entry)
                        .take(max_packed as u64 + 1)
                        .read_to_end(&mut packed)?;
                    if packed.len() < 4
                        || packed.len() > max_packed
                        || u32::from_le_bytes(packed[..4].try_into()?) as usize != expected
                    {
                        return Err(".efude tile has an invalid LZ4 size header".into());
                    }
                    let mut decoded = vec![0; expected];
                    let written = lz4_flex::block::decompress_into(&packed[4..], &mut decoded)?;
                    if written != expected {
                        return Err(".efude tile has an invalid LZ4 payload".into());
                    }
                    Some(decoded)
                } else if let Ok(mut entry) = z.by_name(&format!("tiles/{id}/{tx}_{ty}.rgba")) {
                    if entry.size() != expected as u64 {
                        return Err(".efude legacy tile has an invalid size".into());
                    }
                    let mut decoded = vec![0; expected];
                    entry.read_exact(&mut decoded)?;
                    Some(decoded)
                } else {
                    None
                };
                if let Some(mut tile) = tile {
                    reserve_efude_tile_memory(&mut allocated_tile_memory)?;
                    if premultiplied {
                        for pixel in tile.chunks_exact_mut(4) {
                            let alpha = pixel[3] as u16;
                            if alpha == 0 {
                                pixel[..3].fill(0);
                            } else if alpha < 255 {
                                for channel in &mut pixel[..3] {
                                    *channel =
                                        ((*channel as u32 * 255 + alpha as u32 / 2) / alpha as u32)
                                            .min(255) as u8;
                                }
                            }
                        }
                    }
                    for y in 0..th {
                        let dst = (((ty * TILE_SIZE + y) * width + tx * TILE_SIZE) * 4) as usize;
                        let src = (y * tw * 4) as usize;
                        l.pixels[dst..dst + (tw * 4) as usize]
                            .copy_from_slice(&tile[src..src + (tw * 4) as usize]);
                    }
                }
            }
        }
        if let Some(mask) = &mut l.mask {
            for ty in 0..height.div_ceil(TILE_SIZE) {
                for tx in 0..width.div_ceil(TILE_SIZE) {
                    let tw = TILE_SIZE.min(width - tx * TILE_SIZE);
                    let th = TILE_SIZE.min(height - ty * TILE_SIZE);
                    if let Ok(mut entry) = z.by_name(&format!("masks/{id}/{tx}_{ty}.gray")) {
                        let expected = (tw * th) as usize;
                        if entry.size() != expected as u64 {
                            return Err(".efude mask tile has an invalid size".into());
                        }
                        let mut data = vec![0; expected];
                        entry.read_exact(&mut data)?;
                        // Uniform tiles (the inside and outside of a panel
                        // mask) share one allocation.
                        if let Some(&v) = data.first()
                            && data.iter().all(|&b| b == v)
                        {
                            let tile = uniform_mask_tiles
                                .entry(v)
                                .or_insert_with(|| {
                                    std::sync::Arc::new(
                                        [v, v, v, 255].repeat((TILE_SIZE * TILE_SIZE) as usize),
                                    )
                                })
                                .clone();
                            mask.insert_shared_tile(tx, ty, tile);
                            continue;
                        }
                        reserve_efude_tile_memory(&mut allocated_tile_memory)?;
                        mask.ensure_tile_filled(tx * TILE_SIZE, ty * TILE_SIZE, [255; 4]);
                        for y in 0..th {
                            for x in 0..tw {
                                let v = data[(y * tw + x) as usize];
                                mask.set_pixel(
                                    tx * TILE_SIZE + x,
                                    ty * TILE_SIZE + y,
                                    [v, v, v, 255],
                                );
                            }
                        }
                    }
                }
            }
        }
        layers.push(l);
    }
    for layer in &layers {
        if let Some(parent_id) = layer.parent_id {
            let parent = layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .ok_or(".efude layer references a missing parent")?;
            if parent.kind != LayerKind::Folder {
                return Err(".efude layer parent is not a folder".into());
            }
        }
        let mut current = layer.parent_id;
        let mut ancestors = std::collections::HashSet::new();
        while let Some(parent_id) = current {
            if !ancestors.insert(parent_id) || parent_id == layer.id {
                return Err(".efude folder hierarchy contains a cycle".into());
            }
            current = layers
                .iter()
                .find(|candidate| candidate.id == parent_id)
                .and_then(|p| p.parent_id);
        }
    }
    let mut metadata = std::collections::BTreeMap::new();
    if let Ok(mut entry) = z.by_name("metadata.json") {
        if entry.size() > 16 * 1024 * 1024 {
            return Err(".efude metadata exceeds the size limit".into());
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        metadata = serde_json::from_slice(&bytes)?;
    }
    efude_canvas::tidy_layer_order(&mut layers);
    Ok(Document {
        width,
        height,
        dpi,
        layers,
        metadata,
    })
}
/// The preview stored in an `.efude` file (at most 256×256), as
/// straight-alpha RGBA, without loading the document.
pub fn load_thumbnail(path: &Path) -> Result<(u32, u32, Vec<u8>), Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut archive = ZipArchive::new(File::open(path)?)?;
    let mut entry = archive.by_name("thumbnail.png")?;
    if entry.size() > 4 * 1024 * 1024 {
        return Err("thumbnail too large".into());
    }
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.to_rgba8();
    Ok((image.width(), image.height(), image.into_raw()))
}

pub fn export_png(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err("image dimensions exceed safety limits".into());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut e = png::Encoder::new(temp.reopen()?, doc.width, doc.height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?
            .write_image_data(&composite_transparent(doc))?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn export_jpeg(
    path: &Path,
    doc: &Document,
    quality: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    if !efude_canvas::valid_document_dimensions(doc.width, doc.height) {
        return Err("image dimensions exceed safety limits".into());
    }
    let rgba = composite(doc);
    let mut rgb = Vec::with_capacity((doc.width * doc.height * 3) as usize);
    for px in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(temp.reopen()?, quality.clamp(1, 100))
        .encode(&rgb, doc.width, doc.height, image::ExtendedColorType::Rgb8)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
pub fn export_psd(path: &Path, doc: &Document) -> Result<(), Box<dyn std::error::Error>> {
    export_psd_report(path, doc).map(|_| ())
}

pub fn export_psd_report(
    path: &Path,
    doc: &Document,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    const MAX_PSD_LAYERS: usize = 200;
    // PSD has no tone layers: write them as the dots they show.
    let baked;
    let doc = if doc.layers.iter().any(|layer| layer.tone.is_some()) {
        baked = efude_canvas::bake_tone_layers(doc);
        &baked
    } else {
        doc
    };
    if doc.width > MAX_DOCUMENT_DIMENSION || doc.height > MAX_DOCUMENT_DIMENSION {
        return Err("PSD dimensions cannot exceed the supported maximum".into());
    }
    let pixel_count = (doc.width as u64)
        .checked_mul(doc.height as u64)
        .ok_or("PSD dimensions overflow")?;
    if pixel_count == 0 || pixel_count > MAX_DOCUMENT_PIXELS {
        return Err("PSD image exceeds the 100 million pixel safety limit".into());
    }
    let image = composite(doc);
    struct PsdLayer<'a> {
        layer: Option<&'a Layer>,
        section: Option<u32>,
        name: String,
    }
    fn flatten<'a>(layers: &'a [Layer], parent: Option<u64>, out: &mut Vec<PsdLayer<'a>>) {
        for layer in layers.iter().rev().filter(|l| l.parent_id == parent) {
            if layer.kind == LayerKind::Folder {
                out.push(PsdLayer {
                    layer: Some(layer),
                    section: Some(if layer.expanded { 1 } else { 2 }),
                    name: layer.name.clone(),
                });
                flatten(layers, Some(layer.id), out);
                out.push(PsdLayer {
                    layer: None,
                    section: Some(3),
                    name: String::new(),
                });
            } else {
                out.push(PsdLayer {
                    layer: Some(layer),
                    section: None,
                    name: layer.name.clone(),
                });
            }
        }
    }
    let mut ps_layers = Vec::new();
    flatten(&doc.layers, None, &mut ps_layers);
    if doc.layers.len() > MAX_PSD_LAYERS {
        return Err("PSD export supports documents with at most 200 layers".into());
    }
    if ps_layers.len() > MAX_PSD_LAYERS * 2 {
        return Err("PSD folder structure exceeds the supported layer-record limit".into());
    }
    fn encode_packbits(row: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < row.len() {
            let mut run = 1;
            while i + run < row.len() && row[i + run] == row[i] && run < 128 {
                run += 1;
            }
            if run >= 3 {
                out.push((1i16 - run as i16) as u8);
                out.push(row[i]);
                i += run;
            } else {
                let start = i;
                i += run;
                while i < row.len() && i - start < 128 {
                    let mut next_run = 1;
                    while i + next_run < row.len() && row[i + next_run] == row[i] && next_run < 128
                    {
                        next_run += 1;
                    }
                    if next_run >= 3 {
                        break;
                    }
                    i += next_run.min(128 - (i - start));
                }
                let len = i - start;
                out.push((len - 1) as u8);
                out.extend_from_slice(&row[start..i]);
            }
        }
    }
    fn encode_channel<F: FnMut(u32, u32) -> u8>(
        width: u32,
        height: u32,
        mut sample: F,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut output = Vec::new();
        output.extend_from_slice(&1u16.to_be_bytes());
        let lengths_at = output.len();
        output.resize(lengths_at + height as usize * 2, 0);
        let mut row = vec![0; width as usize];
        for y in 0..height {
            for x in 0..width {
                row[x as usize] = sample(x, y);
            }
            let start = output.len();
            encode_packbits(&row, &mut output);
            let length = output.len() - start;
            if length > u16::MAX as usize {
                return Err("PSD PackBits row exceeds format limits".into());
            }
            output[lengths_at + y as usize * 2..lengths_at + y as usize * 2 + 2]
                .copy_from_slice(&(length as u16).to_be_bytes());
        }
        Ok(output)
    }
    fn encode_layer_channel(
        pixels: &efude_canvas::TilePixels,
        width: u32,
        height: u32,
        channel: usize,
        default: u8,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut tiles_by_row = std::collections::HashMap::<u32, Vec<(u32, &[u8])>>::new();
        for ((tile_x, tile_y), data) in pixels.tiles() {
            tiles_by_row.entry(tile_y).or_default().push((tile_x, data));
        }
        let mut default_row = vec![default; width as usize];
        let mut default_encoded = Vec::new();
        encode_packbits(&default_row, &mut default_encoded);
        let mut output = Vec::new();
        output.extend_from_slice(&1u16.to_be_bytes());
        let lengths_at = output.len();
        output.resize(lengths_at + height as usize * 2, 0);
        for y in 0..height {
            let start = output.len();
            if let Some(tiles) = tiles_by_row.get(&(y / TILE_SIZE)) {
                let row = &mut default_row;
                row.fill(default);
                let local_y = y % TILE_SIZE;
                for &(tile_x, tile) in tiles {
                    let start_x = tile_x * TILE_SIZE;
                    if start_x >= width {
                        continue;
                    }
                    let tile_width = TILE_SIZE.min(width - start_x);
                    for local_x in 0..tile_width {
                        let source = ((local_y * TILE_SIZE + local_x) * 4) as usize + channel;
                        row[(start_x + local_x) as usize] =
                            tile.get(source).copied().unwrap_or(default);
                    }
                }
                encode_packbits(row, &mut output);
            } else {
                output.extend_from_slice(&default_encoded);
            }
            let length = output.len() - start;
            if length > u16::MAX as usize {
                return Err("PSD PackBits row exceeds format limits".into());
            }
            output[lengths_at + y as usize * 2..lengths_at + y as usize * 2 + 2]
                .copy_from_slice(&(length as u16).to_be_bytes());
        }
        Ok(output)
    }
    let mut layer_channels = Vec::with_capacity(ps_layers.len());
    let mut compressed_layer_bytes = 0u64;
    for ps in &ps_layers {
        let Some(layer) = ps.layer else {
            layer_channels.push(Vec::new());
            continue;
        };
        let raster = layer.kind == LayerKind::Raster;
        let mut channels = Vec::with_capacity(if layer.mask.is_some() {
            if raster { 5 } else { 1 }
        } else if raster {
            4
        } else {
            0
        });
        if raster {
            for channel in 0..4 {
                channels.push(encode_layer_channel(
                    &layer.pixels,
                    doc.width,
                    doc.height,
                    channel,
                    0,
                )?);
            }
        }
        if let Some(mask) = &layer.mask {
            channels.push(encode_layer_channel(mask, doc.width, doc.height, 0, 255)?);
        }
        for channel in &channels {
            compressed_layer_bytes = compressed_layer_bytes
                .checked_add(channel.len() as u64)
                .ok_or("PSD compressed layer size overflow")?;
        }
        if compressed_layer_bytes > 1536 * 1024 * 1024 {
            return Err("PSD compressed layer data exceeds the 1.5 GiB safety limit".into());
        }
        layer_channels.push(channels);
    }
    let mut records = Vec::new();
    for (layer_index, ps) in ps_layers.iter().enumerate() {
        let layer = ps.layer;
        let raster = layer.is_some_and(|l| l.kind == LayerKind::Raster);
        for value in if raster {
            [0i32, 0, doc.height as i32, doc.width as i32]
        } else {
            [0i32; 4]
        } {
            records.extend_from_slice(&value.to_be_bytes());
        }
        let channel_count = layer_channels[layer_index].len() as u16;
        records.extend_from_slice(&channel_count.to_be_bytes());
        let channel_ids = if raster {
            [
                Some(0i16),
                Some(1),
                Some(2),
                Some(-1),
                layer.unwrap().mask.as_ref().map(|_| -2),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
        } else {
            layer
                .unwrap()
                .mask
                .as_ref()
                .map_or_else(Vec::new, |_| vec![-2])
        };
        for (channel_index, id) in channel_ids.into_iter().enumerate() {
            records.extend_from_slice(&id.to_be_bytes());
            let data_bytes = layer_channels[layer_index][channel_index].len();
            if data_bytes > u32::MAX as usize {
                return Err("PSD channel exceeds format size limits".into());
            }
            records.extend_from_slice(&(data_bytes as u32).to_be_bytes());
        }
        records.extend_from_slice(b"8BIM");
        if matches!(ps.section, Some(1 | 2)) {
            records.extend_from_slice(b"pass");
        } else {
            records.extend_from_slice(match layer.map_or(BlendMode::Normal, |l| l.blend) {
                BlendMode::Normal => b"norm",
                BlendMode::Multiply => b"mul ",
                BlendMode::Screen => b"scrn",
                BlendMode::Overlay => b"over",
                BlendMode::Darken => b"dark",
                BlendMode::Lighten => b"lite",
                BlendMode::ColorDodge => b"div ",
                BlendMode::ColorBurn => b"idiv",
                BlendMode::HardLight => b"hLit",
                BlendMode::SoftLight => b"sLit",
                BlendMode::Difference => b"diff",
                BlendMode::Exclusion => b"smud",
                BlendMode::Add => b"lddg",
                BlendMode::Subtract => b"fsub",
            });
        }
        records.push(layer.map_or(255, |l| (l.opacity.clamp(0., 1.) * 255.) as u8));
        records.push(if layer.is_some_and(|l| l.clipping) {
            1
        } else {
            0
        });
        records.push(if layer.is_none_or(|l| l.visible) {
            0
        } else {
            2
        });
        records.push(0);
        let mut extra = Vec::new();
        if layer.is_some_and(|l| l.mask.is_some()) {
            extra.extend_from_slice(&18u32.to_be_bytes());
            for v in [0i32, 0, doc.height as i32, doc.width as i32] {
                extra.extend_from_slice(&v.to_be_bytes());
            }
            extra.push(255);
            extra.push(0);
            extra.push(0);
        } else {
            extra.extend_from_slice(&0u32.to_be_bytes());
        }
        extra.extend_from_slice(&0u32.to_be_bytes());
        // The Pascal name is read in the reader's legacy encoding, so it
        // only carries ASCII; the real name goes in the Unicode 'luni' block.
        let ascii_name: Vec<u8> = ps
            .name
            .chars()
            .take(255)
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() {
                    c as u8
                } else {
                    b'?'
                }
            })
            .collect();
        extra.push(ascii_name.len() as u8);
        extra.extend_from_slice(&ascii_name);
        while extra.len() % 4 != 0 {
            extra.push(0)
        }
        let utf16: Vec<u16> = ps.name.encode_utf16().collect();
        let mut unicode_name = Vec::with_capacity(4 + utf16.len() * 2);
        unicode_name.extend_from_slice(&(utf16.len() as u32).to_be_bytes());
        for unit in &utf16 {
            unicode_name.extend_from_slice(&unit.to_be_bytes());
        }
        while unicode_name.len() % 4 != 0 {
            unicode_name.push(0);
        }
        extra.extend_from_slice(b"8BIMluni");
        extra.extend_from_slice(&(unicode_name.len() as u32).to_be_bytes());
        extra.extend_from_slice(&unicode_name);
        if let Some(section) = ps.section {
            extra.extend_from_slice(b"8BIMlsct");
            extra.extend_from_slice(&4u32.to_be_bytes());
            extra.extend_from_slice(&section.to_be_bytes());
        }
        records.extend_from_slice(&(extra.len() as u32).to_be_bytes());
        records.extend_from_slice(&extra);
    }
    let channel_data = layer_channels
        .into_iter()
        .flatten()
        .flatten()
        .collect::<Vec<_>>();
    let mut layer_info = Vec::new();
    layer_info.extend_from_slice(&(ps_layers.len() as u16).to_be_bytes());
    layer_info.extend_from_slice(&records);
    layer_info.extend_from_slice(&channel_data);
    if layer_info.len() % 2 != 0 {
        layer_info.push(0)
    }
    let mut layer_mask = Vec::new();
    layer_mask.extend_from_slice(&(layer_info.len() as u32).to_be_bytes());
    layer_mask.extend_from_slice(&layer_info);
    // Empty global layer mask info; no further tagged blocks.
    layer_mask.extend_from_slice(&0u32.to_be_bytes());
    let dpi = if doc.dpi.is_finite() {
        doc.dpi.clamp(1.0, 65535.0)
    } else {
        300.0
    };
    let dpi_fixed = (dpi * 65536.0).round() as u32;
    let mut resolution = Vec::with_capacity(16);
    resolution.extend_from_slice(&dpi_fixed.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes()); // pixels per inch
    resolution.extend_from_slice(&1u16.to_be_bytes()); // display unit: inches
    resolution.extend_from_slice(&dpi_fixed.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes());
    resolution.extend_from_slice(&1u16.to_be_bytes());
    let mut image_resources = Vec::with_capacity(28);
    image_resources.extend_from_slice(b"8BIM");
    image_resources.extend_from_slice(&0x03EDu16.to_be_bytes()); // ResolutionInfo
    image_resources.extend_from_slice(&[0, 0]); // empty Pascal name and even padding
    image_resources.extend_from_slice(&(resolution.len() as u32).to_be_bytes());
    image_resources.extend_from_slice(&resolution);
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(parent)?;
    let mut f = temp.as_file().try_clone()?;
    f.write_all(b"8BPS")?;
    f.write_all(&1u16.to_be_bytes())?;
    f.write_all(&[0; 6])?;
    f.write_all(&3u16.to_be_bytes())?;
    f.write_all(&doc.height.min(30000).to_be_bytes())?;
    f.write_all(&doc.width.min(30000).to_be_bytes())?;
    f.write_all(&8u16.to_be_bytes())?;
    f.write_all(&3u16.to_be_bytes())?;
    f.write_all(&0u32.to_be_bytes())?;
    f.write_all(&(image_resources.len() as u32).to_be_bytes())?;
    f.write_all(&image_resources)?;
    f.write_all(&(layer_mask.len() as u32).to_be_bytes())?;
    f.write_all(&layer_mask)?;
    // Merged image data: one compression field for the whole section, then
    // the PackBits row-length table for every channel, then every row.
    // (Per-channel compression fields are only used in layer channel data.)
    f.write_all(&1u16.to_be_bytes())?;
    let mut row_lengths = Vec::with_capacity(3 * doc.height as usize * 2);
    let mut rows = Vec::new();
    for c in 0..3 {
        let channel = encode_channel(doc.width, doc.height, |x, y| {
            image[((y * doc.width + x) * 4 + c) as usize]
        })?;
        let table_end = 2 + doc.height as usize * 2;
        row_lengths.extend_from_slice(&channel[2..table_end]);
        rows.extend_from_slice(&channel[table_end..]);
    }
    f.write_all(&row_lengths)?;
    f.write_all(&rows)?;
    f.sync_all()?;
    drop(f);
    temp.persist(path)?;
    let mut warnings = Vec::new();
    if ps_layers
        .iter()
        .any(|ps| ps.layer.is_some_and(|l| l.linear_blend))
    {
        warnings.push("リニア合成設定はPSDに保存されません".to_string());
    }
    if doc
        .layers
        .iter()
        .any(|layer| layer.sketch || layer.reference)
    {
        warnings.push("下描き・参照レイヤー属性はPSDに保存されません".to_string());
    }
    if ps_layers
        .iter()
        .any(|ps| ps.layer.is_some_and(|l| l.locked))
    {
        warnings.push("レイヤーロック設定はPSDに保存されません".to_string());
    }
    if doc
        .layers
        .iter()
        .any(|layer| layer.kind == LayerKind::Folder && layer.blend != BlendMode::Normal)
    {
        warnings.push("フォルダーの合成モードはPSDに保存されません".to_string());
    }
    Ok(warnings)
}
pub struct PsdImport {
    pub document: Document,
    pub warnings: Vec<String>,
    pub warnings_en: Vec<String>,
}

pub fn import_psd(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
    Ok(import_psd_report(path)?.document)
}

pub fn import_psd_report(path: &Path) -> Result<PsdImport, Box<dyn std::error::Error>> {
    use std::io::Read;
    let mut f = File::open(path)?;
    let mut header = [0u8; 26];
    f.read_exact(&mut header)?;
    if &header[..4] != b"8BPS" || u16::from_be_bytes([header[4], header[5]]) != 1 {
        return Err("unsupported PSD header".into());
    }
    let channels = u16::from_be_bytes([header[12], header[13]]);
    let height = u32::from_be_bytes(header[14..18].try_into()?);
    let width = u32::from_be_bytes(header[18..22].try_into()?);
    let depth = u16::from_be_bytes([header[22], header[23]]);
    let mode = u16::from_be_bytes([header[24], header[25]]);
    let color_channels = match mode {
        1 => 1usize,
        3 => 3,
        4 => 4,
        _ => return Err("PSD color mode is not supported".into()),
    };
    if (channels as usize) < color_channels
        || channels > 56
        || !matches!(depth, 8 | 16)
        || width > MAX_DOCUMENT_DIMENSION
        || height > MAX_DOCUMENT_DIMENSION
    {
        return Err("PSD must be 8-bit or 16-bit grayscale, RGB, or CMYK".into());
    }
    let mut skip = [0u8; 4];
    f.read_exact(&mut skip)?;
    let color_mode_len = u32::from_be_bytes(skip) as u64;
    std::io::copy(
        &mut std::io::Read::by_ref(&mut f).take(color_mode_len),
        &mut std::io::sink(),
    )?;
    f.read_exact(&mut skip)?;
    let resources_len = u32::from_be_bytes(skip) as usize;
    if resources_len > 16 * 1024 * 1024 {
        return Err("PSD image resources exceed safety limit".into());
    }
    let mut resources = vec![0; resources_len];
    f.read_exact(&mut resources)?;
    let image_dpi = parse_psd_dpi(&resources).unwrap_or(300.0);
    f.read_exact(&mut skip)?;
    let layer_mask_len = u32::from_be_bytes(skip) as usize;
    if layer_mask_len > 512 * 1024 * 1024 {
        return Err("PSD layers exceed memory safety limit".into());
    }
    let mut layer_mask = vec![0; layer_mask_len];
    f.read_exact(&mut layer_mask)?;
    if width == 0 || height == 0 {
        return Err("PSD dimensions must be non-zero".into());
    }
    let mut compression = [0; 2];
    f.read_exact(&mut compression)?;
    let compression = u16::from_be_bytes(compression);
    if compression > 1 {
        return Err("unsupported PSD composite compression".into());
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or("PSD dimensions overflow")?;
    if n as u64 > MAX_DOCUMENT_PIXELS {
        return Err("PSD image exceeds safety limit".into());
    }
    let sample_bytes = (depth / 8) as usize;
    let estimated_bytes = n
        .checked_mul(4usize + channels as usize + sample_bytes)
        .ok_or("PSD allocation size overflow")?;
    if estimated_bytes > 512 * 1024 * 1024 {
        return Err("PSD image exceeds memory safety limit".into());
    }
    let mut planes = vec![vec![0; n]; channels as usize];
    if compression == 0 {
        for plane in &mut planes {
            let mut raw = vec![0; n * sample_bytes];
            f.read_exact(&mut raw)?;
            for (i, out) in plane.iter_mut().enumerate() {
                *out = if depth == 8 {
                    raw[i]
                } else {
                    let sample = u16::from_be_bytes([raw[i * 2], raw[i * 2 + 1]]) as u32;
                    ((sample * 255 + 32767) / 65535) as u8
                };
            }
        }
    } else {
        let row_count = channels as usize * height as usize;
        let mut row_lengths = vec![0u16; row_count];
        for length in &mut row_lengths {
            let mut bytes = [0; 2];
            f.read_exact(&mut bytes)?;
            *length = u16::from_be_bytes(bytes);
        }
        for channel in 0..channels as usize {
            for y in 0..height as usize {
                let length = row_lengths[channel * height as usize + y] as usize;
                let mut encoded = vec![0; length];
                f.read_exact(&mut encoded)?;
                let row = decode_packbits(&encoded, width as usize * sample_bytes)?;
                for x in 0..width as usize {
                    let value = if depth == 8 {
                        row[x]
                    } else {
                        let sample = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                        ((sample * 255 + 32767) / 65535) as u8
                    };
                    planes[channel][y * width as usize + x] = value;
                }
            }
        }
    }
    let mut doc = Document::new(width, height);
    doc.dpi = image_dpi;
    let alpha_channel = if channels as usize > color_channels {
        Some(color_channels)
    } else {
        None
    };
    for (i, first_channel) in planes[0].iter().enumerate() {
        let p = i * 4;
        match mode {
            1 => {
                let gray = *first_channel;
                doc.layers[0].pixels[p..p + 3].fill(gray);
            }
            3 => {
                for (c, plane) in planes.iter().take(3).enumerate() {
                    doc.layers[0].pixels[p + c] = plane[i];
                }
            }
            4 => {
                let (c, m, y, k) = (
                    planes[0][i] as u32,
                    planes[1][i] as u32,
                    planes[2][i] as u32,
                    planes[3][i] as u32,
                );
                doc.layers[0].pixels[p] = (((255 - c) * (255 - k) + 127) / 255) as u8;
                doc.layers[0].pixels[p + 1] = (((255 - m) * (255 - k) + 127) / 255) as u8;
                doc.layers[0].pixels[p + 2] = (((255 - y) * (255 - k) + 127) / 255) as u8;
            }
            _ => unreachable!(),
        }
        doc.layers[0].pixels[p + 3] = alpha_channel.map_or(255, |a| planes[a][i]);
    }
    let mut unsupported_reasons = Vec::new();
    let mut has_text_layers = false;
    let mut layers_preserved = false;
    if matches!(mode, 1 | 3 | 4) {
        let (layers, unsupported, text_layers, reasons) =
            parse_psd_layers(&layer_mask, width, height, mode, depth)?;
        unsupported_reasons = reasons;
        has_text_layers = text_layers;
        if !unsupported && let Some(layers) = layers {
            doc.layers = layers;
            layers_preserved = true;
        }
    }
    let mut warnings = Vec::new();
    let mut warnings_en = Vec::new();
    if depth != 8 {
        warnings.push("PSDの16bitレイヤーチャンネルを8bitへ変換しました".into());
        warnings_en.push("Converted 16-bit PSD layer channels to 8-bit.".into());
    }
    if mode == 4 && !layers_preserved && unsupported_reasons.is_empty() {
        warnings
            .push("レイヤー構造を読み取れないCMYK画像を8bit RGBの統合画像へ変換しました".into());
        warnings_en.push(
            "Could not preserve the CMYK layer structure; imported it as a merged 8-bit RGB image."
                .into(),
        );
    } else if !layers_preserved && unsupported_reasons.is_empty() {
        warnings.push("PSDに編集レイヤー情報がないため、統合画像として読み込みました".into());
        warnings_en
            .push("The PSD has no editable layer data; imported it as a merged image.".into());
    }
    if !unsupported_reasons.is_empty() {
        let reasons_ja = unsupported_reasons.join("、");
        let reasons_en = unsupported_reasons
            .iter()
            .map(|reason| match *reason {
                "合成モード" => "blend modes",
                "塗りつぶしレイヤー" => "fill layers",
                "スマートオブジェクト" => "smart objects",
                "レイヤー効果" => "layer effects",
                "ベクターマスク" => "vector masks",
                "調整レイヤー" => "adjustment layers",
                "圧縮方式" => "channel compression methods",
                "画素チャンネルのないレイヤー" => {
                    "layers without supported pixel channels"
                }
                _ => "unsupported layer features",
            })
            .collect::<Vec<_>>()
            .join(", ");
        warnings.push(format!(
            "未対応の{reasons_ja}があるため、PSDを統合画像として読み込みました"
        ));
        warnings_en.push(format!(
            "Unsupported {reasons_en} were found; imported the PSD as a merged image."
        ));
    }
    if has_text_layers && unsupported_reasons.is_empty() {
        warnings.push("PSDの文字レイヤーをラスター化して読み込みました".into());
        warnings_en.push("PSD text layers were imported as raster layers.".into());
    }
    Ok(PsdImport {
        document: doc,
        warnings,
        warnings_en,
    })
}

fn parse_psd_dpi(resources: &[u8]) -> Option<f32> {
    let mut pos = 0usize;
    while pos.checked_add(12)? <= resources.len() {
        if &resources[pos..pos + 4] != b"8BIM" {
            return None;
        }
        let id = u16::from_be_bytes([resources[pos + 4], resources[pos + 5]]);
        pos += 6;
        let name_len = *resources.get(pos)? as usize;
        let name_field_len = 1usize.checked_add(name_len)?;
        pos = pos.checked_add((name_field_len + 1) & !1)?;
        let data_len =
            u32::from_be_bytes(resources.get(pos..pos.checked_add(4)?)?.try_into().ok()?) as usize;
        pos += 4;
        let end = pos.checked_add(data_len)?;
        let data = resources.get(pos..end)?;
        if id == 0x03ED && data.len() >= 16 {
            let resolution = u32::from_be_bytes(data[..4].try_into().ok()?) as f32 / 65536.0;
            let unit = u16::from_be_bytes(data[4..6].try_into().ok()?);
            let dpi = match unit {
                1 => resolution,
                2 => resolution * 2.54,
                _ => return None,
            };
            return (dpi.is_finite() && dpi > 0.0).then_some(dpi);
        }
        pos = end.checked_add(data_len & 1)?;
    }
    None
}

fn be_u16<R: std::io::Read>(r: &mut R) -> Result<u16, Box<dyn std::error::Error>> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}
fn be_u32<R: std::io::Read>(r: &mut R) -> Result<u32, Box<dyn std::error::Error>> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}
fn be_i32<R: std::io::Read>(r: &mut R) -> Result<i32, Box<dyn std::error::Error>> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}
#[allow(clippy::type_complexity)] // Compact private parse result for feature support flags and layers.
fn parse_psd_layers(
    section: &[u8],
    width: u32,
    height: u32,
    color_mode: u16,
    depth: u16,
) -> Result<(Option<Vec<Layer>>, bool, bool, Vec<&'static str>), Box<dyn std::error::Error>> {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    if section.len() < 4 {
        return Ok((None, false, false, Vec::new()));
    }
    let mut outer = Cursor::new(section);
    let info_len = be_u32(&mut outer)? as usize;
    if info_len == 0 {
        return Ok((None, false, false, Vec::new()));
    }
    if info_len > section.len().saturating_sub(4) {
        return Err("invalid PSD layer info length".into());
    }
    let mut info = vec![0; info_len];
    outer.read_exact(&mut info)?;
    let mut r = Cursor::new(info);
    let count = (be_u16(&mut r)? as i16).unsigned_abs() as usize;
    if count > 400 {
        return Err("PSD layer count exceeds safety limit".into());
    }
    struct Channel {
        id: i16,
        len: usize,
    }
    struct Record {
        top: i32,
        left: i32,
        bottom: i32,
        right: i32,
        channels: Vec<Channel>,
        name: String,
        opacity: u8,
        visible: bool,
        clipping: bool,
        blend: BlendMode,
        has_mask: bool,
        mask_bounds: Option<(i32, i32, i32, i32)>,
        mask_default: u8,
        section: Option<u32>,
    }
    let mut records = Vec::with_capacity(count);
    let mut has_unsupported_features = false;
    let mut has_text_layers = false;
    let mut unsupported_reasons = Vec::new();
    for _ in 0..count {
        let top = be_i32(&mut r)?;
        let left = be_i32(&mut r)?;
        let bottom = be_i32(&mut r)?;
        let right = be_i32(&mut r)?;
        let n = be_u16(&mut r)? as usize;
        if n > 64 {
            return Err("PSD layer channel count exceeds safety limit".into());
        }
        let mut channels = Vec::with_capacity(n);
        for _ in 0..n {
            channels.push(Channel {
                id: be_u16(&mut r)? as i16,
                len: be_u32(&mut r)? as usize,
            });
        }
        let mut fixed = [0; 12];
        r.read_exact(&mut fixed)?;
        if &fixed[..4] != b"8BIM" {
            return Err("invalid PSD layer blend signature".into());
        }
        let blend = match &fixed[4..8] {
            b"norm" | b"pass" => BlendMode::Normal,
            b"mul " => BlendMode::Multiply,
            b"scrn" => BlendMode::Screen,
            b"over" => BlendMode::Overlay,
            b"dark" => BlendMode::Darken,
            b"lite" => BlendMode::Lighten,
            b"div " => BlendMode::ColorDodge,
            b"idiv" => BlendMode::ColorBurn,
            b"hLit" => BlendMode::HardLight,
            b"sLit" => BlendMode::SoftLight,
            b"diff" => BlendMode::Difference,
            b"smud" => BlendMode::Exclusion,
            b"lddg" => BlendMode::Add,
            b"fsub" => BlendMode::Subtract,
            _ => {
                has_unsupported_features = true;
                if !unsupported_reasons.contains(&"合成モード") {
                    unsupported_reasons.push("合成モード");
                }
                BlendMode::Normal
            }
        };
        let opacity = fixed[8];
        let clipping = fixed[9] != 0;
        let visible = fixed[10] & 2 == 0;
        let extra_len = be_u32(&mut r)? as usize;
        let start = r.position() as usize;
        let end = start
            .checked_add(extra_len)
            .ok_or("PSD extra data overflow")?;
        if end > r.get_ref().len() {
            return Err("invalid PSD layer extra data".into());
        }
        let mask_len = be_u32(&mut r)? as usize;
        let has_mask = mask_len >= 18;
        let mut mask_bounds = None;
        let mut mask_default = 255;
        if has_mask {
            let mut mask_header = [0u8; 18];
            r.read_exact(&mut mask_header)?;
            mask_bounds = Some((
                i32::from_be_bytes(mask_header[0..4].try_into()?),
                i32::from_be_bytes(mask_header[4..8].try_into()?),
                i32::from_be_bytes(mask_header[8..12].try_into()?),
                i32::from_be_bytes(mask_header[12..16].try_into()?),
            ));
            mask_default = mask_header[16];
            r.seek(SeekFrom::Current(mask_len.saturating_sub(18) as i64))?;
        } else {
            r.seek(SeekFrom::Current(mask_len as i64))?;
        }
        let blend_len = be_u32(&mut r)? as usize;
        r.seek(SeekFrom::Current(blend_len as i64))?;
        let mut name_len = [0u8; 1];
        r.read_exact(&mut name_len)?;
        let name_len = name_len[0] as usize;
        let mut name_bytes = vec![0; name_len];
        r.read_exact(&mut name_bytes)?;
        let mut name = String::from_utf8_lossy(&name_bytes).into_owned();
        let pad = (4 - ((name_len + 1) % 4)) % 4;
        r.seek(SeekFrom::Current(pad as i64))?;
        let mut section = None;
        while (r.position() as usize).saturating_add(12) <= end {
            let mut sig = [0; 4];
            r.read_exact(&mut sig)?;
            let mut key = [0; 4];
            r.read_exact(&mut key)?;
            let len = be_u32(&mut r)? as usize;
            if &sig != b"8BIM" {
                break;
            }
            if &key == b"TySh" {
                has_text_layers = true;
            }
            if matches!(
                &key,
                b"SoCo"
                    | b"GdFl"
                    | b"PtFl"
                    | b"PlLd"
                    | b"lnk2"
                    | b"lfx2"
                    | b"lrFX"
                    | b"vmsk"
                    | b"vsms"
                    | b"levl"
                    | b"curv"
                    | b"brit"
                    | b"hue2"
                    | b"selc"
                    | b"mixr"
                    | b"clrL"
                    | b"expA"
                    | b"blwh"
                    | b"phfl"
                    | b"vibA"
                    | b"grdm"
                    | b"nvrt"
                    | b"thrs"
                    | b"post"
            ) {
                has_unsupported_features = true;
                let reason = match &key {
                    b"SoCo" | b"GdFl" | b"PtFl" => "塗りつぶしレイヤー",
                    b"PlLd" | b"lnk2" => "スマートオブジェクト",
                    b"lfx2" | b"lrFX" => "レイヤー効果",
                    b"vmsk" | b"vsms" => "ベクターマスク",
                    _ => "調整レイヤー",
                };
                if !unsupported_reasons.contains(&reason) {
                    unsupported_reasons.push(reason);
                }
            }
            if &key == b"lsct" && len >= 4 {
                section = Some(be_u32(&mut r)?);
                r.seek(SeekFrom::Current(len.saturating_sub(4) as i64))?;
            } else if &key == b"luni" && len >= 4 {
                // Unicode layer name: u32 length, then UTF-16BE units.
                let count = be_u32(&mut r)? as usize;
                let units = count.min((len - 4) / 2);
                let mut utf16 = Vec::with_capacity(units);
                for _ in 0..units {
                    utf16.push(be_u16(&mut r)?);
                }
                while utf16.last() == Some(&0) {
                    utf16.pop();
                }
                name = String::from_utf16_lossy(&utf16);
                r.seek(SeekFrom::Current((len - 4 - units * 2) as i64))?;
            } else {
                r.seek(SeekFrom::Current(len as i64))?;
            }
        }
        r.set_position(end as u64);
        let required_color_channels: &[i16] = match color_mode {
            1 => &[0],
            4 => &[0, 1, 2, 3],
            _ => &[0, 1, 2],
        };
        if !matches!(section, Some(1..=3))
            && required_color_channels
                .iter()
                .any(|channel_id| !channels.iter().any(|channel| channel.id == *channel_id))
        {
            // Adjustment and fill layers commonly have no RGB pixel planes.
            // Keep the reliable merged composite instead of importing a blank layer.
            has_unsupported_features = true;
            if !unsupported_reasons.contains(&"画素チャンネルのないレイヤー") {
                unsupported_reasons.push("画素チャンネルのないレイヤー");
            }
        }
        records.push(Record {
            top,
            left,
            bottom,
            right,
            channels,
            name,
            opacity,
            visible,
            clipping,
            blend,
            has_mask,
            mask_bounds,
            mask_default,
            section,
        });
    }
    let mut layers: Vec<Layer> = Vec::with_capacity(count);
    let mut group_stack: Vec<u64> = Vec::new();
    let mut decoded_channel_bytes = 0u64;
    let mut cmyk_scratch_bytes = 0usize;
    for record in records {
        if record.section == Some(3) {
            group_stack.pop();
            continue;
        }
        let id = layers.iter().map(|l| l.id).max().unwrap_or(0) + 1;
        let mut layer = Layer::new(id, record.name, width, height);
        layer.parent_id = group_stack.last().copied();
        if matches!(record.section, Some(1 | 2)) {
            layer.kind = LayerKind::Folder;
            layer.expanded = record.section == Some(1);
        }
        layer.opacity = record.opacity as f32 / 255.;
        layer.visible = record.visible;
        layer.clipping = record.clipping;
        layer.blend = record.blend;
        if record.has_mask {
            layer.mask = Some(efude_canvas::TilePixels::new(width, height));
            if record.mask_default != 255 {
                for y in (0..height).step_by(efude_canvas::TILE_SIZE as usize) {
                    for x in (0..width).step_by(efude_canvas::TILE_SIZE as usize) {
                        layer.mask.as_mut().unwrap().ensure_tile_filled(
                            x,
                            y,
                            [record.mask_default; 4],
                        );
                    }
                }
            }
        }
        let lw = (record.right as i64 - record.left as i64).max(0) as usize;
        let lh = (record.bottom as i64 - record.top as i64).max(0) as usize;
        if lw > width as usize * 2
            || lh > height as usize * 2
            || lw.saturating_mul(lh) > 100_000_000
        {
            return Err("PSD layer dimensions exceed safety limit".into());
        }
        let mut cmyk_black = if color_mode == 4 && layer.kind == LayerKind::Raster {
            let scratch_len = lw.checked_mul(lh).ok_or("PSD CMYK layer size overflow")?;
            cmyk_scratch_bytes = cmyk_scratch_bytes
                .checked_add(scratch_len)
                .ok_or("PSD CMYK scratch size overflow")?;
            if cmyk_scratch_bytes > 512 * 1024 * 1024 {
                return Err("PSD CMYK layers exceed memory safety limit".into());
            }
            Some(vec![0; scratch_len])
        } else {
            None
        };
        let mask_rect =
            record
                .mask_bounds
                .unwrap_or((record.top, record.left, record.bottom, record.right));
        let mask_width = (mask_rect.3 as i64 - mask_rect.1 as i64).max(0) as usize;
        let mask_height = (mask_rect.2 as i64 - mask_rect.0 as i64).max(0) as usize;
        if mask_width > width as usize * 2
            || mask_height > height as usize * 2
            || mask_width.saturating_mul(mask_height) > 100_000_000
        {
            return Err("PSD layer mask dimensions exceed safety limit".into());
        }
        let has_alpha = record.channels.iter().any(|c| c.id == -1);
        if !has_alpha {
            for y in 0..lh {
                let dy = record.top + y as i32;
                if dy < 0 || dy >= height as i32 {
                    continue;
                }
                for x in 0..lw {
                    let dx = record.left + x as i32;
                    if dx >= 0 && dx < width as i32 {
                        let i = ((dy as u32 * width + dx as u32) * 4 + 3) as usize;
                        layer.pixels[i] = 255;
                    }
                }
            }
        }
        for channel in record.channels {
            let (channel_left, channel_top, channel_width, channel_height) = if channel.id == -2 {
                (mask_rect.1, mask_rect.0, mask_width, mask_height)
            } else {
                (record.left, record.top, lw, lh)
            };
            let decoded_len = channel_width
                .checked_mul(channel_height)
                .ok_or("PSD channel dimensions overflow")?;
            let sample_bytes = (depth / 8) as usize;
            let decoded_bytes = decoded_len
                .checked_mul(sample_bytes)
                .ok_or("PSD channel byte size overflow")?;
            let row_bytes = channel_width
                .checked_mul(sample_bytes)
                .ok_or("PSD channel row size overflow")?;
            let encoded_limit = decoded_bytes
                .checked_mul(2)
                .and_then(|n| n.checked_add(channel_height.saturating_mul(2)))
                .and_then(|n| n.checked_add(2))
                .ok_or("PSD channel size overflow")?;
            if channel.len > encoded_limit {
                return Err("PSD channel is larger than its layer dimensions allow".into());
            }
            decoded_channel_bytes = decoded_channel_bytes
                .checked_add(decoded_bytes as u64)
                .ok_or("PSD decoded channel size overflow")?;
            if decoded_channel_bytes > 512 * 1024 * 1024 {
                return Err("PSD decoded layers exceed the 512 MiB safety limit".into());
            }
            if channel.len < 2 {
                return Err("PSD layer channel is missing its compression field".into());
            }
            let compression = be_u16(&mut r)?;
            let mut copy_row = |y: usize, row: &[u8]| {
                let dy = channel_top as i64 + y as i64;
                if dy < 0 || dy >= height as i64 {
                    return;
                }
                for x in 0..channel_width {
                    let dx = channel_left as i64 + x as i64;
                    if dx < 0 || dx >= width as i64 {
                        continue;
                    }
                    let dst = ((dy as u32 * width + dx as u32) * 4) as usize;
                    let value = if sample_bytes == 1 {
                        row[x]
                    } else {
                        let sample = u16::from_be_bytes([row[x * 2], row[x * 2 + 1]]) as u32;
                        ((sample * 255 + 32767) / 65535) as u8
                    };
                    match channel.id {
                        0 => {
                            layer.pixels[dst] = value;
                            if color_mode == 1 {
                                layer.pixels[dst + 1] = value;
                                layer.pixels[dst + 2] = value;
                            }
                        }
                        1 => layer.pixels[dst + 1] = value,
                        2 => layer.pixels[dst + 2] = value,
                        3 if color_mode == 4 => {
                            if let Some(black) = &mut cmyk_black {
                                black[y * channel_width + x] = value;
                            }
                        }
                        -1 => layer.pixels[dst + 3] = value,
                        -2 => {
                            if let Some(mask) = &mut layer.mask {
                                mask.set_pixel(dx as u32, dy as u32, [value; 4]);
                            }
                        }
                        _ => {}
                    }
                }
            };
            match compression {
                0 => {
                    if channel.len < 2 + decoded_bytes {
                        return Err("PSD raw layer channel is shorter than its dimensions".into());
                    }
                    let mut row = vec![0; row_bytes];
                    for y in 0..channel_height {
                        r.read_exact(&mut row)?;
                        copy_row(y, &row);
                    }
                    r.seek(SeekFrom::Current((channel.len - 2 - decoded_bytes) as i64))?;
                }
                1 => {
                    let table_bytes = channel_height
                        .checked_mul(2)
                        .ok_or("PSD row table size overflow")?;
                    if channel.len < 2 + table_bytes {
                        return Err("PSD PackBits row table is truncated".into());
                    }
                    let mut lengths = vec![0u16; channel_height];
                    for length in &mut lengths {
                        *length = be_u16(&mut r)?;
                    }
                    let mut consumed = 2 + table_bytes;
                    for (y, length) in lengths.into_iter().enumerate() {
                        let length = length as usize;
                        consumed = consumed
                            .checked_add(length)
                            .ok_or("PSD PackBits channel size overflow")?;
                        if consumed > channel.len {
                            return Err("PSD PackBits rows exceed their channel size".into());
                        }
                        let mut encoded = vec![0; length];
                        r.read_exact(&mut encoded)?;
                        let row = decode_packbits(&encoded, row_bytes)?;
                        copy_row(y, &row);
                    }
                    r.seek(SeekFrom::Current((channel.len - consumed) as i64))?;
                }
                _ => {
                    has_unsupported_features = true;
                    if !unsupported_reasons.contains(&"圧縮方式") {
                        unsupported_reasons.push("圧縮方式");
                    }
                    r.seek(SeekFrom::Current((channel.len - 2) as i64))?;
                }
            }
        }
        if let Some(black) = cmyk_black {
            for y in 0..lh {
                let dy = record.top as i64 + y as i64;
                if dy < 0 || dy >= height as i64 {
                    continue;
                }
                for x in 0..lw {
                    let dx = record.left as i64 + x as i64;
                    if dx < 0 || dx >= width as i64 {
                        continue;
                    }
                    let index = ((dy as u32 * width + dx as u32) * 4) as usize;
                    let (c, m, yellow, k) = (
                        layer.pixels[index] as u32,
                        layer.pixels[index + 1] as u32,
                        layer.pixels[index + 2] as u32,
                        black[y * lw + x] as u32,
                    );
                    layer.pixels[index] = (((255 - c) * (255 - k) + 127) / 255) as u8;
                    layer.pixels[index + 1] = (((255 - m) * (255 - k) + 127) / 255) as u8;
                    layer.pixels[index + 2] = (((255 - yellow) * (255 - k) + 127) / 255) as u8;
                }
            }
        }
        if layer.kind == LayerKind::Folder {
            group_stack.push(layer.id);
        }
        layers.push(layer);
    }
    if layers.len() > 200 {
        return Err("PSD contains more than 200 editable layers".into());
    }
    if layers.is_empty() {
        Ok((
            None,
            has_unsupported_features,
            has_text_layers,
            unsupported_reasons,
        ))
    } else {
        layers.reverse();
        Ok((
            Some(layers),
            has_unsupported_features,
            has_text_layers,
            unsupported_reasons,
        ))
    }
}
fn decode_packbits(input: &[u8], expected: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while i < input.len() && out.len() < expected {
        let n = input[i] as i8;
        i += 1;
        match n {
            0..=127 => {
                let count = n as usize + 1;
                if i + count > input.len() {
                    return Err("invalid PSD PackBits literal run".into());
                }
                out.extend_from_slice(&input[i..i + count]);
                i += count;
            }
            -127..=-1 => {
                let count = 1 + (-(n as i16)) as usize;
                let value = *input.get(i).ok_or("invalid PSD PackBits repeat run")?;
                i += 1;
                out.extend(std::iter::repeat_n(value, count));
            }
            -128 => {}
        }
    }
    if out.len() != expected {
        return Err("PSD PackBits row has an unexpected length".into());
    }
    Ok(out)
}
pub fn save_backup(
    path: &Path,
    doc: &Document,
    generations: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let dir = parent.join(".efude-backups");
    fs::create_dir_all(&dir)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let backup = dir.join(format!("{stem}.{stamp}.efude"));
    save(&backup, doc)?;
    let mut entries = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_ok_and(|kind| kind.is_file())
                && e.file_name().to_str().is_some_and(|name| {
                    name.starts_with(&format!("{stem}.")) && name.ends_with(".efude")
                })
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| e.file_name());
    let excess = entries.len().saturating_sub(generations.max(1));
    for entry in entries.into_iter().take(excess) {
        let _ = fs::remove_file(entry.path());
    }
    Ok(())
}
pub fn backup_interval(minutes: u32) -> Duration {
    Duration::from_secs(minutes.clamp(1, 120) as u64 * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> Document {
        let mut document = Document::new(4, 3);
        document.layers[0]
            .pixels
            .set_pixel(1, 1, [220, 48, 76, 255]);
        document.layers[0]
            .pixels
            .set_pixel(2, 1, [18, 90, 240, 127]);
        document
    }

    #[test]
    fn efude_round_trip_preserves_canvas_and_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.efude");
        let original = sample_document();
        save(&path, &original).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!((loaded.width, loaded.height), (4, 3));
        assert_eq!(loaded.layers[0].pixels.pixel(1, 1), [220, 48, 76, 255]);
        assert_eq!(loaded.layers[0].pixels.pixel(2, 1), [18, 90, 240, 127]);
    }

    #[test]
    fn psd_keeps_japanese_layer_names() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("names.psd");
        let mut document = sample_document();
        document.layers[0].name = "線画 レイヤー1".into();
        export_psd(&path, &document).unwrap();
        let loaded = import_psd(&path).unwrap();
        assert_eq!(loaded.layers[0].name, "線画 レイヤー1");
    }

    #[test]
    fn psd_round_trip_preserves_canvas_and_layer_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("round-trip.psd");
        let original = sample_document();
        export_psd(&path, &original).unwrap();
        let loaded = import_psd(&path).unwrap();
        assert_eq!((loaded.width, loaded.height), (4, 3));
        assert_eq!(loaded.layers[0].pixels.pixel(1, 1), [220, 48, 76, 255]);
        assert_eq!(loaded.layers[0].pixels.pixel(2, 1), [18, 90, 240, 127]);
    }

    #[test]
    fn tone_layers_and_metadata_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.efude");
        let mut doc = Document::new(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                doc.layers[0].pixels.set_pixel(x, y, [0, 0, 0, 77]);
            }
        }
        doc.layers[0].tone = Some(efude_canvas::ToneSettings {
            lines_per_inch: 85.0,
            shape: efude_canvas::DotShape::Diamond,
            ..Default::default()
        });
        doc.metadata.insert("comic".into(), "{\"page\":1}".into());
        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].tone, doc.layers[0].tone);
        assert_eq!(loaded.metadata, doc.metadata);
        // PSD gets the dots, not the grey density.
        let psd = dir.path().join("tone.psd");
        export_psd(&psd, &doc).unwrap();
        let imported = import_psd(&psd).unwrap();
        let values: std::collections::HashSet<u8> = (0..48)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .map(|(x, y)| imported.layers[0].pixels.pixel(x, y)[3])
            .collect();
        assert!(
            values.contains(&0) && values.contains(&255),
            "tone was not baked: {values:?}"
        );
    }

    #[test]
    fn vector_strokes_round_trip_and_folders_load_in_panel_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vector.efude");
        let mut doc = Document::new(64, 48);
        let stroke = efude_canvas::VectorStroke {
            points: vec![
                efude_canvas::VectorPoint {
                    x: 4.0,
                    y: 10.5,
                    width: 3.0,
                },
                efude_canvas::VectorPoint {
                    x: 60.0,
                    y: 10.5,
                    width: 1.5,
                },
            ],
            color: [10, 20, 30, 200],
            hardness: 0.8,
            anchors: Vec::new(),
        };
        // Saved with its curve.
        let stroke = efude_canvas::VectorStroke::fitted(stroke.points, stroke.color, 0.8);
        assert_eq!(stroke.anchors.len(), 2);
        doc.layers[0].vector = Some(vec![stroke.clone()]);
        efude_canvas::vector::render_all(&mut doc.layers[0], 64, 48, None);
        let mut folder = efude_canvas::Layer::new(2, "folder", 64, 48);
        folder.kind = LayerKind::Folder;
        doc.layers.push(folder);
        let mut child = efude_canvas::Layer::new(3, "child", 64, 48);
        child.parent_id = Some(2);
        doc.layers.push(child);
        save(&path, &doc).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.layers[0].vector, Some(vec![stroke]));
        assert_eq!(
            loaded.layers[0].pixels.to_dense(),
            doc.layers[0].pixels.to_dense()
        );
        let ids = loaded.layers.iter().map(|l| l.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 3, 2]);
    }
}
