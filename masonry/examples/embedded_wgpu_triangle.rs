// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

//! Headless embedding example.
//!
//! This example does not create a window and does not use `winit`.
//! A host-owned WGPU setup renders a simple triangle scene into one texture,
//! Masonry renders a button into a second transparent texture, and the host
//! composites the two together. A simulated click on the Masonry button then
//! toggles the triangle color and writes a second PNG.

use std::error::Error;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Wake, Waker};

use masonry::app::{
    RenderRoot, RenderRootOptions, RenderRootSignal, VisualLayerKind, WindowSizePolicy,
};
use masonry::core::{
    ErasedAction, NewWidget, PointerButton, PointerButtonEvent, PointerEvent, PointerId,
    PointerInfo, PointerState, PointerType, WidgetTag,
};
use masonry::dpi::{PhysicalPosition, PhysicalSize};
use masonry::kurbo::Point;
use masonry::peniko::{Blob, Color};
use masonry::theme::default_property_set;
use masonry::widgets::{Button, ButtonPress};
use masonry_imaging::texture_render::{RenderTarget, Renderer as MasonryRenderer};
use masonry_imaging::{Layer as ImagingLayer, PreparedFrame};
use masonry_testing::ROBOTO;
use wgpu::util::TextureBlitterBuilder;

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
const SCALE_FACTOR: f64 = 1.0;
const BUTTON_TAG: WidgetTag<Button> = WidgetTag::named("toggle-triangle-color");
const PRIMARY_MOUSE: PointerInfo = PointerInfo {
    pointer_id: Some(PointerId::PRIMARY),
    persistent_device_id: None,
    pointer_type: PointerType::Mouse,
};

const TRIANGLE_SHADER: &str = r#"
struct SceneUniforms {
    color: vec4f,
};

@group(0) @binding(0)
var<uniform> uniforms: SceneUniforms;

struct VertexOutput {
    @builtin(position) position: vec4f,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2f, 3>(
        vec2f(0.0, 0.78),
        vec2f(-0.72, -0.58),
        vec2f(0.72, -0.58),
    );

