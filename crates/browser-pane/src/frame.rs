//! A tab keeps one frame handle for its lifetime. The socket reader and the retained Rune
//! element share it, including when a reconnect paints before the UI has read the tab list.

use rune::{El, NativeSurface, SurfaceFit, Waker};

pub use rune::NativeSurface as TabSurface;

pub(crate) fn preview() -> TabSurface {
    let surface = NativeSurface::new();
    surface.push_bytes(rune::native_surface::synthetic(640, 360, 0), 1).ok();
    surface
}

pub(crate) fn wake_window(surface: &TabSurface, waker: &Waker) {
    surface.set_waker(waker.clone());
}

pub(crate) fn draw(page: El, surface: &TabSurface) -> El {
    page.native_surface(surface, SurfaceFit::Fill)
}

#[cfg(feature = "engine")]
pub(crate) fn push_bgra(surface: &TabSurface, width: u32, height: u32, bgra: Vec<u8>, frame_id: u64) {
    use rune::native_surface::{ByteFrame, ColorInfo, Format};
    let frame = ByteFrame { width, height, format: Format::Bgra, color: ColorInfo::default(), planes: [bgra, Vec::new()] };
    surface.push_bytes(frame, frame_id).ok();
}
