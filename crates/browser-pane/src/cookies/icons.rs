//! The installed application of each browser `sources::list_sources` names, and its icon as the
//! OS draws it (official `profile/sources.ts:183-233` `applications`, `:535-605` `windowsIcons` /
//! `windowsAppPaths`, `profile/index.ts:693-712` `applicationIcons`): the executable's own 64 px
//! icon on Windows, the bundle's icon on macOS. Linux has no single file that carries a browser's
//! icon, so it has none there and the row keeps the globe, as official's does.

use std::path::{Path, PathBuf};

/// A browser's icon: straight-alpha RGBA pixels (Windows) or an encoded PNG (macOS).
pub enum AppIcon {
    Rgba { width: u32, height: u32, pixels: Vec<u8> },
    Png(Vec<u8>),
}

/// Where the browser `source_id` is installed, if anywhere this knows to look.
pub fn application(source_id: &str) -> Option<PathBuf> {
    let found = candidates(source_id).into_iter().find(|p| p.exists());
    #[cfg(windows)]
    let found = found.or_else(|| app_path_name(source_id).and_then(windows_app_path));
    found
}

/// The icon of the application at `path`; `None` when the OS has none for it.
pub fn application_icon(path: &Path) -> Option<AppIcon> {
    #[cfg(windows)]
    return windows_icon(path);
    #[cfg(target_os = "macos")]
    return bundle_icon(path);
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = path;
        None
    }
}

#[cfg(windows)]
fn candidates(id: &str) -> Vec<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let roots: Vec<PathBuf> = [std::env::var_os("ProgramFiles").map(PathBuf::from), std::env::var_os("ProgramFiles(x86)").map(PathBuf::from), local]
        .into_iter()
        .flatten()
        .collect();
    let parts: &[&str] = match id {
        "chrome" => &["Google", "Chrome", "Application", "chrome.exe"],
        "edge" => &["Microsoft", "Edge", "Application", "msedge.exe"],
        "helium" => &["imput", "Helium", "Application", "chrome.exe"],
        "brave" => &["BraveSoftware", "Brave-Browser", "Application", "brave.exe"],
        "vivaldi" => &["Vivaldi", "Application", "vivaldi.exe"],
        "opera" => &["Programs", "Opera", "opera.exe"],
        "chromium" => &["Chromium", "Application", "chrome.exe"],
        "firefox" => &["Mozilla Firefox", "firefox.exe"],
        "zen" => &["Zen Browser", "zen.exe"],
        "librewolf" => &["LibreWolf", "librewolf.exe"],
        "floorp" => &["Ablaze Floorp", "floorp.exe"],
        _ => &[],
    };
    if parts.is_empty() {
        return Vec::new();
    }
    roots.into_iter().map(|root| parts.iter().fold(root, |p, part| p.join(part))).collect()
}

