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

/// A page's texture holds at most this many pixels, about a 4K screen's twice over, as Desktop
/// caps a slide's bitmap (`maxBitmapPixels`); past it the page renders at a lower density.
const MAX_PIXELS: f64 = 4096.0 * 4096.0;

#[derive(Default)]
pub struct OfficeCompositor {
    gpu: Option<Result<Gpu, (DeviceKey, String)>>,
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
    Failed(String),
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

impl OfficeCompositor {
    /// Only while a page is on screen: otherwise the window's frame is Rune's own.
    pub fn draws_own(&self) -> bool {
        matches!(self.gpu, Some(Ok(_)))
    }

    /// Renders the pages on screen into their targets before the frame records. False when no
    /// page is on screen.
    pub fn prepare<S>(&mut self, f: &mut Frame<'_, S>) -> bool {
        let key = f.device_key();
        self.prepare_inner(&f.rt.instances, f.device, f.queue, key)
    }

    fn prepare_inner(&mut self, instances: &[Instance], device: &wgpu::Device, queue: &wgpu::Queue, key: DeviceKey) -> bool {
        let slots: Vec<(u32, [f32; 4])> = instances.iter().filter(|i| i.params[1] == KIND_EFFECT).filter(|i| registry::is_mark(mark(i))).map(|i| (mark(i), i.rect)).collect();
        if slots.is_empty() {
            self.gpu = None;
            return false;
        }
        let marks: Vec<u32> = slots.iter().map(|(m, _)| *m).collect();
        let same = match &self.gpu {
            Some(Ok(g)) => g.key == key,
            Some(Err((k, _))) => *k == key,
            None => false,
        };
        if !same {
            self.gpu = Some(Gpu::new(device, key).map_err(|e| (key, e)));
        }
        let gpu = match self.gpu.as_mut() {
            Some(Ok(gpu)) => gpu,
            Some(Err((_, reason))) => {
                registry::fail_marks(&marks, reason);
                return false;
            }
            None => return false,
        };
        gpu.poll_vello();
        gpu.targets.retain(|m, _| slots.iter().any(|(s, _)| s == m));
        match &mut gpu.vello {
            Vello::Starting(_) => {}
            Vello::Failed(reason) => {
                registry::fail_marks(&marks, reason);
                return false;
            }
            Vello::Ready(renderer) => {
                let max = f64::from(device.limits().max_texture_dimension_2d);
                let blit = &gpu.blit;
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
                    let existing = gpu.targets.remove(mark).filter(|t| t.size == size);
                    let rendered = catch(|| {
                        let target = existing.unwrap_or_else(|| Target::new(device, blit, size));
                        let page = draw.scene();
                        let mut scene = vello::Scene::new();
                        let transform = vello::kurbo::Affine::scale_non_uniform(f64::from(size[0]) / page.width, f64::from(size[1]) / page.height);
                        scene.append(&page.scene, Some(transform));
                        let params = vello::RenderParams { base_color: vello::peniko::Color::TRANSPARENT, width: size[0], height: size[1], antialiasing_method: vello::AaConfig::Area };
                        renderer.render_to_texture(device, queue, &scene, &target.view, &params).map_err(|e| e.to_string())?;
                        Ok(target)
                    });
                    match rendered {
                        Ok(target) => {
                            gpu.targets.insert(*mark, Target { version, ..target });
                        }
                        Err(reason) => {
                            eprintln!("office: page could not be shown: {reason}");
                            registry::fail_marks(&[*mark], &reason);
                        }
                    }
                }
            }
        }
        true
    }

    /// Draws page slot `site` into the open pass.
    pub fn draw(&mut self, pass: &mut wgpu::RenderPass<'_>, site: &rune::render::EffectSite<'_>, queue: &wgpu::Queue, device: &wgpu::Device) {
        let Some(Ok(gpu)) = self.gpu.as_mut() else { return };
        let Some(target) = gpu.targets.get(&site.mark) else { return };
        let i = site.instance;
        let uniform = SlotUniform {
            rect: [i.rect[0] - site.origin[0], i.rect[1] - site.origin[1], i.rect[2], i.rect[3]],
            clip: i.clip,
            pass: [site.origin[0], site.origin[1], site.size[0] as f32, site.size[1] as f32],
            fade: i.fade,
            flags: [if site.srgb_blend { 1.0 } else { 0.0 }, 1.0, 1.0, 0.0],
        };
        let blit = &mut gpu.blit;
        let format = site.format;
        let pipeline = match catch(|| Ok(blit.pipeline(device, format) as *const wgpu::RenderPipeline)) {
            Ok(ptr) => ptr,
            Err(reason) => {
                eprintln!("office: page blit failed: {reason}");
                registry::fail_marks(&[site.mark], &reason);
                return;
            }
        };
        queue.write_buffer(&target.uniform, 0, bytemuck::bytes_of(&uniform));
        // Fetch again without raw-pointer dereference now that `blit.pipelines` holds `format`.
        let _ = pipeline;
        pass.set_pipeline(gpu.blit.pipeline(device, format));
        pass.set_bind_group(0, &target.bind, &[]);
        pass.draw(0..4, 0..1);
    }

    pub fn animating(&self) -> bool {
        matches!(&self.gpu, Some(Ok(g)) if matches!(g.vello, Vello::Starting(_)))
    }
}

/// An effect slot's tag, from the two 16-bit halves its record keeps it in (`El::effect`).
fn mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
}

/// Rune's native Metal backend (`GpuBackend::Metal`, macOS's default unless `RUNE_GPU_BACKEND=wgpu`)
/// does not implement `dispatch_workgroups_indirect`, which Vello's GPU `path_count` and
/// `path_tiling` passes call when `use_cpu` is false. With `use_cpu: true`, Vello runs those
/// stages on the CPU and still dispatches `fine` rasterization on Metal via `dispatch_workgroups`.
#[allow(clippy::disallowed_methods)]
fn vello_uses_cpu() -> bool {
    cfg!(target_os = "macos") && !std::env::var("RUNE_GPU_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("wgpu"))
}

fn panic_reason(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "the Office renderer panicked".to_owned()
    }
}

fn catch<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|p| Err(panic_reason(p)))
}

impl Gpu {
    fn new(device: &wgpu::Device, key: DeviceKey) -> Result<Gpu, String> {
        let blit = catch(|| Ok(Blit::new(device)))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let shared = device.clone();
        let use_cpu = vello_uses_cpu();
        let started = std::thread::Builder::new().name("office-vello".into()).spawn(move || {
            let res = catch(|| {
                let options = vello::RendererOptions { use_cpu, antialiasing_support: vello::AaSupport::area_only(), num_init_threads: std::num::NonZeroUsize::new(2), pipeline_cache: None };
                vello::Renderer::new(&shared, options).map_err(|e| e.to_string())
            });
            tx.send(res).ok();
        });
        let vello = match started {
            Ok(_) => Vello::Starting(rx),
            Err(e) => Vello::Failed(e.to_string()),
        };
        Ok(Gpu { key, vello, blit, targets: HashMap::new() })
    }

    fn poll_vello(&mut self) {
        if let Vello::Starting(rx) = &self.vello {
            self.vello = match rx.try_recv() {
                Ok(Ok(renderer)) => Vello::Ready(Box::new(renderer)),
                Ok(Err(e)) => {
                    eprintln!("office: Vello could not start: {e}");
                    Vello::Failed(e)
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Vello::Failed("the Office renderer stopped before starting".into()),
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
