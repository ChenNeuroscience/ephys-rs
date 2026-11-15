//! Showing a camera frame shaded on the GPU (see `frame.wgsl`), rather than coloured pixel by pixel on the GUI's
//! thread and uploaded in colour: the frame's uploaded as it is, a byte a pixel, only when a new one comes, and the
//! shader shades it as it draws, from the detection settings. The same as `colour_image`, which the GUI uses without
//! the GPU (and the tests check this against).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Rect};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use ephys_rs::gui::gpu::Target;

use super::overlay::Overlays;
use super::tracker_view::FrameView;
use super::{Crop, Detection};

/// The newest frame a view's been given, for its callback to upload: kept (not taken) so the texture can be made
/// again, if the GPU's resources were.
#[derive(Default)]
pub struct Upload {
    frame: Mutex<Pending>,
}

#[derive(Default)]
struct Pending {
    id: Option<u64>,
    size: [u32; 2],
    pixels: Vec<u8>,
    /// Whether it's changed since it was last uploaded.
    fresh: bool,
}

impl Upload {
    /// Keeps `frame` to upload, unless it's the one already kept.
    pub fn set(&self, frame: FrameView) {
        let mut pending = self.frame.lock().unwrap();
        let size = [frame.width as u32, frame.height as u32];
        if pending.id == Some(frame.id) && pending.size == size {
            return;
        }
        pending.pixels.clear();
        pending.pixels.extend_from_slice(&frame.pixels[..frame.width * frame.height]);
        (pending.id, pending.size, pending.fresh) = (Some(frame.id), size, true);
    }
}

/// Paints a view's frame (`upload`) in `rect`, shaded as `detection` and `overlays` say.
pub fn paint(
    ui: &egui::Ui,
    id: egui::Id,
    upload: Arc<Upload>,
    rect: Rect,
    detection: Option<&Detection>,
    overlays: &Overlays,
) {
    let callback = FrameCallback { id, upload, rect, params: Params::of(detection, overlays) };
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}

struct FrameCallback {
    id: egui::Id,
    upload: Arc<Upload>,
    rect: Rect,
    params: Params,
}

/// The shader's `Params`, but for where it's drawn (filled in as it's prepared, in physical pixels).
#[derive(Clone, Copy)]
struct Params {
    thresholds: [u32; 3],
    flags: u32,
    shapes: [[f32; 8]; 3],
}

const FLAG_DETECTION: u32 = 1;
const FLAG_CROP_SHOWN: u32 = 2;
const FLAG_DETECTION_SHOWN: u32 = 4;
const FLAG_TIMING_SHOWN: u32 = 8;
const FLAG_CROPPED: u32 = 16;
const FLAG_LINEAR_OUTPUT: u32 = 32;

/// A shape the shader takes everything to be in, or nothing.
const EVERYTHING: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
const NOTHING: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0];

/// The shader's `Shape` for `crop`: everything for no crop.
fn shape(crop: &Crop) -> [f32; 8] {
    let (Some(center), Some([hw, hh])) = (crop.center(), crop.half_extents()) else { return EVERYTHING };
    let (s, c) = crop.angle().sin_cos();
    let kind = if matches!(crop, Crop::Circle { .. }) { 2.0 } else { 3.0 };
    [center[0], center[1], hw, hh, c, s, kind, 0.0]
}

