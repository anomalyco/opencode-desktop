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
    let dir = links::link_directory(app, app.active_session_id()?);
    if is_file_scheme(href) {
        // This form names a file inside the workspace, never a foreign host or a path
        // escaping its root through a symlink. Remote paths use the server's file route.
        browser_policy::local_file_path(href)?;
        if app.can_open_in() && !file_url_within(href, std::slice::from_ref(&dir)) {
            return None;
        }
    }
    let link = links::classify(href, "", &dir, app.home_directory());
    if is_file_scheme(href) && !matches!(link, Link::Inside { .. }) {
        return None;
    }
    Some((dir, link))
}

fn web_url(href: &str) -> bool {
    destination_origin(href).is_some()
}

pub(crate) fn video(app: &App, href: &str) -> Option<String> {
    if !cfg!(any(target_os = "macos", target_os = "windows")) { return None; }
    let (dir, link) = target(app, href)?;
    match link {
        Link::Web(url) if web_url(&url) => {
            let (_, rest) = url.split_once("://")?;
            let (_, path) = rest.split(['?', '#']).next()?.split_once('/')?;
            video::is_video(path).then_some(url)
        }
        Link::Inside { path, .. } if video::is_video(&path) => video::source(app.live_server(), &dir, &path),
        Link::Outside { path } if app.can_open_in() && video::is_video(&path) => {
            browser_policy::verify_local_path(&path).ok().filter(|v| v.is_file).map(|v| v.path)
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
        Link::Inside { path, .. } => (true, opencode_kit::path::absolute(&dir, &path)),
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
