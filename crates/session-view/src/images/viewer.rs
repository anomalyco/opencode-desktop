use rune::{Cx, El, Image, Spring, SpringPair, SurfaceFit, Transition, Tx, el};
use rune_kit::{ImageZoom, Preview};

use opencode_kit::model::Attachment;

use super::{Dragged, draggable};
use crate::TimelineHost;
use crate::palette::Pal;

// The viewer: Desktop's image preview (`@opencode/ui` `image-preview.tsx`). A click on any
// timeline image (an attachment, a tool's picture, a reply's markdown image) grows it out of its
// place into the image, fitted to the window over a scrim with the zoom bar under it
// (rune-kit's `Preview`: zoom, pan, keys); Escape, the close button or a click on the backdrop
// shrinks it back into its thumbnail.

pub struct Viewing {
    /// The thumbnail's `El::matched` id.
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) image: Image,
    /// What the open image's right-click menu acts on.
    pub(crate) dragged: Dragged,
}

/// The key of the empty box that holds the viewed image's place: one shows at a time.
pub(crate) const HELD: &str = "image-viewer-held";

/// Desktop flies on a 0.3 s spring launched with a push, as a spring from rest starts slowly;
/// from rest, 0.25 s lands on the same shape (the Timeline knob's default).
pub(crate) fn flight<S: TimelineHost>(s: &S) -> Spring {
    Spring::new(s.knobs().get(&crate::knobs::IMAGE_FLIGHT), 0.0)
}

/// Opens the viewer on `v`, fitted, with focus on its close button (`image-preview.tsx:710-714`).
pub(crate) fn open<S: TimelineHost>(s: &mut S, v: Viewing, tx: &mut Tx) {
    let store = s.media_store_mut();
    store.zoom.reset();
    store.viewing = Some(v);
    tx.focus(rune_kit::image_zoom::CLOSE_KEY, false);
}

/// A thumbnail that opens the viewer; while the viewer shows it, an empty box of its size (keyed
/// apart, so the thumbnail leaves the tree) holds its place, and the image flies out of it and
/// back into it. Dragged past a few points, it goes out as its image's file instead.
pub fn opens<S: TimelineHost>(s: &S, thumb: El, held: El, id: String, f: &Attachment, image: Image, message: Option<&str>) -> El {
    if s.media_store().viewing.as_ref().is_some_and(|v| v.id == id) {
        return held.key(HELD);
    }
    let (name, dragged) = (f.name.clone(), Dragged::File(f.clone()));
    let thumb = super::menu::on_image::<S>(draggable::<S>(thumb, dragged.clone()), dragged.clone(), message);
    thumb.matched(id.clone()).token_motion(flight(s)).on_press_tx(move |s: &mut S, tx: &mut Tx| {
        open(s, Viewing { id: id.clone(), name: name.clone(), image: image.clone(), dragged: dragged.clone() }, tx);
    })
}

/// Opens the viewer on `f`, the `index`th image of the gallery keyed `key`, as pressing its
/// thumbnail does (the storybook's images story). False while its image isn't loaded.
pub fn view_image<S: TimelineHost>(s: &mut S, key: &str, index: usize, f: &Attachment, tx: &mut Tx) -> bool {
    let Some(image) = s.media_store().image(f) else { return false };
    open(s, Viewing { id: super::gallery_id(key, index), name: f.name.clone(), image, dragged: Dragged::File(f.clone()) }, tx);
    true
}

/// Where the viewer shows an image of `size` pixels in a `w`×`h` window at fit: a pixel a point,
/// shrunk to fit inside 56 points top and bottom and 3% of the width (20 to 48 points) at the
/// sides, centred.
pub fn shown(size: Option<[u32; 2]>, w: f32, h: f32) -> [f32; 4] {
    let side = (w * 0.03).clamp(8.0, 36.0) + 12.0;
    let (bw, bh) = ((w - 2.0 * side).max(1.0), (h - 2.0 * 56.0).max(1.0));
    let [iw, ih] = size.map_or([bw, bh], |[a, b]| [a.max(1) as f32, b.max(1) as f32]);
    let k = (bw / iw).min(bh / ih).min(1.0);
    let (sw, sh) = (iw * k, ih * k);
    [(w - sw) / 2.0, (h - sh) / 2.0, sw, sh]
}

/// Steps the open image's zoom glide (the app's poll).
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    s.media_store_mut().zoom.poll(now, tx)
}

fn close<S: TimelineHost>(s: &mut S, _: &mut Tx) {
    s.media_store_mut().viewing = None;
}

/// Whether the image's place shows whole in the timeline. Then it flies home over the header
/// and composer, as it flew out; a place scrolled under them takes it home inside the
/// timeline's clip, so it slides under the edge instead of landing unseen.
fn home_in_view<S: TimelineHost>(s: &S, cx: &Cx<S>) -> bool {
    let Some(session) = s.active_session_id() else { return false };
    let (Some(home), Some(port)) = (cx.window_layout(HELD), cx.window_layout(&format!("scroll-{session}"))) else { return false };
    home[0] >= port[0] - 0.5 && home[1] >= port[1] - 0.5 && home[0] + home[2] <= port[0] + port[2] + 0.5 && home[1] + home[3] <= port[1] + port[3] + 0.5
}

fn zoom<S: TimelineHost>(s: &mut S) -> &mut ImageZoom {
    &mut s.media_store_mut().zoom
}

/// The open viewer, over the whole window: a scrim, and the preview.
pub fn viewer<S: TimelineHost>(s: &S, p: &Pal, cx: &Cx<S>) -> Vec<El> {
    let Some(v) = &s.media_store().viewing else { return Vec::new() };
    let window = cx.layout_size("window").unwrap_or([1280.0, 820.0]);
    let fade = SpringPair::new(Spring::new(0.15, 0.0), Spring::new(0.12, 0.0));
    let scrim = el().key("image-viewer-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::opacity().springs(fade));
    let natural = v.image.size().map_or([1.0, 1.0], |[w, h]| [w as f32, h as f32]);
    // With its place off screen there's nothing to fly from: it fades and grows in instead.
    let arrive = Transition::scale(0.96).springs(fade);
    let image = super::menu::on_image::<S>(el(), v.dragged.clone(), None)
        .key("image-viewer")
        .role("img")
        .label(v.name.clone())
        .matched(v.id.clone())
        .token_motion(flight(s))
        .pixels_fit(v.image.pixels(), SurfaceFit::Fill)
        .transition(arrive);
    let store = s.media_store();
    let unclipped = home_in_view(s, cx);
    let preview = Preview::new(zoom::<S>, natural, image)
        .icons(&rune_theme_opencode::icons::MINUS, &rune_theme_opencode::icons::PLUS)
        .labels(
            opencode_i18n::t!("ui.imagePreview.zoomOut"),
            opencode_i18n::t!("ui.imagePreview.zoomToFit"),
            opencode_i18n::t!("ui.imagePreview.zoomIn"),
            opencode_i18n::t!("ui.common.close"),
        )
        .format_level(|n| opencode_i18n::percent(f64::from(n) / 100.0))
        .controls_transition(Transition::opacity().springs(fade))
        .on_close(move |s: &mut S, tx: &mut Tx| {
            if unclipped {
                tx.unclipped_flights();
            }
            close(s, tx)
        })
        .build(&store.zoom, crate::palette::kit(p), window, cx.is_reduced_motion())
        .role("dialog")
        .label(v.name.clone());
    vec![scrim, preview]
}

