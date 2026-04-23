use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use masonry::core::Widget as _;
use masonry::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use masonry::peniko::Color;
use masonry::theme::default_property_set;
use masonry::ui_events::pointer::{
    PointerButtonEvent, PointerEvent, PointerGestureEvent, PointerInfo, PointerScrollEvent,
    PointerUpdate,
};
use masonry::widgets::{Align, Button, ButtonPress};
use masonry_core::app::{
    RenderRoot, RenderRootOptions, RenderRootSignal, VisualLayerKind, WindowSizePolicy,
};
use masonry_core::core::{Ime, TextEvent, WindowEvent as MasonryWindowEvent};
use masonry_imaging::texture_render::{
    RenderTarget as ImagingRenderTarget, Renderer as UiRenderer,
};
use masonry_imaging::{Layer as ImagingLayer, PreparedFrame};
use ui_events_winit::{WindowEventReducer, WindowEventTranslation};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

const UI_LOGICAL_WIDTH: f64 = 300.0;
const UI_LOGICAL_HEIGHT: f64 = 100.0;
const UI_TRANSPARENT: Color = Color::from_rgba8(0, 0, 0, 0);
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

const UI_COMPOSITE_SHADER: &str = r#"
struct QuadParams {
    origin: vec2<f32>,
    size: vec2<f32>,
    surface_size: vec2<f32>,
    _padding: vec2<f32>,
}

@group(0) @binding(0)
var ui_texture: texture_2d<f32>;

@group(0) @binding(1)
var ui_sampler: sampler;

@group(0) @binding(2)
var<uniform> quad: QuadParams;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );

    let uv = corners[vertex_index];
    let pixel = quad.origin + uv * quad.size;
    let ndc = vec2<f32>(
        (pixel.x / quad.surface_size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / quad.surface_size.y) * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(ui_texture, ui_sampler, in.uv);
}
"#;

const SCENE_SHADER: &str = r#"
struct SceneParams {
    angle: f32,
    aspect: f32,
    palette_mix: f32,
    _padding: f32,
}

@group(0) @binding(0)
var<uniform> scene: SceneParams;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

fn rotate_y(p: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
}

