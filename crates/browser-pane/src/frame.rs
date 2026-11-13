//! A tab keeps one frame handle for its lifetime. The socket reader and the retained Rune
//! element share it, including when a reconnect paints before the UI has read the tab list.

use rune::{El, NativeSurface, SurfaceFit, Waker};

#[cfg(not(target_os = "linux"))]
pub use rune::NativeSurface as TabSurface;

#[cfg(target_os = "linux")]
pub use linux::TabSurface;

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{Arc, Mutex};

    use rune::{NativeSurface, Pixels, Waker};

    #[derive(Clone)]
    pub enum TabSurface {
        Page { pixels: Pixels, observer: Arc<Mutex<Option<Waker>>> },
        Preview(NativeSurface),
    }

    impl std::fmt::Debug for TabSurface {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Page { pixels, .. } => pixels.fmt(f),
                Self::Preview(surface) => surface.fmt(f),
            }
        }
    }

    impl TabSurface {
        pub(crate) fn new() -> Self {
            Self::Page { pixels: Pixels::new(), observer: Arc::default() }
        }

        /// A frame observer for consumers such as the headless load timer. Rune itself watches
        /// the Pixels handle and coalesces changes for the windows that display it.
        pub fn set_waker(&self, waker: Waker) {
            match self {
                Self::Page { observer, .. } => *observer.lock().unwrap() = Some(waker),
                Self::Preview(surface) => surface.set_waker(waker),
            }
        }

        pub(crate) fn clear(&self) {
            match self {
                Self::Page { pixels, .. } => pixels.set(1, 1, vec![0; 4]),
                Self::Preview(surface) => surface.clear(),
            }
        }
    }
}

pub(crate) fn preview() -> TabSurface {
    let surface = NativeSurface::new();
    let _ = surface.push_bytes(rune::native_surface::synthetic(640, 360, 0), 1);
    #[cfg(target_os = "linux")]
    let surface = TabSurface::Preview(surface);
    surface
}

pub(crate) fn wake_window(surface: &TabSurface, waker: &Waker) {
    #[cfg(target_os = "linux")]
    if matches!(surface, TabSurface::Page { .. }) {
        return;
    }
    surface.set_waker(waker.clone());
}

pub(crate) fn draw(page: El, surface: &TabSurface) -> El {
    #[cfg(target_os = "linux")]
    let surface = match surface {
        TabSurface::Page { pixels, .. } => return page.pixels_fit(pixels, SurfaceFit::Fill),
        TabSurface::Preview(surface) => surface,
    };
    page.native_surface(surface, SurfaceFit::Fill)
}

#[cfg(feature = "engine")]
pub(crate) fn push_bgra(surface: &TabSurface, width: u32, height: u32, bgra: Vec<u8>, frame_id: u64) {
    #[cfg(target_os = "linux")]
    let surface = match surface {
        TabSurface::Page { pixels, observer } => {
            // CEF paints onto the engine's opaque background; Rune streams top-down RGBA.
            let mut rgba = bgra;
            for pixel in rgba.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            pixels.set(width, height, rgba);
            let wake = observer.lock().unwrap().clone();
            if let Some(wake) = wake {
                wake();
            }
            return;
        }
        TabSurface::Preview(surface) => surface,
    };
    use rune::native_surface::{ByteFrame, ColorInfo, Format};
    let frame = ByteFrame { width, height, format: Format::Bgra, color: ColorInfo::default(), planes: [bgra, Vec::new()] };
    let _ = surface.push_bytes(frame, frame_id);
}
