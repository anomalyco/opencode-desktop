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
//! more peeking behind it; the pointer over the stack spreads them out, and while it's there or
//! the window is hidden no toast's time runs. One shown again in front pulses. Each also sets
//! `s.status`, which `--debug`'s status bar shows.

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
    /// Puts back what the toast says was done (`crate::undo`): its offer goes with the toast.
    Undo,
    /// Copies a failure's details (`crate::errors::UserError`).
    CopyDetails(String),
    /// A plugin's button (its label) that shows a path in the file manager (desktop.reveal).
    Reveal(String, PathBuf),
    /// A tip's Install (`crate::tips`): the tip is done and its page opens.
    Tip(crate::tips::queue::Tip),
}

/// Built-in recovery toast buttons (`crate::crash_notice`): first is primary neutral, rest ghost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeButton {
    OpenSkippedSession,
    ExportLogs,
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
    /// Someone else's words, shown as theirs under the description (a server's explanation).
    pub quote: Option<String>,
    /// What to do now, a line of its own under the rest.
    pub next: Option<String>,
    pub action: Option<ToastAction>,
    /// The glyph before the words: a success toast's tick.
    pub icon: Option<&'static Icon>,
    pub persistent: bool,
    /// Built-in recovery buttons (`crash_notice`), in place of `action`.
    pub native_buttons: Vec<NativeButton>,
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
    pulse: Pulse,
}

/// Desktop's `pulseToast`: a toast shown again while it's in front grows 2.5% and back over
/// 160 ms on one ease-out (`element.animate([{ scale: 1 }, { scale: 1.025 }, { scale: 1 }], {
/// duration: 160, easing: "ease-out" })`), as two tweens that meet at the peak.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Pulse {
    #[default]
    Still,
    /// Growing to the peak, which it reaches at `until`.
    Rising { id: u64, until: f64 },
    /// Back to its size, which it reaches at `until`.
    Falling { id: u64, until: f64 },
}

enum PulseEvent {
    Start { id: u64, now: f64 },
    Tick { now: f64 },
}

/// `pulseToast`'s length and peak.
const PULSE_SECS: f32 = 0.16;
const PULSE_PEAK: f32 = 1.025;

impl Pulse {
    /// Steps the pulse; when to wake for its next phase.
    fn step(&mut self, event: PulseEvent) -> Option<f64> {
        let (rise, fall) = pulse_phases();
        *self = match (*self, event) {
            (_, PulseEvent::Start { id, now }) => Pulse::Rising { id, until: now + f64::from(rise) },
            (Pulse::Rising { id, until }, PulseEvent::Tick { now }) if now + 1e-3 >= until => Pulse::Falling { id, until: until + f64::from(fall) },
            (Pulse::Falling { until, .. }, PulseEvent::Tick { now }) if now + 1e-3 >= until => Pulse::Still,
            (pulse, PulseEvent::Tick { .. }) => pulse,
        };
        self.due()
    }

    fn due(&self) -> Option<f64> {
        match *self {
            Pulse::Still => None,
            Pulse::Rising { until, .. } | Pulse::Falling { until, .. } => Some(until),
        }
    }

    /// Toast `id`'s scale and how it moves there while it pulses.
    fn of(&self, id: u64) -> Option<(f32, Anim)> {
        let (rise, fall) = pulse_phases();
        match *self {
            Pulse::Rising { id: at, .. } if at == id => Some((PULSE_PEAK, Anim::new(Curve::tween(rise, pulse_easing(true))))),
            Pulse::Falling { id: at, .. } if at == id => Some((1.0, Anim::new(Curve::tween(fall, pulse_easing(false))))),
            _ => None,
        }
    }
}

/// Seconds to the peak and back: the peak is halfway through the eased progress, which
/// `ease-out` reaches early.
fn pulse_phases() -> (f32, f32) {
    static SPLIT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let half = *SPLIT.get_or_init(|| {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..40 {
            let mid = (lo + hi) / 2.0;
            if Easing::EASE_OUT.eval(mid) < 0.5 { lo = mid } else { hi = mid }
        }
        (lo + hi) / 2.0
    });
    (PULSE_SECS * half, PULSE_SECS * (1.0 - half))
}

