//! Native actions for links in assistant Markdown. Follow the file plugin's resolve-link.ts
//! and browser policy; never reinterpret a remote session's paths on the client, never probe
//! UNC/mapped/subst/reparse paths before showing a menu, and never execute local files via the OS
//! default handler.
use crate::App;
use crate::files::{FilesHost, Link, links, video};
use crate::popup::{self, Item, Menu};
use browser_policy::{destination_origin, file_url_within, is_file_scheme};
use opencode_i18n::t;
use rune::{PointerPress, Tx};

fn target(app: &App, href: &str) -> Option<(String, Link)> {
    if browser_policy::is_raw_unc_href(href) {
        return None;
    }
    let href = href.strip_prefix(opencode_ui::style::CODE_LINK).unwrap_or(href).trim();
    let raw_dir = links::link_directory(app, app.active_session_id()?);
    let pinned = if app.can_reveal_outside() {
        app.pinned_local_root(&raw_dir)
    } else {
        Some(raw_dir.clone())
    };
    if is_file_scheme(href) {
        // This form names a file inside the workspace, never a foreign host or a path
        // escaping its root through a symlink. Remote paths use the server's file route.
        browser_policy::local_file_path(href)?;
        let root = pinned.as_deref()?;
        if app.can_open_in() && !file_url_within(href, &[root.to_owned()]) {
            return None;
        }
    }
    let mut link = links::classify(href, "", &raw_dir, app.home_directory());
    if matches!(link, Link::Outside { .. })
        && let Some(ref phys) = pinned
        && phys != &raw_dir
    {
        let phys_link = links::classify(href, "", phys, app.home_directory());
        if matches!(phys_link, Link::Inside { .. }) {
            link = phys_link;
        }
    }
    if is_file_scheme(href) && !matches!(link, Link::Inside { .. }) {
        return None;
    }
    let dir = pinned.unwrap_or(raw_dir);
    Some((dir, link))
}

fn web_url(href: &str) -> bool {
    destination_origin(href).is_some()
}

fn is_remote_session(app: &App) -> bool {
    let non_local = app.live_server().is_some_and(|s| !s.is_local());
    #[cfg(not(target_arch = "wasm32"))]
    let non_sidecar = !app.browser_server_for_session(app.active_session_id()).is_sidecar();
    #[cfg(target_arch = "wasm32")]
    let non_sidecar = false;
    non_local || non_sidecar
}

fn display_local_path(clean: String) -> String {
    if cfg!(windows) {
        clean.replace('/', "\\")
    } else {
        clean
    }
}

/// Decides on reply display whether a standalone video link shows a click-to-load play control.
/// **No video in a reply ever loads without a user click** (neither inside-workspace nor
/// outside-workspace), keeping cloned-repo bytes out of in-process OS demuxers/decoders until
/// clicked. Returns the real sanitized display path shown on the control with zero disk or
/// network I/O on display. `http(s)` video links stay ordinary links and open in the browser pane.
pub(crate) fn video(app: &App, href: &str) -> Option<String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) { return None; }
    let (dir, link) = target(app, href)?;
    let remote = is_remote_session(app);
    match link {
        Link::Inside { path, .. } if video::is_video(&path) => {
            if remote {
                let unsafe_char = path.chars().any(|c| {
                    let n = c as u32;
                    c.is_control()
                        || matches!(n, 0x7f..=0x9f | 0x061c | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060 | 0x2066..=0x2069 | 0xfeff)
                });
                if browser_policy::is_raw_unc_href(&path) || unsafe_char {
                    return None;
                }
                Some(path)
            } else {
                // A workspace whose aliased root was refused at start-up (`pinned_local_root`)
                // shows no Play control. Pure lexical pre-check on display; full
                // `verify_within_root` runs in `load_video` only when the user clicks Play.
                let root = app.pinned_local_root(&dir)?;
                let abs = opencode_kit::path::absolute(&root, &path);
                browser_policy::safe_local_reveal_path(&abs).map(display_local_path)
            }
        }
        Link::Outside { path } if app.can_open_in() && !remote && video::is_video(&path) => {
            // Pure lexical pre-check on display (zero disk/SMB/autofs I/O); full
            // `verify_local_path` runs in `load_video` only when the user clicks Play.
            browser_policy::safe_local_reveal_path(&path).map(display_local_path)
        }
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn keep_reply_temp_media(temp: opencode_kit::client::TempMediaFile) -> Option<String> {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<VecDeque<opencode_kit::client::TempMediaFile>>> = OnceLock::new();
    let path = temp.path_str()?.to_owned();
    let mut guard = CACHE.get_or_init(|| Mutex::new(VecDeque::new())).lock().unwrap_or_else(|e| e.into_inner());
    guard.push_back(temp);
    while guard.len() > 16 {
        guard.pop_front();
    }
    Some(path)
}

/// Verifies and resolves a standalone reply video target strictly on an explicit user click on
/// the Play control:
/// - `Link::Inside`: verifies inside workspace via `video::source` (`verify_within_root` on local
///   sessions; ranged URL for unauthenticated remote servers or `Authorization`-header temp download
///   for password-protected remote servers — never `auth_token` in a URL).
/// - `Link::Outside` (local sessions only): verifies via `browser_policy::verify_local_path`
///   (fixed drive / `MNT_LOCAL`, no UNC, mapped/`subst`-to-UNC, symlinks/junctions, or autofs;
///   regular file only).
pub(crate) fn load_video(app: &mut App, href: &str) -> Option<String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) { return None; }
    let Some((dir, link)) = target(app, href) else {
        crate::status_toast::fail(app, t!("rune.openIn.failed"), t!("rune.files.link.outside"));
        return None;
    };
    let remote = is_remote_session(app);
    match link {
        Link::Inside { path, .. } if video::is_video(&path) => {
            let src = if remote {
                match app.live_server() {
                    Some(srv) if srv.has_auth() => {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            let (req, ext) = video::remote_read_request(&dir, &path);
                            srv.download_media_temp(&req, &ext).ok().and_then(keep_reply_temp_media)
                        }
                        #[cfg(target_arch = "wasm32")]
                        {
                            None
                        }
                    }
                    srv => video::source(srv, &dir, &path),
                }
            } else {
                app.pinned_local_root(&dir).and_then(|root| video::source(app.live_server(), &root, &path))
            };
            if src.is_none() {
                crate::status_toast::fail(app, t!("rune.openIn.failed"), t!("rune.files.link.outside"));
            }
            src
        }
        Link::Outside { path } if app.can_open_in() && !remote && video::is_video(&path) => {
            match browser_policy::verify_local_path(&path) {
                Ok(verified) if verified.is_file => Some(verified.path),
                Ok(_) => {
                    crate::status_toast::fail(app, t!("rune.openIn.failed"), t!("rune.files.link.outside"));
                    None
                }
                Err(e) => {
                    crate::status_toast::fail(app, t!("rune.openIn.failed"), e.to_string());
                    None
                }
            }
        }
        _ => None,
    }
}

