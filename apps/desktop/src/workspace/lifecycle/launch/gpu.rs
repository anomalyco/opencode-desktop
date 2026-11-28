//! The GPU backend this Mac can draw with, chosen before Rune's prewarm.
//!
//! Rune's native Metal backend signals command-buffer completion on a `MTLSharedEvent` and waits
//! on it from the CPU (`rune-gpu`'s `submit.rs` reads `signaledValue` and `waitUntilSignaledValue`).
//! A Metal driver without shared events — an older Intel GPU, or one patched to run a newer macOS
//! — makes `newSharedEvent` return nil, and `rune-gpu` reaches for it with `expect`: the launch
//! dies as a panic on the prewarm thread, then again on the main one, before a window exists and
//! without a word about why. wgpu's Metal backend signals completion itself and needs no shared
//! event, and Rune already lets a launch pick it (`RUNE_GPU_BACKEND=wgpu`, no rebuild). This does
//! the picking for such a machine, so the app draws instead of saying nothing.
//!
//! The probe is one `MTLCreateSystemDefaultDevice` (31 ms on an Iris Pro) plus the event; Rune's
//! prewarm would create that same device moments later and finds it cached. A launch that already
//! names a backend keeps its choice.

use std::sync::OnceLock;

use rune::GpuBackend;

/// What `select` chose; unset until it has run.
static SELECTED: OnceLock<GpuBackend> = OnceLock::new();

/// Picks the backend for this machine, once, and returns it. Call it before Rune's prewarm and
/// before the first window, so both the prewarm and the window's app agree on the backend.
pub fn select() -> GpuBackend {
    let backend = if std::env::var_os("RUNE_GPU_BACKEND").is_some() {
        // Named by the launch, as `rune_app`'s doc says: not second-guessed here.
        GpuBackend::Metal
    } else if missing_shared_event() {
        eprintln!(
            "opencode-desktop: this Mac's Metal driver provides no shared events, which Rune's \
             Metal backend signals on; drawing with wgpu instead"
        );
        GpuBackend::Wgpu
    } else {
        GpuBackend::Metal
    };
    *SELECTED.get_or_init(|| backend)
}

/// The backend `select` picked (Metal until it has run, which is what every launch used before).
pub fn selected() -> GpuBackend {
    *SELECTED.get().unwrap_or(&GpuBackend::Metal)
}

/// Whether this Mac's default Metal device has no shared events. A machine with no default device
/// at all answers `false`: Rune's prewarm is what reports that, and it has the better sentence.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
#[allow(unsafe_code)]
fn missing_shared_event() -> bool {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    #[link(name = "Metal", kind = "framework")]
    unsafe extern "C" {
        fn MTLCreateSystemDefaultDevice() -> *mut AnyObject;
    }

    // SAFETY: `MTLCreateSystemDefaultDevice` returns a +1 device or null, and `newSharedEvent` is
    // sent to that device only when it is not null. Both are read and dropped on this stack; the
    // device's one leaked reference is the driver's own singleton, kept alive to the process's end.
    unsafe {
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            return false;
        }
        let event: *mut AnyObject = msg_send![device, newSharedEvent];
        event.is_null()
    }
}

#[cfg(not(all(target_os = "macos", not(target_arch = "wasm32"))))]
fn missing_shared_event() -> bool {
    false
}
