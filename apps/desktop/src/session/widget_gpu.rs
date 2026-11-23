//! Live widgets' GPU canvases on Rune's Metal backend (`widgets-gpu.md`): Rune's backend validates
//! nothing, so runs replay on a stock wgpu Metal device made on Rune's own `MTLDevice` (wgpu's
//! validation), and the window shows their textures through Rune's backend. What that takes in
//! `unsafe` (wgpu's hal) lives here; `opencode_widget_view::shader::share_metal` uses it.

/// Lets live GPU canvases share Rune's Metal device.
pub fn setup() {
    #[cfg(target_os = "macos")]
    os::share();
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod os {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_metal::MTLTexture;

    use crate::session::widgets::shader::{GpuDevice, MetalShare, gpu_limits, share_metal};

    pub fn share() {
        share_metal(MetalShare { stock, raw_texture });
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
        Some(GpuDevice { device, queue, format: wgpu::TextureFormat::Bgra8Unorm, copies: true })
    }

    /// A stock device texture's `MTLTexture`.
    fn raw_texture(t: &wgpu::Texture) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        // SAFETY: the hal texture is only read, to retain its MTLTexture, while `t` keeps it alive.
        let guard = unsafe { t.as_hal::<wgpu::hal::api::Metal>() }?;
        let raw: &ProtocolObject<dyn MTLTexture> = guard.raw_handle();
        // SAFETY: `raw` is a live MTLTexture; retaining it adds a reference the caller owns.
        unsafe { Retained::retain(raw as *const ProtocolObject<dyn MTLTexture> as *mut ProtocolObject<dyn MTLTexture>) }
    }
}