pub(crate) fn menu(app: &mut App, href: &str, press: PointerPress, tx: &mut Tx) -> bool {
    let menu = Menu::new(t!("ui.message.moreActions")).min_width(160.0);
    if web_url(href) {
        let url = href.to_owned();
        let menu = menu.item(Item::new("external", t!("rune.browser.menu.external")))
            .item(Item::new("copy", t!("rune.browser.menu.copyLink")));
        popup::open(app, menu, press, tx, move |app, id, tx| match id {
            Some("external") => app.route_web_or_app_link(None, &url, crate::surfaces::browser::links::LinkSource::Chat, Some(true), tx),
            Some("copy") => tx.copy(url),
            _ => {}
        });
        return true;
    }
    if !app.can_reveal_outside() { return false; }
    let Some((dir, link)) = target(app, href) else { return false };
    let (inside, path) = match link {
        Link::Inside { path, .. } => {
            let Some(root) = app.pinned_local_root(&dir) else { return false };
            (true, opencode_kit::path::absolute(&root, &path))
        }
        Link::Outside { path } => (false, path),
        _ => return false,
    };
    // Verify local fixed drive / MNT_LOCAL and zero reparse points BEFORE touching disk so
    // right-clicking a mapped drive, subst-to-UNC, junction, or autofs link never leaks credentials.
    let Ok(verified) = browser_policy::verify_local_path(&path) else { return false };
    let is_plain_dir = verified.is_dir
        && !browser_policy::is_macos_bundle(&verified.path)
        && !browser_policy::is_clsid_folder_name(&verified.path);
    let manager = crate::open_in::finder_label(app);
    let menu = if is_plain_dir {
        menu.item(Item::new("manager", t!("plugin.file.open.ariaLabel", app = manager)))
    } else if inside && verified.is_file {
        // Inside-workspace files open in the built-in viewer (never OS default-app execution).
        menu.item(Item::new("open", t!("ui.sessionReview.openFile")))
            .item(Item::new("manager", t!("plugin.file.view.reveal", app = manager)))
    } else if verified.is_file || verified.is_dir {
        // Outside-workspace files and bundles offer reveal only — never OS default-app execution.
        menu.item(Item::new("manager", t!("plugin.file.view.reveal", app = manager)))
    } else {
        return false;
    };
    let reveal_path = verified.path;
    let open_href = href.to_owned();
    popup::open(app, menu, press, tx, move |app, id, tx| match id {
        Some("open") if inside => app.open_reply_link(&open_href, tx),
        Some("manager") if is_plain_dir && app.can_open_in() => app.open_in_at(crate::open_in::FINDER, &reveal_path),
        Some("manager") => crate::open_in::reveal_abs(app, reveal_path),
        _ => {}
    });
    true
}
