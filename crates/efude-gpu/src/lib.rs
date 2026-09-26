// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Hakoniwa
//! WGPU dab compositing primitives. Canvas storage remains owned by the caller
//! so the UI can migrate from CPU rasterization incrementally.

mod cover;
pub use cover::GpuCover;

/// Builds one mip level from the level above it: each pixel is the mean of
/// the 2×2 pixels it covers, averaged in linear light.
const MIP_SHADER: &str = r#"
struct MipParams {
    origin: vec2<u32>,
    extent: vec2<u32>,
}
@group(0) @binding(0) var mip_source: texture_2d<f32>;
@group(0) @binding(1) var mip_target: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: MipParams;

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}
fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, c <= vec3<f32>(0.0031308));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.extent.x || id.y >= params.extent.y) { return; }
    let p = params.origin + id.xy;
    let target_size = textureDimensions(mip_target);
    if (p.x >= target_size.x || p.y >= target_size.y) { return; }
    let last = textureDimensions(mip_source, 0) - vec2<u32>(1u);
    var sum = vec4<f32>(0.0);
    for (var dy = 0u; dy < 2u; dy += 1u) {
        for (var dx = 0u; dx < 2u; dx += 1u) {
            let c = textureLoad(mip_source, min(p * 2u + vec2<u32>(dx, dy), last), 0);
            sum += vec4<f32>(to_linear(c.rgb), c.a);
        }
    }
    let mean = sum * 0.25;
    textureStore(mip_target, p, vec4<f32>(to_srgb(mean.rgb), mean.a));
}
"#;

const COMPOSITE_SHADER: &str = r#"
struct LayerInfo {
    opacity: f32,
    blend_mode: u32,
    flags: u32,
    _padding: u32,
}
struct CompositeParams {
    origin: vec2<u32>,
    extent: vec2<u32>,
    // Square size of the transparency checkerboard (0: white backdrop).
    checker: u32,
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}
@group(0) @binding(0) var<storage, read> layer_pixels: array<u32>;
@group(0) @binding(1) var<storage, read> layer_coverage: array<f32>;
@group(0) @binding(2) var<storage, read> layer_info: array<LayerInfo>;
@group(0) @binding(3) var output_texture: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(4) var<uniform> params: CompositeParams;
// The grey squares of the checkerboard (`efude_canvas::CHECKER_GREY`).
const CHECKER_GREY: f32 = 214.0;

fn to_linear(c: f32) -> f32 {
    if (c <= 0.04045) { return c / 12.92; }
    return pow((c + 0.055) / 1.055, 2.4);
}
fn to_srgb(c: f32) -> f32 {
    let v = clamp(c, 0.0, 1.0);
    if (v <= 0.0031308) { return v * 12.92; }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}
