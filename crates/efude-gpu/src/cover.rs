// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! GPU painting of opacity-capped strokes.
//!
//! `GpuCover` implements [`CoverAccelerator`]: it paints a batch of dabs the
//! same way as `efude_brush::engine::StrokeRaster::stamp` does on the CPU
//! (same coverage formulas, same f16 stroke state, same compositing), one
//! invocation per pixel. `tests` compare the two pixel by pixel.

use efude_brush::engine::{COVER_TILE, CoverAccelerator, CoverBatch, CoverTileResult};

const COVER_SHADER: &str = r#"
struct Params {
    dab_count: u32,
    tile_count: u32,
    eraser: u32,
    pencil: u32,
    grain_strength: f32,
    grain_scale: f32,
    grain_fixed: u32,
    grain_origin_x: f32,
    grain_origin_y: f32,
    tip_width: u32,
    tip_height: u32,
    tip_offset: u32,
    grain_width: u32,
    grain_height: u32,
    grain_offset: u32,
    tile_offset: u32,
    cell_offset: u32,
    grain_rotation: f32,
    _padding1: u32,
    _padding2: u32,
    _padding3: u32,
};

const TILE: u32 = 64u;
const DAB_FLOATS: u32 = 16u;
const TILE_WORDS: u32 = 8u;

// Two storage buffers only, so the pipeline also runs on devices with the
// downlevel limit of four storage buffers per stage.
@group(0) @binding(0) var<uniform> params: Params;
// Read-only words, at the offsets in `params`:
//   dabs (16 f32 bits each: cx cy radius thin_alpha aspect sin cos edge reach
//   alpha r g b), tile headers (origin_x origin_y width height first count
//   _ _) followed by the dab index lists, cells (packed RGBA8 base pixel,
//   selection 0..255), then tip and grain coverage (one texel per word).
@group(0) @binding(1) var<storage, read> input: array<u32>;
// Per cell, 6 words: stroke state r g b a (f32 bits), packed RGBA8 result,
// and 1 when a dab reached the pixel.
@group(0) @binding(2) var<storage, read_write> io: array<u32>;

fn dab_value(d: u32, field: u32) -> f32 {
    return bitcast<f32>(input[d + field]);
}

fn f16_round(value: f32) -> f32 {
    return unpack2x16float(pack2x16float(vec2<f32>(value, 0.0))).x;
}

fn round_half_up(value: f32) -> f32 {
    return floor(value + 0.5);
}

fn round_half_up3(value: vec3<f32>) -> vec3<f32> {
    return floor(value + vec3<f32>(0.5));
}

fn texel(offset: u32, width: u32, x: u32, y: u32) -> f32 {
    return f32(input[offset + y * width + x]) / 255.0;
}

fn tip_bilinear(u_in: f32, v_in: f32) -> f32 {
    let width = params.tip_width;
    let height = params.tip_height;
    let max_u = f32(width - 1u);
    let max_v = f32(height - 1u);
    if (u_in < -0.5 || u_in > max_u + 0.5 || v_in < -0.5 || v_in > max_v + 0.5) {
        return 0.0;
    }
    let u = clamp(u_in, 0.0, max_u);
    let v = clamp(v_in, 0.0, max_v);
    let x0 = u32(floor(u));
    let y0 = u32(floor(v));
    let x1 = min(x0 + 1u, width - 1u);
    let y1 = min(y0 + 1u, height - 1u);
    let fx = u - f32(x0);
    let fy = v - f32(y0);
    let offset = params.tip_offset;
    let top = texel(offset, width, x0, y0) * (1.0 - fx) + texel(offset, width, x1, y0) * fx;
    let bottom = texel(offset, width, x0, y1) * (1.0 - fx) + texel(offset, width, x1, y1) * fx;
    return top * (1.0 - fy) + bottom * fy;
}

fn rem_euclid(value: i32, modulus: i32) -> i32 {
    return ((value % modulus) + modulus) % modulus;
}

