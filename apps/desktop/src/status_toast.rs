//! Toast notifications at the window's bottom-right, matching OpenCode Desktop's `Toast.Region`
//! (`packages/ui/src/feedback/toast/toast.tsx`, `toast.css`), with instant entrance and exit
//! (`docs/kit-design.md`).
//!
//! - [`rewound`]: `"Reverted"` with an `"Undo"` button, auto-closing after [`DURATION`] s unless
//!   hovered or dismissed.
//! - [`say`]: a message that leaves after [`DURATION`] s.
//! - [`fail`]: `"Request failed"` (or custom title) with the server's error message, staying until
//!   Kit dismisses it (`×` or `Escape`); [`fail_retry`] adds its fix as a Retry button.
//! - [`from_server`]: a line from the server's worker threads, as the toast Desktop shows for it.
//!
//! Each also sets `s.status`, which `--debug`'s status bar shows.

use std::path::PathBuf;
use std::time::Duration;

use rune::{Color, El, Icon, Key, KeyEvent, Shadow, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};

use crate::palette::{Elevate as _, Pal};
use crate::rewind::UndoRewind;
use crate::view::INSTANT;
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
    /// A plugin's button (its label) that shows a path in the file manager (desktop.reveal).
    Reveal(String, PathBuf),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub title: String,
    pub description: Option<String>,
    pub action: Option<ToastAction>,
    /// The glyph before the words: a success toast's tick.
    pub icon: Option<&'static Icon>,
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

/// Shows `"Reverted"` with an `"Undo"` button bound to `undo`.
pub fn rewound(s: &mut App, undo: UndoRewind) -> u64 {
    s.toast.next_id += 1;
    let id = s.toast.next_id;
    s.toast.hovered = false;
    s.toast.armed = None;
    s.toast.current = Some(Entry {
        id,
        title: opencode_i18n::t!("session.rewind.toast.title").into(),
        description: None,
        action: Some(ToastAction::UndoRewind(undo)),
        icon: None,
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

/// A success (`variant: "success"`): a message that leaves after [`DURATION`] s, after Desktop's
/// `circle-check`.
pub fn succeed(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    say(s, title, description);
    if let Some(e) = s.toast.current.as_mut() {
        e.icon = Some(&rune_theme_opencode::icons::CIRCLE_CHECK);
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
        let line = opencode_i18n::t!("toast.session.export.success.description", filename = path);
        return succeed(s, opencode_i18n::t!("toast.session.export.success.title"), line);
    }
    let Some((call, why)) = e.split_once(": ") else {
        return fail(s, opencode_i18n::t!("common.requestFailed"), e);
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
        ("POST", "prompt" | "agent" | "model") => opencode_i18n::t!("prompt.toast.promptSendFailed.title"),
        ("POST", "command") => opencode_i18n::t!("prompt.toast.commandSendFailed.title"),
        ("POST", "shell") => opencode_i18n::t!("prompt.toast.shellSendFailed.title"),
        ("POST", "session") => opencode_i18n::t!("prompt.toast.sessionCreateFailed.title"),
        ("PATCH", _) if session_only => opencode_i18n::t!("rune.toast.renameFailed"),
        ("DELETE", _) if session_only => opencode_i18n::t!("session.delete.failed.title"),
        // Desktop's ⌃B move, and anything else, says only this.
        _ => opencode_i18n::t!("common.requestFailed"),
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
        icon: None,
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

/// Sonner's close glyph, which `toast.css` draws at 16 px: two strokes 6 to 18 across 24, 1.5
/// wide with round caps.
static CLOSE: Icon = Icon {
    name: "toast-close",
    view: 24.0,
    body: r#"<path d="M18 6L6 18M6 6L18 18" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// The card (`.toast-v2`): 320 wide, 12 in, its icon's, content's and close button's columns
/// 12 apart, the actions 12 under the content in its column.
pub fn view(s: &App, p: &Pal) -> Option<El> {
    let entry = s.toast.current.as_ref()?;
    let id = entry.id;
    let close = row()
        .key("toast-dismiss")
        .role("button")
        .label(opencode_i18n::t!("ui.common.dismiss"))
        .help(opencode_i18n::t!("ui.common.dismiss"))
        .size(20.0, 20.0)
        .radius(4.0)
        .center()
        .hover(p.hover)
        .press(p.pressed)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            dismiss(s);
        })
        // `icon-muted` at rest, `icon-base` under the pointer.
        .child(icon(&CLOSE, 16.0).color(p.icon_muted))
        .child(icon(&CLOSE, 16.0).color(p.icon).absolute(2.0, 2.0).group_opacity(0.0, 1.0));

    let action_btn = entry.action.as_ref().map(|action| {
        let (key, label) = match action {
            ToastAction::UndoRewind(_) => ("toast-undo", opencode_i18n::t!("session.rewind.toast.undo").to_owned()),
            ToastAction::Attach(_) => ("toast-retry", opencode_i18n::t!("common.retry").to_owned()),
            ToastAction::Reveal(label, _) => ("toast-reveal", label.clone()),
        };
        let action = action.clone();
        row()
                .key(key)
                .role("button")
                .label(label.clone())
                .h(24.0)
                .px(9.0)
                .radius(4.0)
                .center()
                // `button-v2` neutral at size small (`button.css`). Dark's fill is `alpha-light-6`
                // over the toast, which the theme's token flattened over `bg-base` isn't.
                .fill(if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral })
                .elevate(neutral_lift(p))
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
                        ToastAction::Reveal(_, path) => crate::plugin_host::reveal(&path.to_string_lossy()),
                    }
                })
                .child(text(label).font(TITLE).color(p.text))
    });

    let mut content = col()
        .key("toast-content")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .gap(4.0)
        .child(text(entry.title.clone()).key("toast-title").font(TITLE).color(p.text).truncate().w_full())
        .children(entry.description.as_ref().map(|desc| text(desc.clone()).key("toast-description").font(DESC).color(p.muted).wrap().w_full()));
    if let Some(btn) = action_btn {
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).child(btn).child(spacer().fill(Color::TRANSPARENT)));
    }
    let lead = entry.icon.map(|glyph| el().key("toast-icon").w(16.0).h(20.0).center().child(icon(glyph, 16.0).color(p.icon)));
    let body = row().key("toast-head").w_full().gap(12.0).children(lead).child(content).child(close);

    let card = col()
        .key("status-toast")
        .role("status")
        .w(320.0)
        .padding(12.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
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

    // Sonner's region: 32 in from the window's right, 48 up from its bottom (`toast.tsx` offset);
    // at 600 pt or narrower, 16 in from its left and bottom, 64 short of its width
    // (`toast.css:226`, Sonner's mobile offsets).
    let width = s.layout.narrow.width();
    if width > 0.0 && width <= 600.0 {
        return Some(card.w(width - 64.0).pin(Some(16.0), None, None, Some(16.0)).raise());
    }
    Some(card.pin(None, None, Some(32.0), Some(48.0)).raise())
}

/// `--v2-elevation-button-neutral`: a drop and the 0.5px ring outside the button, and in dark the
/// 0.5px lit top edge (`theme.css`).
pub(crate) fn neutral_lift(p: &Pal) -> [Shadow; 4] {
    let (y, blur) = if p.dark { (1.0, 2.0) } else { (1.0, 1.5) };
    let top = if p.dark { Shadow { y: -0.5, color: Color::WHITE.alpha(0.1), ..Shadow::default() } } else { Shadow::default() };
    [Shadow { y, blur, color: p.q.neutral_shadow, ..Shadow::default() }, Pal::ring_out(p.q.neutral_ring), top, Shadow::default()]
}