    var output: VertexOutput;
    output.position = vec4f(positions[vertex_index], 0.0, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4f {
    return uniforms.color;
}
"#;

fn main() -> Result<(), Box<dyn Error>> {
    let output_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/example-output/embedded_wgpu_triangle"));
    std::fs::create_dir_all(&output_dir)?;

    let mut example = EmbeddedTriangleExample::new(PhysicalSize::new(WIDTH, HEIGHT))?;

    let before_path = output_dir.join("before_click.png");
    example.render_to_png(&before_path)?;

    example.click_toggle_button();

    let after_path = output_dir.join("after_click.png");
    example.render_to_png(&after_path)?;

    println!("Wrote {}", before_path.display());
    println!("Wrote {}", after_path.display());

    Ok(())
}

struct EmbeddedTriangleExample {
    gpu: GpuState,
    scene_renderer: TriangleRenderer,
    masonry_renderer: MasonryRenderer,
    ui_blitter: wgpu::util::TextureBlitter,
    scene_target: OffscreenTexture,
    ui_target: OffscreenTexture,
    render_root: RenderRoot,
    host: HostState,
    size: PhysicalSize<u32>,
}

impl EmbeddedTriangleExample {
    fn new(size: PhysicalSize<u32>) -> Result<Self, Box<dyn Error>> {
        let gpu = GpuState::new()?;
        let scene_renderer = TriangleRenderer::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm);
        let masonry_renderer = MasonryRenderer::new();
        let ui_blitter = TextureBlitterBuilder::new(&gpu.device, wgpu::TextureFormat::Rgba8Unorm)
            .blend_state(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)
            .build();
        let scene_target = OffscreenTexture::new(
            &gpu.device,
            size,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let ui_target = OffscreenTexture::new(
            &gpu.device,
            size,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::STORAGE_BINDING,
        );
        let mut render_root = create_render_root(size);
        let mut host = HostState::default();
        host.redraw_requested = true;
        process_host_signals(&mut render_root, &mut host);

        Ok(Self {
            gpu,
            scene_renderer,
            masonry_renderer,
            ui_blitter,
            scene_target,
            ui_target,
            render_root,
            host,
            size,
        })
    }

    fn render_to_png(&mut self, path: &Path) -> Result<(), Box<dyn Error>> {
        self.scene_renderer.render(
            &self.gpu.device,
            &self.gpu.queue,
            &self.scene_target.view,
            self.host.triangle_color.as_rgba(),
        );
        self.render_masonry()?;
        self.composite_ui();

        let pixels = read_texture_rgba(
            &self.gpu.device,
            &self.gpu.queue,
            &self.scene_target.texture,
            self.size,
        )?;
        let image = image::RgbaImage::from_raw(self.size.width, self.size.height, pixels)
            .ok_or_else(|| std::io::Error::other("failed to build RGBA image from GPU readback"))?;
        image.save(path)?;
        self.host.redraw_requested = false;
        Ok(())
    }

    fn click_toggle_button(&mut self) {
        let center = self
            .render_root
            .get_widget_with_tag(BUTTON_TAG)
            .expect("toggle button should exist")
            .ctx()
            .bounding_box()
            .center();

        self.send_pointer_event(pointer_move(center));
        self.send_pointer_event(pointer_press(center));
        self.send_pointer_event(pointer_release(center));
    }

    fn send_pointer_event(&mut self, event: PointerEvent) {
        self.render_root.handle_pointer_event(event);
        process_host_signals(&mut self.render_root, &mut self.host);
    }

    fn render_masonry(&mut self) -> Result<(), Box<dyn Error>> {
        let (visual_layers, _tree_update) = self.render_root.redraw();
        let overlays: Vec<_> = visual_layers
            .overlay_layers()
            .map(|layer| {
                let VisualLayerKind::Scene(scene) = &layer.kind else {
                    unreachable!("overlay_layers only yields scene layers");
                };
                ImagingLayer {
                    scene,
                    transform: layer.transform,
                }
            })
            .collect();
        let root_layer = visual_layers
            .root_layer()
            .expect("paint should always produce a root scene layer");
        let VisualLayerKind::Scene(root_scene) = &root_layer.kind else {
            unreachable!("root_layer always returns a scene layer");
        };
        let frame = PreparedFrame::new(
            self.size.width,
            self.size.height,
            SCALE_FACTOR,
            Color::TRANSPARENT,
            root_scene,
            &overlays,
        );

        self.masonry_renderer.render_to_texture(
            RenderTarget {
                adapter: &self.gpu.adapter,
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                texture: &self.ui_target.texture,
                view: &self.ui_target.view,
            },
            frame,
        )?;

        Ok(())
    }

    fn composite_ui(&self) {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("embedded_wgpu_triangle composite"),
            });
        self.ui_blitter.copy(
            &self.gpu.device,
            &mut encoder,
            &self.ui_target.view,
            &self.scene_target.view,
        );
        self.gpu.queue.submit([encoder.finish()]);
    }
}

#[derive(Debug)]
struct HostState {
    triangle_color: TriangleColor,
    redraw_requested: bool,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            triangle_color: TriangleColor::Mint,
            redraw_requested: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum TriangleColor {
    Mint,
    Coral,
}

impl TriangleColor {
    fn toggle(&mut self) {
        *self = match self {
            Self::Mint => Self::Coral,
            Self::Coral => Self::Mint,
        };
    }

    fn as_rgba(self) -> [f32; 4] {
        match self {
            Self::Mint => [0.26, 0.82, 0.73, 1.0],
            Self::Coral => [0.93, 0.43, 0.35, 1.0],
        }
    }
}

struct GpuState {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

impl GpuState {
    fn new() -> Result<Self, Box<dyn Error>> {
        block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::default(),
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .await?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("embedded_wgpu_triangle device"),
                    required_features: wgpu::Features::empty(),
                    ..Default::default()
                })
                .await?;

            Ok(Self {
                adapter,
                device,
                queue,
            })
        })
    }
}

