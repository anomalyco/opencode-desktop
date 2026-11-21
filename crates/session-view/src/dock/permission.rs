//! The permission dock: Desktop's `session-permission-dock.tsx` in the composer's place. When the
//! open page's session (or a subagent under it) waits on a permission, the composer's surface
//! morphs into it the way it does for a question: a warning and "Permission required", what the
//! tool does when Desktop names it (else the tool's own name, `shell` for a command), the
//! request's message, the tool's arguments when it sends them, the patterns it asks for, then
//! Deny, Allow always and Allow once. Several wait one at a time, the page's own first, then
//! oldest first. With auto-accept on it never shows.
//!
//! A press claims the request: its buttons go off until the server takes the reply, then the dock
//! goes and the composer comes back with its draft (`permission-machine.ts`).

use opencode_kit::model::Pending;
use rune::{El, Shadow, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;

use super::{DockHost, Variant, button, face_transition, morph};
use crate::palette::Pal;

/// `permission-header-title`: 13 px medium on the base line height.
const TITLE: TextStyle = TextStyle::new(13.0, 500.0).lh(20.0);
const HINT: TextStyle = TextStyle::new(13.0, 400.0).lh(20.0);
/// `permission-action`, `permission-input code` and `permission-patterns code`.
const CODE: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0).tracking(-0.04);
/// The icon's column; the title and everything under it start past it and the gap.
const ICON_W: f32 = 20.0;
const INDENT: f32 = ICON_W + 8.0;
/// A long list of patterns scrolls inside the dock past this height.
const PATTERNS_MAX_H: f32 = 320.0;
/// The tool's arguments scroll past this (`permission-input`'s `max-height`).
const INPUT_MAX_H: f32 = 160.0;

/// What the tool does, in Desktop's words (`settings.permissions.tool.<action>.description`).
/// Desktop has none for `shell` (its key is still `bash`), so a command shows the tool's name.
pub fn description(action: &str) -> Option<&'static str> {
    Some(match action {
        "read" => opencode_i18n::t!("settings.permissions.tool.read.description"),
        "edit" => opencode_i18n::t!("settings.permissions.tool.edit.description"),
        "glob" => opencode_i18n::t!("settings.permissions.tool.glob.description"),
        "grep" => opencode_i18n::t!("settings.permissions.tool.grep.description"),
        "list" => opencode_i18n::t!("settings.permissions.tool.list.description"),
        "bash" => opencode_i18n::t!("settings.permissions.tool.bash.description"),
        "task" => opencode_i18n::t!("settings.permissions.tool.task.description"),
        "skill" => opencode_i18n::t!("settings.permissions.tool.skill.description"),
        "lsp" => opencode_i18n::t!("settings.permissions.tool.lsp.description"),
        "todowrite" => opencode_i18n::t!("settings.permissions.tool.todowrite.description"),
        "webfetch" => opencode_i18n::t!("settings.permissions.tool.webfetch.description"),
        "websearch" => opencode_i18n::t!("settings.permissions.tool.websearch.description"),
        "external_directory" => opencode_i18n::t!("settings.permissions.tool.external_directory.description"),
        "doom_loop" => opencode_i18n::t!("settings.permissions.tool.doom_loop.description"),
        _ => return None,
    })
}