fn blend(mode: u32, s: f32, d: f32) -> f32 {
    if (mode == 1u) { return s * d; }
    if (mode == 2u) { return 1.0 - (1.0 - s) * (1.0 - d); }
    if (mode == 3u) {
        if (d < 0.5) { return 2.0 * s * d; }
        return 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
    }
    if (mode == 4u) { return min(s, d); }
    if (mode == 5u) { return max(s, d); }
    if (mode == 6u) {
        if (s >= 1.0) { return 1.0; }
        return min(1.0, d / (1.0 - s));
    }
    if (mode == 7u) {
        if (s <= 0.0) { return 0.0; }
        return 1.0 - min(1.0, (1.0 - d) / s);
    }
    if (mode == 8u) {
        if (s < 0.5) { return 2.0 * s * d; }
        return 1.0 - 2.0 * (1.0 - s) * (1.0 - d);
    }
    if (mode == 9u) {
        if (s <= 0.5) { return d - (1.0 - 2.0 * s) * d * (1.0 - d); }
        var g = sqrt(max(d, 0.0));
        if (d <= 0.25) { g = ((16.0 * d - 12.0) * d + 4.0) * d; }
        return d + (2.0 * s - 1.0) * (g - d);
    }
    if (mode == 10u) { return abs(d - s); }
    if (mode == 11u) { return s + d - 2.0 * s * d; }
    if (mode == 12u) { return min(s + d, 1.0); }
    if (mode == 13u) { return max(d - s, 0.0); }
    return s;
}
/// One colour channel of a layer over what is below, rounded to 8 bits.
fn mix_channel(mode: u32, source: f32, destination: f32, alpha: f32, linear: bool) -> f32 {
    var s = source;
    var d = destination;
    if (linear) {
        s = to_linear(s);
        d = to_linear(d);
    }
    let mixed = blend(mode, s, d);
    let result = mixed * alpha + d * (1.0 - alpha);
    let encoded = select(result, to_srgb(result), linear);
    return floor(encoded * 255.0 + 0.5) / 255.0;
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_texture);
    if (id.x >= params.extent.x || id.y >= params.extent.y) { return; }
    let output_coord = params.origin + id.xy;
    if (output_coord.x >= dims.x || output_coord.y >= dims.y) { return; }
    let pixel_index = id.y * 256u + id.x;
    var destination = vec3<f32>(1.0);
    if (params.checker > 0u) {
        let square = output_coord / vec2<u32>(params.checker);
        if (((square.x + square.y) & 1u) == 1u) {
            destination = vec3<f32>(CHECKER_GREY / 255.0);
        }
    }
    var base_alpha = 0.0;
    for (var i = 0u; i < arrayLength(&layer_info); i += 1u) {
        let info = layer_info[i];
        let packed = layer_pixels[i * 65536u + pixel_index];
        let source = vec3<f32>(
            f32(packed & 255u),
            f32((packed >> 8u) & 255u),
            f32((packed >> 16u) & 255u)
        ) / 255.0;
        let source_alpha = f32((packed >> 24u) & 255u) / 255.0;
        var alpha = source_alpha * info.opacity * layer_coverage[i * 65536u + pixel_index];
        let clipping = (info.flags & 1u) != 0u;
        let linear = (info.flags & 2u) != 0u;
        if (clipping) { alpha *= base_alpha; }
        if (alpha <= 0.0) { continue; }
        // One call per channel: indexing a vector as an l-value in a loop
        // makes the D3D shader compiler try to unroll the layer loop.
        destination = vec3<f32>(
            mix_channel(info.blend_mode, source.x, destination.x, alpha, linear),
            mix_channel(info.blend_mode, source.y, destination.y, alpha, linear),
            mix_channel(info.blend_mode, source.z, destination.z, alpha, linear)
        );
        if (!clipping) { base_alpha = alpha + base_alpha * (1.0 - alpha); }
    }
    textureStore(output_texture, vec2<i32>(output_coord), vec4<f32>(destination, 1.0));
}
"#;

#[derive(Clone)]
pub struct GpuCompositeLayer {
    /// Complete 256×256 RGBA8 tile, in the order the document displays layers.
    pub pixels: Vec<u8>,
    /// Per-pixel mask coverage after applying the layer and ancestor masks.
    pub coverage: Vec<f32>,
    pub opacity: f32,
    /// Blend-mode index in the `BlendMode` declaration order.
    pub blend_mode: u32,
    pub linear_blend: bool,
    pub clipping: bool,
}

pub struct GpuCompositeTileInput {
    pub layers: Vec<GpuCompositeLayer>,
    pub width: u32,
    pub height: u32,
    pub origin: [u32; 2],
    /// Square size of the transparency checkerboard behind the picture, in
    /// document pixels (0: white).
    pub checker: u32,
}

