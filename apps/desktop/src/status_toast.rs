//! Toast notifications at the window's bottom-right, matching OpenCode Desktop's `Toast.Region`
//! (`packages/ui/src/feedback/toast/toast.tsx`, `toast.css`), with instant entrance and exit
//! (`docs/kit-design.md`).
//!
//! - [`rewound`]: `"Rewound"` with an `"Undo"` button, auto-closing after [`DURATION`] s unless
//!   hovered or dismissed.
//! - [`say`]: a message that leaves after [`DURATION`] s.
//! - [`fail`]: `"Request failed"` (or custom title) with the server's error message, staying until
//!   Kit dismisses it (`×` or `Escape`); [`fail_retry`] adds its fix as a Retry button.
//! - [`from_server`]: a line from the server's worker threads, as the toast Desktop shows for it.
//!
//! Each also sets `s.status`, which `--debug`'s status bar shows.

use std::path::PathBuf;
use std::time::Duration;

use rune::{Align, Color, El, Key, KeyEvent, Side, Spring, TextStyle, Transition, Tx, col, icon, row, spacer, text};
use rune_theme_opencode::icons as ic;

use crate::palette::Pal;
use crate::rewind::UndoRewind;
use crate::view::{INSTANT, UI, UI_MED};
use crate::App;

/// Desktop's `duration={5000}` on `Toast.Region`.
pub const DURATION: f64 = 5.0;

const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const DESC: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

#[derive(Clone, Debug)]
pub enum ToastAction {
    UndoRewind(UndoRewind),
    /// Retry: attach this file again (an upload that failed).
    Attach(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub description: Option<String>,
    pub action: Option<ToastAction>,
    pub persistent: bool,
    pub at: f64,
}

#[derive(Default)]
pub struct StatusToast {
    pub current: Option<Entry>,
    next_id: u64,
    hovered: bool,
    armed: Option<u64>,
}

/// Shows `"Rewound"` with an `"Undo"` button bound to `undo`.
pub fn rewound(s: &mut App, undo: UndoRewind) -> u64 {
    s.toast.next_id += 1;
    let id = s.toast.next_id;
    s.toast.hovered = false;
    s.toast.armed = None;
    s.toast.current = Some(Entry {
        id,
        title: "Rewound".into(),
        description: None,
        action: Some(ToastAction::UndoRewind(undo)),
        persistent: false,
        at: s.clock,
    });
    id
}

/// Shows a message that leaves after [`DURATION`] s.
pub fn say(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    fail(s, title, description);
    if let Some(e) = s.toast.current.as_mut() {
        e.persistent = false;
    }
}

/// [`fail`] with a Retry button that runs `fix`.
pub fn fail_retry(s: &mut App, title: impl Into<String>, description: impl Into<String>, fix: ToastAction) {
    fail(s, title, description);
    if let Some(e) = s.toast.current.as_mut() {
        e.action = Some(fix);
    }
}

/// A line from the server's worker threads (`Incoming::Status`) that revert, fork and stop
/// didn't claim. A failed write shows Desktop's toast for it; a failed read or a dropped event
/// stream stays in the debug bar, since the app retries those itself.
pub fn from_server(s: &mut App, e: String) {
    if let Some(path) = e.strip_prefix("Saved session to ") {
        let line = format!("Saved session to {path}");
        return say(s, "Session exported", line);
    }
    let Some((call, why)) = e.split_once(": ") else {
        return fail(s, crate::background::FAILED, e);
    };
    let (method, path) = call.split_once(' ').unwrap_or(("", call));
    if method == "GET" || call == "events" {
        s.status = e;
        return;
    }
    let why = plain(why);
    fail(s, write_title(method, path), why);
}

/// Desktop's titles for a failed write (`prompt.toast.*`, `session.delete.failed.title`…).
fn write_title(method: &str, path: &str) -> &'static str {
    let path = path.split('?').next().unwrap_or(path);
    let last = path.rsplit('/').next().unwrap_or("");
    let session_only = path.strip_prefix("/api/session/").is_some_and(|rest| !rest.contains('/'));
    match (method, last) {
        ("POST", "prompt" | "agent" | "model") => "Failed to send prompt",
        ("POST", "command") => "Failed to send command",
        ("POST", "shell") => "Failed to send shell command",
        ("POST", "session") => "Failed to create session",
        ("PATCH", _) if session_only => "Failed to rename session",
        ("DELETE", _) if session_only => "Failed to delete session",
        // Desktop's ⌃B move, and anything else, says only this.
        _ => crate::background::FAILED,
    }
}

/// An error's words without the OS's code: "No such file or directory".
pub fn plain(e: &str) -> String {
    let e = e.trim();
    let e = match e.rfind(" (os error ") {
        Some(i) if e.ends_with(')') => &e[..i],
        _ => e,
    };
    opencode_kit::plain_error::plain_error(e)
}

/// The calm line for a server that has stayed silent: it stays until [`dismiss_id`] clears it
/// (the read that answers) and is not a failure. Returns its id.
pub fn waiting(s: &mut App, title: &str) -> u64 {
    fail(s, title, "");
    s.toast.current.as_ref().map_or(0, |e| e.id)
}

