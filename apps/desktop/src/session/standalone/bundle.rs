//! A standalone widget as an app on the desktop (macOS): a widget dragged out of the timeline
//! carries a small `.app` to Finder, made for it as the drag starts. The app's icon is the
//! widget's picture (the drag's snapshot, `DragItem::on_image`, written before the drop), and its
//! executable is a launcher that starts this build of the app with the widget's link
//! (`standalone::parse`): a running app takes the link (`rune::single_instance`) and opens the
//! widget in a window of its own; otherwise the app starts with it.
//!
//! A bundle and not a document or a URL scheme alone: only a bundle shows its own icon on the
//! desktop without Finder's custom-icon metadata, and it works with a build that isn't installed
//! (an unbundled `target/debug` binary registers no document types or schemes).

use std::path::Path;

use sha2::{Digest, Sha256};

/// The drag carrying widget `title`'s app, which opens `link`. Made fresh in the app's cache
/// for each drag; `None` when it can't be written.
pub fn drag_item(link: &str, title: &str) -> Option<rune::DragItem> {
    let digest = Sha256::digest(link.as_bytes());
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let dir = crate::dirs::cache()?.join("widget-apps").join(&tag);
    // An earlier drag's app may have another name (the widget's title changed).
    let _ = std::fs::remove_dir_all(&dir);
    let app = dir.join(format!("{}.app", file_name(title)));
    if let Err(e) = write(&app, link, title, &tag) {
        eprintln!("opencode-rune: couldn't make the widget's app: {e}");
        return None;
    }
    let icon = app.join("Contents/Resources/widget.icns");
    Some(rune::DragItem::new().file(&app).on_image(move |image| {
        // Off the UI thread: Finder copies the app at the drop, well after this.
        let (png, icon) = (image.png_data().to_vec(), icon.clone());
        std::thread::spawn(move || {
            if let Err(e) = write_icon(&icon, &png) {
                eprintln!("opencode-rune: couldn't write the widget's icon: {e}");
            }
        });
    }))
}

/// The app at `app`: its `Info.plist` and its launcher.
fn write(app: &Path, link: &str, title: &str, tag: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let contents = app.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::create_dir_all(contents.join("Resources"))?;
    std::fs::write(contents.join("Info.plist"), plist(title, tag))?;
    let launcher = contents.join("MacOS/widget");
    std::fs::write(&launcher, script(link)?)?;
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755))
}

/// The bundle's identity: the widget's name, the icon, and no Dock icon of its own (the launcher
/// hands over and ends).
fn plist(title: &str, tag: &str) -> String {
    let title = xml(title);
    let id = format!("{}.widget.{tag}", crate::launch::product().family());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>{title}</string>
  <key>CFBundleDisplayName</key><string>{title}</string>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleExecutable</key><string>widget</string>
  <key>CFBundleIconFile</key><string>widget</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>LSUIElement</key><true/>
</dict>
</plist>
"#
    )
}

/// The launcher: this build of the app with the widget's link. An installed app (an executable
/// in its bundle) gets it through Launch Services (`open -a`), which hands it to the running app
/// or starts it; a build outside a bundle (`target/debug`) is started as it was (its server, its
/// own folders, `--dirs-root`), and hands the link to the running one itself (`rune::single_instance`).
fn script(link: &str) -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    let bundle = exe.parent().and_then(|macos| macos.parent()).and_then(|contents| contents.parent()).filter(|b| b.extension().is_some_and(|e| e == "app"));
    let command = match bundle {
        Some(bundle) => format!("exec /usr/bin/open -a {} {}", quote(&bundle.to_string_lossy()), quote(link)),
        None => {
            let mut args = vec![quote(&exe.to_string_lossy())];
            if let Some(server) = crate::arg("--server") {
                args.push("--server".to_owned());
                args.push(quote(&server));
            }
            if let Some(root) = std::env::var_os("RUNE_DIRS_ROOT") {
                args.push(quote(&format!("--dirs-root={}", root.to_string_lossy())));
            }
            args.push(quote(link));
            format!("{} >/dev/null 2>&1 &", args.join(" "))
        }
    };
    Ok(format!("#!/bin/sh\n# Opens a widget of OpenCode (Rune) in a window of its own.\n{command}\n"))
}

/// The widget's picture as the app's icon: `png` fit into the icon's square at each size.
fn write_icon(path: &Path, png: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let picture = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    let mut out = b"icns\0\0\0\0".to_vec();
    for (kind, size) in [(b"ic07", 128u32), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024)] {
        let mut bytes = Vec::new();
        square(&picture, size).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)?;
        out.extend_from_slice(kind);
        out.extend_from_slice(&u32::try_from(bytes.len() + 8)?.to_be_bytes());
        out.extend_from_slice(&bytes);
    }
    let len = u32::try_from(out.len())?.to_be_bytes();
    out[4..8].copy_from_slice(&len);
    let partial = path.with_extension("icns.partial");
    std::fs::write(&partial, out)?;
    std::fs::rename(partial, path)?;
    Ok(())
}

/// `picture` centred in a transparent `size` square, as large as fits less a margin.
fn square(picture: &image::RgbaImage, size: u32) -> image::RgbaImage {
    let room = f64::from(size) * 0.92;
    let (w, h) = (f64::from(picture.width().max(1)), f64::from(picture.height().max(1)));
    let scale = (room / w).min(room / h);
    let (sw, sh) = (((w * scale).round() as u32).max(1), ((h * scale).round() as u32).max(1));
    let scaled = image::imageops::resize(picture, sw, sh, image::imageops::FilterType::Lanczos3);
    let mut out = image::RgbaImage::new(size, size);
    image::imageops::overlay(&mut out, &scaled, i64::from((size - sw) / 2), i64::from((size - sh) / 2));
    out
}

/// A name Finder takes for the app: the widget's title less path separators, at most 60
/// characters.
fn file_name(title: &str) -> String {
    let name: String = title.chars().map(|c| if c == '/' || c == ':' || c.is_control() { '-' } else { c }).take(60).collect();
    let name = name.trim().trim_start_matches('.');
    if name.is_empty() { "Widget".to_owned() } else { name.to_owned() }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `s` in single quotes for `sh`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