#[cfg(target_os = "macos")]
fn candidates(id: &str) -> Vec<PathBuf> {
    let names: &[&str] = match id {
        "chrome" => &["Google Chrome"],
        "edge" => &["Microsoft Edge"],
        "helium" => &["Helium"],
        "arc" => &["Arc"],
        "brave" => &["Brave Browser"],
        "dia" => &["Dia"],
        "comet" => &["Comet"],
        "vivaldi" => &["Vivaldi"],
        "opera" => &["Opera"],
        "chromium" => &["Chromium"],
        "safari" => &["Safari"],
        "firefox" => &["Firefox"],
        "zen" => &["Zen", "Zen Browser"],
        "librewolf" => &["LibreWolf"],
        "floorp" => &["Floorp"],
        _ => &[],
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    names
        .iter()
        .flat_map(|name| {
            let bundle = format!("{name}.app");
            [Some(Path::new("/Applications").join(&bundle)), home.as_ref().map(|h| h.join("Applications").join(&bundle))]
        })
        .flatten()
        .collect()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn candidates(_id: &str) -> Vec<PathBuf> {
    Vec::new()
}

/// The program names Windows registers an install path under (`sources.ts:567-577`).
#[cfg(windows)]
fn app_path_name(id: &str) -> Option<&'static str> {
    Some(match id {
        "chrome" => "chrome.exe",
        "edge" => "msedge.exe",
        "brave" => "brave.exe",
        "vivaldi" => "vivaldi.exe",
        "opera" => "opera.exe",
        "firefox" => "firefox.exe",
        "zen" => "zen.exe",
        "librewolf" => "librewolf.exe",
        "floorp" => "floorp.exe",
        _ => return None,
    })
}

/// The program's "App Paths" entry, the user's first, then the machine's; it counts only when it is
/// an absolute path that ends in `name` (`sources.ts:583-605`).
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_app_path(name: &str) -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};

    let key = HSTRING::from(format!("Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{name}"));
    [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(|hive| {
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` writable bytes for the duration of the call, and the key and
        // value names are valid NUL-terminated wide strings.
        let status = unsafe {
            RegGetValueW(hive, &key, PCWSTR::null(), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes))
        };
        if status.is_err() {
            return None;
        }
        let len = (bytes as usize / 2).saturating_sub(1).min(buf.len());
        let value = String::from_utf16_lossy(&buf[..len]);
        let path = PathBuf::from(value.trim().trim_matches('"'));
        let named = path.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name));
        (path.is_absolute() && named && path.exists()).then_some(path)
    })
}

/// The executable's own 64 px icon (`PrivateExtractIcons`), which a row draws at 20 px and stays
/// sharp at 2x (`sources.ts:538-553`).
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_icon(path: &Path) -> Option<AppIcon> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};

    const SIZE: i32 = 64;
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.len() >= 260 {
        return None;
    }
    let mut name = [0u16; 260];
    name[..wide.len()].copy_from_slice(&wide);
    let mut icons = [HICON::default()];
    // SAFETY: `name` is a NUL-terminated MAX_PATH buffer and `icons` has room for the one icon asked for.
    let got = unsafe { PrivateExtractIconsW(&name, 0, SIZE, SIZE, Some(&mut icons), None, 0) };
    let icon = icons[0];
    if got == 0 || got == u32::MAX || icon.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    // SAFETY: `icon` is a live icon handle and `info` is a valid out pointer.
    let ok = unsafe { GetIconInfo(icon, &mut info) }.is_ok();
    let pixels = ok.then(|| {
        let mut bm = BITMAP::default();
        // SAFETY: `hbmColor` is the icon's colour bitmap and `bm` is large enough for a `BITMAP`.
        let read = unsafe { GetObjectW(HGDIOBJ(info.hbmColor.0), size_of::<BITMAP>() as i32, Some((&mut bm as *mut BITMAP).cast())) };
        if read == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        // SAFETY: a memory DC compatible with the screen; deleted below.
        let dc = unsafe { CreateCompatibleDC(None) };
        // SAFETY: `bgra` holds `w * h` 32-bit pixels, the size `bmi` describes, for the call.
        let lines = unsafe { GetDIBits(dc, info.hbmColor, 0, h as u32, Some(bgra.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) };
        // SAFETY: `dc` came from `CreateCompatibleDC` above and is deleted once.
        unsafe {
            let _ = DeleteDC(dc);
        }
        if lines != h {
            return None;
        }
        // An icon drawn without an alpha channel is opaque where it draws.
        let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
        for p in bgra.chunks_exact_mut(4) {
            p.swap(0, 2);
            if !has_alpha {
                p[3] = 255;
            }
        }
        Some(AppIcon::Rgba { width: w as u32, height: h as u32, pixels: bgra })
    });
    // SAFETY: the bitmaps `GetIconInfo` created belong to this call and the icon to `PrivateExtractIcons`;
    // each is released once.
    unsafe {
        if ok {
            let _ = DeleteObject(HGDIOBJ(info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
        }
        let _ = DestroyIcon(icon);
    }
    pixels.flatten()
}

/// The bundle's own icon: the `.icns` its `Info.plist` names (`CFBundleIconFile`, else `AppIcon`
/// or the only `.icns` in `Resources`), as the PNG it holds closest to 64 px. Read through the
/// bundle's real path, as official reads Safari's (`/Applications/Safari.app` is a symlink).
#[cfg(target_os = "macos")]
fn bundle_icon(path: &Path) -> Option<AppIcon> {
    let bundle = std::fs::canonicalize(path).ok()?;
    let resources = bundle.join("Contents").join("Resources");
    let named = std::fs::read_to_string(bundle.join("Contents").join("Info.plist")).ok().and_then(|plist| {
        let at = plist.find("<key>CFBundleIconFile</key>")?;
        let rest = &plist[at..];
        let start = rest.find("<string>")? + "<string>".len();
        let end = rest[start..].find("</string>")?;
        let name = rest[start..start + end].trim().to_owned();
        Some(if name.ends_with(".icns") { name } else { format!("{name}.icns") })
    });
    let file = named
        .map(|n| resources.join(n))
        .filter(|p| p.is_file())
        .or_else(|| Some(resources.join("AppIcon.icns")).filter(|p| p.is_file()))
        .or_else(|| {
            let icns: Vec<PathBuf> = std::fs::read_dir(&resources)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "icns"))
                .collect();
            (icns.len() == 1).then(|| icns[0].clone())
        })?;
    let data = std::fs::read(&file).ok().filter(|d| d.len() <= 16 * 1024 * 1024)?;
    icns_png(&data).map(AppIcon::Png)
}

/// The PNG entry of an `.icns` file closest to 64 px, preferring larger: `ic12` (64), `ic07`
/// (128), `ic13` (256), `ic08` (256), `ic11` (32), `ic09` (512).
#[cfg(target_os = "macos")]
fn icns_png(data: &[u8]) -> Option<Vec<u8>> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const ORDER: [&[u8; 4]; 6] = [b"ic12", b"ic07", b"ic13", b"ic08", b"ic11", b"ic09"];
    if data.len() < 8 || &data[..4] != b"icns" {
        return None;
    }
    let mut entries: Vec<(&[u8], &[u8])> = Vec::new();
    let mut at = 8;
    while at + 8 <= data.len() {
        let kind = &data[at..at + 4];
        let len = u32::from_be_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]) as usize;
        if len < 8 || at + len > data.len() {
            break;
        }
        entries.push((kind, &data[at + 8..at + len]));
        at += len;
    }
    ORDER
        .iter()
        .find_map(|want| entries.iter().find(|(k, body)| *k == &want[..] && body.starts_with(PNG)))
        .map(|(_, body)| body.to_vec())
}