impl Params {
    /// As `row_spans` and `look` (in `tracker_view`) work it out.
    fn of(detection: Option<&Detection>, overlays: &Overlays) -> Self {
        let Some(d) = detection else {
            return Params { thresholds: [0; 3], flags: 0, shapes: [EVERYTHING, EVERYTHING, NOTHING] };
        };
        let crop = shape(&d.crop);
        let inner = if d.edge_width() <= 0.0 || d.crop == Crop::None {
            crop
        } else {
            d.crop.inset(d.edge_width()).map_or(NOTHING, |inset| shape(&inset))
        };
        let timing = d.timing_spot().map_or(NOTHING, |spot| shape(&spot));
        let flags = FLAG_DETECTION
            | if overlays.crop.visible { FLAG_CROP_SHOWN } else { 0 }
            | if overlays.detection.visible { FLAG_DETECTION_SHOWN } else { 0 }
            | if overlays.timing.visible { FLAG_TIMING_SHOWN } else { 0 }
            | if d.crop != Crop::None { FLAG_CROPPED } else { 0 };
        let thresholds = [d.threshold, d.edge_threshold(), d.timing_threshold()].map(u32::from);
        Params { thresholds, flags, shapes: [crop, inner, timing] }
    }

    /// As the shader's uniform: drawn at `rect` (physical pixels: left, top, width, height), into `target`.
    fn bytes(&self, rect: [f32; 4], target: wgpu::TextureFormat) -> Vec<u8> {
        let flags = self.flags | if target.is_srgb() { FLAG_LINEAR_OUTPUT } else { 0 };
        let mut bytes = Vec::with_capacity(UNIFORM_SIZE as usize);
        bytes.extend(rect.iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(self.thresholds.iter().chain(&[0]).flat_map(|v| v.to_le_bytes()));
        bytes.extend([flags, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()));
        bytes.extend(self.shapes.iter().flatten().flat_map(|v| v.to_le_bytes()));
        bytes
    }
}

/// The uniform's size: rect, thresholds and flags (a vec4 each), then the three shapes (two each).
const UNIFORM_SIZE: u64 = (3 + 3 * 2) * 16;

/// What the GPU keeps for showing frames: the pipeline, and each view's texture and uniform.
struct Resources {
    target: wgpu::TextureFormat,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    views: HashMap<egui::Id, View>,
}

struct View {
    size: [u32; 2],
    texture: wgpu::Texture,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Resources {
    fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame"),
            source: wgpu::ShaderSource::Wgsl(include_str!("frame.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frame"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("frame"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Resources { target, pipeline, layout, views: HashMap::new() }
    }

    /// View `id`'s texture, made for frames of `size` if it isn't already.
    fn view(&mut self, device: &wgpu::Device, id: egui::Id, size: [u32; 2]) -> &View {
        if self.views.get(&id).is_none_or(|view| view.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frame"),
                size: UNIFORM_SIZE,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("frame"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                ],
            });
            self.views.insert(id, View { size, texture, uniforms, bind_group });
        }
        &self.views[&id]
    }
}

impl CallbackTrait for FrameCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(&Target { format }) = resources.get::<Target>() else { return Vec::new() };
        if resources.get::<Resources>().is_none_or(|r| r.target != format) {
            resources.insert(Resources::new(device, format));
        }
        let gpu = resources.get_mut::<Resources>().expect("just made");
        let mut pending = self.upload.frame.lock().unwrap();
        if pending.id.is_none() || pending.size.contains(&0) {
            return Vec::new();
        }
        let fresh =
            std::mem::take(&mut pending.fresh) || gpu.views.get(&self.id).is_none_or(|v| v.size != pending.size);
        let view = gpu.view(device, self.id, pending.size);
        if fresh {
            let [width, height] = pending.size;
            queue.write_texture(
                view.texture.as_image_copy(),
                &pending.pixels,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width), rows_per_image: Some(height) },
                wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            );
        }
        let ppp = screen.pixels_per_point;
        let rect = [self.rect.left() * ppp, self.rect.top() * ppp, self.rect.width() * ppp, self.rect.height() * ppp];
        queue.write_buffer(&view.uniforms, 0, &self.params.bytes(rect, format));
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let Some(gpu) = resources.get::<Resources>() else { return };
        let Some(view) = gpu.views.get(&self.id) else { return };
        render_pass.set_pipeline(&gpu.pipeline);
        render_pass.set_bind_group(0, &view.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}
