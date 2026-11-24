use std::collections::HashMap;

use aion_ast::{NodeId, NodeKind, Program, UiArg, UiBody, UiNode};
use bytemuck::{Pod, Zeroable};
use cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};

use crate::layout::LayoutTree;
use crate::signals::SignalStore;

/// WGSL shader rendering rounded rectangles with alpha anti-aliasing.
pub const ROUNDED_RECT_WGSL: &str = r#"
struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct RectInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radius: f32,
};

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) radius: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, inst: RectInstance) -> VertexOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let uv = corners[vid];
    let px = inst.pos + uv * inst.size;
    let ndc_x = (px.x / uniforms.viewport.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (px.y / uniforms.viewport.y) * 2.0;

    var out: VertexOut;
    out.clip_pos = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.local_pos = uv * inst.size;
    out.size = inst.size;
    out.color = inst.color;
    out.radius = inst.radius;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let half = in.size * 0.5;
    let p = abs(in.local_pos - half) - (half - vec2<f32>(in.radius, in.radius));
    let dist = length(max(p, vec2<f32>(0.0, 0.0))) + min(max(p.x, p.y), 0.0) - in.radius;
    let alpha = clamp(0.5 - dist, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
"#;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub radius: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ViewportUniform {
    pub viewport: [f32; 2],
    pub _pad: [f32; 2],
}

/// Draw primitive emitted from an AION [`LayoutTree`] + [`SignalStore`].
#[derive(Debug, Clone, PartialEq)]
pub enum RenderPrimitive {
    RoundedRect {
        node_id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        color: [f32; 4],
    },
    Text {
        node_id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        content: String,
        font_size: f32,
        color: [u8; 4],
    },
}

/// Converts a hex color (`#RRGGBB`, `#RRGGBBAA`, `#RGB`) into normalized RGBA `[0.0..1.0]`.
pub fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
    let raw = hex.trim().strip_prefix('#')?;
    match raw.len() {
        6 => {
            let r = u8::from_str_radix(&raw[0..2], 16).ok()?;
            let g = u8::from_str_radix(&raw[2..4], 16).ok()?;
            let b = u8::from_str_radix(&raw[4..6], 16).ok()?;
            Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
        }
        8 => {
            let r = u8::from_str_radix(&raw[0..2], 16).ok()?;
            let g = u8::from_str_radix(&raw[2..4], 16).ok()?;
            let b = u8::from_str_radix(&raw[4..6], 16).ok()?;
            let a = u8::from_str_radix(&raw[6..8], 16).ok()?;
            Some([
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ])
        }
        3 => {
            let r = u8::from_str_radix(&raw[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&raw[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&raw[2..3], 16).ok()? * 17;
            Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
        }
        _ => None,
    }
}

/// Builds an ordered display list of rounded-rect and text primitives for GPU rendering.
pub fn build_display_list(
    program: &Program,
    layout: &LayoutTree,
    signals: &SignalStore,
) -> Vec<RenderPrimitive> {
    let ui_map: HashMap<&NodeId, &UiNode> = program
        .nodes
        .iter()
        .filter_map(|n| match &n.kind {
            NodeKind::Ui(ui) => Some((&n.id, ui)),
            _ => None,
        })
        .collect();

    let mut list = Vec::new();
    for id in &layout.paint_order {
        let (Some(ui), Some(lbox)) = (ui_map.get(id), layout.get(id)) else {
            continue;
        };

        match ui.widget.as_str() {
            "ROOT" => {
                if let Some(color) = ui
                    .args
                    .get(4)
                    .and_then(|a| match a {
                        UiArg::HexColor(c) => parse_hex_color(c),
                        _ => None,
                    })
                {
                    list.push(RenderPrimitive::RoundedRect {
                        node_id: id.clone(),
                        x: lbox.x,
                        y: lbox.y,
                        width: lbox.width,
                        height: lbox.height,
                        radius: 0.0,
                        color,
                    });
                }
            }
            "BTN" => {
                let radius = match ui.args.get(2) {
                    Some(UiArg::Int(r)) => *r as f32,
                    Some(UiArg::Float(r)) => *r as f32,
                    _ => 0.0,
                };
                let color = ui
                    .args
                    .get(3)
                    .and_then(|a| match a {
                        UiArg::HexColor(c) => parse_hex_color(c),
                        _ => None,
                    })
                    .unwrap_or([0.15, 0.39, 0.92, 1.0]);
                list.push(RenderPrimitive::RoundedRect {
                    node_id: id.clone(),
                    x: lbox.x,
                    y: lbox.y,
                    width: lbox.width,
                    height: lbox.height,
                    radius,
                    color,
                });
            }
            "TXT" => {
                let font_size = match ui.args.first() {
                    Some(UiArg::Int(s)) => *s as f32,
                    Some(UiArg::Float(s)) => *s as f32,
                    _ => 16.0,
                };
                let rgba_f = ui
                    .args
                    .get(2)
                    .and_then(|a| match a {
                        UiArg::HexColor(c) => parse_hex_color(c),
                        _ => None,
                    })
                    .unwrap_or([0.0, 0.0, 0.0, 1.0]);
                let color = [
                    (rgba_f[0] * 255.0) as u8,
                    (rgba_f[1] * 255.0) as u8,
                    (rgba_f[2] * 255.0) as u8,
                    (rgba_f[3] * 255.0) as u8,
                ];
                let content = match &ui.body {
                    UiBody::Text { template, .. } => signals.resolve_text(template),
                    _ => String::new(),
                };
                list.push(RenderPrimitive::Text {
                    node_id: id.clone(),
                    x: lbox.x,
                    y: lbox.y,
                    width: lbox.width,
                    height: lbox.height,
                    content,
                    font_size,
                    color,
                });
            }
            _ => {}
        }
    }
    list
}

/// Text shaping engine backed by `cosmic-text` (compatible with native desktop and `wasm32`).
pub struct TextShaper {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl TextShaper {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Shapes `content` at `font_size` and returns the number of laid-out glyph runs.
    pub fn shape_text(&mut self, content: &str, font_size: f32, max_width: f32, rgba: [u8; 4]) -> usize {
        let metrics = Metrics::new(font_size, font_size * 1.25);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(&mut self.font_system, Some(max_width.max(1.0)), None);
        let attrs = Attrs::new().color(Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]));
        buffer.set_text(&mut self.font_system, content, attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut glyph_count = 0usize;
        for run in buffer.layout_runs() {
            glyph_count += run.glyphs.len();
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                let _ = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical.cache_key);
            }
        }
        glyph_count
    }
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

/// Core `wgpu` rounded-rectangle pipeline (shared across native desktop and `wasm32-unknown-unknown`).
pub struct WgpuRectPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

impl WgpuRectPipeline {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aion_rounded_rect_shader"),
            source: wgpu::ShaderSource::Wgsl(ROUNDED_RECT_WGSL.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("aion_viewport_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("aion_rect_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RectInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 8,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("aion_rounded_rect_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[instance_layout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group_layout,
        }
    }
}

/// Converts a display list into GPU [`RectInstance`]s ready for instanced `wgpu` rendering.
pub fn extract_rect_instances(primitives: &[RenderPrimitive]) -> Vec<RectInstance> {
    primitives
        .iter()
        .filter_map(|p| match p {
            RenderPrimitive::RoundedRect {
                x,
                y,
                width,
                height,
                radius,
                color,
                ..
            } => Some(RectInstance {
                pos: [*x, *y],
                size: [*width, *height],
                color: *color,
                radius: *radius,
            }),
            RenderPrimitive::Text { .. } => None,
        })
        .collect()
}

/// Native desktop window launcher (`winit` + `wgpu`), gated out on `wasm32-unknown-unknown`.
#[cfg(not(target_arch = "wasm32"))]
pub fn create_desktop_wgpu_instance() -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor::default())
}

/// Async WebGPU/WebGL adapter request entrypoint prepared for `wasm32-unknown-unknown`.
pub async fn request_wgpu_device(
    instance: &wgpu::Instance,
    compatible_surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Device, wgpu::Queue), String> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface,
            force_fallback_adapter: false,
        })
        .await
        .ok_or_else(|| "no compatible wgpu adapter found".to_string())?;

    adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .map_err(|e| format!("failed to request wgpu device: {e}"))
}