/// Each phase's share of the one ease-out, as a tween from its start to its end value.
fn pulse_easing(rising: bool) -> Easing {
    static STOPS: std::sync::OnceLock<[&'static [[f32; 2]]; 2]> = std::sync::OnceLock::new();
    let stops = STOPS.get_or_init(|| {
        let (rise, _) = pulse_phases();
        let split = rise / PULSE_SECS;
        let phase = |from: f32, to: f32| -> &'static [[f32; 2]] {
            let points: Vec<[f32; 2]> = (0..=32)
                .map(|i| {
                    let x = i as f32 / 32.0;
                    let p = Easing::EASE_OUT.eval(from + (to - from) * x);
                    let start = Easing::EASE_OUT.eval(from);
                    let end = Easing::EASE_OUT.eval(to);
                    [x, (p - start) / (end - start)]
                })
                .collect();
            Box::leak(points.into_boxed_slice())
        };
        [phase(0.0, split), phase(split, 1.0)]
    });
    Easing::Piecewise(stops[if rising { 0 } else { 1 }])
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
fn step(s: &mut App, event: ToastEvent<Shown>, mut tx: Option<&mut Tx>) {
    let effects = s.toast.stack.step(event);
    for effect in effects {
        let wake = match effect {
            ToastEffect::WakeAt(due) => Some(due),
            // A toast shown again in front pulses (`pulseToast`), unless motion is reduced.
            ToastEffect::Pulse(id) if !*s.reduced_motion => s.toast.pulse.step(PulseEvent::Start { id, now: *s.clock }),
            ToastEffect::Closed(id) => {
                crate::undo::toast_closed(s, id);
                crate::crash_notice::toast_closed(s, id);
                None
            }
            ToastEffect::Pulse(_) => None,
        };
        // Without a `Tx`, the next poll wakes for it.
        if let (Some(due), Some(tx)) = (wake, tx.as_deref_mut()) {
            tx.wake_at(due);
        }
    }
}

/// Shows `entry` in front, for `secs` or until dismissed; its id. A copy still in front updates
/// in place, a buried one comes to the front (`showToast`'s key).
fn show(s: &mut App, entry: Entry, secs: Option<f64>) -> u64 {
    let action = entry.action.as_ref().map(|a| match a {
        ToastAction::UndoRewind(_) | ToastAction::Undo => "undo".to_owned(),
        ToastAction::Reveal(label, _) => label.clone(),
        ToastAction::CopyDetails(_) => "details".to_owned(),
        ToastAction::Tip(tip) => tip.install().to_owned(),
    });
    let parts = [entry.description.as_deref(), entry.quote.as_deref(), entry.next.as_deref()].map(|p| p.unwrap_or(""));
    let key = format!("{}\u{1f}{}\u{1f}{secs:?}\u{1f}{action:?}\u{1f}{:?}", entry.title, parts.join("\u{1f}"), entry.native_buttons);
    let now = *s.clock;
    let words = std::iter::once(entry.title.as_str()).chain(parts).map(str::trim).filter(|w| !w.is_empty()).fold(String::new(), |out, w| match out.chars().last() {
        None => w.to_owned(),
        Some('.' | ':' | '!' | '?') => format!("{out} {w}"),
        Some(_) => format!("{out}. {w}"),
    });
    s.speech.say(words, rune::LiveRegion::Polite);
    step(s, ToastEvent::Shown { key, options: Shown::Status(entry), secs, now: now }, None);
    // A toast shown from a handler or a poll times out on the next poll's wake.
    let Some(front) = s.toast.stack.front().map(|f| f.id) else { return 0 };
    if let Some(e) = s.toast.current_mut() {
        e.id = front;
    }
    front
}

fn entry(title: impl Into<String>, description: Option<String>, persistent: bool) -> Entry {
    Entry { id: 0, title: title.into(), description, quote: None, next: None, action: None, icon: None, persistent, native_buttons: Vec::new(), buttons: Vec::new(), handle: None }
}

/// Non-modal next-launch notice after an unexpected close (`crate::crash_notice`).
pub fn crash_notice(
    s: &mut App,
    title: String,
    description: String,
    restored_line: Option<String>,
    native_buttons: Vec<NativeButton>,
) -> u64 {
    let summary = match &restored_line {
        Some(r) => format!("{title}: {description} {r}"),
        None => format!("{title}: {description}"),
    };
    *s.status = summary.into();
    let id = show(
        s,
        Entry {
            next: restored_line,
            native_buttons,
            ..entry(title, Some(description), true)
        },
        None,
    );
    s.speech.urgent();
    id
}

/// Shows `"Reverted"` with an `"Undo"` button bound to `undo`.
pub fn rewound(s: &mut App, undo: UndoRewind) -> u64 {
    let e = Entry { action: Some(ToastAction::UndoRewind(undo)), ..entry(opencode_i18n::t!("session.rewind.toast.title"), None, false) };
    show(s, e, Some(DURATION))
}

/// Shows `title` with an `Undo` button for [`crate::undo::SECS`] s; its id, which the offer goes by.
pub fn undoable(s: &mut App, title: &str) -> u64 {
    let e = Entry { action: Some(ToastAction::Undo), ..entry(title, None, false) };
    show(s, e, Some(crate::undo::SECS))
}

/// Shows `tip` until the person closes it or presses Install (`tips/model.ts`: `persistent`,
/// one action); its id.
pub fn tip(s: &mut App, tip: crate::tips::queue::Tip) -> u64 {
    let e = Entry { action: Some(ToastAction::Tip(tip)), ..entry(tip.title(), Some(tip.description().to_owned()), true) };
    show(s, e, None)
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

/// A line from the server's worker threads (`Incoming::Status`) that isn't a failure: an export
/// saved, the configuration reloaded, or the event stream's own line (the link said it already).
pub fn from_server(s: &mut App, e: String) {
    if e == "Configuration reloaded" {
        *s.status = String::new();
        return succeed(s, opencode_i18n::t!("command.config.reload.done"), String::new());
    }
    // A failure comes typed (`Incoming::Failed`, `from_wire`); what's left is a line for the
    // debug bar (the event stream's own, which the link already said).
    *s.status = e;
}
/// A call titled `what` failed with `wire`: why and what next, the server's words quoted where
/// it gave them, built from the typed error (`crate::errors`).
pub fn failed_wire(s: &mut App, what: &str, wire: &opencode_kit::wire_error::WireError) {
    let server = crate::servers::runtime::label(crate::servers::runtime::active(s));
    error(s, &crate::errors::UserError::from_wire(what, &server, wire));
}

/// A read or write that failed (`Incoming::Failed`): a write shows Desktop's toast for it, said
/// from the typed error; a read stays in the debug bar, since the app retries those itself.
pub fn from_wire(s: &mut App, e: opencode_kit::wire_error::WireError) {
    if e.method == "GET" {
        *s.status = e.to_string();
        return;
    }
    let server = crate::servers::runtime::label(crate::servers::runtime::active(s));
    let title = if e.method.is_empty() { opencode_i18n::t!("common.requestFailed") } else { write_title(e.method, &e.path) };
    error(s, &crate::errors::UserError::from_wire(title, &server, &e));
}

/// Shows `e`: what happened, why and what to do, with Copy details, until dismissed.
pub fn error(s: &mut App, e: &crate::errors::UserError) {
    let (title, _) = noted(s, e.what.clone(), e.description().replace('\n', " "));
    let cut = |t: &str| t.trim().chars().take(240).collect::<String>();
    let entry = Entry {
        quote: e.quote.as_deref().map(cut),
        next: e.next.as_deref().map(cut),
        action: Some(ToastAction::CopyDetails(e.details.clone())),
        ..entry(title, Some(cut(&e.why)), true)
    };
    show(s, entry, None);
    s.speech.urgent();
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
        ("POST", "reload") => opencode_i18n::t!("command.config.reload.failed"),
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
    // A failure interrupts what a reader is saying (`role="alert"`).
    s.speech.urgent();
}

/// Records a toast's words on `s.status`; its title and its description, trimmed and cut at
/// 240 characters, or none.
fn noted(s: &mut App, title: String, desc: String) -> (String, Option<String>) {
    let trimmed = desc.trim();
    *s.status = if trimmed.is_empty() { title.clone() } else { format!("{title}: {trimmed}").into() };
    let description = (!trimmed.is_empty()).then(|| trimmed.chars().take(240).collect());
    (title, description)
}

/// The upload toast shows (a first upload started), in front of the toasts before it.
pub fn uploads_shown(s: &mut App) {
    if s.toast.stack.entries().iter().any(|e| matches!(e.options, Shown::Uploads)) {
        return;
    }
    let now = *s.clock;
    step(s, ToastEvent::Shown { key: "\u{1}uploads".into(), options: Shown::Uploads, secs: None, now: now }, None);
}

/// Dismisses the newest status toast, if any.
pub fn dismiss(s: &mut App) -> bool {
    let Some(id) = s.toast.current().map(|e| e.id) else { return false };
    let now = *s.clock;
    step(s, ToastEvent::Dismissed { id: Some(id), now: now }, None);
    true
}

/// Dismisses toast `id` if it is still showing.
pub fn dismiss_id(s: &mut App, id: u64) {
    if s.toast.stack.entries().iter().any(|e| e.id == id) {
        let now = *s.clock;
        step(s, ToastEvent::Dismissed { id: Some(id), now: now }, None);
    }
}

/// Closes toasts whose time ran out, and the upload toast once no upload is left; wakes for the
/// next timer.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let pulsed = match s.toast.pulse.due() {
        Some(due) if now + 1e-3 >= due => {
            if let Some(next) = s.toast.pulse.step(PulseEvent::Tick { now }) { tx.wake_at(next); }
            true
        }
        Some(due) => {
            tx.wake_at(due);
            false
        }
        None => false,
    };
    toast_timers(s, now, tx) || pulsed
}

