//! Checks the hand-off between a live run's GPU and the window on Rune's Metal backend
//! (`opencode_widget::gpu`, `shader::live`): a run replays on a stock wgpu Metal device made on
//! Rune's `MTLDevice`, and the window copies each finished version onto Rune's device, with the
//! hand-off's own calls (`poll`, `due`, `copied`). A replay thread clears a canvas red and blue in
//! turn, a frame each; the window thread copies what's due and reads the copy back: every copy
//! must hold its version's color (with no hand-off, 121 of 300 didn't), at 120 Hz and with the
//! replay going as fast as it can. Then it times the copy of
//! a full-screen canvas (5K, clamped to 4096 x 2304), and the replay's waits for copies.
//! macOS only. `cargo run --release -p opencode-widget-view --example gpu_handoff_check`.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout, unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("macOS only: the hand-off is Rune's Metal backend's");
}

#[cfg(target_os = "macos")]
fn main() {
    mac::main();
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::{MTLDevice, MTLTexture};
    use opencode_widget::gpu::{self, Arg, Canvases, Device};
    use opencode_widget::live::gpu::RunGpu;
    use serde_json::json;

    fn block<F: std::future::Future>(f: F) -> F::Output {
        pollster::block_on(f)
    }

    /// Rune's device, as a window gets it, and a stock wgpu Metal device on the same MTLDevice.
    fn devices() -> ((wgpu::Device, wgpu::Queue), Device) {
        let rune = rune_gpu::metal::instance();
        let adapter = block(rune.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("Rune's adapter");
        let (rd, rq) = block(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("Rune's device");
        let raw = rune_gpu::metal::raw_device(&rd).expect("Rune's Metal backend") as *const ProtocolObject<dyn MTLDevice> as *const u8;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::METAL, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapters = block(instance.enumerate_adapters(wgpu::Backends::METAL));
        let adapter = adapters.into_iter().find(|a| unsafe { a.as_hal::<wgpu::hal::api::Metal>() }.is_some_and(|h| std::ptr::eq(h.raw_device() as *const _ as *const u8, raw))).expect("the stock adapter on Rune's MTLDevice");
        let (device, queue) = block(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: gpu::device_limits(), ..Default::default() })).expect("a stock device");
        ((rd, rq), Device { device, queue, format: wgpu::TextureFormat::Bgra8Unorm, copies: true, aborts: None })
    }

    fn ops(v: serde_json::Value) -> Vec<Arg> {
        match Arg::from_json(&v) {
            Arg::List(l) => l,
            _ => Vec::new(),
        }
    }

    /// The run's canvas texture as a texture of Rune's device, as the compositor wraps it.
    fn wrap(t: &wgpu::Texture) -> wgpu::Texture {
        let guard = unsafe { t.as_hal::<wgpu::hal::api::Metal>() }.unwrap();
        let raw: &ProtocolObject<dyn MTLTexture> = guard.raw_handle();
        let raw = unsafe { Retained::retain(raw as *const _ as *mut ProtocolObject<dyn MTLTexture>) }.unwrap();
        drop(guard);
        let desc = wgpu::TextureDescriptor { label: None, size: t.size(), mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: t.format(), usage: t.usage(), view_formats: &[] };
        rune_gpu::metal::texture_from_raw(raw, &desc, Arc::new(t.clone()))
    }

    fn target(rd: &wgpu::Device, size: wgpu::Extent3d) -> wgpu::Texture {
        rd.create_texture(&wgpu::TextureDescriptor { label: None, size, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm, usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] })
    }

    /// A run with canvas `c` at `w` x `h`, configured.
    fn run(w: u32, h: u32) -> Arc<RunGpu> {
        let run = RunGpu::new(|_| {}, Arc::new(|| {}));
        run.rendered(vec![("c".into(), "0/:c".into())]);
        run.resize("c", w, h);
        let setup = ops(json!([
            ["context", 1, "c", "bgra8unorm", "opaque"],
            ["view", 2, 1, { "dimension": "2d", "baseMipLevel": 0, "mipLevelCount": 1, "baseArrayLayer": 0, "arrayLayerCount": 1 }],
        ]));
        assert!(run.apply(&setup, &[]).is_none(), "the canvas is configured");
        run
    }

    /// One frame of the replay: canvas `c` cleared red (even) or blue (odd).
    fn frame(i: u32) -> Vec<Arg> {
        let color = if i.is_multiple_of(2) { [1, 0, 0, 1] } else { [0, 0, 1, 1] };
        ops(json!([
            ["pass", { "colorAttachments": [{ "view": 2, "resolveTarget": null, "clearValue": color, "loadOp": "clear", "storeOp": "store" }], "depthStencilAttachment": null }],
            ["end"],
            ["submit"],
        ]))
    }

    /// Frames replayed at `hz` (0: as fast as it goes, for a second), the window copying what's
    /// due at 240 Hz: frames replayed, copies, wrong copies, and the replay's waits.
    fn ordering(rd: &wgpu::Device, rq: &wgpu::Queue, hz: f64, frames: u32) -> (u32, u32, u32, (u64, Duration, Duration)) {
        let run = run(256, 256);
        let canvases: Arc<Canvases> = run.canvases.clone();
        let done = Arc::new(AtomicBool::new(false));
        let replayed = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let replay = {
            let (run, done, replayed) = (run.clone(), done.clone(), replayed.clone());
            std::thread::spawn(move || {
                let start = Instant::now();
                for i in 0..frames {
                    if hz == 0.0 && start.elapsed() > Duration::from_secs(1) {
                        break;
                    }
                    replayed.store(i + 1, Ordering::Release);
                    assert!(run.apply(&frame(i), &[]).is_none(), "frame {i} replays");
                    run.end_frame();
                    if hz > 0.0 {
                        let next = start + Duration::from_secs_f64(f64::from(i + 1) / hz);
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                    }
                }
                done.store(true, Ordering::Release);
            })
        };
        let mut copy: Option<(wgpu::Texture, wgpu::Texture, wgpu::Texture)> = None;
        let read = rd.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256 * 256 * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let (mut copies, mut wrong) = (0, 0);
        loop {
            let finished = done.load(Ordering::Acquire);
            canvases.poll();
            if let Some(due) = canvases.due(rd) {
                let mut enc = rd.create_command_encoder(&Default::default());
                let mut version = 0;
                for (_, c) in due {
                    if copy.as_ref().is_none_or(|(of, ..)| *of != c.texture) {
                        copy = Some((c.texture.clone(), wrap(&c.texture), target(rd, c.texture.size())));
                    }
                    let (_, from, to) = copy.as_ref().unwrap();
                    enc.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), c.texture.size());
                    version = c.version;
                }
                rq.submit([enc.finish()]);
                let c = canvases.clone();
                rq.on_submitted_work_done(move || c.copied());
                // The check: the copy holds its version's color (version v is frame v - 1's).
                let (_, _, to) = copy.as_ref().unwrap();
                let mut enc = rd.create_command_encoder(&Default::default());
                enc.copy_texture_to_buffer(to.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(1024), rows_per_image: None } }, to.size());
                rq.submit([enc.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                rd.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let px: Vec<u8> = read.slice(..).get_mapped_range().unwrap().to_vec();
                read.unmap();
                let want: [u8; 4] = if (version - 1) % 2 == 0 { [0, 0, 255, 255] } else { [255, 0, 0, 255] };
                copies += 1;
                if px.chunks(4).any(|p| p != want) {
                    wrong += 1;
                }
            } else if finished && !canvases.behind() {
                break;
            }
            std::thread::sleep(Duration::from_micros(4166));
        }
        replay.join().unwrap();
        (replayed.load(Ordering::Acquire), copies, wrong, canvases.waits())
    }

    /// The GPU time of one copy of a `w` x `h` canvas, and of the CPU encoding it.
    fn copy_cost(rd: &wgpu::Device, rq: &wgpu::Queue, stock: &wgpu::Device, w: u32, h: u32) -> (Duration, Duration) {
        let run = run(w, h);
        assert!(run.apply(&frame(0), &[]).is_none());
        stock.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let c = run.canvases.canvas("0/:c").expect("drawn");
        let (from, to) = (wrap(&c.texture), target(rd, c.texture.size()));
        let n = 200;
        let cpu = Instant::now();
        let mut enc = rd.create_command_encoder(&Default::default());
        for _ in 0..n {
            enc.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), c.texture.size());
        }
        let encoded = cpu.elapsed();
        rd.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let gpu = Instant::now();
        rq.submit([enc.finish()]);
        rd.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        (gpu.elapsed() / n, encoded / n)
    }

    pub fn main() {
        let ((rd, rq), stock) = devices();
        println!("Rune: {}; runs on: {:?} {}", rune_gpu::describe_gpu(&rd), stock.device.adapter_info().backend, stock.device.adapter_info().name);
        let stock_device = stock.device.clone();
        gpu::provide(move || Some(stock.clone()));
        let mut bad = false;
        for (hz, frames, label) in [(120.0, 300, "at 120 Hz"), (0.0, u32::MAX, "as fast as it replays, for a second")] {
            let (frames, copies, wrong, (waits, total, longest)) = ordering(&rd, &rq, hz, frames);
            bad |= wrong > 0;
            let mean = if waits > 0 { total / waits as u32 } else { Duration::ZERO };
            println!("{}  {frames} frames {label}: {copies} copies, {wrong} wrong; replay waited for a copy {waits} times, mean {:.3} ms, longest {:.3} ms", if wrong == 0 { "ok  " } else { "FAIL" }, mean.as_secs_f64() * 1e3, longest.as_secs_f64() * 1e3);
        }
        let (w, h) = gpu::clamp_size(5120, 2880);
        let (gpu_time, cpu_time) = copy_cost(&rd, &rq, &stock_device, w, h);
        let ms = gpu_time.as_secs_f64() * 1e3;
        println!("     a full-screen canvas ({w} x {h}): {ms:.3} ms of GPU a copy ({:.1}% of a 120 Hz frame), {:.1} us to encode", ms / (1000.0 / 120.0) * 100.0, cpu_time.as_secs_f64() * 1e6);
        if bad {
            std::process::exit(1);
        }
    }
}
