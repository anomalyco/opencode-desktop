use rune::{Cx, El, Image, Spring, SpringPair, SurfaceFit, Transition, el};

use opencode_kit::model::Attachment;

use super::identity;
use crate::TimelineHost;
use crate::palette::Pal;

// The viewer: Desktop's image preview (`@opencode/ui` `image-preview`). A click on a thumbnail
// grows it into the image, fitted to the window over a scrim; Escape or a click outside the
// image shrinks it back into its thumbnail.

pub struct Viewing {
    /// The thumbnail's `El::matched` id.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) image: Image,
}

/// Desktop flies on a 0.3 s spring launched with a push, as a spring from rest starts slowly;
/// from rest, 0.25 s lands on the same shape (the Timeline knob's default).
pub(crate) fn flight<S: TimelineHost>(s: &S) -> Spring {
    Spring::new(s.knobs().get(&crate::knobs::IMAGE_FLIGHT), 0.0)
}

/// A thumbnail that opens the viewer; while the viewer shows it, an empty box of its size (keyed
/// apart, so the thumbnail leaves the tree) holds its place, and the image flies out of it and
/// back into it.
pub fn opens<S: TimelineHost>(s: &S, thumb: El, held: El, id: String, f: &Attachment, image: Image) -> El {
    if s.media_store().viewing.as_ref().is_some_and(|v| v.id == id) {
        return held;
    }
    let name = f.name.clone();
    thumb.matched(id.clone()).token_motion(flight(s)).on_press(move |s: &mut S| {
        s.media_store_mut().viewing = Some(Viewing { id: id.clone(), name: name.clone(), image: image.clone() });
    })
}

/// Opens the viewer on `f` in the gallery keyed `key`, as pressing its thumbnail does (the
/// storybook's images story). False while its image isn't loaded.
pub fn view_image<S: TimelineHost>(s: &mut S, key: &str, f: &Attachment) -> bool {
    let Some(image) = s.media_store().image(f) else { return false };
    s.media_store_mut().viewing = Some(Viewing { id: format!("{key}:{}", identity(f)), name: f.name.clone(), image });
    true
}

/// Where the viewer shows an image of `size` pixels in a `w`×`h` window: a pixel a point, shrunk
/// to fit inside 56 points top and bottom and 3% of the width (20 to 48 points) at the sides,
/// centred.
pub fn shown(size: Option<[u32; 2]>, w: f32, h: f32) -> [f32; 4] {
    let side = (w * 0.03).clamp(8.0, 36.0) + 12.0;
    let (bw, bh) = ((w - 2.0 * side).max(1.0), (h - 2.0 * 56.0).max(1.0));
    let [iw, ih] = size.map_or([bw, bh], |[a, b]| [a.max(1) as f32, b.max(1) as f32]);
    let k = (bw / iw).min(bh / ih).min(1.0);
    let (sw, sh) = (iw * k, ih * k);
    [(w - sw) / 2.0, (h - sh) / 2.0, sw, sh]
}

/// The open viewer, over the whole window: a scrim, and the image.
pub fn viewer<S: TimelineHost>(s: &S, p: &Pal, cx: &Cx<S>) -> Vec<El> {
    let Some(v) = &s.media_store().viewing else { return Vec::new() };
    let [win_w, win_h] = cx.layout_size("window").unwrap_or([1280.0, 820.0]);
    let [x, y, w, h] = shown(v.image.size(), win_w, win_h);
    let fade = SpringPair::new(Spring::new(0.15, 0.0), Spring::new(0.12, 0.0));
    let scrim = el().key("image-viewer-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::opacity().springs(fade));
    let image = el()
        .key("image-viewer")
        .role("dialog")
        .label(v.name.clone())
        .matched(v.id.clone())
        .token_motion(flight(s))
        .size(w, h)
        .radius(6.0)
        .pixels_fit(v.image.pixels(), SurfaceFit::Fill)
        .transition(Transition::opacity().springs(fade))
        .modal(true, |s: &mut S| s.media_store_mut().viewing = None)
        .absolute(x, y);
    vec![scrim, image]
}
