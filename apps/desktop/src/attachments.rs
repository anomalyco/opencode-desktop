//! Images on prompts: the composer's attachments (pasted or dropped), their chips, the images
//! user messages and tool results carry in the transcript, the local echo of a sent prompt, and
//! the viewer a thumbnail opens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use rune::{
    ClipboardContent, Color, Cx, El, Image, ImageCache, Spring, SpringPair, SurfaceFit, TextStyle, Theme, Transition, Tx, Waker, col, el,
    icon, row, text,
};
use rune_theme_opencode::icons as ic;

use opencode_kit::client::{FileInput, data_url};
use opencode_kit::model::{Attachment, Message, Transcript};

use crate::App;
use crate::palette::Pal;

/// The server's cap on one attachment.
const MAX_BYTES: usize = 20 << 20;
const SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "heic", "bmp"];

#[derive(Default)]
pub struct Attach {
    /// Waiting in the composer for the next send.
    pub files: Vec<Attachment>,
    /// A file drag is over the composer.
    pub drop_over: bool,
    /// Prompts shown before the server's copy: (session, local id, text).
    pub echoes: Vec<(String, String, String)>,
    /// The image open in the viewer.
    pub viewing: Option<Viewing>,
    waker: Option<Waker>,
    cache: RefCell<Option<ImageCache>>,
    /// Decoded handles by the bytes' address, so a view doesn't re-hash megabytes each frame.
    images: RefCell<HashMap<(usize, usize), (Arc<[u8]>, Image)>>,
}

impl Attach {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// The decoded image (PNG and JPEG; None for other types, which show as a named chip).
    pub fn image(&self, a: &Attachment) -> Option<Image> {
        if !matches!(a.mime.as_str(), "image/png" | "image/jpeg" | "image/jpg") {
            return None;
        }
        let key = (a.bytes.as_ptr() as usize, a.bytes.len());
        if let Some((_, img)) = self.images.borrow().get(&key) {
            return Some(img.clone());
        }
        let mut cache = self.cache.borrow_mut();
        let cache = cache.get_or_insert_with(|| ImageCache::new(self.waker.clone().unwrap_or_else(|| Arc::new(|| {}))));
        let img = cache.load(a.bytes.clone());
        self.images.borrow_mut().insert(key, (a.bytes.clone(), img.clone()));
        Some(img)
    }

    pub fn add(&mut self, a: Attachment) {
        if !self.files.iter().any(|f| f.bytes == a.bytes) {
            self.files.push(a);
        }
    }
}

pub fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

pub fn from_path(path: &Path) -> Result<Attachment, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("image").to_owned();
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() > MAX_BYTES {
        return Err(format!("{name} is over 20 MB"));
    }
    Ok(Attachment { mime: mime_for(path).into(), name, bytes: bytes.into() })
}

/// The prompt's `files`: each attachment inline as a data URL.
pub fn inputs(files: &[Attachment]) -> Vec<FileInput> {
    files.iter().map(|f| FileInput { uri: data_url(&f.mime, &f.bytes), name: f.name.clone(), ..Default::default() }).collect()
}

/// A paste into the composer: an image becomes an attachment (true: the editor skips it).
pub fn paste(s: &mut App, content: &ClipboardContent, _: &mut Tx) -> bool {
    let Some(img) = content.image() else { return false };
    let n = s.attach.files.len() + 1;
    s.attach.add(Attachment { mime: "image/png".into(), name: format!("pasted-image-{n}.png"), bytes: img.png().into() });
    true
}

pub fn drop_files(s: &mut App, paths: &[std::path::PathBuf], _: &mut Tx) {
    s.attach.drop_over = false;
    for p in paths {
        match from_path(p) {
            Ok(a) => s.attach.add(a),
            Err(e) => s.status = e,
        }
    }
}

