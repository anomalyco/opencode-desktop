use std::path::PathBuf;

use opencode_kit::model::Attachment;
use rune::{Cx, El, Env, Runtime, col};
use rune_knobs::Knobs;

use super::{COPY, OPEN, SAVE, clipboard};
use crate::images::Dragged;
use crate::palette::pal;
use crate::{TimelineHost, ViewState};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 40])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[derive(Default)]
struct Host {
    knobs: Knobs,
    view: ViewState,
    files: Vec<Attachment>,
    opened: Vec<PathBuf>,
}

impl TimelineHost for Host {
    fn knobs(&self) -> &Knobs { &self.knobs }
    fn clock(&self) -> f64 { 0.0 }
    fn active_session_id(&self) -> Option<&str> { None }
    fn open_file(&mut self, path: PathBuf) { self.opened.push(path); }
    crate::delegate_view_state!(view);
}

fn app(s: &Host, _cx: &mut Cx<Host>) -> El {
    col().key("root").w_full().h_full().children(crate::images::gallery(s, pal(true), "images", &s.files, false, Some("msg_1")))
}

fn shown(bytes: &[u8]) -> (Runtime<Host>, Attachment) {
    let f = Attachment { mime: "image/png".into(), name: "blood.png".into(), bytes: bytes.to_vec().into(), media: None };
    let h = Host { files: vec![f.clone()], ..Default::default() };
    h.view.media.image(&f).unwrap().wait().unwrap();
    let mut rt = Runtime::new(h, app, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    (rt, f)
}

fn right_click(rt: &mut Runtime<Host>, key: &str) {
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).expect(key);
    let (x, y) = (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(0.1);
}

/// Kit right-clicked a timeline image and nothing happened: it opens Copy Image, Save Image
/// As… and Open in Preview at the pointer.
#[test]
fn right_clicking_an_image_offers_its_items() {
    let (mut rt, _) = shown(&png(40, 30));
    right_click(&mut rt, "img-0");
    let menu = rt.popup_menu().expect("a menu opens");
    for id in [COPY, SAVE, OPEN] {
        assert!(menu.find(id).is_some_and(|i| i.is_enabled()), "{id}");
    }
    rt.resolve_popup(Some(OPEN)).unwrap();
    rt.frame(0.2);
    let opened = rt.state.opened.clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].file_name().unwrap(), "blood.png");
}

#[test]
fn copy_image_carries_the_picture_and_its_file() {
    let bytes = png(8, 6);
    let (rt, f) = shown(&bytes);
    let content = clipboard(&rt.state, &Dragged::File(f)).expect("copyable");
    assert!(content.image().is_some(), "the picture itself");
    assert_eq!(content.files().len(), 1);
    assert_eq!(std::fs::read(&content.files()[0]).unwrap(), bytes);
}

#[test]
fn save_image_as_writes_the_bytes_where_the_panel_says() {
    let bytes = png(8, 6);
    let (mut rt, _) = shown(&bytes);
    right_click(&mut rt, "img-0");
    rt.resolve_popup(Some(SAVE)).unwrap();
    rt.frame(0.2);
    let (_, options) = rt.file_dialog().expect("a save panel");
    assert_eq!(options.file_name.as_deref(), Some("blood.png"));
    let to = std::env::temp_dir().join(format!("menu-test-{}.png", std::process::id()));
    rt.resolve_file_dialog(Some(to.clone())).unwrap();
    rt.frame(0.3);
    assert_eq!(std::fs::read(&to).unwrap(), bytes);
    let _ = std::fs::remove_file(to);
}
