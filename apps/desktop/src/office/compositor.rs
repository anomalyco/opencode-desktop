//! Draws the window, and Office pages into their effect slots: each page's Vello scene renders on
//! the window's GPU into a texture the size the slot shows it at on the display, so a page is as
//! sharp at 1x, 1.5x and 2x, and at any zoom, as the rest of the window. Desktop paints the same
//! display lists into a canvas at the device pixel ratio (`pages.tsx`). A window with no page on
//! screen draws exactly as Rune's default frame does.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};

use rune::compose::{DeviceKey, Frame};
use rune::render::{Instance, KIND_EFFECT};
use vello::wgpu;

use super::registry;
use crate::App;

/// A page's texture holds at most this many pixels, about a 4K screen's twice over, as Desktop
/// caps a slide's bitmap (`maxBitmapPixels`); past it the page renders at a lower density.
const MAX_PIXELS: f64 = 4096.0 * 4096.0;

#[derive(Default)]
pub struct OfficeCompositor {
    gpu: Option<Gpu>,
}

struct Gpu {
    key: DeviceKey,
    vello: Vello,
    blit: Blit,
    targets: HashMap<u32, Target>,
}

/// Vello compiles its shaders once per device, off the UI thread.
enum Vello {
    Starting(Receiver<Result<vello::Renderer, String>>),
    Ready(Box<vello::Renderer>),
    Failed,
}

/// One page's rendered texture.
struct Target {
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    size: [u32; 2],
    version: u64,
}

struct Blit {
    layout: wgpu::BindGroupLayout,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SlotUniform {
    rect: [f32; 4],
    clip: [f32; 4],
    pass: [f32; 4],
    fade: [u32; 4],
    flags: [f32; 4],
}

impl rune::Compositor<App> for OfficeCompositor {
    fn render(&mut self, f: &mut Frame<'_, App>) {
        let slots: Vec<(u32, [f32; 4])> = f.rt.instances.iter().filter(|i| i.params[1] == KIND_EFFECT).filter(|i| registry::is_mark(mark(i))).map(|i| (mark(i), i.rect)).collect();
        if slots.is_empty() {
            self.gpu = None;
            return f.draw_ui();
        }
        let key = f.device_key();
        if self.gpu.as_ref().is_none_or(|g| g.key != key) {
            self.gpu = Some(Gpu::new(f.device, key));
        }
        let Some(gpu) = self.gpu.as_mut() else { return f.draw_ui() };
        gpu.poll_vello();
        gpu.targets.retain(|m, _| slots.iter().any(|(s, _)| s == m));
        if let Vello::Ready(renderer) = &mut gpu.vello {
            let max = f64::from(f.device.limits().max_texture_dimension_2d);
            for (mark, rect) in &slots {
                let Some(draw) = registry::scene(*mark) else { continue };
                let (w, h) = (f64::from(rect[2]).max(1.0), f64::from(rect[3]).max(1.0));
                let fit = (MAX_PIXELS / (w * h)).sqrt().min(max / w.max(h)).min(1.0);
                let size = [(w * fit).round().max(1.0) as u32, (h * fit).round().max(1.0) as u32];
                registry::drawn_at(*mark, f64::from(size[0]) / draw.scene().width);
                let version = draw.version();
                if gpu.targets.get(mark).is_some_and(|t| t.size == size && t.version == version) {
                    continue;
                }
                let target = gpu.targets.remove(mark).filter(|t| t.size == size).unwrap_or_else(|| Target::new(f.device, &gpu.blit, size));
                let page = draw.scene();
                let mut scene = vello::Scene::new();
                let transform = vello::kurbo::Affine::scale_non_uniform(f64::from(size[0]) / page.width, f64::from(size[1]) / page.height);
                scene.append(&page.scene, Some(transform));
                let params = vello::RenderParams { base_color: vello::peniko::Color::TRANSPARENT, width: size[0], height: size[1], antialiasing_method: vello::AaConfig::Area };
                if renderer.render_to_texture(f.device, f.queue, &scene, &target.view, &params).is_ok() {
                    gpu.targets.insert(*mark, Target { version, ..target });
                }
            }
        }
        let (queue, clear) = (f.queue, f.rt.clear_color());
        f.ui.prepare(f.device, f.queue, &mut f.rt.text.atlas, &f.rt.instances, f.size);
        let (targets, blit, device) = (&gpu.targets, &mut gpu.blit, f.device);
        f.ui.draw_with(f.encoder, f.target, clear, &mut |pass, site| {
            let Some(target) = targets.get(&site.mark) else { return };
            let i = site.instance;
            let uniform = SlotUniform {
                rect: [i.rect[0] - site.origin[0], i.rect[1] - site.origin[1], i.rect[2], i.rect[3]],
                clip: i.clip,
                pass: [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32],
                fade: i.fade,
                flags: [if site.srgb_blend { 1.0 } else { 0.0 }, 1.0, 1.0, 0.0],
            };
            queue.write_buffer(&target.uniform, 0, bytemuck::bytes_of(&uniform));
            pass.set_pipeline(blit.pipeline(device, site.format));
            pass.set_bind_group(0, &target.bind, &[]);
            pass.draw(0..4, 0..1);
        });
    }

    fn animating(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| matches!(g.vello, Vello::Starting(_)))
    }
}

/// An effect slot's tag, from the two 16-bit halves its record keeps it in (`El::effect`).
fn mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Gpu {
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = device.clone();
        let started = std::thread::Builder::new().name("office-vello".into()).spawn(move || {
            let options = vello::RendererOptions { use_cpu: false, antialiasing_support: vello::AaSupport::area_only(), num_init_threads: std::num::NonZeroUsize::new(2), pipeline_cache: None };
            tx.send(vello::Renderer::new(&shared, options).map_err(|e| e.to_string())).ok();
        });
        let vello = if started.is_ok() { Vello::Starting(rx) } else { Vello::Failed };
        Gpu { key, vello, blit: Blit::new(device), targets: HashMap::new() }
    }

    fn poll_vello(&mut self) {
        if let Vello::Starting(rx) = &self.vello {
            self.vello = match rx.try_recv() {
                Ok(Ok(renderer)) => Vello::Ready(Box::new(renderer)),
                Ok(Err(e)) => {
                    eprintln!("office: Vello could not start: {e}");
                    Vello::Failed
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Vello::Failed,
            };
            registry::redraw();
        }
    }
}

impl Target {
    fn new(device: &wgpu::Device, blit: &Blit, size: [u32; 2]) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("office page"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("office slot"), size: std::mem::size_of::<SlotUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("office slot"),
            layout: &blit.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&blit.sampler) },
            ],
        });
        Target { view, bind, uniform, size, version: u64::MAX }
    }
}

impl Blit {
    fn new(device: &wgpu::Device) -> Blit {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("office blit"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", rune::render::CLIP_WGSL, include_str!("blit.wgsl")).into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("office blit"),
            entries: &[
                entry(0, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("office blit"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("office page"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Blit { layout, pipelines: HashMap::new(), shader, pipeline_layout, sampler }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (shader, layout) = (&self.shader, &self.pipeline_layout);
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("office blit"),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        })
    }
}
