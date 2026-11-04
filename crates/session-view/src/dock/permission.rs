//! The permission dock: Desktop's `session-permission-dock.tsx` in the composer's place. When the
//! open page's session (or a subagent under it) waits on a permission, the composer's surface
//! morphs into it the way it does for a question: a warning and "Permission required", what the
//! tool does when Desktop names it, the patterns it asks for, then Deny, Allow always and Allow
//! once. Several wait one at a time, the page's own first, then oldest first. With auto-accept on
//! it never shows.
//!
//! A press answers at once: the dock goes and the composer comes back with its draft.

use opencode_kit::model::Pending;
use rune::{El, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;

use super::{DockHost, Variant, button, face_transition, morph};
use crate::palette::Pal;

const TITLE: TextStyle = TextStyle::new(14.0, 500.0).lh(21.0);
const HINT: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
const PATTERN: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0).tracking(-0.04);
/// The icon's column; the title and everything under it start past it and the gap.
const ICON_W: f32 = 20.0;
const INDENT: f32 = ICON_W + 8.0;
/// A long list of patterns scrolls inside the dock past this height.
const PATTERNS_MAX_H: f32 = 320.0;

/// What the tool does, in Desktop's words (`settings.permissions.tool.<action>.description`).
pub fn description(action: &str) -> Option<&'static str> {
    Some(match action {
        "read" => "Reading a file (matches the file path)",
        "edit" => "Modify files, including edits, writes, and patches",
        "glob" => "Match files using glob patterns",
        "grep" => "Search file contents using regular expressions",
        "list" => "List files within a directory",
        "bash" => "Run shell commands",
        "task" => "Launch sub-agents",
        "skill" => "Load a skill by name",
        "lsp" => "Run language server queries",
        "todowrite" => "Update the todo list",
        "webfetch" => "Fetch content from a URL",
        "websearch" => "Search the web",
        "external_directory" => "Access files outside the project directory",
        "doom_loop" => "Detect repeated tool calls with identical input",
        _ => return None,
    })
}

/// The permission the open page shows: none while a question stands in the composer's place or
/// auto-accept answers them. A subagent's page shows its own and its subagents'.
pub fn asked<S: DockHost>(s: &S) -> Option<Pending> {
    let id = s.active_session_id()?;
    if s.dock_auto_accept() || super::question::id(s).is_some() {
        return None;
    }
    let child = s.dock_parent(id).is_some();
    let asks: Vec<Pending> = s
        .dock_pending(id)
        .into_iter()
        .filter(|p| matches!(p, Pending::Permission { .. }) && (!child || under(s, p.session(), id)))
        .collect();
    let own = asks.iter().position(|p| p.session() == id).unwrap_or(0);
    asks.into_iter().nth(own)
}

/// Whether `session` is `top` or a subagent somewhere under it.
fn under<S: DockHost>(s: &S, session: &str, top: &str) -> bool {
    let mut at = Some(session.to_owned());
    for _ in 0..32 {
        match at {
            Some(x) if x == top => return true,
            Some(x) => at = s.dock_parent(&x),
            None => return false,
        }
    }
    false
}

/// A press on the dock: the reply goes, and the composer (or the next request) takes the dock's
/// place.
pub fn decide<S: DockHost>(s: &mut S, id: &str, decision: &str, tx: &mut Tx) {
    s.dock_reply_permission(id, decision);
    morph(s, tx);
}

/// The permission's face of the composer surface: header and patterns, then the answers.
pub fn face<S: DockHost>(s: &S, p: &Pal) -> Option<Vec<El>> {
    let Pending::Permission { id, action, resources, .. } = asked(s)? else { return None };
    let fade = || face_transition(s.knobs());
    let header = row()
        .key("permission-header")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(el().key("permission-icon").size(ICON_W, 21.0).center().child(icon(&ic::WARNING, 16.0).color(p.warning)))
        .child(text("Permission required").key("permission-title").font(TITLE).color(p.text).shrink());
    let hint = description(&action).map(|d| text(d).key("permission-hint").font(HINT).color(p.muted).wrap());
    let patterns = (!resources.is_empty()).then(|| {
        col()
            .key("permission-patterns")
            .w_full()
            .max_h(PATTERNS_MAX_H)
            .scroll(rune::ScrollSpec { stick_bottom: false })
            .scroll_edge_fade(16.0)
            .mt(8.0)
            .mb(16.0)
            .child(col().key("permission-pattern-list").w_full().gap(6.0).pt(1.0).pb(8.0).children(resources.iter().enumerate().map(|(i, r)| {
                text(r.clone()).key(format!("pattern-{i}")).font(PATTERN).color(p.text).wrap_anywhere()
            })))
    });
    let body = col()
        .key("permission")
        .w_full()
        .px(12.0)
        .pt(12.0)
        .gap(16.0)
        .transition(fade())
        .child(header)
        // Keyed by request: the next one's words fade in over the last's instead of swapping.
        .children((hint.is_some() || patterns.is_some()).then(|| {
            col().key(format!("permission-{id}")).w_full().pl(INDENT).gap(4.0).transition(fade()).children(hint).children(patterns)
        }));
    let answer = |key: &str, label: &str, variant: Variant, decision: &'static str| {
        let id = id.clone();
        button(p, key, label, variant, None, true).h(28.0).px(12.0).on_press_tx(move |s: &mut S, tx: &mut Tx| decide(s, &id, decision, tx))
    };
    let footer = row()
        .key("permission-footer")
        .w_full()
        .padding(8.0)
        .center_y()
        .transition(fade())
        .child(spacer())
        .child(
            row()
                .key("permission-actions")
                .gap(8.0)
                .child(answer("perm-deny", "Deny", Variant::Ghost, "reject"))
                .child(answer("perm-always", "Allow always", Variant::Neutral, "always"))
                .child(answer("perm-once", "Allow once", Variant::Submit, "once")),
        );
    Some(vec![body, footer])
}
