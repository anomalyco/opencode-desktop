use std::sync::Arc;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el};
use rune_knobs::Knobs;

use crate::palette::pal;
use crate::timeline::markdown_view_find;
use crate::{TimelineHost, ViewState};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([40, 120, 200])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[derive(Default)]
struct Host {
    knobs: Knobs,
    view: ViewState,
    /// The height of the timeline the replies show in, low in the window, or none for no timeline.
    timeline: Option<f32>,
}

impl TimelineHost for Host {
    fn knobs(&self) -> &Knobs { &self.knobs }
    fn clock(&self) -> f64 { 0.0 }
    fn active_session_id(&self) -> Option<&str> { self.timeline.map(|_| "s1") }
    crate::delegate_view_state!(view);
}

/// The same picture in two replies, as in Kit's session that showed `after.png` twice.
const REPLY: &str = "Here it is:\n\n![After](/shots/after.png)\n";

fn app(s: &Host, cx: &mut Cx<Host>) -> El {
    let p = pal(true);
    let reply = |row: &str, message: &str| el().key(format!("{row}-box")).w(320.0).h(300.0).child(markdown_view_find(s, p, row, REPLY, 1.0, false, Some(message)));
    let replies = [reply("row-a", "msg_a"), reply("row-b", "msg_b")];
    let window = col().key("window").w_full().h_full();
    let window = match s.timeline {
        None => window.children(replies),
        Some(h) => window.child(el().h(400.0)).child(col().key("scroll-s1").w_full().h(h).clip_soft(0.0).children(replies)),
    };
    window.children(super::viewer(s, p, cx))
}

fn runtime() -> Runtime<Host> {
    runtime_with(None)
}

fn runtime_with(timeline: Option<f32>) -> Runtime<Host> {
    let h = Host { timeline, ..Host::default() };
    let bytes = png(1200, 800);
    h.view.media.markdown.set_waker(Arc::new(|| {}));
    h.view.media.markdown.set_fetch(Arc::new(move |_: &str, _: &str| Ok(bytes.clone())), Arc::new(|_: &str| Err("offline".into())));
    let mut rt = Runtime::new(h, app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(|s: &mut Host, now, tx| s.view.media.poll() | super::poll(s, now, tx));
    for _ in 0..400 {
        // The test's waker wakes nothing: ask for the view again.
        rt.update(|_, _| {});
        rt.frame(1.0 / 60.0);
        if images(&rt).len() == 2 { break; }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(images(&rt).len(), 2, "both replies show the picture");
    rt
}

/// The pictures the replies show, top to bottom.
fn images(rt: &Runtime<Host>) -> Vec<[f32; 4]> {
    pictures(rt).into_iter().map(|e| e.rect).collect()
}

fn pictures(rt: &Runtime<Host>) -> Vec<rune::ElementInfo> {
    rt.elements().into_iter().filter(|e| e.role == "button" && e.label.as_deref() == Some("After") && e.rect[2] > 1.0).collect()
}

/// Opens the first picture, lets it land, closes it, and gives the first frame of its way home.
fn first_frame_home(timeline: f32) -> rune::ElementInfo {
    let mut rt = runtime_with(Some(timeline));
    let first = images(&rt)[0];
    click(&mut rt, first);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    escape(&mut rt);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.view.media.viewing.is_none());
    pictures(&rt).remove(0)
}

/// Kit's picture went home invisible, then slid in cut off: its flight drew inside the timeline,
/// and it started above it, where the viewer showed it. A place in full view takes it home whole.
#[test]
fn a_picture_flies_home_over_the_timeline_edge_when_its_place_shows_whole() {
    let home = first_frame_home(400.0);
    assert!(home.rect[1] < 400.0, "it starts above the timeline: {:?}", home.rect);
    assert!(close_to(home.visible, home.rect, 0.5), "and shows whole: {:?} of {:?}", home.visible, home.rect);
}

/// A place the timeline cuts off keeps the flight inside the timeline, sliding under its edge
/// instead of landing whole where nothing shows.
#[test]
fn a_picture_whose_place_is_cut_off_flies_home_inside_the_timeline() {
    let home = first_frame_home(120.0);
    assert!(home.visible[3] < home.rect[3] - 1.0, "the timeline clips it: {:?} of {:?}", home.visible, home.rect);
}

fn viewer(rt: &Runtime<Host>) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("image-viewer")).map(|e| e.rect)
}

fn click(rt: &mut Runtime<Host>, r: [f32; 4]) {
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
}

fn escape(rt: &mut Runtime<Host>) {
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
}