#[derive(Debug)]
struct OffscreenTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OffscreenTexture {
    fn new(device: &wgpu::Device, size: PhysicalSize<u32>, usage: wgpu::TextureUsages) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("embedded_wgpu_triangle texture"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

#[derive(Debug)]
struct TriangleRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
}

impl TriangleRenderer {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("embedded_wgpu_triangle uniform buffer"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("embedded_wgpu_triangle bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("embedded_wgpu_triangle bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("embedded_wgpu_triangle pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("embedded_wgpu_triangle shader"),
            source: wgpu::ShaderSource::Wgsl(TRIANGLE_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("embedded_wgpu_triangle pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            bind_group,
            uniform_buffer,
        }
    }

    fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_view: &wgpu::TextureView,
        color: [f32; 4],
    ) {
        queue.write_buffer(&self.uniform_buffer, 0, &encode_color(color));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("embedded_wgpu_triangle scene encoder"),
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("embedded_wgpu_triangle scene pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.07,
                        g: 0.08,
                        b: 0.10,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        queue.submit([encoder.finish()]);
    }
}

fn create_render_root(size: PhysicalSize<u32>) -> RenderRoot {
    let button = NewWidget::new(Button::with_text("Toggle triangle color")).with_tag(BUTTON_TAG);
    RenderRoot::new(
        button,
        RenderRootOptions {
            default_properties: Arc::new(default_property_set()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size,
            scale_factor: SCALE_FACTOR,
            test_font: Some(Blob::new(Arc::new(ROBOTO))),
        },
    )
}

fn process_host_signals(render_root: &mut RenderRoot, host: &mut HostState) {
    render_root.process_signals(|_, signal| match signal {
        RenderRootSignal::Action(action, _widget_id) => handle_action(action, host),
        RenderRootSignal::RequestRedraw | RenderRootSignal::RequestAnimFrame => {
            host.redraw_requested = true;
        }
        RenderRootSignal::SetCursor(_)
        | RenderRootSignal::TakeFocus
        | RenderRootSignal::StartIme
        | RenderRootSignal::EndIme
        | RenderRootSignal::ImeMoved(_, _)
        | RenderRootSignal::ClipboardStore(_)
        | RenderRootSignal::SetSize(_)
        | RenderRootSignal::SetTitle(_)
        | RenderRootSignal::DragWindow
        | RenderRootSignal::DragResizeWindow(_)
        | RenderRootSignal::ToggleMaximized
        | RenderRootSignal::Minimize
        | RenderRootSignal::Exit
        | RenderRootSignal::ShowWindowMenu(_)
        | RenderRootSignal::WidgetSelectedInInspector(_) => {}
    });
}

fn handle_action(action: ErasedAction, host: &mut HostState) {
    if action.is::<ButtonPress>() {
        host.triangle_color.toggle();
        host.redraw_requested = true;
    }
}

fn pointer_move(position: Point) -> PointerEvent {
    PointerEvent::Move(wgpu_pointer_update(position, false))
}

fn pointer_press(position: Point) -> PointerEvent {
    PointerEvent::Down(PointerButtonEvent {
        pointer: PRIMARY_MOUSE,
        button: Some(PointerButton::Primary),
        state: pointer_state(position, true),
    })
}

fn pointer_release(position: Point) -> PointerEvent {
    PointerEvent::Up(PointerButtonEvent {
        pointer: PRIMARY_MOUSE,
        button: Some(PointerButton::Primary),
        state: pointer_state(position, false),
    })
}

fn wgpu_pointer_update(position: Point, pressed: bool) -> masonry::core::PointerUpdate {
    masonry::core::PointerUpdate {
        pointer: PRIMARY_MOUSE,
        current: pointer_state(position, pressed),
        coalesced: vec![],
        predicted: vec![],
    }
}

fn pointer_state(position: Point, pressed: bool) -> PointerState {
    let mut state = PointerState {
        position: PhysicalPosition {
            x: position.x,
            y: position.y,
        },
        scale_factor: SCALE_FACTOR,
        ..PointerState::default()
    };
    if pressed {
        state.buttons.insert(PointerButton::Primary);
        state.count = 1;
        state.pressure = 0.5;
    }
    state
}

fn encode_color(color: [f32; 4]) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    for (chunk, component) in bytes.chunks_exact_mut(4).zip(color) {
        chunk.copy_from_slice(&component.to_ne_bytes());
    }
    bytes
}

fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    size: PhysicalSize<u32>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let bytes_per_row = (size.width * 4).next_multiple_of(256);
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("embedded_wgpu_triangle readback buffer"),
        size: u64::from(bytes_per_row) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("embedded_wgpu_triangle readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let (sender, receiver) = mpsc::channel();
    readback_buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    receiver
        .recv()
        .map_err(|err| std::io::Error::other(format!("readback channel failed: {err}")))??;

    let mapped = readback_buffer.slice(..).get_mapped_range();
    let mut pixels = Vec::with_capacity((size.width * size.height * 4) as usize);
    for row in mapped.chunks_exact(bytes_per_row as usize) {
        pixels.extend_from_slice(&row[..(size.width * 4) as usize]);
    }
    drop(mapped);
    readback_buffer.unmap();

    Ok(pixels)
}

fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);

    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}