/// The window went off screen or came back (`App::on_visibility`): toasts' time waits while
/// it's hidden, as Sonner pauses them while `document.hidden`.
pub fn visibility(s: &mut App, shown: bool, tx: &mut Tx) {
    let page = if shown { rune_kit::ToastPage::Shown } else { rune_kit::ToastPage::Hidden };
    // The app's clock moves on in polls, which a hidden window may not have run.
    let now = tx.clock();
    step(s, ToastEvent::Page { page, now }, Some(tx));
}

fn toast_timers(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    // Keyboard focus on a toast spreads the stack and holds its timers, as the hotkey does, so
    // a toast never closes under the control a keyboard user is about to press.
    if !s.toast.stack.stack().expanded() && !s.toast.stack.entries().is_empty() && tx.focused().is_some_and(|k| k.starts_with("toast-")) {
        step(s, ToastEvent::Hotkey { now }, Some(tx));
        tx.motion_curve(MOVE);
    }
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

/// Sonner's hotkey, Alt+T wherever focus is, spreads the stack and focuses it (its document
/// `keydown`, which takes nothing from the field typed in).
pub fn hotkey(s: &mut App, ev: &KeyEvent, tx: &mut Tx) {
    let t = |c: char| c.eq_ignore_ascii_case(&'t');
    let on_t = tx.key_base().is_some_and(t) || matches!(&ev.key, Key::Command(c) if t(*c)) || matches!(&ev.key, Key::Text(x) if x.chars().eq(['t']) || x.chars().eq(['T']));
    if !ev.mods.alt || !on_t || s.toast.stack.entries().is_empty() || s.toast.stack.stack().expanded() {
        return;
    }
    let now = *s.clock;
    step(s, ToastEvent::Hotkey { now: now }, Some(tx));
    tx.motion_curve(MOVE);
    // The list takes focus from the field typed in, as `listRef.current.focus()` does.
    s.composer.focused = false;
    tx.focus(STACK, false);
}

/// Escape with focus in the stack puts a stack spread out back (Sonner's `keydown`: Escape while
/// `document.activeElement` is the list or inside it), and goes on to whatever else hears it.
fn stack_key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && s.toast.stack.stack() == rune_kit::ToastStackState::Expanded {
        let now = tx.clock();
        step(s, ToastEvent::Escape { now }, Some(tx));
        tx.motion_curve(MOVE);
    }
    false
}

