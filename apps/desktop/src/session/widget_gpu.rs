//! Live widgets' GPU canvases on Rune's Metal backend (`widgets-gpu.md`): Rune's backend validates
//! nothing, so runs replay on a stock wgpu Metal device made on Rune's own `MTLDevice` (wgpu's
//! validation), and their frames reach the window on the GPU, synchronized with shared events.
//! What that takes in `unsafe` (wgpu's hal, Metal blocks) lives here;
//! `opencode_widget_view::shader::share_metal` uses it.

/// Lets live GPU canvases share Rune's Metal device.
pub fn setup() {
    #[cfg(target_os = "macos")]
    os::share();
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod os {
    use std::ptr::NonNull;
    use std::sync::{Mutex, OnceLock};

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::{MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLEvent, MTLSharedEvent, MTLSharedEventListener, MTLTexture};

    use crate::session::widgets::shader::{GpuDevice, MetalShare, RawEvent, gpu_limits, share_metal};

    pub fn share() {
        share_metal(MetalShare { stock, raw_texture, copy, notify });
    }

    /// What `f` resolves to; wgpu's native futures are ready at once.
    fn block<F: std::future::Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    /// A stock wgpu Metal device on the `MTLDevice` at `raw` (Rune's), with the spec's limits.
    fn stock(raw: usize) -> Option<GpuDevice> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::METAL, ..wgpu::InstanceDescriptor::new_without_display_handle() });
        let adapters = block(instance.enumerate_adapters(wgpu::Backends::METAL));
        let on = |a: &wgpu::Adapter| {
            // SAFETY: the adapter's hal is only read (its MTLDevice's address) while it lives.
            unsafe { a.as_hal::<wgpu::hal::api::Metal>() }.is_some_and(|h| std::ptr::eq(h.raw_device() as *const ProtocolObject<dyn objc2_metal::MTLDevice> as *const u8, raw as *const u8))
        };
        let adapter = adapters.into_iter().find(on)?;
        let made = block(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("widget gpu"), required_limits: gpu_limits(), ..Default::default() }));
        let (device, queue) = made.map_err(|e| eprintln!("widget gpu: no device: {e}")).ok()?;
        Some(GpuDevice { device, queue, format: wgpu::TextureFormat::Bgra8Unorm, handoff: None })
    }

    /// A stock device texture's `MTLTexture`.
    fn raw_texture(t: &wgpu::Texture) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        // SAFETY: the hal texture is only read, to retain its MTLTexture, while `t` keeps it alive.
        let guard = unsafe { t.as_hal::<wgpu::hal::api::Metal>() }?;
        let raw: &ProtocolObject<dyn MTLTexture> = guard.raw_handle();
        // SAFETY: `raw` is a live MTLTexture; retaining it adds a reference the caller owns.
        unsafe { Retained::retain(raw as *const ProtocolObject<dyn MTLTexture> as *mut ProtocolObject<dyn MTLTexture>) }
    }

    /// Commits on `queue` a command buffer that waits for `wait`, copies `from` into `to` and
    /// signals `signal`, retaining what it uses.
    fn copy(queue: &wgpu::Queue, from: &wgpu::Texture, to: &wgpu::Texture, wait: Option<(&RawEvent, u64)>, signal: (&RawEvent, u64)) -> bool {
        // SAFETY: the hal queue is only read, to retain its MTLCommandQueue.
        let Some(raw) = (unsafe { queue.as_hal::<wgpu::hal::api::Metal>() }).map(|q| objc2::Message::retain(q.as_raw())) else { return false };
        let (Some(from), Some(to)) = (raw_texture(from), raw_texture(to)) else { return false };
        objc2::rc::autoreleasepool(|_| {
            let Some(cb) = raw.commandBuffer() else { return false };
            cb.setLabel(Some(objc2_foundation::ns_string!("widget gpu canvas hand-off")));
            if let Some((event, value)) = wait {
                cb.encodeWaitForEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&**event), value);
            }
            let Some(blit) = cb.blitCommandEncoder() else { return false };
            // SAFETY: two live textures of one device, of one size and format; the command buffer
            // retains both.
            unsafe { blit.copyFromTexture_toTexture(&from, &to) };
            blit.endEncoding();
            cb.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&**signal.0), signal.1);
            cb.commit();
            true
        })
    }

    /// The listener shared events call back on (its own dispatch queue).
    fn listener() -> &'static MTLSharedEventListener {
        static LISTENER: OnceLock<Retained<MTLSharedEventListener>> = OnceLock::new();
        LISTENER.get_or_init(MTLSharedEventListener::new)
    }

    /// Calls `then` once `event` reaches `value`.
    fn notify(event: &RawEvent, value: u64, then: Box<dyn FnOnce() + Send>) {
        let then = Mutex::new(Some(then));
        let block = block2::RcBlock::new(move |_: NonNull<ProtocolObject<dyn MTLSharedEvent>>, _: u64| {
            if let Some(then) = then.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                then();
            }
        });
        // SAFETY: Metal copies the block, and calls it once with the event and its value.
        unsafe { event.notifyListener_atValue_block(listener(), value, block2::RcBlock::as_ptr(&block)) };
    }
}
