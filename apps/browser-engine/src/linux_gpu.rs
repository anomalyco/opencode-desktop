//! Windowless CEF renders on the session's GPU, through the display backend's buffer allocator.
//!
//! Only X11 Ozone hands `OnAcceleratedPaint` dmabufs: under Wayland Ozone (and headless) CEF
//! 156's frame capture can't allocate GPU buffers, and Wayland Ozone also turns Vulkan off. So
//! when the app imports dmabufs and an X server answers (XWayland on a Wayland session), the
//! engine uses X11 with ANGLE on Vulkan, and copies each frame into its own dmabufs
//! (`dmabuf_frames`). Otherwise frames come as BGRA paints, still rendered on the GPU, on the
//! session's own backend.
use std::sync::{Arc, OnceLock};

use cef::{CommandLine, ImplCommandLine};

use crate::dmabuf_frames::{Gpu, Transport};

/// The Ozone platform, chosen once before CEF starts.
#[derive(Clone, Copy, Debug)]
enum Ozone {
    /// X11 with dmabufs, on the app's render node.
    X11Dmabuf,
    X11,
    Wayland,
    /// No display answers: only the repo's headless tooling runs the engine like this (the app
    /// needs a display), and its pages paint on the CPU path.
    Headless,
}

static OZONE: OnceLock<Ozone> = OnceLock::new();

fn set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|s| !s.is_empty())
}

/// Whether the X server `DISPLAY` names accepts a connection (libxcb, loaded only to ask).
#[allow(unsafe_code)]
fn x_answers() -> bool {
    if !set("DISPLAY") {
        return false;
    }
    // SAFETY: dlopen/dlsym of libxcb's stable C API; the connection is closed before returning
    // and the library stays loaded (xcb keeps per-process state).
    unsafe {
        let lib = libc::dlopen(c"libxcb.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return false;
        }
        let (connect, has_error, disconnect) = (
            libc::dlsym(lib, c"xcb_connect".as_ptr()),
            libc::dlsym(lib, c"xcb_connection_has_error".as_ptr()),
            libc::dlsym(lib, c"xcb_disconnect".as_ptr()),
        );
        if connect.is_null() || has_error.is_null() || disconnect.is_null() {
            return false;
        }
        let connect: unsafe extern "C" fn(*const libc::c_char, *mut i32) -> *mut libc::c_void = std::mem::transmute(connect);
        let has_error: unsafe extern "C" fn(*mut libc::c_void) -> i32 = std::mem::transmute(has_error);
        let disconnect: unsafe extern "C" fn(*mut libc::c_void) = std::mem::transmute(disconnect);
        let connection = connect(std::ptr::null(), std::ptr::null_mut());
        let ok = !connection.is_null() && has_error(connection) == 0;
        if !connection.is_null() {
            disconnect(connection);
        }
        ok
    }
}

/// Chooses the platform; returns the render node to copy frames on, when the app imports
/// dmabufs (`--dmabuf-device`) and an X server answers.
pub fn plan(node: Option<String>) -> Option<String> {
    let x = x_answers();
    let ozone = match (&node, x, set("WAYLAND_DISPLAY")) {
        (Some(_), true, _) => Ozone::X11Dmabuf,
        (_, _, true) => Ozone::Wayland,
        (_, true, false) => Ozone::X11,
        (_, false, false) => {
            eprintln!("browser engine: no display answers (WAYLAND_DISPLAY unset, DISPLAY {}); headless ozone", if set("DISPLAY") { "doesn't answer" } else { "unset" });
            Ozone::Headless
        }
    };
    let _ = OZONE.set(ozone);
    node.filter(|_| matches!(ozone, Ozone::X11Dmabuf))
}

fn ozone() -> Ozone {
    OZONE.get().copied().unwrap_or(Ozone::Headless)
}

/// The transport the engine starts with, from its EGL context's start.
pub fn transport(gpu: Option<Result<Arc<Gpu>, String>>) -> Transport {
    let ozone = ozone();
    match gpu {
        Some(Ok(gpu)) => {
            eprintln!("browser engine: ozone={ozone:?} transport=dmabuf ({})", gpu.node);
            Transport::Dmabuf(gpu)
        }
        Some(Err(reason)) => {
            eprintln!("browser engine: ozone={ozone:?} transport=bgra ({reason})");
            Transport::Bgra
        }
        None => {
            eprintln!("browser engine: ozone={ozone:?} transport=bgra");
            Transport::Bgra
        }
    }
}

pub fn configure(cmd: &mut CommandLine) {
    let value = |key: &str, val: &str| cmd.append_switch_with_value(Some(&key.into()), Some(&val.into()));
    let platform = match ozone() {
        Ozone::X11Dmabuf | Ozone::X11 => "x11",
        Ozone::Wayland => "wayland",
        Ozone::Headless => "headless",
    };
    value("ozone-platform", platform);
    value("use-gl", "angle");
    value("use-angle", "vulkan");
    // ANGLE and the compositor share one Vulkan device: with two, X11's WebGL canvases reached
    // the frame black (WebGPU's were fine); without Vulkan, WebGPU has no adapter.
    value("enable-features", "Vulkan,WebGPUService,VulkanFromANGLE,DefaultANGLEVulkan");
    cmd.append_switch(Some(&"disable-software-rasterizer".into()));
}
