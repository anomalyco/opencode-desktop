//! Toast notifications at the window's bottom-right, matching OpenCode Desktop's `Toast.Region`
//! (`packages/ui/src/feedback/toast/toast.tsx`, `toast.css`), with instant entrance and exit
//! (`docs/kit-design.md`).
//!
//! - [`rewound`]: `"Reverted"` with an `"Undo"` button, auto-closing after [`DURATION`] s.
//! - [`say`]: a message that leaves after [`DURATION`] s.
//! - [`fail`]: `"Request failed"` (or custom title) with the server's error message, staying until
//!   Kit dismisses it (`×` or `Escape`).
//! - [`from_server`]: a line from the server's worker threads, as the toast Desktop shows for it.
//! - [`uploads_shown`]: the uploads in flight (`attachments::toast`), until the last settles.
//!
//! They stack as Sonner stacks them (`rune_kit::ToastStack`): the newest in front, up to two
//! more peeking behind it; the pointer over the stack spreads them out, and while it's there no
//! toast's time runs. Each also sets `s.status`, which `--debug`'s status bar shows.

use std::path::PathBuf;

use rune::{Anim, Color, Curve, Easing, El, Icon, Key, KeyEvent, Shadow, Spring, TextStyle, Transition, Tx, col, el, icon, row, spacer, text};
use rune_kit::toast_stack::{CONTENT_FADE_SECS, FADE_SECS, MOVE_SECS};
use rune_kit::{ToastEffect, ToastEvent, ToastStack};

use crate::palette::{Elevate as _, Pal};
use crate::rewind::UndoRewind;
use crate::view::INSTANT;
use crate::App;

/// Desktop's `duration={5000}` on `Toast.Region`.
pub const DURATION: f64 = 5.0;

const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const DESC: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0).tracking(-0.04);

/// `toast.css`: `transform 280ms cubic-bezier(0.2, 0, 0, 1)`, `height` the same.
const MOVE: Anim = Anim::new(Curve::tween(MOVE_SECS, Easing::cubic(0.2, 0.0, 0.0, 1.0)));
/// `opacity 160ms ease-out`.
const FADE: Anim = Anim::new(Curve::tween(FADE_SECS, Easing::EASE_OUT));
/// Sonner's `[data-sonner-toast] > * { transition: opacity 400ms }`.
const CONTENT_FADE: Anim = Anim::new(Curve::tween(CONTENT_FADE_SECS, Easing::EASE));

#[derive(Clone, Debug)]
pub enum ToastAction {
    UndoRewind(UndoRewind),
    /// A plugin's button (its label) that shows a path in the file manager (desktop.reveal).
    Reveal(String, PathBuf),
}

/// A plugin toast's button (`ToastAction` with `run`): it closes the toast, then runs `run` in its
/// plugin or opens `url`. `secondary` is Desktop's ghost button, the rest its neutral one.
#[derive(Clone, Debug)]
pub struct PluginButton {
    pub label: String,
    pub secondary: bool,
    pub plugin: std::rc::Rc<str>,
    pub run: Option<mlua::Function>,
    pub url: Option<String>,
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
    /// A plugin's buttons, in place of `action`.
    pub buttons: Vec<PluginButton>,
    /// The handle `Toast:Show` answered with, which `Dismiss` names.
    pub handle: Option<u64>,
}

/// What a toast of the stack shows.
#[derive(Clone, Debug)]
pub enum Shown {
    Status(Entry),
    /// The uploads in flight (`toaster.show`'s custom upload toast).
    Uploads,
}

#[derive(Default)]
pub struct StatusToast {
    pub stack: ToastStack<Shown>,
}

impl StatusToast {
    /// The newest status toast.
    pub fn current(&self) -> Option<&Entry> {
        self.stack.entries().iter().rev().find_map(|e| match &e.options {
            Shown::Status(entry) => Some(entry),
            Shown::Uploads => None,
        })
    }

    /// [`Self::current`], to change.
    pub fn current_mut(&mut self) -> Option<&mut Entry> {
        self.stack.entries_mut().iter_mut().rev().find_map(|e| match &mut e.options {
            Shown::Status(entry) => Some(entry),
            Shown::Uploads => None,
        })
    }
}

