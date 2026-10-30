use image::{Rgba, RgbaImage};

use super::*;
use crate::find::{Kind, candidates, data_url, icns_png, png_dims};

fn png(w: u32, h: u32, alpha: u8) -> Vec<u8> {
    let img = RgbaImage::from_pixel(w, h, Rgba([200, 40, 40, alpha]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    bytes
        .chunks(3)
        .flat_map(|c| {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            (0..4).map(move |i| if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' })
        })
        .collect()
}

fn palette(dark: bool) -> Palette {
    let c = |v: f32| Color { r: v, g: v * 0.5, b: v * 0.25, a: 1.0 };
    Palette { dark, avatars: std::array::from_fn(|i| (c(i as f32 / 10.0), c(i as f32 / 10.0 + 0.05))), fg: Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, tile: c(0.2), ring: c(0.3) }
}

#[test]
fn icns_picks_the_fitting_png() {
    let mut icns = b"icns\0\0\0\0".to_vec();
    for (ty, side) in [(b"ic07", 128), (b"ic09", 512), (b"ic08", 256)] {
        let data = png(side, side, 255);
        icns.extend_from_slice(ty);
        icns.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        icns.extend_from_slice(&data);
    }
    assert_eq!(png_dims(icns_png(&icns).unwrap()), Some((256, 256)));
}

#[test]
fn data_urls_decode_raster_and_svg() {
    let bytes = png(4, 4, 255);
    assert_eq!(data_url(&format!("data:image/png;base64,{}", b64(&bytes))), Some(Data::Raster(bytes)));
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"/>"#;
    assert_eq!(data_url(&format!("data:image/svg+xml;base64,{}", b64(svg))), Some(Data::Svg(svg.to_vec())));
    assert_eq!(data_url("data:image/svg+xml;utf8,%3Csvg%2F%3E"), Some(Data::Svg(b"<svg/>".to_vec())));
    assert_eq!(data_url("data:image/gif;base64,AAAA"), None);
    assert_eq!(data_url("https://opencode.ai/favicon.svg"), None);
}

#[test]
fn server_svg_icons_rasterize_at_the_size_they_show() {
    let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#54b3fd"/></svg>"##;
    let url = format!("data:image/svg+xml;base64,{}", b64(svg));
    let mut icons = Icons::default();
    icons.set_sync(true);
    let p = Project { id: "p", name: "rune", directory: "", image: Some(&url), color: "" };
    let _ = icons.view(&palette(true), p, 24.0);
    let found = icons.found(&p);
    let Some(Shown::Vector(bytes, hash)) = found else { panic!("an SVG icon") };
    assert_eq!(icons.vector(&bytes, hash, 24.0).size(), Some([48, 48]));
    assert_eq!(icons.vector(&bytes, hash, 16.0).size(), Some([32, 32]));
}

#[test]
fn finds_the_best_square_icon() {
    let root = std::env::temp_dir().join(format!("project-icon-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("public")).unwrap();
    std::fs::create_dir_all(root.join("App/Assets.xcassets/AppIcon.appiconset")).unwrap();
    std::fs::write(root.join("public/favicon.png"), png(32, 32, 255)).unwrap();
    std::fs::write(root.join("logo.png"), png(600, 200, 255)).unwrap();
    std::fs::write(root.join("App/Assets.xcassets/AppIcon.appiconset/icon_128.png"), png(128, 128, 255)).unwrap();
    std::fs::write(root.join("App/Assets.xcassets/AppIcon.appiconset/icon_512.png"), png(512, 512, 255)).unwrap();
    let mut c = candidates(&root, "demo");
    c.sort_by_key(|c| c.rank());
    assert_eq!(c[0].kind, Kind::App);
    assert!(c[0].file.as_ref().unwrap().ends_with("icon_512.png"));
    assert!(c.iter().any(|x| x.kind == Kind::Web));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn trim_centres_visible_pixels() {
    let mut img = RgbaImage::new(100, 100);
    for y in 10..90 {
        for x in 20..80 {
            img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        }
    }
    let b = Buf::from_rgba8(&img).trim();
    assert_eq!((b.w, b.h), (80, 80));
    assert!(b.coverage() > 0.7 && b.coverage() < 0.8);
}

#[test]
fn marks_are_stable_rounded_and_differ_by_name() {
    let p = palette(true);
    for style in Style::ALL {
        let look = Look::style(style);
        let a = styles::draw(&p, "rune", 3, &look, 16.0, 32);
        assert!(a == styles::draw(&p, "rune", 3, &look, 16.0, 32), "{style:?} is deterministic");
        assert_eq!(a[3], 0, "{style:?}: the corner stays clear");
        assert_eq!(a[(16 * 32 + 16) * 4 + 3], 255, "{style:?}: the middle is opaque");
        if style != Style::Desktop {
            let differs = ["scrawl", "motel", "seer"].iter().any(|n| a != styles::draw(&p, n, 3, &look, 16.0, 32));
            assert!(differs, "{style:?} varies by name");
        }
    }
    assert_eq!(Style::from_name("mesh"), Some(Style::Mesh));
}

#[test]
fn icon_files_in_the_directory_can_be_turned_off() {
    let root = std::env::temp_dir().join(format!("project-icon-repo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("apps/desktop/assets")).unwrap();
    // Seer's repo: the Rune app's icon two folders down, which isn't the project's own.
    let data = png(128, 128, 255);
    let mut icns = b"icns\0\0\0\0".to_vec();
    icns.extend_from_slice(b"ic07");
    icns.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
    icns.extend_from_slice(&data);
    std::fs::write(root.join("apps/desktop/assets/icon.icns"), icns).unwrap();
    let dir = root.to_string_lossy().into_owned();
    let p = Project { id: "p", name: "seer", directory: &dir, image: None, color: "" };
    let mut icons = Icons::default();
    icons.set_sync(true);
    assert!(icons.found(&p).is_some());
    let mut icons = Icons::default();
    icons.set_sync(true);
    icons.set_repo_icons(false);
    assert!(icons.found(&p).is_none(), "Desktop shows the letter tile when the server has no image");
    let _ = std::fs::remove_dir_all(&root);
}
