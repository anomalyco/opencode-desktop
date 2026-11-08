use std::sync::Arc;

use opencode_kit::model::Attachment;
use rune::{Cx, El, Env, Runtime, col};
use rune_knobs::Knobs;

use super::{Dragged, file_name, write};
use crate::images::MediaStore;
use crate::palette::pal;
use crate::{TimelineHost, ViewState};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([200, 30, 40])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn jpeg() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])).write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

#[test]
fn a_dragged_file_is_named_for_the_image_with_the_bytes_own_extension() {
    let (p, j) = (png(2, 2), jpeg());
    assert_eq!(file_name("blood.png", &p), "blood.png");
    assert_eq!(file_name("photo.jpeg", &j), "photo.jpeg");
    assert_eq!(file_name("clipboard", &p), "clipboard.png");
    assert_eq!(file_name("shot.png", &j), "shot.jpg", "the bytes decide the type");
    assert_eq!(file_name("", &p), "image.png");
    assert_eq!(file_name("https://example.com/a/drop.png?w=200#x", &p), "drop.png");
    assert_eq!(file_name("../../.hidden", &p), "hidden.png");
    assert_eq!(file_name("diagram.svg", b"<svg/>"), "diagram.svg", "an unsniffed type keeps its name");
}

#[test]
fn writing_a_drag_file_keeps_its_bytes_and_reuses_it() {
    let root = std::env::temp_dir().join(format!("opencode-drag-test-{}", std::process::id()));
    let bytes = png(3, 3);
    let path = write(&root, &bytes, "blood.png").unwrap();
    assert_eq!(path.file_name().unwrap(), "blood.png");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(write(&root, &bytes, "blood.png"), Some(path.clone()), "a second drag reuses the file");
    let other = write(&root, &png(4, 4), "blood.png").unwrap();
    assert_ne!(other, path, "another image with the same name gets its own folder");
    assert!(write(&root, &[], "x.png").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_ref_drags_its_landed_bytes() {
    let bytes = png(5, 5);
    let served = bytes.clone();
    let mut store = MediaStore::default();
    store.set_fetch(Arc::new(move |_: &str| -> Result<Vec<u8>, String> { Ok(served.clone()) }));
    let f = Attachment { mime: "image/png".into(), name: "drop.png".into(), media: Some("c".repeat(64)), ..Default::default() };
    assert!(store.drag_item(&Dragged::File(f.clone())).is_none(), "nothing to drag before the bytes land");
    for _ in 0..1000 {
        if store.image(&f).is_some() { break; }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let item = store.drag_item(&Dragged::File(f)).expect("landed bytes drag");
    assert_eq!(std::fs::read(&item.paths()[0]).unwrap(), bytes);
}

#[derive(Default)]
struct Host {
    knobs: Knobs,
    view: ViewState,
    files: Vec<Attachment>,
}

impl TimelineHost for Host {
    fn knobs(&self) -> &Knobs { &self.knobs }
    fn clock(&self) -> f64 { 0.0 }
    fn active_session_id(&self) -> Option<&str> { None }
    crate::delegate_view_state!(view);
}

fn app(s: &Host, _cx: &mut Cx<Host>) -> El {
    col().key("root").w_full().h_full().children(crate::images::gallery(s, pal(true), "images", &s.files, false, None))
}

/// Kit dragged a tool's image toward Finder and nothing happened: every thumbnail is a drag
/// source carrying its image's file, and a press still opens the viewer.
#[test]
fn a_timeline_thumbnail_drags_out_as_its_image_file() {
    let bytes = png(40, 30);
    let f = Attachment { mime: "image/png".into(), name: "blood.png".into(), bytes: bytes.clone().into(), media: None };
    let h = Host { files: vec![f.clone()], ..Default::default() };
    h.view.media.image(&f).unwrap().wait().unwrap();
    let mut rt = Runtime::new(h, app, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let thumb = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("img-0")).expect("thumbnail");
    assert!(thumb.drag_out, "the thumbnail is a drag source");
    let item = rt.state.view.media.drag_item(&Dragged::File(f)).unwrap();
    assert_eq!(item.paths()[0].file_name().unwrap(), "blood.png");
    assert_eq!(std::fs::read(&item.paths()[0]).unwrap(), bytes);
    assert!(item.drops_within_app(), "the composer can take it");
}