/// Steps the stack and carries out what it asks.
fn step(s: &mut App, event: ToastEvent<Shown>, tx: Option<&mut Tx>) {
    let effects = s.toast.stack.step(event);
    let Some(tx) = tx else { return };
    for effect in effects {
        match effect {
            ToastEffect::WakeAt(due) => tx.wake_at(due),
            // Desktop pulses a toast shown again in front (`pulseToast`); Rune's lands still.
            ToastEffect::Pulse(_) | ToastEffect::Closed(_) => {}
        }
    }
}

/// Shows `entry` in front, for `secs` or until dismissed; its id. A copy still in front updates
/// in place, a buried one comes to the front (`showToast`'s key).
fn show(s: &mut App, entry: Entry, secs: Option<f64>) -> u64 {
    let action = entry.action.as_ref().map(|a| match a {
        ToastAction::UndoRewind(_) => "undo".to_owned(),
        ToastAction::Reveal(label, _) => label.clone(),
    });
    let key = format!("{}\u{1f}{}\u{1f}{secs:?}\u{1f}{action:?}", entry.title, entry.description.as_deref().unwrap_or(""));
    let now = s.clock;
    step(s, ToastEvent::Shown { key, options: Shown::Status(entry), secs, now }, None);
    // A toast shown from a handler or a poll times out on the next poll's wake.
    let Some(front) = s.toast.stack.front().map(|f| f.id) else { return 0 };
    if let Some(e) = s.toast.current_mut() {
        e.id = front;
    }
    front
}

fn entry(title: impl Into<String>, description: Option<String>, persistent: bool) -> Entry {
    Entry { id: 0, title: title.into(), description, action: None, icon: None, persistent, buttons: Vec::new(), handle: None }
}

/// Shows `"Reverted"` with an `"Undo"` button bound to `undo`.
pub fn rewound(s: &mut App, undo: UndoRewind) -> u64 {
    let e = Entry { action: Some(ToastAction::UndoRewind(undo)), ..entry(opencode_i18n::t!("session.rewind.toast.title"), None, false) };
    show(s, e, Some(DURATION))
}

/// Shows a message that leaves after [`DURATION`] s.
pub fn say(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    show(s, entry(title, description, false), Some(DURATION));
}

/// A success (`variant: "success"`): a message that leaves after [`DURATION`] s, after Desktop's
/// `circle-check`.
pub fn succeed(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    let e = Entry { icon: Some(&rune_theme_opencode::icons::CIRCLE_CHECK), ..entry(title, description, false) };
    show(s, e, Some(DURATION));
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
    let (title, description) = noted(s, title.to_owned(), String::new());
    show(s, entry(title, description, true), None)
}

/// Shows a failure toast that waits until dismissed, and records the message on `s.status`.
pub fn fail(s: &mut App, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    show(s, entry(title, description, true), None);
}

/// Records a toast's words on `s.status`; its title and its description, trimmed and cut at
/// 240 characters, or none.
fn noted(s: &mut App, title: String, desc: String) -> (String, Option<String>) {
    let trimmed = desc.trim();
    s.status = if trimmed.is_empty() { title.clone() } else { format!("{title}: {trimmed}") };
    let description = (!trimmed.is_empty()).then(|| trimmed.chars().take(240).collect());
    (title, description)
}

/// The upload toast shows (a first upload started), in front of the toasts before it.
pub fn uploads_shown(s: &mut App) {
    if s.toast.stack.entries().iter().any(|e| matches!(e.options, Shown::Uploads)) {
        return;
    }
    let now = s.clock;
    step(s, ToastEvent::Shown { key: "\u{1}uploads".into(), options: Shown::Uploads, secs: None, now }, None);
}

/// Dismisses the newest status toast, if any.
pub fn dismiss(s: &mut App) -> bool {
    let Some(id) = s.toast.current().map(|e| e.id) else { return false };
    let now = s.clock;
    step(s, ToastEvent::Dismissed { id: Some(id), now }, None);
    true
}

/// Dismisses toast `id` if it is still showing.
pub fn dismiss_id(s: &mut App, id: u64) {
    if s.toast.stack.entries().iter().any(|e| e.id == id) {
        let now = s.clock;
        step(s, ToastEvent::Dismissed { id: Some(id), now }, None);
    }
}