/// Before merging a fetched page: settles the open echoes against the server's messages. An
/// echo whose id came back is done (the merge replaces it); one the server stored under another
/// id (a server that ignores the client id) is dropped once a user message with its text lands.
pub fn reconcile(echoes: &mut Vec<(String, String, String)>, t: &mut Transcript, fresh: &[Message]) {
    echoes.retain(|(session, id, body)| {
        if *session != t.session {
            return true;
        }
        if fresh.iter().any(|m| m.id() == id) {
            return false;
        }
        let known = |mid: &str| t.messages.iter().any(|m| m.id() == mid);
        let twin = fresh.iter().any(|m| matches!(m, Message::User { id: mid, text } if mid != id && text.trim() == body && !known(mid)));
        if twin {
            t.messages.retain(|m| m.id() != id);
        }
        !twin
    });
}

/// The composer's attachment chips: thumbnail, name and a remove button.
pub fn chips(s: &App, p: &Pal) -> Option<El> {
    if s.attach.files.is_empty() {
        return None;
    }
    let mut r = row().key("attachments").w_full().px(12.0).pt(12.0).gap(8.0).flex_wrap();
    for (i, f) in s.attach.files.iter().enumerate() {
        let thumb = match s.attach.image(f) {
            Some(img) => {
                let thumb = el().key("thumb").size(32.0, 32.0).radius(4.0).fill(p.layer3).pixels_fit(img.pixels(), SurfaceFit::Fill);
                opens(s, thumb, el().key("held").size(32.0, 32.0), format!("composer:{}", identity(f)), f, img)
            }
            None => el().key("thumb").size(32.0, 32.0).radius(4.0).fill(p.layer3).center().child(icon(&ic::FOLDER, 14.0).color(p.icon_faint)),
        };
        let remove = el()
            .key("remove")
            .role("button")
            .label(format!("Remove {}", f.name))
            .size(20.0, 20.0)
            .radius(4.0)
            .center()
            .hover(p.hover)
            .on_press(move |s: &mut App| {
                if i < s.attach.files.len() {
                    s.attach.files.remove(i);
                }
            })
            .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_muted));
        r = r.child(
            row()
                .key(format!("chip-{i}-{}", f.name))
                .h(40.0)
                .max_w(220.0)
                .pl(4.0)
                .pr(4.0)
                .gap(6.0)
                .center_y()
                .radius(8.0)
                .fill(p.layer2)
                .child(thumb)
                .child(text(f.name.clone()).font(SMALL).color(p.muted).truncate().shrink())
                .child(remove),
        );
    }
    Some(r)
}

/// Images in the transcript, at most 320×240 points (2× pixels), right-aligned for a user.
pub fn gallery(s: &App, p: &Pal, key: &str, files: &[Attachment], end: bool) -> Option<El> {
    let images: Vec<&Attachment> = files.iter().filter(|f| f.is_image()).collect();
    if images.is_empty() {
        return None;
    }
    let mut r = row().key(key).w_full().gap(8.0).flex_wrap();
    if end {
        r = r.justify_end();
    }
    for (i, f) in images.into_iter().enumerate() {
        let k = format!("img-{i}");
        r = r.child(match s.attach.image(f) {
            Some(img) => {
                let (w, h) = img.size().map_or((160.0, 120.0), |[w, h]| fit(w as f32 / 2.0, h as f32 / 2.0, 320.0, 240.0));
                let held = el().key(format!("{k}-held")).size(w, h);
                let thumb = el().key(k).label(f.name.clone()).size(w, h).radius(8.0).fill(p.layer2).border(0.5, p.border_muted);
                opens(s, thumb.pixels_fit(img.pixels(), SurfaceFit::Fill), held, format!("{key}:{}", identity(f)), f, img)
            }
            None => row().key(k).h(32.0).px(10.0).center_y().radius(8.0).fill(p.layer2).child(text(f.name.clone()).font(SMALL).color(p.muted)),
        });
    }
    Some(col().w_full().pb(8.0).child(r))
}