/// GPU layer compositing for the canvas display. (Stroke painting on the
/// GPU is `GpuCover`.)
pub struct GpuDabPipeline {
    composite_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::ComputePipeline,
    mip_layout: wgpu::BindGroupLayout,
    mip_pipeline: wgpu::ComputePipeline,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuDabPipeline {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("efude-composite-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
            ],
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("efude-composite-shader"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_SHADER.into()),
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("efude-composite-pipeline-layout"),
                bind_group_layouts: &[&composite_layout],
                push_constant_ranges: &[],
            });
        let composite_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("efude-composite-pipeline"),
            layout: Some(&composite_pipeline_layout),
            module: &composite_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("efude-mip-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let mip_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("efude-mip-shader"),
            source: wgpu::ShaderSource::Wgsl(MIP_SHADER.into()),
        });
        let mip_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("efude-mip-pipeline-layout"),
            bind_group_layouts: &[&mip_layout],
            push_constant_ranges: &[],
        });
        let mip_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("efude-mip-pipeline"),
            layout: Some(&mip_pipeline_layout),
            module: &mip_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let pipeline = Self {
            composite_layout,
            composite_pipeline,
            mip_layout,
            mip_pipeline,
            device: device.clone(),
            queue: queue.clone(),
        };
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(error.to_string());
        }
        Ok(pipeline)
    }

    /// Rebuilds the mip levels of `texture` (an `Rgba8Unorm` texture with
    /// storage and texture binding) under the level-0 rectangles `rects`
    /// (`[x0, y0, x1, y1)`), so the canvas stays smooth when zoomed out.
    pub fn update_mips(&self, texture: &wgpu::Texture, rects: &[[u32; 4]]) -> Result<(), String> {
        use wgpu::util::DeviceExt;
        let levels = texture.mip_level_count();
        if levels < 2 || rects.is_empty() {
            return Ok(());
        }
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("efude-mip-encoder"),
            });
        let view = |level: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(wgpu::TextureFormat::Rgba8Unorm),
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut keep_alive = Vec::new();
        for level in 1..levels {
            let (source, target) = (view(level - 1), view(level));
            let mut groups = Vec::new();
            for rect in rects {
                let x0 = rect[0] >> level;
                let y0 = rect[1] >> level;
                let x1 = rect[2].div_ceil(1 << level);
                let y1 = rect[3].div_ceil(1 << level);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let mut params = Vec::with_capacity(16);
                for value in [x0, y0, x1 - x0, y1 - y0] {
                    params.extend_from_slice(&value.to_le_bytes());
                }
                let buffer = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("efude-mip-params"),
                        contents: &params,
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("efude-mip-bind-group"),
                    layout: &self.mip_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&target),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buffer.as_entire_binding(),
                        },
                    ],
                });
                groups.push((group, x1 - x0, y1 - y0));
                keep_alive.push(buffer);
            }
            // One pass per level: the next level reads what this one wrote.
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("efude-mip-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.mip_pipeline);
            for (group, w, h) in &groups {
                pass.set_bind_group(0, group, &[]);
                pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(error) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(error.to_string());
        }
        Ok(())
    }

    /// Composite one changed document tile on the GPU. Input layers are already
    /// cropped to tile storage and carry inherited mask coverage from the caller.
    pub fn composite_tile_into(
        &self,
        layers: &[GpuCompositeLayer],
        width: u32,
        height: u32,
        output_view: &wgpu::TextureView,
        origin: [u32; 2],
        checker: u32,
    ) -> Result<(), String> {
        self.composite_tiles_into(
            &[GpuCompositeTileInput {
                layers: layers.to_vec(),
                width,
                height,
                origin,
                checker,
            }],
            output_view,
        )
    }

    /// Composite multiple changed tiles in one queue submission.
    pub fn composite_tiles_into(
        &self,
        tiles: &[GpuCompositeTileInput],
        output_view: &wgpu::TextureView,
    ) -> Result<(), String> {
        use wgpu::util::DeviceExt;
        const SIZE: u32 = 256;
        const PIXELS: usize = (SIZE * SIZE) as usize;
        const TILE_BYTES: u64 = (SIZE * SIZE * 4) as u64;
        if tiles.is_empty() {
            return Ok(());
        }
        for tile in tiles {
            if tile.width == 0
                || tile.height == 0
                || tile.width > SIZE
                || tile.height > SIZE
                || tile.layers.len() > 200
            {
                return Err("invalid GPU composite tile dimensions or layer count".into());
            }
            if tile.layers.iter().any(|layer| {
                layer.pixels.len() != PIXELS * 4
                    || layer.coverage.len() != PIXELS
                    || !layer.opacity.is_finite()
                    || !(0.0..=1.0).contains(&layer.opacity)
                    || layer.blend_mode > 13
                    || layer
                        .coverage
                        .iter()
                        .any(|coverage| !coverage.is_finite() || !(0.0..=1.0).contains(coverage))
            }) {
                return Err("invalid GPU composite layer data".into());
            }
        }
        let limits = self.device.limits();
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("efude-composite-encoder"),
            });
        let mut bind_groups = Vec::with_capacity(tiles.len());
        let mut _keep_alive = Vec::with_capacity(tiles.len() * 4);
        for tile in tiles {
            let input_layer_count = tile.layers.len().max(1) as u64;
            let layer_bytes = input_layer_count
                .checked_mul(TILE_BYTES)
                .ok_or("GPU composite input size overflow")?;
            let coverage_bytes = input_layer_count
                .checked_mul(PIXELS as u64 * 4)
                .ok_or("GPU composite coverage size overflow")?;
            if layer_bytes > limits.max_storage_buffer_binding_size as u64
                || coverage_bytes > limits.max_storage_buffer_binding_size as u64
            {
                let _ = pollster::block_on(self.device.pop_error_scope());
                return Err("GPU composite layer stack exceeds adapter limits".into());
            }
            let mut pixel_bytes = Vec::with_capacity(layer_bytes as usize);
            let mut coverage_bytes_data = Vec::with_capacity(coverage_bytes as usize);
            let mut metadata = Vec::with_capacity(tile.layers.len().max(1) * 16);
            if tile.layers.is_empty() {
                pixel_bytes.resize(TILE_BYTES as usize, 0);
                coverage_bytes_data.resize(coverage_bytes as usize, 0);
                metadata.extend_from_slice(&0.0f32.to_le_bytes());
                metadata.extend_from_slice(&0u32.to_le_bytes());
                metadata.extend_from_slice(&0u32.to_le_bytes());
                metadata.extend_from_slice(&0u32.to_le_bytes());
            }
            for layer in &tile.layers {
                pixel_bytes.extend_from_slice(&layer.pixels);
                for coverage in &layer.coverage {
                    coverage_bytes_data.extend_from_slice(&coverage.to_le_bytes());
                }
                metadata.extend_from_slice(&layer.opacity.to_le_bytes());
                metadata.extend_from_slice(&layer.blend_mode.to_le_bytes());
                let flags = u32::from(layer.clipping) | (u32::from(layer.linear_blend) << 1);
                metadata.extend_from_slice(&flags.to_le_bytes());
                metadata.extend_from_slice(&0u32.to_le_bytes());
            }
            let pixel_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("efude-composite-layer-pixels"),
                    contents: &pixel_bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });
            let coverage_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("efude-composite-layer-coverage"),
                        contents: &coverage_bytes_data,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
            let metadata_buffer =
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("efude-composite-layer-metadata"),
                        contents: &metadata,
                        usage: wgpu::BufferUsages::STORAGE,
                    });
            let mut params = Vec::with_capacity(16);
            params.extend_from_slice(&tile.origin[0].to_le_bytes());
            params.extend_from_slice(&tile.origin[1].to_le_bytes());
            params.extend_from_slice(&tile.width.to_le_bytes());
            params.extend_from_slice(&tile.height.to_le_bytes());
            params.extend_from_slice(&tile.checker.to_le_bytes());
            params.extend_from_slice(&[0; 12]);
            let params_buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("efude-composite-params"),
                    contents: &params,
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("efude-composite-bind-group"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: pixel_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: coverage_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: metadata_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(output_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: params_buffer.as_entire_binding(),
                    },
                ],
            });
            _keep_alive.extend([
                pixel_buffer,
                coverage_buffer,
                metadata_buffer,
                params_buffer,
            ]);
            bind_groups.push(bind_group);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("efude-composite-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.composite_pipeline);
            for (bind_group, tile) in bind_groups.iter().zip(tiles) {
                pass.set_bind_group(0, bind_group, &[]);
                pass.dispatch_workgroups(tile.width.div_ceil(8), tile.height.div_ceil(8), 1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        if let Some(error) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(error.to_string());
        }
        Ok(())
    }
}