/// Closes toasts whose time ran out, and the upload toast once no upload is left; wakes for the
/// next timer.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let uploads = s.toast.stack.entries().iter().find(|e| matches!(e.options, Shown::Uploads)).map(|e| e.id);
    if let Some(id) = uploads.filter(|_| s.attach.uploads.is_empty()) {
        tx.instant();
        step(s, ToastEvent::Dismissed { id: Some(id), now }, Some(tx));
        return true;
    }
    let Some(due) = s.toast.stack.next_due() else { return false };
    if now + 1e-3 >= due {
        tx.instant();
        step(s, ToastEvent::Tick { now }, Some(tx));
        return true;
    }
    tx.wake_at(due);
    false
}

/// Sonner's hotkey, Alt+T wherever focus is, spreads the stack, and Escape puts a stack it
/// spread back (its document `keydown`, which takes nothing from the field typed in).
pub fn hotkey(s: &mut App, ev: &KeyEvent, tx: &mut Tx) {
    if ev.key == Key::Escape && s.toast.stack.stack() == rune_kit::ToastStackState::Expanded {
        let now = s.clock;
        step(s, ToastEvent::Escape { now }, Some(tx));
        tx.motion_curve(MOVE);
        return;
    }
    let t = |c: char| c.eq_ignore_ascii_case(&'t');
    let on_t = tx.key_base().is_some_and(t) || matches!(&ev.key, Key::Command(c) if t(*c)) || matches!(&ev.key, Key::Text(x) if x.chars().eq(['t']) || x.chars().eq(['T']));
    if !ev.mods.alt || !on_t || s.toast.stack.entries().is_empty() || s.toast.stack.stack().expanded() {
        return;
    }
    let now = s.clock;
    step(s, ToastEvent::Hotkey { now }, Some(tx));
    tx.motion_curve(MOVE);
}

