//! The permission dock: Desktop's `session-permission-dock.tsx` in the composer's place. When the
//! open page's session (or a subagent under it) waits on a permission, the composer's surface
//! morphs into it the way it does for a question (`question::face_transition`, `App::morph`): a
//! warning and "Permission required", what the tool does when Desktop names it, the patterns it
//! asks for, then Deny, Allow always and Allow once. Several wait one at a time, the page's own
//! first, then oldest first. With auto-accept on it never shows.
//!
//! A press answers at once: the dock goes and the composer comes back with its draft. A reply
//! the server refuses brings the request back where it stood, with a toast.

use std::collections::HashMap;

use opencode_kit::model::Pending;
use rune::{El, TextStyle, Tx, col, el, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;
use serde_json::json;

use crate::palette::Pal;
use crate::question::{Variant, button, face_transition};
use crate::{App, Route};

const TITLE: TextStyle = TextStyle::new(14.0, 500.0).lh(21.0);
const HINT: TextStyle = TextStyle::new(14.0, 400.0).lh(21.0);
const PATTERN: TextStyle = TextStyle::new(13.0, 440.0).mono().lh(20.0).tracking(-0.04);
/// The icon's column; the title and everything under it start past it and the gap.
const ICON_W: f32 = 20.0;
const INDENT: f32 = ICON_W + 8.0;
/// A long list of patterns scrolls inside the dock past this height.
const PATTERNS_MAX_H: f32 = 320.0;

/// Replies on their way, by request id: where the request stood and what it was, to bring it
/// back if the server refuses the reply.
#[derive(Default)]
pub struct Sent(pub(crate) HashMap<String, (usize, Pending)>);

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

impl App {
    /// The permission the open page shows: none while a question stands in the composer's place
    /// or auto-accept answers them. A subagent's page shows its own and its subagents'.
    pub fn permission_asked(&self) -> Option<&Pending> {
        let Route::Session(id) = &self.route else { return None };
        if self.settings.auto_accept || self.question_shown() {
            return None;
        }
        let child = self.session(id).is_some_and(|i| i.parent.is_some());
        let asks: Vec<&Pending> = self
            .pending_for(id)
            .into_iter()
            .filter(|p| matches!(p, Pending::Permission { .. }) && (!child || self.under(p.session(), id)))
            .collect();
        asks.iter().find(|p| p.session() == id).or(asks.first()).copied()
    }

    /// Whether `session` is `top` or a subagent somewhere under it.
    fn under(&self, session: &str, top: &str) -> bool {
        let mut at = Some(session);
        for _ in 0..32 {
            match at {
                Some(s) if s == top => return true,
                Some(s) => at = self.session(s).and_then(|i| i.parent.as_deref()),
                None => return false,
            }
        }
        false
    }

    /// What stands in the composer's place, by id: the question's form, else a permission.
    pub fn dock_face(&self) -> Option<String> {
        self.question_id().or_else(|| self.permission_asked().map(|p| p.id().to_owned()))
    }

    pub fn dock_shown(&self) -> bool {
        self.dock_face().is_some()
    }

    /// Answers the request at once and keeps it, to bring it back if the server refuses.
    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        let Some(at) = self.pending.iter().position(|p| p.id() == id) else { return };
        // The fixture answers at once, as the server's `permission.replied` would.
        if matches!(self.source, crate::Source::Fixture) && self.stage.is_none() {
            self.forget(id);
            return;
        }
        let p = self.pending[at].clone();
        let w = opencode_kit::client::Write::Permission { session: p.session().to_owned(), request: id.to_owned(), decision: decision.to_owned() };
        if self.write(w) {
            self.forget(id);
            self.permission_sent.0.insert(id.to_owned(), (at, p));
        }
    }

    /// A press on the dock: the reply goes, and the composer (or the next request) takes the
    /// dock's place.
    fn decide(&mut self, id: &str, decision: &str, tx: &mut Tx) {
        self.reply_permission(id, decision);
        self.morph(tx);
    }
}

/// A failed write from the server's worker: a permission's reply puts the request back where it
/// stood, and the toast says so. False when it isn't a permission's.
pub fn failed(s: &mut App, e: &str) -> bool {
    let (path, why) = e.split_once(": ").unwrap_or((e, ""));
    let Some(rest) = path.strip_prefix("POST /api/session/").and_then(|r| r.strip_suffix("/reply")) else { return false };
    let Some((_, rid)) = rest.split_once("/permission/") else { return false };
    if let Some((at, p)) = s.permission_sent.0.remove(rid) {
        s.requests.insert((p.session().to_owned(), rid.to_owned()));
        if !s.pending.iter().any(|x| x.id() == rid) {
            s.pending.insert(at.min(s.pending.len()), p);
        }
    }
    crate::status_toast::fail(s, "Couldn't answer the permission", why);
    true
}

/// The permission's face of the composer surface: header and patterns, then the answers.
pub fn face(s: &App, p: &Pal) -> Option<Vec<El>> {
    let Pending::Permission { id, action, resources, .. } = s.permission_asked()? else { return None };
    let header = row()
        .key("permission-header")
        .w_full()
        .gap(8.0)
        .center_y()
        .child(el().key("permission-icon").size(ICON_W, 21.0).center().child(icon(&ic::WARNING, 16.0).color(p.warning)))
        .child(text("Permission required").key("permission-title").font(TITLE).color(p.text).shrink());
    let hint = description(action).map(|d| text(d).key("permission-hint").font(HINT).color(p.muted).wrap());
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
        .transition(face_transition(s))
        .child(header)
        // Keyed by request: the next one's words fade in over the last's instead of swapping.
        .children((hint.is_some() || patterns.is_some()).then(|| {
            col().key(format!("permission-{id}")).w_full().pl(INDENT).gap(4.0).transition(face_transition(s)).children(hint).children(patterns)
        }));
    let answer = |key: &str, label: &str, variant: Variant, decision: &'static str| {
        let id = id.clone();
        button(p, key, label, variant, None, true).h(28.0).px(12.0).on_press_tx(move |s: &mut App, tx: &mut Tx| s.decide(&id, decision, tx))
    };
    let footer = row()
        .key("permission-footer")
        .w_full()
        .padding(8.0)
        .center_y()
        .transition(face_transition(s))
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

// The fixture session at rest; a bash permission arrives 0.6 s in (`--at 1.5` for a still).
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("permission", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let Route::Session(id) = s.route.clone() else { return };
        let ask = json!({ "id": "per_scene_bash", "sessionID": id, "action": "bash", "resources": ["bun test test/net/backoff.test.ts"] });
        s.playback.push_back((s.clock + 0.6, "permission.asked".into(), ask));
        s.wake_at_playback();
    }),
]);

mod stories;
#[cfg(test)]
mod tests;