/// The stack's key: Sonner's list (`ol`, `tabIndex={-1}`), which the hotkey focuses.
const STACK: &str = "toast-stack";

/// Escape dismisses a persistent failure toast in front when no higher overlay owns the key.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain_esc = ev.key == Key::Escape && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift;
    // A tip isn't a failure: it stays until its own × (Sonner's toasts take no Escape).
    if plain_esc && s.toast.current().is_some_and(|e| e.persistent && !matches!(e.action, Some(ToastAction::Tip(_)))) {
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
/// (`packages/ui/src/feedback/toast/toast.tsx:48-51`), at 600 pt or narrower 16 in from both
/// sides (`max-width: calc(100vw - 32px)`, `toast.css:226-233`, `mobileOffset={16}`). Official's
/// `<Toast.Region />` (`packages/app/src/shell/notifications/toast.tsx:26`) stays at `bottom: 16`
/// on narrow screens and covers the composer; here [`toast_bottom`] lifts the stack 8 pt above
/// `"dock"` / `"composer"` whenever the two overlap horizontally (deliberate difference).
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
    let w = if narrow { (width - 32.0).max(0.0) } else { 320.0 };
    let mut reach = 0.0f32;
    // Focusable outside the Tab order, so the hotkey can focus it and Escape there reaches it;
    // a press on a toast focuses it as one on a toast's `li` does.
    let mut stack = el().key(STACK).role("region").label("Notifications alt+T").w(w).focusable(true).tab_index(-1).on_key(stack_key);
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
        let (pulse, scale_motion) = s.toast.pulse.of(e.id).unwrap_or((1.0, MOVE));
        stack = stack.child(
            card.offset(0.0, -at.lift)
                .offset_motion(MOVE)
                .scale(at.scale * pulse)
                .scale_motion(scale_motion)
                .opacity(if at.visible { 1.0 } else { 0.0 })
                .opacity_motion(FADE),
        );
    }
    let bottom = toast_bottom(s, narrow, w, reach);
    let stack = stack.h(reach.max(1.0)).on_hover_change(|s: &mut App, inside, tx: &mut Tx| {
        let now = *s.clock;
        let event = if inside { ToastEvent::PointerOver { now: now } } else { ToastEvent::PointerLeft { now: now } };
        let was = s.toast.stack.stack();
        step(s, event, Some(tx));
        if s.toast.stack.stack() != was {
            // Every toast moves and resizes on Sonner's curve (`toast.css`).
            tx.motion_curve(MOVE);
        }
    });
    Some(if narrow { stack.pin(Some(16.0), None, None, Some(bottom)).raise() } else { stack.pin(None, None, Some(32.0), Some(bottom)).raise() })
}