/// Shows a failure toast that waits until dismissed, and records the message on `s.status`.
pub fn fail(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    let title = title.into();
    let desc = description.into();
    let trimmed = desc.trim();
    s.status = if trimmed.is_empty() { title.clone() } else { format!("{title}: {trimmed}") };
    s.toast.next_id += 1;
    let id = s.toast.next_id;
    s.toast.hovered = false;
    s.toast.armed = None;
    s.toast.current = Some(Entry {
        id,
        title,
        description: (!trimmed.is_empty()).then(|| trimmed.chars().take(240).collect()),
        action: None,
        persistent: true,
        at: s.clock,
    });
}

/// Dismisses the open toast, if any.
pub fn dismiss(s: &mut App) -> bool {
    s.toast.hovered = false;
    s.toast.armed = None;
    s.toast.current.take().is_some()
}

/// Dismisses toast `id` if it is still the one showing.
pub fn dismiss_id(s: &mut App, id: u64) {
    if s.toast.current.as_ref().is_some_and(|e| e.id == id) {
        dismiss(s);
    }
}

/// Auto-closes non-persistent toasts after [`DURATION`], scheduling a single wake per arm.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let Some(entry) = s.toast.current.as_ref() else { return false };
    if entry.persistent || s.toast.hovered {
        return false;
    }
    if now - entry.at >= DURATION - 1e-3 {
        tx.instant();
        dismiss(s);
        return true;
    }
    if s.toast.armed != Some(entry.id) {
        s.toast.armed = Some(entry.id);
        let rem = (entry.at + DURATION - now).max(0.001);
        tx.wake_after(Duration::from_secs_f64(rem));
    }
    false
}

/// Escape dismisses a persistent failure toast when no higher overlay owns the key.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain_esc = ev.key == Key::Escape && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift;
    if plain_esc && s.toast.current.as_ref().is_some_and(|e| e.persistent) {
        tx.instant();
        dismiss(s);
        return true;
    }
    false
}

pub fn view(s: &App, p: &Pal) -> Option<El> {
    let entry = s.toast.current.as_ref()?;
    let id = entry.id;
    let close = row()
        .key("toast-dismiss")
        .role("button")
        .label("Dismiss")
        .help("Dismiss")
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            dismiss(s);
        })
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));

    let action_btn = entry.action.as_ref().map(|action| {
        let (key, label) = match action {
            ToastAction::UndoRewind(_) => ("toast-undo", "Undo"),
            ToastAction::Attach(_) => ("toast-retry", "Retry"),
        };
        let action = action.clone();
        row()
                .key(key)
                .role("button")
                .label(label)
                .h(24.0)
                .px(8.0)
                .radius(5.0)
                .center()
                .fill(if p.dark { p.layer2 } else { p.bg_deep })
                .border(0.5, p.border_muted)
                .hover(p.hover)
                .press(p.pressed)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    dismiss(s);
                    match action.clone() {
                        ToastAction::UndoRewind(undo) => s.undo_rewind(undo, tx),
                        ToastAction::Attach(path) => {
                            tx.instant();
                            s.attach_path(&path);
                        }
                    }
                })
                .child(text(label).font(UI_MED).color(p.text))
    });

    let body = if let Some(desc) = &entry.description {
        let mut content = col()
            .key("toast-content")
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .gap(2.0)
            .child(text(entry.title.clone()).key("toast-title").font(TITLE).color(p.text).truncate_fade())
            .child(text(desc.clone()).key("toast-description").font(DESC).color(p.muted).wrap().w_full());
        if let Some(btn) = action_btn {
            content = content.child(row().key("toast-actions").pt(6.0).child(btn).child(spacer().fill(Color::TRANSPARENT)));
        }
        row().key("toast-head").w(296.0).gap(10.0).child(content).child(close)
    } else {
        row()
            .key("toast-head")
            .gap(10.0)
            .center_y()
            .child(text(entry.title.clone()).key("toast-title").font(TITLE).color(p.text))
            .children(action_btn)
            .child(close)
    };

    let card = col()
        .key("status-toast")
        .role("status")
        .px(12.0)
        .py(8.0)
        .radius(8.0)
        .fill(p.layer1)
        .border(0.5, p.ring)
        .shadow(4.0, 12.0, p.shadow)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if s.toast.current.as_ref().is_none_or(|e| e.id != id) {
                return;
            }
            s.toast.hovered = inside;
            if !inside
                && let Some(e) = s.toast.current.as_mut()
                && !e.persistent
            {
                e.at = s.clock;
                s.toast.armed = None;
                tx.wake_after(Duration::from_secs_f64(DURATION));
            }
        })
        .on_press(|_: &mut App| {})
        .child(body);

    let _ = UI;
    let placed = if s.has_composer() && !s.settings.open {
        card.anchored("composer", Side::Top, Align::End, 8.0)
    } else {
        card.pin(None, None, Some(28.0), Some(28.0))
    };
    Some(placed.raise())
}
