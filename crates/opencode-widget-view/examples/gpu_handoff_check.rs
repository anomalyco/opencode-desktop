//! Checks the GPU-side hand-off between a live run's GPU and the window on Rune's Metal backend
//! (`opencode_widget::gpu::Handoff`, `shader::handoff`): a run replays on a stock wgpu Metal device
//! made on Rune's `MTLDevice` and its frames reach a window on Rune's device through the canvas's
//! ring of slots, synchronized with shared events only. A replay thread clears a canvas to its
//! frame's number, a frame each; the window thread shows the newest slot, as the compositor does,
//! and reads it back in the same frame: every frame it shows must be one whole frame, never older
//! than one it showed before, and the last frame must show. At 120 Hz and with the replay going as
//! fast as it can; then the GPU time of a full-screen canvas's hand-off copy.
//! macOS only. `cargo run --release -p opencode-widget-view --example gpu_handoff_check [slots]`
//! (a ring of `shader::handoff::SLOTS` by default).
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
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::{MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLEvent, MTLSharedEvent, MTLSharedEventListener, MTLTexture};
    use opencode_widget::gpu::{self, Arg, Device, Handoff};
    use opencode_widget::live::gpu::RunGpu;
    use opencode_widget_view::shader::handoff::{Rings, SLOTS};
    use opencode_widget_view::shader::{MetalShare, RawEvent, RawTexture};
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
        ((rd, rq), Device { device, queue, format: wgpu::TextureFormat::Bgra8Unorm, handoff: None })
    }

    // The app's `unsafe` glue (`apps/desktop/src/session/widget_gpu.rs`), as the check needs it.
    fn raw_texture(t: &wgpu::Texture) -> Option<RawTexture> {
        let guard = unsafe { t.as_hal::<wgpu::hal::api::Metal>() }?;
        let raw: &ProtocolObject<dyn MTLTexture> = guard.raw_handle();
        unsafe { Retained::retain(raw as *const ProtocolObject<dyn MTLTexture> as *mut ProtocolObject<dyn MTLTexture>) }
    }

    fn copy(queue: &wgpu::Queue, from: &wgpu::Texture, to: &wgpu::Texture, wait: Option<(&RawEvent, u64)>, signal: (&RawEvent, u64)) -> bool {
        let Some(raw) = (unsafe { queue.as_hal::<wgpu::hal::api::Metal>() }).map(|q| objc2::Message::retain(q.as_raw())) else { return false };
        let (Some(from), Some(to)) = (raw_texture(from), raw_texture(to)) else { return false };
        objc2::rc::autoreleasepool(|_| {
            let Some(cb) = raw.commandBuffer() else { return false };
            if let Some((event, value)) = wait {
                cb.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&**event), value);
            }
            let Some(blit) = cb.blitCommandEncoder() else { return false };
            unsafe { blit.copyFromTexture_toTexture(&from, &to) };
            blit.endEncoding();
            cb.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&**signal.0), signal.1);
            cb.commit();
            true
        })
    }

    fn listener() -> &'static MTLSharedEventListener {
        static LISTENER: OnceLock<Retained<MTLSharedEventListener>> = OnceLock::new();
        LISTENER.get_or_init(MTLSharedEventListener::new)
    }

    fn notify(event: &RawEvent, value: u64, then: Box<dyn FnOnce() + Send>) {
        let then = Mutex::new(Some(then));
        let block = block2::RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLSharedEvent>>, _: u64| {
            if let Some(then) = then.lock().unwrap().take() {
                then();
            }
        });
        unsafe { event.notifyListener_atValue_block(listener(), value, block2::RcBlock::as_ptr(&block)) };
    }

    fn ops(v: serde_json::Value) -> Vec<Arg> {
        match Arg::from_json(&v) {
            Arg::List(l) => l,
            _ => Vec::new(),
        }
    }

    /// Slot `t` as a texture of Rune's device, as the compositor wraps it.
    fn wrap(t: &wgpu::Texture) -> wgpu::Texture {
        let raw = raw_texture(t).unwrap();
        let desc = wgpu::TextureDescriptor { label: None, size: t.size(), mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: t.format(), usage: t.usage(), view_formats: &[] };
        rune_gpu::metal::texture_from_raw(raw, &desc, Arc::new(t.clone()))
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

    /// Frame `i` of the replay: canvas `c` cleared to its number (red: low byte, green: high).
    fn frame(i: u32) -> Vec<Arg> {
        let color = [f64::from(i % 256) / 255.0, f64::from((i / 256) % 256) / 255.0, 0.0, 1.0];
        ops(json!([
            ["pass", { "colorAttachments": [{ "view": 2, "resolveTarget": null, "clearValue": color, "loadOp": "clear", "storeOp": "store" }], "depthStencilAttachment": null }],
            ["end"],
            ["submit"],
        ]))
    }

    /// What the window saw: frames replayed, window frames that showed a slot, wrong ones (not
    /// one whole frame, older than one shown before, or not replayed yet) and the last frame shown.
    struct Seen {
        frames: u32,
        shown: u32,
        wrong: u32,
        last: Option<u32>,
        /// The most frames the one shown was behind the replay.
        behind: u32,
    }

    /// Frames replayed at `hz` (0: as fast as it goes, for a second), the window showing at 120 Hz.
    fn ordering(rd: &wgpu::Device, rq: &wgpu::Queue, rings: &Rings, hz: f64, frames: u32) -> Seen {
        let (w, h) = (256u32, 64u32);
        let run = run(w, h);
        let canvases = run.canvases.clone();
        let done = Arc::new(AtomicBool::new(false));
        let replayed = Arc::new(AtomicU32::new(0));
        let replay = {
            let (run, done, replayed) = (run.clone(), done.clone(), replayed.clone());
            std::thread::spawn(move || {
                let start = Instant::now();
                for i in 0..frames {
                    if hz == 0.0 && start.elapsed() > Duration::from_secs(1) {
                        break;
                    }
                    assert!(run.apply(&frame(i), &[]).is_none(), "frame {i} replays");
                    run.end_frame();
                    replayed.store(i + 1, Ordering::Release);
                    if hz > 0.0 {
                        let next = start + Duration::from_secs_f64(f64::from(i + 1) / hz);
                        std::thread::sleep(next.saturating_duration_since(Instant::now()));
                    }
                }
                done.store(true, Ordering::Release);
            })
        };
        let read = rd.create_buffer(&wgpu::BufferDescriptor { label: None, size: u64::from(w * h * 4), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut wrapped: Option<((u64, usize), wgpu::Texture)> = None;
        let mut seen = Seen { frames: 0, shown: 0, wrong: 0, last: None, behind: 0 };
        let mut quiet = 0;
        loop {
            let finished = done.load(Ordering::Acquire);
            let before = replayed.load(Ordering::Acquire);
            canvases.retry();
            let mut enc = rd.create_command_encoder(&Default::default());
            let shown = rings.show(&canvases, "0/:c", &mut enc);
            if let Some(s) = &shown {
                if wrapped.as_ref().is_none_or(|(k, _)| *k != (s.generation, s.slot)) {
                    wrapped = Some(((s.generation, s.slot), wrap(&s.texture)));
                }
                let (_, t) = wrapped.as_ref().unwrap();
                enc.copy_texture_to_buffer(t.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &read, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: None } }, t.size());
            }
            rq.submit([enc.finish()]);
            if shown.is_some() {
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                rd.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                let px: Vec<u8> = read.slice(..).get_mapped_range().unwrap().to_vec();
                read.unmap();
                // BGRA: red is byte 2, green byte 1.
                let n = |p: &[u8]| u32::from(p[2]) | (u32::from(p[1]) << 8);
                let first = n(&px[0..4]);
                let whole = px.chunks(4).all(|p| n(p) == first);
                let older = seen.last.is_some_and(|l| first < l);
                if !whole || older || first >= replayed.load(Ordering::Acquire).max(1) {
                    seen.wrong += 1;
                }
                seen.behind = seen.behind.max(before.saturating_sub(first + 1));
                seen.last = Some(first);
                seen.shown += 1;
            }
            if finished {
                let last = replayed.load(Ordering::Acquire);
                if seen.last.is_some_and(|l| l + 1 == last) {
                    break;
                }
                quiet += 1;
                if quiet > 240 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_micros(8333));
        }
        replay.join().unwrap();
        seen.frames = replayed.load(Ordering::Acquire);
        seen
    }

    /// The GPU time of one hand-off copy of a `w` x `h` canvas.
    fn copy_cost(stock: &Device, rings_device: &ProtocolObject<dyn MTLDevice>, w: u32, h: u32) -> Duration {
        let desc = wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Bgra8Unorm, usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] };
        let (from, to) = (stock.device.create_texture(&desc), stock.device.create_texture(&desc));
        let event = rings_device.newSharedEvent().unwrap();
        assert!(copy(&stock.queue, &from, &to, None, (&event, 1)));
        assert!(event.waitUntilSignaledValue_timeoutMS(1, 5000));
        let n = 200;
        let t = Instant::now();
        for i in 0..n {
            assert!(copy(&stock.queue, &from, &to, None, (&event, 2 + i)));
        }
        assert!(event.waitUntilSignaledValue_timeoutMS(1 + n, 10000));
        t.elapsed() / n as u32
    }

    pub fn main() {
        let slots = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(SLOTS);
        let ((rd, rq), mut stock) = devices();
        println!("Rune: {}; runs on: {:?} {}; a ring of {slots} slots", rune_gpu::describe_gpu(&rd), stock.device.adapter_info().backend, stock.device.adapter_info().name);
        let share = MetalShare { stock: |_| None, raw_texture, copy, notify };
        let raw_device = objc2::Message::retain(rune_gpu::metal::raw_device(&rd).unwrap());
        let rings = Arc::new(Rings::new(share, &raw_device, slots));
        stock.handoff = Some(rings.clone() as Arc<dyn Handoff>);
        let plain = stock.clone();
        gpu::provide(move || Some(stock.clone()));
        let mut bad = false;
        for (hz, frames, label) in [(120.0, 300, "at 120 Hz"), (0.0, u32::MAX, "as fast as it replays, for a second")] {
            let (copies, behind) = rings.stats();
            let s = ordering(&rd, &rq, &rings, hz, frames);
            let (copies, behind) = (rings.stats().0 - copies, rings.stats().1 - behind);
            let ok = s.wrong == 0 && s.last.is_some_and(|l| l + 1 == s.frames);
            bad |= !ok;
            println!(
                "{}  {} frames {label}: the window showed a slot {} times, {} wrong, the last frame {}, at most {} frames behind the replay; {copies} hand-off copies, {behind} found no free slot",
                if ok { "ok  " } else { "FAIL" },
                s.frames,
                s.shown,
                s.wrong,
                if s.last.is_some_and(|l| l + 1 == s.frames) { "showed" } else { "never showed" },
                s.behind,
            );
        }
        let (w, h) = gpu::clamp_size(5120, 2880);
        let ms = copy_cost(&plain, &raw_device, w, h).as_secs_f64() * 1e3;
        println!("     a full-screen canvas ({w} x {h}): {ms:.3} ms a hand-off copy, on the run's queue ({:.1}% of a 120 Hz frame)", ms / (1000.0 / 120.0) * 100.0);
        if bad {
            std::process::exit(1);
        }
    }
}