fn rotate_x(p: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(p.x, p.y * c - p.z * s, p.y * s + p.z * c);
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    let positions = array<vec3<f32>, 8>(
        vec3<f32>(-1.0, -1.0,  1.0),
        vec3<f32>( 1.0, -1.0,  1.0),
        vec3<f32>( 1.0,  1.0,  1.0),
        vec3<f32>(-1.0,  1.0,  1.0),
        vec3<f32>(-1.0, -1.0, -1.0),
        vec3<f32>( 1.0, -1.0, -1.0),
        vec3<f32>( 1.0,  1.0, -1.0),
        vec3<f32>(-1.0,  1.0, -1.0),
    );

    let indices = array<u32, 36>(
        0u, 1u, 2u, 0u, 2u, 3u,
        5u, 4u, 7u, 5u, 7u, 6u,
        3u, 2u, 6u, 3u, 6u, 7u,
        4u, 5u, 1u, 4u, 1u, 0u,
        1u, 5u, 6u, 1u, 6u, 2u,
        4u, 0u, 3u, 4u, 3u, 7u,
    );

    let normals = array<vec3<f32>, 6>(
        vec3<f32>( 0.0,  0.0,  1.0),
        vec3<f32>( 0.0,  0.0, -1.0),
        vec3<f32>( 0.0,  1.0,  0.0),
        vec3<f32>( 0.0, -1.0,  0.0),
        vec3<f32>( 1.0,  0.0,  0.0),
        vec3<f32>(-1.0,  0.0,  0.0),
    );

    let palette_a = array<vec3<f32>, 6>(
        vec3<f32>(0.16, 0.87, 0.71),
        vec3<f32>(0.08, 0.65, 0.55),
        vec3<f32>(0.40, 0.95, 0.82),
        vec3<f32>(0.12, 0.43, 0.36),
        vec3<f32>(0.64, 0.99, 0.89),
        vec3<f32>(0.21, 0.73, 0.61),
    );

    let palette_b = array<vec3<f32>, 6>(
        vec3<f32>(0.34, 0.64, 0.98),
        vec3<f32>(0.22, 0.47, 0.83),
        vec3<f32>(0.74, 0.84, 1.00),
        vec3<f32>(0.18, 0.25, 0.49),
        vec3<f32>(0.99, 0.71, 0.34),
        vec3<f32>(0.82, 0.44, 0.20),
    );

    let face = vertex_index / 6u;
    var position = positions[indices[vertex_index]];
    var normal = normals[face];

    position = rotate_y(position, scene.angle);
    position = rotate_x(position, scene.angle * 0.6);
    normal = rotate_y(normal, scene.angle);
    normal = rotate_x(normal, scene.angle * 0.6);

    position.z = position.z + 4.5;

    let perspective = 2.2 / position.z;
    let ndc = vec2<f32>(
        position.x * perspective / scene.aspect,
        position.y * perspective,
    );
    let depth = clamp((position.z - 2.0) / 6.0, 0.0, 1.0);

    let base_color =
        palette_a[face] * (1.0 - scene.palette_mix) + palette_b[face] * scene.palette_mix;
    let light_dir = normalize(vec3<f32>(0.35, 0.6, 1.0));
    let lit = 0.28 + 0.72 * max(dot(normalize(normal), light_dir), 0.0);
    let rim = 0.14 * max(1.0 - abs(normalize(normal).z), 0.0);

    var out: VertexOut;
    out.position = vec4<f32>(ndc, depth, 1.0);
    out.color = base_color * (lit + rim);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

#[derive(Clone, Copy)]
struct OverlayRect {
    origin: PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
}

impl OverlayRect {
    fn contains(self, position: PhysicalPosition<f64>) -> bool {
        position.x >= self.origin.x
            && position.y >= self.origin.y
            && position.x < self.origin.x + f64::from(self.size.width)
            && position.y < self.origin.y + f64::from(self.size.height)
    }
}

#[derive(Clone, Copy)]
struct QuadParams {
    origin: [f32; 2],
    size: [f32; 2],
    surface_size: [f32; 2],
    padding: [f32; 2],
}

impl QuadParams {
    fn into_bytes(self) -> [u8; 32] {
        let values = [
            self.origin[0],
            self.origin[1],
            self.size[0],
            self.size[1],
            self.surface_size[0],
            self.surface_size[1],
            self.padding[0],
            self.padding[1],
        ];
        let mut bytes = [0; 32];
        for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

#[derive(Clone, Copy)]
struct SceneParams {
    angle: f32,
    aspect: f32,
    palette_mix: f32,
    padding: f32,
}

impl SceneParams {
    fn into_bytes(self) -> [u8; 16] {
        let values = [self.angle, self.aspect, self.palette_mix, self.padding];
        let mut bytes = [0; 16];
        for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
            chunk.copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

struct DemoScene {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    angle: f32,
    palette_mix: f32,
}

impl DemoScene {
    fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        size: PhysicalSize<u32>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("embed scene shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SCENE_SHADER)),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("embed scene bind group layout"),
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
            label: Some("embed scene pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("embed scene pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("embed scene uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("embed scene bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let (depth_texture, depth_view) = create_depth_target(device, size);

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            depth_texture,
            depth_view,
            angle: 0.85,
            palette_mix: 0.0,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, size: PhysicalSize<u32>) {
        let (texture, view) = create_depth_target(device, size);
        self.depth_texture = texture;
        self.depth_view = view;
    }

    fn advance(&mut self) {
        self.angle += std::f32::consts::PI / 7.0;
        if self.angle > std::f32::consts::TAU {
            self.angle -= std::f32::consts::TAU;
        }
        self.palette_mix = 1.0 - self.palette_mix;
    }

    fn clear_color(&self) -> wgpu::Color {
        wgpu::Color::BLACK
    }

    fn render(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        size: PhysicalSize<u32>,
    ) {
        let aspect = f32::max(size.width as f32 / size.height.max(1) as f32, 0.001);
        let params = SceneParams {
            angle: self.angle,
            aspect,
            palette_mix: self.palette_mix,
            padding: 0.0,
        };
        queue.write_buffer(&self.uniform_buffer, 0, &params.into_bytes());

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("embed scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.clear_color()),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..36, 0..1);
    }
}

struct UiCompositor {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
}

impl UiCompositor {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("embed ui composite shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(UI_COMPOSITE_SHADER)),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("embed ui composite bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("embed ui composite pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("embed ui composite pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("embed ui composite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("embed ui composite uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
        }
    }

    fn composite(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        ui_texture_view: &wgpu::TextureView,
        overlay: OverlayRect,
        surface_size: PhysicalSize<u32>,
    ) {
        if surface_size.width == 0 || surface_size.height == 0 {
            return;
        }

        let params = QuadParams {
            origin: [overlay.origin.x as f32, overlay.origin.y as f32],
            size: [overlay.size.width as f32, overlay.size.height as f32],
            surface_size: [surface_size.width as f32, surface_size.height as f32],
            padding: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, &params.into_bytes());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("embed ui composite bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(ui_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("embed ui composite pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..6, 0..1);
    }
}

struct EmbeddedUi {
    render_root: RenderRoot,
    signal_queue: Rc<RefCell<VecDeque<RenderRootSignal>>>,
    event_reducer: WindowEventReducer,
    renderer: UiRenderer,
    target_texture: wgpu::Texture,
    target_view: wgpu::TextureView,
    logical_size: LogicalSize<f64>,
    physical_size: PhysicalSize<u32>,
    scale_factor: f64,
    hovered: bool,
    pointer_route_active: bool,
}

impl EmbeddedUi {
    fn new(device: &wgpu::Device, scale_factor: f64) -> Self {
        let logical_size = LogicalSize::new(UI_LOGICAL_WIDTH, UI_LOGICAL_HEIGHT);
        let physical_size = logical_size.to_physical(scale_factor);
        let signal_queue = Rc::new(RefCell::new(VecDeque::new()));
        let sink_queue = signal_queue.clone();
        let button = Align::centered(Button::with_text("Rotate cube").prepare());
        let render_root = RenderRoot::new(
            button.prepare(),
            move |signal| {
                sink_queue.borrow_mut().push_back(signal);
            },
            RenderRootOptions {
                default_properties: Arc::new(default_property_set()),
                use_system_fonts: true,
                size_policy: WindowSizePolicy::User,
                size: physical_size,
                scale_factor,
                test_font: None,
            },
        );
        let (target_texture, target_view) = create_render_target(device, physical_size);

        Self {
            render_root,
            signal_queue,
            event_reducer: WindowEventReducer::default(),
            renderer: UiRenderer::new(),
            target_texture,
            target_view,
            logical_size,
            physical_size,
            scale_factor,
            hovered: false,
            pointer_route_active: false,
        }
    }

    fn texture_view(&self) -> &wgpu::TextureView {
        &self.target_view
    }

    fn update_scale_factor(&mut self, scale_factor: f64, device: &wgpu::Device) {
        if (self.scale_factor - scale_factor).abs() < f64::EPSILON {
            return;
        }

        self.scale_factor = scale_factor;
        self.physical_size = self.logical_size.to_physical(scale_factor);
        let (texture, view) = create_render_target(device, self.physical_size);
        self.target_texture = texture;
        self.target_view = view;
        self.render_root
            .handle_window_event(MasonryWindowEvent::Rescale(scale_factor));
        self.render_root
            .handle_window_event(MasonryWindowEvent::Resize(self.physical_size));
        self.hovered = false;
        self.pointer_route_active = false;
    }

    fn overlay_rect(&self, window_size: PhysicalSize<u32>) -> OverlayRect {
        let x = (f64::from(window_size.width) - f64::from(self.physical_size.width)).max(0.0) * 0.5;
        let y =
            (f64::from(window_size.height) - f64::from(self.physical_size.height)).max(0.0) * 0.5;
        OverlayRect {
            origin: PhysicalPosition::new(x, y),
            size: self.physical_size,
        }
    }

    fn handle_window_event(&mut self, event: &WindowEvent, overlay: OverlayRect) {
        if !matches!(
            event,
            WindowEvent::KeyboardInput {
                is_synthetic: true,
                ..
            }
        ) && let Some(translated) = self.event_reducer.reduce(self.scale_factor, event)
        {
            match translated {
                WindowEventTranslation::Keyboard(keyboard) => {
                    self.render_root
                        .handle_text_event(TextEvent::Keyboard(keyboard));
                }
                WindowEventTranslation::Pointer(pointer) => {
                    self.route_pointer_event(pointer, overlay);
                }
            }
        }

        match event {
            WindowEvent::Ime(ime) => {
                self.render_root
                    .handle_text_event(TextEvent::Ime(winit_ime_to_masonry(ime.clone())));
            }
            WindowEvent::Focused(is_focused) => {
                self.render_root
                    .handle_text_event(TextEvent::WindowFocusChange(*is_focused));
            }
            _ => {}
        }
    }

    fn route_pointer_event(&mut self, event: PointerEvent, overlay: OverlayRect) {
        match event {
            PointerEvent::Enter(_) => {}
            PointerEvent::Leave(pointer) => {
                if self.pointer_route_active {
                    self.render_root
                        .handle_pointer_event(PointerEvent::Cancel(pointer));
                    self.pointer_route_active = false;
                } else if self.hovered {
                    self.render_root
                        .handle_pointer_event(PointerEvent::Leave(pointer));
                }
                self.hovered = false;
            }
            PointerEvent::Cancel(pointer) => {
                if self.pointer_route_active || self.hovered {
                    self.render_root
                        .handle_pointer_event(PointerEvent::Cancel(pointer));
                }
                self.pointer_route_active = false;
                self.hovered = false;
            }
            event => {
                let Some(pointer) = pointer_info(&event) else {
                    return;
                };
                let position = pointer_position(&event)
                    .expect("pointer events other than enter/leave/cancel carry positions");
                let inside = overlay.contains(position);

                if inside && !self.hovered {
                    self.render_root
                        .handle_pointer_event(PointerEvent::Enter(pointer));
                    self.hovered = true;
                } else if !inside && self.hovered {
                    self.render_root
                        .handle_pointer_event(PointerEvent::Leave(pointer));
                    self.hovered = false;
                }

                let captured = self.render_root.pointer_capture_target().is_some();
                let should_route = inside || self.pointer_route_active || captured;
                if !should_route {
                    return;
                }

                let translated = translate_pointer_event(event, overlay.origin);
                self.pointer_route_active = pointer_event_buttons_active(&translated) || captured;
                self.render_root.handle_pointer_event(translated);
                self.pointer_route_active = self.pointer_route_active
                    || self.render_root.pointer_capture_target().is_some();
            }
        }
    }

    fn render(&mut self, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue) {
        let (visual_layers, _) = self.render_root.redraw();
        let overlays: Vec<_> = visual_layers
            .overlay_layers()
            .map(|layer| {
                let VisualLayerKind::Scene(scene) = &layer.kind else {
                    unreachable!("overlay_layers only returns scene layers");
                };
                ImagingLayer {
                    scene,
                    transform: layer.transform,
                }
            })
            .collect();
        let root_layer = visual_layers
            .root_layer()
            .expect("paint should always produce a root layer");
        let VisualLayerKind::Scene(root_scene) = &root_layer.kind else {
            unreachable!("root_layer always returns a scene layer");
        };
        let frame = PreparedFrame::new(
            self.physical_size.width,
            self.physical_size.height,
            self.scale_factor,
            UI_TRANSPARENT,
            root_scene,
            &overlays,
        );
        self.renderer
            .render_to_texture(
                ImagingRenderTarget {
                    adapter,
                    device,
                    queue,
                    texture: &self.target_texture,
                    view: &self.target_view,
                },
                frame,
            )
            .expect("failed to render Masonry content");
    }

    fn process_signals(&mut self, window: &Window, scene: &mut DemoScene) -> bool {
        let mut should_exit = false;
        while let Some(signal) = self.signal_queue.borrow_mut().pop_front() {
            match signal {
                RenderRootSignal::Action(action, _) => {
                    if action.is::<ButtonPress>() {
                        scene.advance();
                        window.request_redraw();
                    }
                }
                RenderRootSignal::RequestRedraw | RenderRootSignal::RequestAnimFrame => {
                    window.request_redraw();
                }
                RenderRootSignal::SetCursor(cursor) => {
                    window.set_cursor(cursor);
                }
                RenderRootSignal::TakeFocus => {
                    window.focus_window();
                }
                RenderRootSignal::NewLayer(_, root, pos) => {
                    self.render_root.add_layer(root, pos);
                }
                RenderRootSignal::RemoveLayer(root_id) => {
                    self.render_root.remove_layer(root_id);
                }
                RenderRootSignal::RepositionLayer(root_id, new_origin) => {
                    self.render_root.reposition_layer(root_id, new_origin);
                }
                RenderRootSignal::Exit => {
                    should_exit = true;
                }
                RenderRootSignal::StartIme
                | RenderRootSignal::EndIme
                | RenderRootSignal::ImeMoved(_, _)
                | RenderRootSignal::ClipboardStore(_)
                | RenderRootSignal::SetSize(_)
                | RenderRootSignal::SetTitle(_)
                | RenderRootSignal::DragWindow
                | RenderRootSignal::DragResizeWindow(_)
                | RenderRootSignal::ToggleMaximized
                | RenderRootSignal::Minimize
                | RenderRootSignal::ShowWindowMenu(_)
                | RenderRootSignal::WidgetSelectedInInspector(_) => {
                    // These are host responsibilities. For this PoC we only handle what the
                    // embedded button demo needs.
                }
            }
        }
        should_exit
    }
}

struct State {
    window: Arc<Window>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    size: PhysicalSize<u32>,
    scale_factor: f64,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    scene: DemoScene,
    ui: EmbeddedUi,
    compositor: UiCompositor,
}

impl State {
    async fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .expect("failed to find a WGPU adapter");

        let supported_features = adapter.features();
        let requested_features = supported_features & wgpu::Features::CLEAR_TEXTURE;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: requested_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            })
            .await
            .expect("failed to create a WGPU device");

        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        let cap = surface.get_capabilities(&adapter);
        let surface_format = cap
            .formats
            .first()
            .copied()
            .expect("surface should expose at least one format");
        let scene = DemoScene::new(&device, surface_format.add_srgb_suffix(), size);
        let ui = EmbeddedUi::new(&device, scale_factor);
        let compositor = UiCompositor::new(&device, surface_format.add_srgb_suffix());

        let state = Self {
            window,
            adapter,
            device,
            queue,
            size,
            scale_factor,
            surface,
            surface_format,
            scene,
            ui,
            compositor,
        };

        if state.size.width > 0 && state.size.height > 0 {
            state.configure_surface();
        }

        state
    }

    fn window(&self) -> &Window {
        &self.window
    }

    fn configure_surface(&self) {
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.surface_format,
            view_formats: vec![self.surface_format.add_srgb_suffix()],
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            width: self.size.width,
            height: self.size.height,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::AutoVsync,
        };
        self.surface.configure(&self.device, &surface_config);
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.size = new_size;
        if self.size.width > 0 && self.size.height > 0 {
            self.configure_surface();
            self.scene.resize(&self.device, self.size);
        }
        self.window.request_redraw();
    }

    fn update_scale_factor(&mut self, scale_factor: f64) -> bool {
        self.scale_factor = scale_factor;
        self.ui.update_scale_factor(scale_factor, &self.device);
        self.ui.process_signals(&self.window, &mut self.scene)
    }

    fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        let overlay = self.ui.overlay_rect(self.size);
        self.ui.handle_window_event(event, overlay);
        self.ui.process_signals(&self.window, &mut self.scene)
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }

        self.ui.render(&self.adapter, &self.device, &self.queue);
        let should_exit = self.ui.process_signals(&self.window, &mut self.scene);
        if should_exit {
            return Ok(());
        }

        let surface_texture = self.surface.get_current_texture()?;
        let texture_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.surface_format.add_srgb_suffix()),
                ..Default::default()
            });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("embed frame encoder"),
            });

        self.scene
            .render(&self.queue, &mut encoder, &texture_view, self.size);

        self.compositor.composite(
            &self.device,
            &self.queue,
            &mut encoder,
            &texture_view,
            self.ui.texture_view(),
            self.ui.overlay_rect(self.size),
            self.size,
        );

        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        surface_texture.present();
        let _ = self.ui.process_signals(&self.window, &mut self.scene);
        Ok(())
    }
}