fn rect_identity(r: [f32; 4]) -> [f32; 4] {
    r
}

fn rect_top(r: [f32; 4]) -> f32 {
    r[1]
}

fn toast_bottom(s: &App, narrow: bool, w: f32, reach: f32) -> f32 {
    let base = if narrow { 16.0 } else { 48.0 };
    let on_session = !s.settings.open
        && matches!(*s.route, crate::Route::Session { .. })
        && (!s.narrow() || s.narrow_view() == crate::narrow::machine::CONVERSATION);
    let on_draft = !s.settings.open && matches!(*s.route, crate::Route::Draft(_));
    if !on_session && !on_draft {
        return base;
    }
    let Some([wx, wy, ww, wh]) = rune::memo::layout_with("window", rect_identity) else {
        return base;
    };
    let Some([cx, cy, cw, ch]) = rune::memo::layout_with("composer", rect_identity) else {
        return base;
    };
    let (tx0, tx1) = if narrow {
        (wx + 16.0, wx + 16.0 + w)
    } else if crate::view::ui_direction() == rune::Direction::Rtl {
        (wx + 32.0, wx + 32.0 + w)
    } else {
        (wx + ww - 32.0 - w, wx + ww - 32.0)
    };
    if !(tx0 < cx + cw && tx1 > cx) {
        return base;
    }
    let top_y = if on_session {
        rune::memo::layout_with("dock", rect_top).map_or(cy, |dy| cy.min(dy))
    } else {
        cy
    };
    if on_draft {
        let toast_h = reach.max(64.0);
        if !((wy + wh - base) > top_y && (wy + wh - base - toast_h) < cy + ch) {
            return base;
        }
    }
    base.max(wy + wh - top_y + 8.0)
}