fn fit(w: f32, h: f32, max_w: f32, max_h: f32) -> (f32, f32) {
    let k = (max_w / w.max(1.0)).min(max_h / h.max(1.0)).min(1.0);
    ((w * k).max(24.0), (h * k).max(24.0))
}

// The viewer: Desktop's image preview (`@opencode/ui` `image-preview`). A click on a thumbnail
// grows it into the image, fitted to the window over a scrim; Escape or a click outside the
// image shrinks it back into its thumbnail.

pub struct Viewing {
    /// The thumbnail's `El::matched` id.
    id: String,
    name: String,
    image: Image,
}

/// Desktop flies on a 0.3 s spring launched with a push, as a spring from rest starts slowly;
/// from rest, 0.25 s lands on the same shape.
const FLIGHT: Spring = Spring::new(0.25, 0.0);

/// One attachment's bytes, so each thumbnail has its own shared-element id.
fn identity(f: &Attachment) -> String {
    format!("{:x}:{}", f.bytes.as_ptr() as usize, f.bytes.len())
}

/// A thumbnail that opens the viewer; while the viewer shows it, an empty box of its size (keyed
/// apart, so the thumbnail leaves the tree) holds its place, and the image flies out of it and
/// back into it.
fn opens(s: &App, thumb: El, held: El, id: String, f: &Attachment, image: Image) -> El {
    if s.attach.viewing.as_ref().is_some_and(|v| v.id == id) {
        return held;
    }
    let name = f.name.clone();
    thumb.matched(id.clone()).motion(FLIGHT).on_press(move |s: &mut App| {
        s.attach.viewing = Some(Viewing { id: id.clone(), name: name.clone(), image: image.clone() });
    })
}

/// Where the viewer shows an image of `size` pixels in a `w`×`h` window: a pixel a point, shrunk
/// to fit inside 56 points top and bottom and 3% of the width (20 to 48 points) at the sides,
/// centred.
fn shown(size: Option<[u32; 2]>, w: f32, h: f32) -> [f32; 4] {
    let side = (w * 0.03).clamp(8.0, 36.0) + 12.0;
    let (bw, bh) = ((w - 2.0 * side).max(1.0), (h - 2.0 * 56.0).max(1.0));
    let [iw, ih] = size.map_or([bw, bh], |[a, b]| [a.max(1) as f32, b.max(1) as f32]);
    let k = (bw / iw).min(bh / ih).min(1.0);
    let (sw, sh) = (iw * k, ih * k);
    [((w - sw) / 2.0).round(), ((h - sh) / 2.0).round(), sw, sh]
}

fn scrim(dark: bool) -> Color {
    static SCRIM: OnceLock<[Color; 2]> = OnceLock::new();
    let c = |dark| Theme::from_json(include_str!("../assets/theme.json"), dark).unwrap_or_else(|e| panic!("{e}")).color("overlay-simple-overlay-scrim");
    SCRIM.get_or_init(|| [c(false), c(true)])[dark as usize]
}

