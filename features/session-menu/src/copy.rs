//! Copy submenu for session context menus (`packages/app/src/session/session-copy-menu.tsx`,
//! `opencode#66576`): copies Session ID, Session title (when non-fallback), or Project ID.

use opencode_kit::servers::ServerKey;
use opencode_ui::popup::{self, Menu};
use rune::Tx;

use crate::host::{SessionMenuHost, on_server};
use crate::Line;

pub const COPY_ID: &str = "copy:id";
pub const COPY_TITLE: &str = "copy:title";
pub const COPY_PROJECT_ID: &str = "copy:project-id";

/// Recognizes missing and historical timestamped root/child fallback titles
/// (`packages/util/src/session-title-fallback.ts:3,24`):
/// `^(New session|Child session) - YYYY-MM-DDTHH:MM:SS.sssZ$`.
pub fn is_fallback_title(title: &str) -> bool {
    if title.is_empty() {
        return true;
    }
    let Some(ts) = title
        .strip_prefix("New session - ")
        .or_else(|| title.strip_prefix("Child session - "))
    else {
        return false;
    };
    let b = ts.as_bytes();
    b.len() == 24
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[7] == b'-'
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[10] == b'T'
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[13] == b':'
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[16] == b':'
        && b[17..19].iter().all(u8::is_ascii_digit)
        && b[19] == b'.'
        && b[20..23].iter().all(u8::is_ascii_digit)
        && b[23] == b'Z'
}

/// Builds the `Copy >` submenu for `(server, id)` when the session is known
/// (`packages/app/src/session/session-copy-menu.tsx:40-87`).
pub fn submenu(host: &dyn SessionMenuHost, server: &ServerKey, id: &str) -> Option<Line> {
    let (title, project_id) = host.session_copy_info(server, id)?;
    let mut sub = Menu::new(opencode_i18n::t!("session.copy"))
        .min_width(164.0)
        .item(popup::Item::new(COPY_ID, opencode_i18n::t!("session.copy.id")));
    if !is_fallback_title(&title) {
        sub = sub.item(popup::Item::new(COPY_TITLE, opencode_i18n::t!("session.copy.title")));
    }
    if !project_id.is_empty() {
        sub = sub.item(popup::Item::new(COPY_PROJECT_ID, opencode_i18n::t!("session.copy.projectID")));
    }
    Some(Line::Sub {
        id: "copy-menu".into(),
        title: opencode_i18n::t!("session.copy").into(),
        menu: sub,
    })
}

/// Executes one item from [`submenu`], reading the live session on `server` so titles renamed
/// since the row snapshot stay current (`session-copy-menu.tsx:17-21`).
pub fn run(host: &mut dyn SessionMenuHost, server: &ServerKey, id: &str, item: &str, tx: &mut Tx) -> bool {
    match item {
        COPY_ID => {
            tx.copy(id.to_owned());
            true
        }
        COPY_TITLE => {
            let mut live_title = None;
            let _ = on_server(host, server, |h| {
                live_title = h.session(id).map(|i| i.title.clone());
            });
            let title = live_title.or_else(|| host.session_copy_info(server, id).map(|(t, _)| t));
            if let Some(t) = title.filter(|t| !is_fallback_title(t)) {
                tx.copy(t);
            }
            true
        }
        COPY_PROJECT_ID => {
            let mut live_proj = None;
            let _ = on_server(host, server, |h| {
                live_proj = h.session(id).map(|i| i.project.clone());
            });
            let proj = live_proj.or_else(|| host.session_copy_info(server, id).map(|(_, p)| p));
            if let Some(p) = proj.filter(|p| !p.is_empty()) {
                tx.copy(p);
            }
            true
        }
        _ => false,
    }
}