/// The key of the permission's card, which takes focus when the request arrives.
pub const PERMISSION_FOCUS: &str = "permission-card";

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
    let Pending::Permission { id, action, resources, message, input, .. } = asked(s)? else { return None };
    // What a reader hears on the card that takes focus when the request arrives (`speech.rs`):
    // what the tool does, then on what.
    let said = [description(&action).unwrap_or_else(|| crate::turns::tool_title(&action)), message.as_str()].into_iter()
        .chain(input.iter().map(String::as_str)).chain(resources.iter().map(String::as_str))
        .filter(|w| !w.trim().is_empty()).collect::<Vec<_>>().join(". ");
    let fade = || face_transition(s.knobs());
    let header = row()
        .key("permission-header")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(el().key("permission-icon").size(ICON_W, 20.0).center().child(icon(&ic::WARNING, 16.0).color(p.v1_icon_warning)))
        .child(text(opencode_i18n::t!("notification.permission.title")).key("permission-title").font(TITLE).color(p.text).shrink());
    // What the tool does, else its name in code.
    let what = match description(&action) {
        Some(d) => text(d).key("permission-hint").font(HINT).color(p.muted).wrap(),
        None => text(action.clone()).key("permission-action").font(CODE).color(p.text).wrap_anywhere(),
    };
    let message = (!message.is_empty()).then(|| text(message.clone()).key("permission-message").font(HINT).color(p.muted).wrap());
    // The arguments scroll rather than truncate: all of what's approved must be reachable.
    let input = (!input.is_empty()).then(|| {
        // A plain `overflow-y: auto` box's bar (`message-part.css:1583-1586`).
        let list = col().key("permission-input-list").w_full().gap(2.0).children(input.iter().enumerate().map(|(i, line)| {
            text(line.clone()).key(format!("input-{i}")).font(CODE).color(p.v1_text_weak).wrap_anywhere()
        }));
        crate::scroller::scroller("permission-input").build::<S>(p, col().w_full().max_h(INPUT_MAX_H).mt(8.0), list)
    });
    let patterns = (!resources.is_empty()).then(|| {
        // No bar, as Desktop hides it (`message-part.css:1604-1612`).
        let list = col().key("permission-pattern-list").w_full().gap(6.0).pt(1.0).pb(8.0).children(resources.iter().enumerate().map(|(i, r)| {
            text(r.clone()).key(format!("pattern-{i}")).font(CODE).color(p.v1_text_base).wrap_anywhere()
        }));
        crate::scroller::scroller("permission-patterns").hidden(col().w_full().max_h(PATTERNS_MAX_H).scroll_edge_fade(16.0).mt(8.0).mb(16.0), list)
    });
    let body = col()
        .key("permission")
        .w_full()
        .px(12.0)
        .pt(12.0)
        .gap(16.0)
        .child(header)
        // Keyed by request: the next one's words fade in over the last's instead of swapping.
        .child(col().key(format!("permission-{id}")).w_full().pl(INDENT).gap(4.0).transition(fade()).child(what).children(message).children(input).children(patterns));
    let live = !s.dock_busy(&id);
    let answer = |key: &str, label: &str, variant: Variant, decision: &'static str| {
        let id = id.clone();
        let b = button(p, key, label, variant, None, live).h(28.0).px(12.0);
        if live { b.on_press_tx(move |s: &mut S, tx: &mut Tx| decide(s, &id, decision, tx)) } else { b }
    };
    let footer = row()
        .key("permission-footer")
        .w_full()
        .padding(8.0)
        .center_y()
        .child(spacer())
        .child(
            row()
                .key("permission-actions")
                .gap(8.0)
                .child(answer("perm-deny", opencode_i18n::t!("ui.permission.deny"), Variant::Ghost, "reject"))
                .child(answer("perm-always", opencode_i18n::t!("ui.permission.allowAlways"), Variant::Neutral, "always"))
                .child(answer("perm-once", opencode_i18n::t!("ui.permission.allowOnce"), Variant::Submit, "once")),
        );
    // Its own card (`dock-prompt[data-kind="permission"]`): the raised elevation outside the
    // box over a `bg-base` hairline that masks what's behind, where the composer's card has
    // its hairline inside.
    let raised = p.raised;
    let ring = Shadow { spread: 0.5, color: super::flat(raised[2].color, p.bg_base), ..Shadow::default() };
    // Out of the Tab order; its buttons follow it.
    let card = col()
        .key(PERMISSION_FOCUS)
        .role("region")
        .labelled_by("permission-title")
        .description(said)
        .focusable(true)
        .tab_index(-1)
        .w_full()
        .radius(12.0)
        .fill(crate::composer::composer_fill(p))
        .box_shadow(raised[3])
        .box_shadow(ring)
        .box_shadow(raised[1])
        .box_shadow(raised[0])
        .transition(fade())
        .child(body)
        .child(footer);
    Some(vec![card])
}