/// Escape dismisses a persistent failure toast in front when no higher overlay owns the key.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain_esc = ev.key == Key::Escape && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift;
    if plain_esc && s.toast.current().is_some_and(|e| e.persistent) {
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

/// The stack (Sonner's `ol`): 32 in from the window's right and 48 up from its bottom
/// (`toast.tsx` offset), at 600 pt or narrower 16 in from its left and bottom and 64 short of
/// its width (`toast.css:226`, Sonner's mobile offsets). Each toast stands at its bottom where
/// [`ToastStack::place`] puts it, newest over the rest. The pointer anywhere over it, the gaps
/// between spread toasts included (Sonner's `::after`), spreads it.
pub fn view(s: &App, p: &Pal) -> Option<El> {
    let entries = s.toast.stack.entries();
    if entries.is_empty() {
        return None;
    }
    let key_of = |e: &rune_kit::ToastEntry<Shown>| match e.options {
        Shown::Status(_) => format!("status-toast-{}", e.id),
        Shown::Uploads => "upload-toast".to_owned(),
    };
    let height = |id: u64| entries.iter().find(|e| e.id == id).and_then(|e| rune::memo::layout_size(&key_of(e))).map_or(0.0, |[_, h]| h);
    let width = s.layout.narrow.width();
    let narrow = width > 0.0 && width <= 600.0;
    let w = if narrow { width - 64.0 } else { 320.0 };
    let mut reach = 0.0f32;
    let mut stack = el().key("toast-stack").role("region").label("Notifications alt+T").w(w);
    for (order, e) in entries.iter().enumerate() {
        let Some(at) = s.toast.stack.place(e.id, height) else { continue };
        if at.visible {
            reach = reach.max(at.lift + if at.content { at.height } else { at.height * at.scale + (1.0 - at.scale) * at.height * 0.5 });
        }
        let card = match &e.options {
            Shown::Status(entry) => card(p, entry, at.content),
            Shown::Uploads => crate::attachments::toast::card(s, p, at.content),
        };
        let mut card = card.key(key_of(e)).w(w).pin(Some(0.0), None, None, Some(0.0)).paint_order(order as i16);
        if !at.content {
            // Behind the front one, as tall as it (`--front-toast-height`).
            card = card.h(at.height).clip_soft(0.0);
        }
        stack = stack.child(
            card.offset(0.0, -at.lift)
                .offset_motion(MOVE)
                .scale(at.scale)
                .scale_motion(MOVE)
                .opacity(if at.visible { 1.0 } else { 0.0 })
                .opacity_motion(FADE),
        );
    }
    let stack = stack.h(reach.max(1.0)).on_hover_change(|s: &mut App, inside, tx: &mut Tx| {
        let now = s.clock;
        let event = if inside { ToastEvent::PointerOver { now } } else { ToastEvent::PointerLeft { now } };
        let was = s.toast.stack.stack();
        step(s, event, Some(tx));
        if s.toast.stack.stack() != was {
            // Every toast moves and resizes on Sonner's curve (`toast.css`).
            tx.motion_curve(MOVE);
        }
    });
    Some(if narrow { stack.pin(Some(16.0), None, None, Some(16.0)).raise() } else { stack.pin(None, None, Some(32.0), Some(48.0)).raise() })
}

/// The card (`.toast-v2`): 12 in, its icon's, content's and close button's columns 12 apart,
/// the actions 12 under the content in its column. Behind the front toast of a collapsed stack
/// its contents hide (`[data-expanded="false"][data-front="false"] > *`).
fn card(p: &Pal, entry: &Entry, content_shows: bool) -> El {
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
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dismiss_id(s, id);
        })
        // `icon-muted` at rest, `icon-base` under the pointer.
        .child(icon(&CLOSE, 16.0).color(p.icon_muted))
        .child(icon(&CLOSE, 16.0).color(p.icon).absolute(2.0, 2.0).group_opacity(0.0, 1.0));

    let action_btn = entry.action.as_ref().map(|action| {
        let (key, label) = match action {
            ToastAction::UndoRewind(_) => ("toast-undo", opencode_i18n::t!("session.rewind.toast.undo").to_owned()),
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
                    dismiss_id(s, id);
                    match action.clone() {
                        ToastAction::UndoRewind(undo) => s.undo_rewind(undo, tx),
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
    } else if !entry.buttons.is_empty() {
        let buttons = entry.buttons.iter().enumerate().map(|(i, b)| plugin_button(p, id, i, b.clone()));
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).wrap().children(buttons));
    }
    let lead = entry.icon.map(|glyph| el().key("toast-icon").w(16.0).h(20.0).center().child(icon(glyph, 16.0).color(p.icon)));
    let body = row()
        .key("toast-head")
        .w_full()
        .gap(12.0)
        .opacity(if content_shows { 1.0 } else { 0.0 })
        .opacity_motion(CONTENT_FADE)
        .children(lead)
        .child(content)
        .child(close);

    col()
        .role("status")
        .padding(12.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(body)
}

/// A plugin's toast button (`toast.tsx` `createToastActions`): `button-v2` small, neutral or
/// ghost, 13/530; a press closes the toast, then runs the plugin's `run` or opens its URL.
fn plugin_button(p: &Pal, id: u64, i: usize, b: PluginButton) -> El {
    let label = b.label.clone();
    let base = row().key(format!("toast-action-{i}")).role("button").label(label.clone()).h(24.0).px(9.0).radius(4.0).center();
    let styled = if b.secondary { base } else { base.fill(if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral }).elevate(neutral_lift(p)) };
    styled
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            dismiss_id(s, id);
            if let Some(url) = &b.url {
                let _ = tx.open_url(url);
            }
            let Some(run) = &b.run else { return };
            let Some(index) = s.plugins.luau.running().find(|(_, pl)| pl.half.id == b.plugin).map(|(i, _)| i) else { return };
            crate::plugin_host::luau::commands::spawn(s, index, run, (), tx);
        })
        .child(text(label).font(TITLE).color(p.text))
}

/// `--v2-elevation-button-neutral`: a drop and the 0.5px ring outside the button, and in dark the
/// 0.5px lit top edge (`theme.css`).
pub(crate) fn neutral_lift(p: &Pal) -> [Shadow; 4] {
    let (y, blur) = if p.dark { (1.0, 2.0) } else { (1.0, 1.5) };
    let top = if p.dark { Shadow { y: -0.5, color: Color::WHITE.alpha(0.1), ..Shadow::default() } } else { Shadow::default() };
    [Shadow { y, blur, color: p.q.neutral_shadow, ..Shadow::default() }, Pal::ring_out(p.q.neutral_ring), top, Shadow::default()]
}