#[derive(Default)]
struct App {
    state: Option<State>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(window_attributes())
                .expect("failed to create a window"),
        );
        let mut state = pollster::block_on(State::new(window.clone()));
        let should_exit = state.ui.process_signals(&window, &mut state.scene);
        self.state = Some(state);
        window.request_redraw();
        if should_exit {
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let state = self
            .state
            .as_mut()
            .expect("state should exist after resume");
        let should_exit = match &event {
            WindowEvent::CloseRequested => true,
            WindowEvent::RedrawRequested => match state.render() {
                Ok(()) => false,
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    if state.size.width > 0 && state.size.height > 0 {
                        state.configure_surface();
                        state.window().request_redraw();
                    }
                    false
                }
                Err(wgpu::SurfaceError::OutOfMemory) => true,
                Err(wgpu::SurfaceError::Timeout) => false,
                Err(wgpu::SurfaceError::Other) => false,
            },
            WindowEvent::Resized(size) => {
                state.resize(*size);
                false
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.update_scale_factor(*scale_factor)
            }
            _ => state.handle_window_event(&event),
        };

        if should_exit {
            event_loop.exit();
        }
    }
}

fn main() {
    env_logger::init();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}

fn create_render_target(
    device: &wgpu::Device,
    size: PhysicalSize<u32>,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("embed ui texture"),
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_depth_target(
    device: &wgpu::Device,
    size: PhysicalSize<u32>,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("embed depth texture"),
        size: wgpu::Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn pointer_info(event: &PointerEvent) -> Option<PointerInfo> {
    match event {
        PointerEvent::Down(PointerButtonEvent { pointer, .. })
        | PointerEvent::Up(PointerButtonEvent { pointer, .. })
        | PointerEvent::Move(PointerUpdate { pointer, .. })
        | PointerEvent::Scroll(PointerScrollEvent { pointer, .. })
        | PointerEvent::Gesture(PointerGestureEvent { pointer, .. }) => Some(*pointer),
        PointerEvent::Enter(pointer)
        | PointerEvent::Leave(pointer)
        | PointerEvent::Cancel(pointer) => Some(*pointer),
    }
}

fn pointer_position(event: &PointerEvent) -> Option<PhysicalPosition<f64>> {
    match event {
        PointerEvent::Down(PointerButtonEvent { state, .. })
        | PointerEvent::Up(PointerButtonEvent { state, .. })
        | PointerEvent::Scroll(PointerScrollEvent { state, .. })
        | PointerEvent::Gesture(PointerGestureEvent { state, .. }) => Some(state.position),
        PointerEvent::Move(PointerUpdate { current, .. }) => Some(current.position),
        PointerEvent::Enter(_) | PointerEvent::Leave(_) | PointerEvent::Cancel(_) => None,
    }
}

fn pointer_event_buttons_active(event: &PointerEvent) -> bool {
    match event {
        PointerEvent::Down(PointerButtonEvent { state, .. })
        | PointerEvent::Up(PointerButtonEvent { state, .. })
        | PointerEvent::Scroll(PointerScrollEvent { state, .. })
        | PointerEvent::Gesture(PointerGestureEvent { state, .. }) => !state.buttons.is_empty(),
        PointerEvent::Move(PointerUpdate { current, .. }) => !current.buttons.is_empty(),
        PointerEvent::Enter(_) | PointerEvent::Leave(_) | PointerEvent::Cancel(_) => false,
    }
}

fn translate_pointer_event(event: PointerEvent, origin: PhysicalPosition<f64>) -> PointerEvent {
    match event {
        PointerEvent::Down(mut event) => {
            event.state.position = translate_position(event.state.position, origin);
            PointerEvent::Down(event)
        }
        PointerEvent::Up(mut event) => {
            event.state.position = translate_position(event.state.position, origin);
            PointerEvent::Up(event)
        }
        PointerEvent::Move(mut event) => {
            event.current.position = translate_position(event.current.position, origin);
            for coalesced in &mut event.coalesced {
                coalesced.position = translate_position(coalesced.position, origin);
            }
            for predicted in &mut event.predicted {
                predicted.position = translate_position(predicted.position, origin);
            }
            PointerEvent::Move(event)
        }
        PointerEvent::Scroll(mut event) => {
            event.state.position = translate_position(event.state.position, origin);
            PointerEvent::Scroll(event)
        }
        PointerEvent::Gesture(mut event) => {
            event.state.position = translate_position(event.state.position, origin);
            PointerEvent::Gesture(event)
        }
        PointerEvent::Cancel(pointer) => PointerEvent::Cancel(pointer),
        PointerEvent::Enter(pointer) => PointerEvent::Enter(pointer),
        PointerEvent::Leave(pointer) => PointerEvent::Leave(pointer),
    }
}

fn translate_position(
    position: PhysicalPosition<f64>,
    origin: PhysicalPosition<f64>,
) -> PhysicalPosition<f64> {
    PhysicalPosition::new(position.x - origin.x, position.y - origin.y)
}

fn window_attributes() -> WindowAttributes {
    Window::default_attributes()
        .with_title("Embedded Masonry in WGPU")
        .with_inner_size(LogicalSize::new(960.0, 640.0))
}

fn winit_ime_to_masonry(event: winit::event::Ime) -> Ime {
    match event {
        winit::event::Ime::Enabled => Ime::Enabled,
        winit::event::Ime::Disabled => Ime::Disabled,
        winit::event::Ime::Preedit(text, cursor) => Ime::Preedit(text, cursor),
        winit::event::Ime::Commit(text) => Ime::Commit(text),
    }
}