/// The card (`.toast-v2`): 12 in, its icon's, content's and close button's columns 12 apart,
/// the actions 12 under the content in its column. Behind the front toast of a collapsed stack
/// its contents hide (`[data-expanded="false"][data-front="false"] > *`).
fn card(p: &Pal, entry: &Entry, content_shows: bool) -> El {
    let id = entry.id;
    let close = row()
        .key("toast-dismiss")
        .role("button")
        .label_static(opencode_i18n::t!("ui.common.dismiss"))
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
            // `ToastOptions.onDismiss`: the person closed it.
            crate::tips::dismissed(s, id, tx);
        })
        // `icon-muted` at rest, `icon-base` under the pointer.
        .child(icon(&CLOSE, 16.0).color(p.icon_muted))
        .child(icon(&CLOSE, 16.0).color(p.icon).absolute(2.0, 2.0).group_opacity(0.0, 1.0));

    let action_btn = entry.action.as_ref().map(|action| {
        let (key, label) = match action {
            ToastAction::UndoRewind(_) | ToastAction::Undo => ("toast-undo", opencode_i18n::t!("session.rewind.toast.undo").to_owned()),
            ToastAction::Reveal(label, _) => ("toast-reveal", label.clone()),
            ToastAction::CopyDetails(_) => ("toast-copy-details", opencode_i18n::t!("rune.error.copyDetails").to_owned()),
            ToastAction::Tip(tip) => ("toast-tip-install", tip.install().to_owned()),
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
                    // The offer is taken before its toast goes, which would withdraw it.
                    if matches!(action, ToastAction::Undo) {
                        crate::undo::pressed(s, id, tx);
                    }
                    dismiss_id(s, id);
                    match action.clone() {
                        ToastAction::UndoRewind(undo) => s.undo_rewind(undo, tx),
                        ToastAction::Reveal(_, path) => crate::plugin_host::reveal(&path.to_string_lossy()),
                        ToastAction::Undo => {}
                        ToastAction::CopyDetails(details) => tx.copy_rich(rune::ClipboardContent::new(details)),
                        ToastAction::Tip(tip) => crate::tips::installed(s, id, tip, tx),
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
        .children(entry.description.as_ref().map(|desc| text(desc.clone()).key("toast-description").font(DESC).color(p.muted).wrap().w_full()))
        // The server's words, set apart by a rule as a quotation is, in the text colour: theirs.
        .children(entry.quote.as_ref().map(|q| {
            el().key("toast-quote").role("note").w_full().pl(9.0).py(1.0).border_sides([0.0, 0.0, 0.0, 2.0], p.border).child(text(q.clone()).font(DESC).color(p.text).wrap().w_full())
        }))
        .children(entry.next.as_ref().map(|next| text(next.clone()).key("toast-next").font(DESC).color(p.muted).wrap().w_full().mt(4.0)));
    if let Some(btn) = action_btn {
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).child(btn).child(spacer().fill(Color::TRANSPARENT)));
    } else if !entry.native_buttons.is_empty() {
        let buttons = entry.native_buttons.iter().enumerate().map(|(i, &b)| native_button(p, id, i > 0, b));
        content = content.child(row().key("toast-actions").mt(8.0).gap(8.0).wrap().children(buttons));
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

    // A group named by its title: what it says reaches a reader once, through the announcement
    // `show` makes, not as a live region that also reads its buttons as part of the message.
    col()
        .role("group")
        .label(entry.title.clone())
        .padding(12.0)
        .radius(8.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .on_press(|_: &mut App| {})
        .child(body)
}

/// A built-in recovery toast button (`crash_notice`): primary neutral or secondary ghost.
fn native_button(p: &Pal, id: u64, secondary: bool, button: NativeButton) -> El {
    let (key, label) = match button {
        NativeButton::OpenSkippedSession => ("toast-open-anyway", opencode_i18n::t!("rune.crash.openAnyway").to_owned()),
        NativeButton::ExportLogs => ("toast-export-logs", opencode_i18n::t!("command.logs.export").to_owned()),
    };
    let base = row().key(key).role("button").label(label.clone()).h(24.0).px(9.0).radius(4.0).center();
    let styled = if secondary { base } else { base.fill(if p.dark && p.theme == 0 { Color::WHITE.alpha(0.06) } else { p.q.neutral }).elevate(neutral_lift(p)) };
    styled
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            match button {
                NativeButton::OpenSkippedSession => crate::crash_notice::open_anyway(s, id, tx),
                NativeButton::ExportLogs => s.export_logs(),
            }
        })
        .child(text(label).font(TITLE).color(p.text))
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
            let Some(index) = s.plugins.running().find(|(_, pl)| pl.half.id == b.plugin).map(|(i, _)| i) else { return };
            crate::plugin_host::commands::spawn(s, index, run, (), tx);
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