pub const PRESENT_MODE_PREFERENCE: &str = "Mailbox";

#[cfg(test)]
mod mip_tests {
    use super::*;

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: Default::default(),
            },
            None,
        ))
        .ok()
    }

    /// The GPU display composites exactly like the CPU compositor.
    #[test]
    fn gpu_composite_matches_the_cpu_exactly() {
        use efude_canvas::{BlendMode, Document, Layer, composite_display};
        let Some((device, queue)) = device() else {
            return;
        };
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::Difference,
        ];
        let size = 64u32;
        let mut doc = Document::new(size, size);
        let mut next = 7u32;
        let mut random = move || {
            next ^= next << 13;
            next ^= next >> 17;
            next ^= next << 5;
            (next >> 8) as u8
        };
        for (i, mode) in modes.iter().enumerate() {
            let mut layer = Layer::new(i as u64 + 10, "l", size, size);
            layer.blend = *mode;
            layer.opacity = [1.0, 0.7, 0.45][i % 3];
            for y in 0..size {
                for x in 0..size {
                    layer
                        .pixels
                        .set_pixel(x, y, [random(), random(), random(), random()]);
                }
            }
            doc.layers.push(layer);
        }
        // Over the transparency checkerboard, which must line up too.
        let cpu = composite_display(&doc, 8);
        let layers: Vec<GpuCompositeLayer> = doc
            .layers
            .iter()
            .map(|layer| {
                let mut pixels = vec![0u8; 256 * 256 * 4];
                for y in 0..size {
                    for x in 0..size {
                        let i = ((y * 256 + x) * 4) as usize;
                        pixels[i..i + 4].copy_from_slice(&layer.pixels.pixel(x, y));
                    }
                }
                GpuCompositeLayer {
                    pixels,
                    coverage: vec![1.0; 256 * 256],
                    opacity: layer.opacity,
                    blend_mode: layer.blend as u32,
                    linear_blend: false,
                    clipping: false,
                }
            })
            .collect();
        let pipeline = GpuDabPipeline::new(&device, &queue).unwrap();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        pipeline
            .composite_tile_into(&layers, size, size, &view, [0, 0], 8)
            .unwrap();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (256 * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let gpu = buffer.slice(..).get_mapped_range();
        let mut worst = 0;
        for y in 0..size as usize {
            for x in 0..size as usize {
                for c in 0..3 {
                    let a = cpu[(y * size as usize + x) * 4 + c] as i32;
                    let b = gpu[y * 256 + x * 4 + c] as i32;
                    worst = worst.max((a - b).abs());
                }
            }
        }
        assert_eq!(worst, 0, "GPU and CPU composites differ by up to {worst}");
    }

    #[test]
    fn mips_average_in_linear_light() {
        let instance = wgpu::Instance::default();
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                memory_hints: Default::default(),
            },
            None,
        )) else {
            return;
        };
        let pipeline = GpuDabPipeline::new(&device, &queue).unwrap();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 3,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        // Black and white columns.
        let mut pixels = Vec::new();
        for _ in 0..64 {
            for x in 0..64 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                pixels.extend([v, v, v, 255]);
            }
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        pipeline.update_mips(&texture, &[[0, 0, 64, 64]]).unwrap();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 2,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let data = buffer.slice(..).get_mapped_range();
        // Half the light: sRGB ≈ 188, the same everywhere.
        for y in 0..16 {
            for x in 0..16 {
                let v = data[y * 256 + x * 4];
                assert!((186..=189).contains(&v), "{x},{y}: {v}");
            }
        }
    }
}