fn close_to(a: [f32; 4], b: [f32; 4], slack: f32) -> bool {
    (0..4).all(|i| (a[i] - b[i]).abs() < slack)
}

/// Kit opened a picture that a later reply repeats and it jumped in from nowhere: each copy
/// flies from its own place, out and back.
#[test]
fn the_same_picture_in_two_replies_flies_from_the_one_clicked() {
    let mut rt = runtime();
    let [first, second] = images(&rt)[..] else { unreachable!() };
    click(&mut rt, first);
    rt.frame(1.0 / 60.0);
    let start = viewer(&rt).expect("the viewer opens");
    assert!((start[1] - first[1]).abs() < 20.0, "it leaves the first reply ({first:?}), not the second ({second:?}): {start:?}");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    escape(&mut rt);
    rt.frame(1.0 / 60.0);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(rt.state.view.media.viewing.is_none());
    assert_eq!(images(&rt), vec![first, second], "both back in place");
}

/// Zoomed in, the first Escape fits the picture, as Desktop's does; a second while the fit still
/// glides closes it, and it flies home from where it shows rather than landing at once.
#[test]
fn escape_fits_a_zoomed_picture_then_flies_it_home_mid_glide() {
    let mut rt = runtime();
    let first = images(&rt)[0];
    click(&mut rt, first);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let fitted = viewer(&rt).unwrap();
    rt.key(KeyEvent { key: Key::Command('='), mods: Mods { cmd: true, ..Mods::default() } });
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let zoomed = viewer(&rt).unwrap();
    assert!(zoomed[2] > fitted[2] * 1.3, "⌘ + zooms in: {fitted:?} → {zoomed:?}");
    escape(&mut rt);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.view.media.viewing.is_some(), "the first Escape fits it");
    escape(&mut rt);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.view.media.viewing.is_none(), "the second closes it");
    let leaving = images(&rt)[0];
    assert!(leaving[2] > fitted[2], "it starts home from where it showed, still gliding to fit: {leaving:?}");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(close_to(images(&rt)[0], first, 0.5), "and lands in its place");
}

/// Escape a few frames into the flight out turns it around, and the picture lands home.
#[test]
fn escape_mid_open_flies_it_back() {
    let mut rt = runtime();
    let first = images(&rt)[0];
    click(&mut rt, first);
    for _ in 0..4 { rt.frame(1.0 / 60.0); }
    escape(&mut rt);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(rt.state.view.media.viewing.is_none());
    assert!(viewer(&rt).is_none());
    assert!(close_to(images(&rt)[0], first, 0.5), "home: {:?} vs {first:?}", images(&rt)[0]);
}

/// The viewer takes the keyboard when it opens (its close button, `image-preview.tsx:710-714`),
/// so Escape reaches it whatever had focus.
#[test]
fn the_open_viewer_holds_the_keyboard() {
    let mut rt = runtime();
    let first = images(&rt)[0];
    click(&mut rt, first);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let focused = rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    assert_eq!(focused.as_deref(), Some(rune_kit::image_zoom::CLOSE_KEY));
}

/// A double-click on the fitted picture zooms it in about the pointer.
#[test]
fn a_double_click_zooms_a_fitted_picture_in() {
    let mut rt = runtime();
    let first = images(&rt)[0];
    click(&mut rt, first);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let fitted = viewer(&rt).unwrap();
    let (x, y) = (fitted[0] + fitted[2] / 2.0, fitted[1] + fitted[3] / 2.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(rt.state.view.media.viewing.is_some(), "a click on the image keeps it open");
    let full = viewer(&rt).unwrap();
    assert!(full[2] > fitted[2] * 1.1, "zoomed in: {fitted:?} → {full:?}");
}

/// Kit's Escape just after a double-click zoom snapped the picture home: the glide's last frames
/// made the close land at once. With the fit still gliding, a close flies it home.
#[test]
fn closing_as_a_double_click_zoom_settles_still_flies_home() {
    let mut rt = runtime();
    let first = images(&rt)[0];
    click(&mut rt, first);
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    let fitted = viewer(&rt).unwrap();
    let (x, y) = (fitted[0] + fitted[2] * 0.3, fitted[1] + fitted[3] * 0.4);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert!(rt.state.view.media.zoom.owns_escape(), "zoomed in");
    rt.update(|s, tx| super::close(s, tx));
    rt.frame(1.0 / 60.0);
    let leaving = images(&rt)[0];
    assert!(leaving[2] > fitted[2], "it starts home from its zoomed size rather than landing at once: {leaving:?}");
    for _ in 0..40 { rt.frame(1.0 / 60.0); }
    assert!(close_to(images(&rt)[0], first, 0.5));
}