/// The open viewer, over the whole window: a scrim, and the image.
pub fn viewer(s: &App, p: &Pal, cx: &Cx<App>) -> Vec<El> {
    let Some(v) = &s.attach.viewing else { return Vec::new() };
    let win = cx.layout("window").unwrap_or([0.0, 0.0, 1280.0, 820.0]);
    let [x, y, w, h] = shown(v.image.size(), win[2], win[3]);
    let fade = SpringPair::new(Spring::new(0.15, 0.0), Spring::new(0.12, 0.0));
    let scrim = el().key("image-viewer-scrim").absolute(0.0, 0.0).w_full().h_full().fill(scrim(p.dark)).transition(Transition::opacity().springs(fade));
    let image = el()
        .key("image-viewer")
        .role("dialog")
        .label(v.name.clone())
        .matched(v.id.clone())
        .motion(FLIGHT)
        .size(w, h)
        .radius(6.0)
        .pixels_fit(v.image.pixels(), SurfaceFit::Fill)
        .transition(Transition::opacity().springs(fade))
        .modal(true, |s: &mut App| s.attach.viewing = None)
        .absolute(x, y);
    vec![scrim, image]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, text: &str) -> Message {
        Message::User { id: id.into(), text: text.into() }
    }

    #[test]
    fn the_servers_copy_replaces_the_echo() {
        let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "older"), user("msg_5", "hi")] };
        let mut echoes = vec![("s".to_owned(), "msg_5".to_owned(), "hi".to_owned())];
        let fresh = vec![user("msg_1", "older"), user("msg_5", "hi")];
        reconcile(&mut echoes, &mut t, &fresh);
        t.merge(fresh);
        assert!(echoes.is_empty());
        assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_5"]);
    }

    #[test]
    fn an_echo_under_another_id_drops_when_the_servers_lands() {
        let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "hi"), user("msg_9", "hi")] };
        let mut echoes = vec![("s".to_owned(), "msg_9".to_owned(), "hi".to_owned())];
        // An unrelated page first: the echo stays.
        reconcile(&mut echoes, &mut t, &[user("msg_1", "hi")]);
        assert_eq!(echoes.len(), 1);
        let fresh = vec![user("msg_1", "hi"), user("msg_7", "hi")];
        reconcile(&mut echoes, &mut t, &fresh);
        t.merge(fresh);
        assert!(echoes.is_empty());
        assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_7"]);
    }

    #[test]
    fn a_live_send_shows_one_message_under_the_id_it_sends() {
        use opencode_kit::client::{Server, Write};
        let (w, writes) = std::sync::mpsc::channel();
        let mut app = App::empty();
        app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        app.route = crate::Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("look at this");
        app.attach.add(Attachment { mime: "image/png".into(), name: "a.png".into(), bytes: Arc::from(&b"ab"[..]) });
        app.send(&mut Tx::default());
        let Ok(Write::Send { session, prompt }) = writes.try_recv() else { panic!("no prompt") };
        let id = prompt.id.clone().unwrap();
        assert_eq!((session.as_str(), prompt.text.as_str(), prompt.files.len()), ("ses_a", "look at this", 1));
        assert!(app.attach.files.is_empty());
        assert_eq!(opencode_kit::model::user_files(&id).len(), 1, "the echo shows the image");
        // The server's copy comes back under the same id: still one message.
        let t = app.transcripts.get_mut("ses_a").unwrap();
        let fresh = vec![user(&id, "look at this")];
        reconcile(&mut app.attach.echoes, t, &fresh);
        t.merge(fresh);
        assert_eq!(t.messages.len(), 1);
        assert!(app.attach.echoes.is_empty());
    }

    #[test]
    fn dropped_images_become_data_urls() {
        let dir = std::env::temp_dir().join(format!("oc-desktop-attach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Shot.PNG");
        std::fs::write(&path, b"ab").unwrap();
        let a = from_path(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!((a.mime.as_str(), a.name.as_str()), ("image/png", "Shot.PNG"));
        assert_eq!(inputs(&[a]), [FileInput { uri: "data:image/png;base64,YWI=".into(), name: "Shot.PNG".into(), ..Default::default() }]);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&vec![200; (w * h * 4) as usize]).unwrap();
        out
    }

    fn rect(rt: &rune::Runtime<App>, path: &str) -> Option<[f32; 4]> {
        rt.elements().into_iter().find(|e| e.path.ends_with(path) && !e.exiting).map(|e| e.rect)
    }

    /// Every edge heads one way from `from` to `to`, never more than a fifth of the way in one
    /// frame, and lands.
    fn flies(frames: &[[f32; 4]], from: [f32; 4], to: [f32; 4]) {
        let mut last = from;
        for (n, f) in frames.iter().enumerate() {
            for k in 0..4 {
                let (travel, step) = (to[k] - from[k], f[k] - last[k]);
                assert!(step * travel.signum() >= -0.01, "frame {n} edge {k} turns back: {last:?} -> {f:?}");
                assert!(step.abs() <= travel.abs() * 0.2 + 0.5, "frame {n} edge {k} jumps {step} of {travel}");
            }
            last = *f;
        }
        for k in 0..4 {
            assert!((last[k] - to[k]).abs() < 0.5, "lands: {last:?} vs {to:?}");
        }
    }

    #[test]
    fn a_thumbnail_grows_into_the_viewer_and_escape_shrinks_it_back() {
        use rune::{Env, Key, KeyEvent, Mods, Runtime};
        let mut app = App::empty();
        app.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut app);
        app.finish_stream();
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.on_key = Some(crate::on_key);
        rt.poll = Some(crate::poll);
        rt.frame(0.0);
        let a = Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: png(1600, 1000).into() };
        let added = a.clone();
        rt.update(move |s, _| s.attach.add(added));
        rt.state.attach.image(&a).unwrap().wait().unwrap();
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let chip = "chip-0-shot.png/thumb";
        let thumb = rect(&rt, chip).unwrap();
        let (x, y) = rt.find(chip).unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        let mut open = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            open.push(rect(&rt, "/image-viewer").expect("the viewer is open"));
        }
        assert!(rect(&rt, chip).is_none(), "the thumbnail flew out of the chip");
        let window = rect(&rt, "/window").unwrap();
        let large = shown(Some([1600, 1000]), window[2], window[3]);
        assert!(large[2] > 1000.0, "fitted to the window: {large:?}");
        flies(&open, thumb, large);
        // 0.25 s: most of the way by 15 frames.
        assert!((open[14][2] - large[2]).abs() < (large[2] - thumb[2]) * 0.1, "still far at 0.25 s: {:?}", open[14]);
        assert!(rect(&rt, "/image-viewer-scrim").is_some());

        rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
        let mut close = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            close.push(rect(&rt, chip).expect("the thumbnail is back"));
        }
        assert!(rt.state.attach.viewing.is_none());
        assert!(rect(&rt, "/image-viewer").is_none());
        assert!(matches!(rt.state.route, crate::Route::Session(_)), "Escape only closes the viewer");
        flies(&close, large, thumb);
    }

    #[test]
    fn a_sent_image_opens_from_the_transcript_and_a_click_outside_closes_it() {
        use opencode_kit::client::Server;
        use rune::{Env, Runtime};
        let (w, _writes) = std::sync::mpsc::channel();
        let mut app = App::empty();
        app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
        app.route = crate::Route::Session("ses_a".into());
        app.composer = rune::TextEdit::new("look");
        let a = Attachment { mime: "image/png".into(), name: "sent.png".into(), bytes: png(800, 400).into() };
        app.attach.add(a.clone());
        app.send(&mut Tx::default());
        let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
        rt.state.attach.image(&a).unwrap().wait().unwrap();
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        let thumb = rect(&rt, "/img-0").expect("the sent image shows in the transcript");
        let (x, y) = rt.find("img-0").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        let mut open = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            open.push(rect(&rt, "/image-viewer").unwrap());
        }
        let window = rect(&rt, "/window").unwrap();
        let large = shown(Some([800, 400]), window[2], window[3]);
        assert_eq!(large[2..], [800.0, 400.0], "a pixel a point when it fits");
        flies(&open, thumb, large);
        // A click on the image keeps it open; one on the scrim closes it.
        rt.pointer_down(window[2] / 2.0, window[3] / 2.0);
        rt.pointer_up(window[2] / 2.0, window[3] / 2.0);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.attach.viewing.is_some());
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        let mut close = Vec::new();
        for _ in 0..45 {
            rt.frame(1.0 / 60.0);
            close.push(rect(&rt, "/img-0").unwrap());
        }
        assert!(rt.state.attach.viewing.is_none());
        flies(&close, large, thumb);
    }

    #[test]
    fn fits_in_the_box() {
        assert_eq!(fit(1280.0, 800.0, 320.0, 240.0), (320.0, 200.0));
        assert_eq!(fit(100.0, 50.0, 320.0, 240.0), (100.0, 50.0));
    }
}
