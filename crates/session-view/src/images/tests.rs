use std::sync::Arc;

use opencode_kit::model::Attachment;
use sha2::{Digest, Sha256};

use super::{Fetched, MediaStore};

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([20, 40, 60])).write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn media(hash: &str) -> Attachment {
    Attachment { mime: "image/png".into(), name: "shot.png".into(), media: Some(hash.into()), ..Default::default() }
}

fn landed(store: &MediaStore, f: &Attachment) {
    for _ in 0..1000 {
        if !matches!(store.fetched(f), Some(Fetched::Loading) | None) { return; }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the fetch never finished");
}

/// A portrait screenshot (t1378: 1560×1728) sent as a ref held a 4:3 box for good, and showed
/// small inside it.
#[test]
fn a_refs_box_takes_the_images_own_shape_once_its_bytes_land() {
    let bytes = png(1560, 1728);
    let mut store = MediaStore::default();
    store.set_fetch(Arc::new(move |_: &str| -> Result<Vec<u8>, String> { Ok(bytes.clone()) }));
    let f = media(&"a".repeat(64));
    assert!(store.image(&f).is_none());
    assert_eq!(store.thumbnail_size(&f), (160.0, 120.0), "unknown until the bytes land");
    landed(&store, &f);
    assert!(store.image(&f).is_some());
    assert_eq!(store.thumbnail_size(&f), super::fit(780.0, 864.0, 320.0, 240.0));
}

/// The fetch thread wakes the app, and the app's poll must say something changed, or the
/// loading box stays until the next click (t1378).
#[test]
fn a_landed_fetch_is_reported_once_to_the_hosts_poll() {
    let mut store = MediaStore::default();
    store.set_fetch(Arc::new(|_: &str| -> Result<Vec<u8>, String> { Ok(png(4, 4)) }));
    let f = media(&"b".repeat(64));
    assert!(!store.poll());
    assert!(store.image(&f).is_none());
    landed(&store, &f);
    assert!(store.poll(), "the landed bytes change the view");
    assert!(!store.poll(), "once");
}

/// A live event carries a tool's image inline; a later page read sends the same image as a
/// `media:` ref. The image already on screen stays, with its shape, and nothing is fetched.
#[test]
fn a_ref_for_an_inline_image_already_shown_keeps_its_pixels_without_a_fetch() {
    let bytes: Arc<[u8]> = png(300, 600).into();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let mut store = MediaStore::default();
    store.set_fetch(Arc::new(|hash: &str| -> Result<Vec<u8>, String> { panic!("fetched {hash} though its bytes were on screen") }));
    let inline = Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: bytes.clone(), media: None };
    let shown = store.image(&inline).unwrap();
    shown.wait().unwrap();
    let size = store.thumbnail_size(&inline);
    let r = media(&hash);
    let swapped = store.image(&r).expect("no loading box");
    assert_eq!(swapped.id(), shown.id());
    assert!(swapped.is_ready());
    assert_eq!(store.thumbnail_size(&r), size);
    assert_eq!(store.fetched(&r), Some(Fetched::Decoded));
}

/// A phone's JPEG stored sideways with an EXIF quarter turn, as Rune draws it upright.
fn sideways_jpeg(w: u32, h: u32) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(w, h, image::Rgb([20, 40, 60])).write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    let jpeg = out.into_inner();
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut turned = jpeg[..2].to_vec();
    turned.extend_from_slice(&[0xff, 0xe1]);
    turned.extend_from_slice(&(app1.len() as u16 + 2).to_be_bytes());
    turned.extend_from_slice(&app1);
    turned.extend_from_slice(&jpeg[2..]);
    turned
}

/// Kit's capture (t1378): reply thumbnails letterboxed in frames bigger than the picture. Every
/// thumbnail's frame has the drawn picture's shape, inline or ref, tall, wide or turned.
#[test]
fn a_thumbnails_frame_has_the_shape_of_the_picture_it_draws() {
    let shots = [("image/png", png(1560, 1728)), ("image/png", png(1600, 1000)), ("image/png", png(400, 300)), ("image/png", png(90, 1400)), ("image/jpeg", sideways_jpeg(64, 48))];
    for (i, (mime, bytes)) in shots.into_iter().enumerate() {
        let inline = Attachment { mime: mime.into(), name: "shot".into(), bytes: bytes.clone().into(), media: None };
        let r = Attachment { media: Some(format!("{i:064}")), bytes: Arc::from([]), ..inline.clone() };
        let mut store = MediaStore::default();
        store.set_fetch(Arc::new(move |_: &str| -> Result<Vec<u8>, String> { Ok(bytes.clone()) }));
        store.image(&r);
        landed(&store, &r);
        for f in [&inline, &r] {
            let img = store.image(f).expect("decoded");
            img.wait().unwrap();
            let [pw, ph] = img.size().expect("drawn size");
            let (w, h) = store.thumbnail_size(f);
            let (frame, picture) = (w / h, pw as f32 / ph as f32);
            // The 24 pt floor only bends a sliver (90×1400), which Fill crops instead of padding.
            if w > 24.0 && h > 24.0 {
                assert!((frame - picture).abs() / picture < 0.01, "shot {i} ({}): frame {w}×{h} for a {pw}×{ph} picture", f.media.is_some());
            }
        }
    }
}