fn grain_noise(x: u32, y: u32, for_grain: bool, center: vec2<f32>) -> f32 {
    let scale = max(params.grain_scale, 0.05);
    var gx = f32(x);
    var gy = f32(y);
    if (params.grain_fixed == 0u) {
        gx = gx - center.x;
        gy = gy - center.y;
    } else if (params.grain_rotation != 0.0) {
        let dx = gx - params.grain_origin_x;
        let dy = gy - params.grain_origin_y;
        let c = cos(params.grain_rotation);
        let s = sin(params.grain_rotation);
        gx = params.grain_origin_x + c * dx - s * dy;
        gy = params.grain_origin_y + s * dx + c * dy;
    }
    if (params.grain_width > 0u) {
        let w = i32(params.grain_width);
        let h = i32(params.grain_height);
        let tx = rem_euclid(i32(floor(gx * scale)), w);
        let ty = rem_euclid(i32(floor(gy * scale)), h);
        return texel(params.grain_offset, params.grain_width, u32(tx), u32(ty));
    }
    var ix = i32(x);
    var iy = i32(y);
    if (for_grain) {
        ix = i32(floor(gx * scale));
        iy = i32(floor(gy * scale));
    }
    let mixed = (ix * 374761393) ^ (iy * 668265263) ^ 0x51ed270b;
    let bits = bitcast<u32>(mixed * 1274126177);
    return f32(bits) / 4294967295.0;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let tile = id.z;
    if (tile >= params.tile_count) { return; }
    let header = params.tile_offset + tile * TILE_WORDS;
    let origin_x = input[header];
    let origin_y = input[header + 1u];
    let width = input[header + 2u];
    let height = input[header + 3u];
    let first = input[header + 4u];
    let count = input[header + 5u];
    let lx = id.x;
    let ly = id.y;
    if (lx >= width || ly >= height) { return; }
    let cell = tile * TILE * TILE + ly * TILE + lx;
    let x = origin_x + lx;
    let y = origin_y + ly;
    let io_base = cell * 6u;
    var st = vec4<f32>(
        bitcast<f32>(io[io_base]),
        bitcast<f32>(io[io_base + 1u]),
        bitcast<f32>(io[io_base + 2u]),
        bitcast<f32>(io[io_base + 3u]),
    );
    var touched = 0u;
    let cell_word = params.cell_offset + cell * 2u;
    let selection = f32(input[cell_word + 1u]) / 255.0;
    for (var k = 0u; k < count; k = k + 1u) {
        let d = input[first + k] * DAB_FLOATS;
        let cx = dab_value(d, 0u);
        let cy = dab_value(d, 1u);
        let radius = dab_value(d, 2u);
        let thin_alpha = dab_value(d, 3u);
        let aspect = dab_value(d, 4u);
        let s = dab_value(d, 5u);
        let c = dab_value(d, 6u);
        let edge = dab_value(d, 7u);
        let reach = dab_value(d, 8u);
        let alpha = dab_value(d, 9u);
        let dx = f32(x) + 0.5 - cx;
        let dy = f32(y) + 0.5 - cy;
        let local_x = dx * c + dy * s;
        let local_y = -dx * s + dy * c;
        let distance2 = local_x * local_x / (aspect * aspect) + local_y * local_y * (aspect * aspect);
        if (distance2 > reach * reach) { continue; }
        if (selection <= 0.0) { continue; }
        var coverage = clamp((radius - sqrt(distance2)) / edge + 0.5, 0.0, 1.0);
        if (coverage <= 0.0) { continue; }
        touched = 1u;
        coverage = coverage * thin_alpha;
        if (params.tip_width > 0u && params.tip_height > 0u) {
            let u = (local_x / (radius * aspect) * 0.5 + 0.5) * f32(params.tip_width - 1u);
            let v = (local_y * aspect / radius * 0.5 + 0.5) * f32(params.tip_height - 1u);
            coverage = coverage * tip_bilinear(u, v);
        }
        if (params.grain_strength > 0.0) {
            let noise = grain_noise(x, y, true, vec2<f32>(cx, cy));
            coverage = coverage * (1.0 - clamp(params.grain_strength, 0.0, 1.0) * (1.0 - noise));
        }
        var grain = 1.0;
        if (params.pencil != 0u) {
            grain = 0.38 + 0.62 * grain_noise(x, y, false, vec2<f32>(cx, cy));
        }
        let value = clamp(alpha * coverage * grain * selection, 0.0, 1.0);
        if (params.eraser != 0u) {
            if (value > st.w) {
                st.w = f16_round(value);
            }
        } else if (value >= st.w) {
            st.w = f16_round(value);
            st.x = f16_round(dab_value(d, 10u));
            st.y = f16_round(dab_value(d, 11u));
            st.z = f16_round(dab_value(d, 12u));
        }
    }
    io[io_base] = bitcast<u32>(st.x);
    io[io_base + 1u] = bitcast<u32>(st.y);
    io[io_base + 2u] = bitcast<u32>(st.z);
    io[io_base + 3u] = bitcast<u32>(st.w);
    // Exact integer channels, as on the CPU (no unorm round trip).
    let packed = input[cell_word];
    let base_i = vec4<u32>(packed & 255u, (packed >> 8u) & 255u, (packed >> 16u) & 255u, (packed >> 24u) & 255u);
    let base = vec4<f32>(base_i) / 255.0;
    var out = vec4<u32>(0u);
    if (params.eraser != 0u) {
        let a = u32(clamp(round_half_up(f32(base_i.w) * (1.0 - st.w)), 0.0, 255.0));
        if (a > 0u) {
            out = vec4<u32>(base_i.xyz, a);
        }
    } else {
        let a = st.w;
        let out_alpha = a + base.w * (1.0 - a);
        if (out_alpha > 0.0) {
            let rgb = (st.xyz * a + base.xyz * base.w * (1.0 - a)) / out_alpha;
            out = vec4<u32>(vec3<u32>(clamp(round_half_up3(rgb * 255.0), vec3<f32>(0.0), vec3<f32>(255.0))), 0u);
        }
        out.w = u32(clamp(round_half_up(out_alpha * 255.0), 0.0, 255.0));
    }
    io[io_base + 4u] = out.x | (out.y << 8u) | (out.z << 16u) | (out.w << 24u);
    io[io_base + 5u] = touched;
}
"#;

/// Batches with more blocks than this are left to the CPU.
const MAX_TILES: usize = 4096;

pub struct GpuCover {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    batches: std::sync::atomic::AtomicU64,
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl GpuCover {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("efude-cover-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(96),
                    },
                    count: None,
                },
                storage_entry(1, true),
                storage_entry(2, false),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("efude-cover-shader"),
            source: wgpu::ShaderSource::Wgsl(COVER_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("efude-cover-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("efude-cover-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("{error:?}"));
        }
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            pipeline,
            batches: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Number of batches painted on the GPU so far.
    pub fn batches_painted(&self) -> u64 {
        self.batches.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Creates a device of its own (headless use and tests).
    pub fn with_new_device() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok_or("no GPU adapter")?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("efude-cover-device"),
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: Default::default(),
            },
            None,
        ))
        .map_err(|error| error.to_string())?;
        Self::new(&device, &queue)
    }

    fn run(&self, batch: &CoverBatch) -> Result<Vec<CoverTileResult>, String> {
        use wgpu::util::DeviceExt;
        let tile_count = batch.tiles.len();
        if tile_count == 0 {
            return Ok(Vec::new());
        }
        if tile_count > MAX_TILES {
            return Err("batch too large".into());
        }
        let cells_per_tile = (COVER_TILE * COVER_TILE) as usize;
        if batch.tiles.iter().any(|tile| {
            tile.base.len() != cells_per_tile
                || tile.state.len() != cells_per_tile
                || tile.selection.len() != cells_per_tile
                || tile.width > COVER_TILE
                || tile.height > COVER_TILE
                || tile.dabs.iter().any(|&d| d as usize >= batch.dabs.len())
        }) {
            return Err("invalid cover tile".into());
        }
        // Read-only words: dabs, tiles, cells, textures.
        let mut input: Vec<u32> =
            Vec::with_capacity(batch.dabs.len() * 16 + tile_count * (8 + cells_per_tile * 2));
        for dab in &batch.dabs {
            input.extend(
                [
                    dab.center[0],
                    dab.center[1],
                    dab.radius,
                    dab.thin_alpha,
                    dab.aspect,
                    dab.sin,
                    dab.cos,
                    dab.edge,
                    dab.reach,
                    dab.alpha,
                    dab.color[0],
                    dab.color[1],
                    dab.color[2],
                    0.0,
                    0.0,
                    0.0,
                ]
                .map(f32::to_bits),
            );
        }
        let tile_offset = input.len();
        input.resize(tile_offset + tile_count * 8, 0);
        for (index, tile) in batch.tiles.iter().enumerate() {
            let first = input.len() as u32;
            input.extend_from_slice(&tile.dabs);
            let header = tile_offset + index * 8;
            input[header..header + 6].copy_from_slice(&[
                tile.origin[0],
                tile.origin[1],
                tile.width,
                tile.height,
                first,
                tile.dabs.len() as u32,
            ]);
        }
        let cell_offset = input.len();
        for tile in &batch.tiles {
            for (base, selection) in tile.base.iter().zip(&tile.selection) {
                input.push(u32::from_le_bytes(*base));
                input.push(*selection as u32);
            }
        }
        let mut texture = |t: &Option<efude_brush::engine::CoverTexture>| match t {
            Some(t)
                if t.width > 0
                    && t.height > 0
                    && t.coverage.len() == (t.width * t.height) as usize =>
            {
                let offset = input.len() as u32;
                input.extend(t.coverage.iter().map(|&value| value as u32));
                (t.width, t.height, offset)
            }
            _ => (0, 0, 0),
        };
        let (tip_width, tip_height, tip_offset) = texture(&batch.tip);
        let (grain_width, grain_height, grain_offset) = texture(&batch.grain_texture);
        let io_words = tile_count * cells_per_tile * 6;
        let mut io = vec![0u32; io_words];
        for (tile_index, tile) in batch.tiles.iter().enumerate() {
            for (cell, texel) in tile.state.iter().enumerate() {
                let base = (tile_index * cells_per_tile + cell) * 6;
                io[base..base + 4].copy_from_slice(&texel.map(f32::to_bits));
            }
        }
        let limit = self.device.limits().max_storage_buffer_binding_size as usize;
        if input.len() * 4 > limit || io_words * 4 > limit {
            return Err("batch exceeds the storage buffer limit".into());
        }
        let mut params = [0u32; 24];
        params[0] = batch.dabs.len() as u32;
        params[1] = tile_count as u32;
        params[2] = batch.eraser as u32;
        params[3] = batch.pencil as u32;
        params[4] = batch.grain_strength.to_bits();
        params[5] = batch.grain_scale.to_bits();
        params[6] = batch.grain_fixed as u32;
        params[7] = batch.grain_origin[0].to_bits();
        params[8] = batch.grain_origin[1].to_bits();
        params[9] = tip_width;
        params[10] = tip_height;
        params[11] = tip_offset;
        params[12] = grain_width;
        params[13] = grain_height;
        params[14] = grain_offset;
        params[15] = tile_offset as u32;
        params[16] = cell_offset as u32;
        params[17] = batch.grain_rotation.to_bits();

        let bytes = |words: &[u32]| {
            words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("efude-cover-params"),
                contents: &bytes(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let input_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("efude-cover-input"),
                contents: &bytes(&input),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let io_size = (io_words * 4) as u64;
        let io_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("efude-cover-io"),
                contents: &bytes(&io),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("efude-cover-readback"),
            size: io_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("efude-cover-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: io_buffer.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("efude-cover"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("efude-cover-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = COVER_TILE / 8;
            pass.dispatch_workgroups(groups, groups, tile_count as u32);
        }
        encoder.copy_buffer_to_buffer(&io_buffer, 0, &readback, 0, io_size);
        self.queue.submit(Some(encoder.finish()));
        let slice = readback.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let mapped = slice.get_mapped_range();
        let word = |index: usize| {
            let offset = index * 4;
            u32::from_le_bytes([
                mapped[offset],
                mapped[offset + 1],
                mapped[offset + 2],
                mapped[offset + 3],
            ])
        };
        let mut results = Vec::with_capacity(tile_count);
        for tile in 0..tile_count {
            let mut state = Vec::with_capacity(cells_per_tile);
            let mut touched = Vec::with_capacity(cells_per_tile);
            let mut out = Vec::with_capacity(cells_per_tile);
            for cell in 0..cells_per_tile {
                let base = (tile * cells_per_tile + cell) * 6;
                state.push([0, 1, 2, 3].map(|i| f32::from_bits(word(base + i))));
                out.push(word(base + 4).to_le_bytes());
                touched.push(word(base + 5) != 0);
            }
            results.push(CoverTileResult {
                state,
                touched,
                out,
            });
        }
        drop(mapped);
        readback.unmap();
        Ok(results)
    }
}

impl CoverAccelerator for GpuCover {
    fn paint_cover(&self, batch: &CoverBatch) -> Option<Vec<CoverTileResult>> {
        let results = self.run(batch).ok()?;
        self.batches
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use efude_brush::engine::{DabStyle, DabTarget, Dynamics, StrokeRaster, dynamics};
    use efude_brush::{Brush, BrushKind, BrushTip};
    use efude_canvas::{Document, History};
    use efude_core::InkPoint;
    use glam::Vec2;

    fn gpu() -> Option<GpuCover> {
        match GpuCover::with_new_device() {
            Ok(gpu) => Some(gpu),
            // Only a missing adapter skips; any other error is a bug.
            Err(error) if error == "no GPU adapter" => {
                eprintln!("skipping GPU comparison: {error}");
                None
            }
            Err(error) => panic!("GPU cover pipeline failed: {error}"),
        }
    }

    fn background(width: u32, height: u32) -> Document {
        let mut doc = Document::new(width, height);
        for y in 0..height {
            for x in 0..width {
                if (x / 7 + y / 5) % 3 != 0 {
                    doc.layers[0].pixels.set_pixel(
                        x,
                        y,
                        [(x * 3) as u8, (y * 5) as u8, 120, (100 + x) as u8],
                    );
                }
            }
        }
        doc
    }

    fn wavy(n: usize, width: f32, height: f32, seed: f32) -> Vec<InkPoint> {
        (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let mut p = InkPoint::new(
                    8.0 + t * (width - 16.0),
                    height * 0.5 + (t * 9.0 + seed).sin() * height * 0.3,
                    0.2 + 0.8 * ((i * 7 % 23) as f32 / 22.0),
                    i as u64 * 3,
                );
                p.tilt = Vec2::new((t * 5.0).sin() * 0.6, 0.3);
                p.rotation = t * 2.0;
                p
            })
            .collect()
    }

    /// Paints `points` in `batch` sized chunks, on the CPU or the GPU.
    fn paint(
        doc: &mut Document,
        brush: &Brush,
        eraser: bool,
        size: f32,
        points: &[InkPoint],
        selection: Option<&[u8]>,
        accelerator: Option<&dyn CoverAccelerator>,
    ) {
        let mut history = History::default();
        history.begin();
        let mut raster = StrokeRaster::new([0.1, 0.2, 0.3], brush.mix.charge, Vec2::new(3.0, 4.0));
        let style = DabStyle {
            brush,
            kind: brush.kind,
            eraser,
            color: [30, 140, 220, 230],
            size,
        };
        for chunk in points.chunks(9) {
            let stamps: Vec<(InkPoint, Dynamics)> = chunk
                .iter()
                .map(|p| {
                    let d = dynamics(brush, p, raster.last_dab, 1.0);
                    raster.finish_dab(style, *p);
                    (*p, d)
                })
                .collect();
            let mut target = DabTarget {
                doc,
                layer: 0,
                selection,
                history: &mut history,
            };
            raster.stamp_many(&mut target, style, &stamps, accelerator, 0.0);
        }
        history.commit();
    }

    /// The per-dab CPU path against the batched CPU cover and (when there
    /// is an adapter) the GPU.
    fn compare(brush: Brush, eraser: bool, size: f32, with_selection: bool) {
        let (width, height) = (150, 97);
        let points = wavy(70, width as f32, height as f32, size);
        let selection: Vec<u8> = (0..width * height)
            .map(|i| ((i % width) * 255 / width) as u8)
            .collect();
        let selection = with_selection.then_some(selection.as_slice());
        let mut cpu_doc = background(width, height);
        paint(&mut cpu_doc, &brush, eraser, size, &points, selection, None);
        let cpu = cpu_doc.layers[0].pixels.to_dense();
        assert!(
            cpu != background(width, height).layers[0].pixels.to_dense(),
            "nothing painted"
        );
        let gpu = gpu();
        let mut accelerators: Vec<(&str, &dyn CoverAccelerator)> =
            vec![("cpu cover", &efude_brush::engine::CpuCover)];
        if let Some(gpu) = &gpu {
            accelerators.push(("gpu", gpu));
        }
        for (name, accelerator) in accelerators {
            let mut doc = background(width, height);
            paint(
                &mut doc,
                &brush,
                eraser,
                size,
                &points,
                selection,
                Some(accelerator),
            );
            let other = doc.layers[0].pixels.to_dense();
            let mut worst = 0;
            let mut differing = 0;
            for (a, b) in cpu.iter().zip(&other) {
                let d = (*a as i32 - *b as i32).abs();
                worst = worst.max(d);
                if d > 0 {
                    differing += 1;
                }
            }
            assert!(
                worst <= 1,
                "{name} {:?} eraser={eraser}: max difference {worst}, {differing} channels differ",
                brush.kind
            );
        }
    }

    fn brush(index: usize) -> Brush {
        efude_brush::defaults().remove(index)
    }

    #[test]
    fn pen_matches_cpu() {
        compare(brush(0), false, 14.0, false);
    }

    #[test]
    fn thin_pen_matches_cpu() {
        compare(brush(0), false, 1.2, false);
    }

    #[test]
    fn pencil_with_grain_and_selection_matches_cpu() {
        let mut pencil = brush(1);
        pencil.grain = 0.6;
        pencil.grain_scale = 0.7;
        compare(pencil, false, 11.0, true);
    }

    #[test]
    fn brush_with_bitmap_tip_and_tilt_matches_cpu() {
        let mut b = brush(2);
        b.tilt_flattening = 0.6;
        b.tilt_rotation = 1.0;
        b.tip_aspect = 1.7;
        b.hardness = 0.4;
        b.tip = Some(BrushTip {
            width: 9,
            height: 7,
            coverage: (0..63).map(|i| ((i * 37) % 256) as u8).collect(),
        });
        b.grain_tip = Some(BrushTip {
            width: 5,
            height: 4,
            coverage: (0..20).map(|i| (i * 13) as u8).collect(),
        });
        b.grain = 0.5;
        compare(b, false, 25.0, false);
    }

    #[test]
    fn eraser_matches_cpu() {
        let mut eraser = brush(7);
        eraser.opacity = 0.7;
        compare(eraser, true, 18.0, true);
    }

    #[test]
    fn non_mixing_watercolor_matches_cpu() {
        let mut water = brush(3);
        water.mix.blend = 0.0;
        water.mix.persistence = 0.0;
        compare(water, false, 30.0, false);
    }

    #[test]
    fn mixing_brushes_stay_on_the_cpu() {
        let water = brush(3);
        let style = DabStyle {
            brush: &water,
            kind: BrushKind::Watercolor,
            eraser: false,
            color: [0; 4],
            size: 10.0,
        };
        assert!(!efude_brush::engine::accelerator_can_paint(style));
    }
}
