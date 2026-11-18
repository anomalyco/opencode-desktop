//! Toast stack transitions, pulse state machine, timers, visibility, keyboard shortcuts, and
//! high-level toast builders.

use std::sync::OnceLock;

use rune::{Anim, Curve, Easing, Key, KeyEvent, LiveRegion, Tx};
use rune_kit::{ToastEffect, ToastEvent, ToastPage, ToastStackState};

use super::{DURATION, Entry, FailureNotice, MOVE, STACK, Shown, ToastAction, ToastHost};

/// `pulseToast`'s length and peak.
const PULSE_SECS: f32 = 0.16;
const PULSE_PEAK: f32 = 1.025;

/// Desktop's `pulseToast`: a toast shown again while it's in front grows 2.5% and back over
/// 160 ms on one ease-out (`element.animate([{ scale: 1 }, { scale: 1.025 }, { scale: 1 }], {
/// duration: 160, easing: "ease-out" })`), as two tweens that meet at the peak.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Pulse {
    #[default]
    Still,
    /// Growing to the peak, which it reaches at `until`.
    Rising { id: u64, until: f64 },
    /// Back to its size, which it reaches at `until`.
    Falling { id: u64, until: f64 },
}

pub(super) enum PulseEvent {
    Start { id: u64, now: f64 },
    Tick { now: f64 },
}

impl Pulse {
    /// Steps the pulse; returns when to wake for its next phase.
    pub(super) fn step(&mut self, event: PulseEvent) -> Option<f64> {
        let (rise, fall) = pulse_phases();
        *self = match (*self, event) {
            (_, PulseEvent::Start { id, now }) => Pulse::Rising { id, until: now + f64::from(rise) },
            (Pulse::Rising { id, until }, PulseEvent::Tick { now }) if now + 1e-3 >= until => {
                Pulse::Falling { id, until: until + f64::from(fall) }
            }
            (Pulse::Falling { until, .. }, PulseEvent::Tick { now }) if now + 1e-3 >= until => Pulse::Still,
            (pulse, PulseEvent::Tick { .. }) => pulse,
        };
        self.due()
    }

    pub(super) fn due(&self) -> Option<f64> {
        match *self {
            Pulse::Still => None,
            Pulse::Rising { until, .. } | Pulse::Falling { until, .. } => Some(until),
        }
    }

    /// Toast `id`'s scale and how it moves there while it pulses.
    pub(super) fn of(&self, id: u64) -> Option<(f32, Anim)> {
        let (rise, fall) = pulse_phases();
        match *self {
            Pulse::Rising { id: at, .. } if at == id => {
                Some((PULSE_PEAK, Anim::new(Curve::tween(rise, pulse_easing(true)))))
            }
            Pulse::Falling { id: at, .. } if at == id => {
                Some((1.0, Anim::new(Curve::tween(fall, pulse_easing(false)))))
            }
            _ => None,
        }
    }
}

/// Seconds to the peak and back: the peak is halfway through the eased progress, which
/// `ease-out` reaches early.
fn pulse_phases() -> (f32, f32) {
    static SPLIT: OnceLock<f32> = OnceLock::new();
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
    static STOPS: OnceLock<[&'static [[f32; 2]]; 2]> = OnceLock::new();
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
    Easing::Piecewise(if rising { stops[0] } else { stops[1] })
}

/// Steps the stack and carries out what it asks.
pub(super) fn step<S: ToastHost>(s: &mut S, event: ToastEvent<Shown<S::CustomAction>>, mut tx: Option<&mut Tx>) {
    let effects = s.toast_mut().stack.step(event);
    for effect in effects {
        let wake = match effect {
            ToastEffect::WakeAt(due) => Some(due),
            // A toast shown again in front pulses (`pulseToast`), unless motion is reduced.
            ToastEffect::Pulse(id) if !s.reduced_motion() => {
                let now = s.clock();
                s.toast_mut().pulse.step(PulseEvent::Start { id, now })
            }
            ToastEffect::Closed(id) => {
                s.on_closed(id);
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

/// Shows `entry` in front, for `secs` or until dismissed; returns its stack id. A copy still in
/// front updates in place, and a buried one comes to the front (`showToast`'s key).
pub fn show<S: ToastHost>(s: &mut S, entry: Entry<S::CustomAction>, secs: Option<f64>) -> u64 {
    let action = entry.action.as_ref().map(|a| match a {
        ToastAction::Reveal(label, _) => label.clone(),
        ToastAction::CopyDetails(_) => "details".to_owned(),
        ToastAction::Custom(c) => s.custom_action_meta(c).dedup,
    });
    let buttons: Vec<(&str, &str, bool)> = entry.buttons.iter().map(|b| (b.key.unwrap_or(""), b.label.as_str(), b.secondary)).collect();
    let parts = [entry.description.as_deref(), entry.quote.as_deref(), entry.next.as_deref()].map(|p| p.unwrap_or(""));
    let key = format!("{}\u{1f}{}\u{1f}{secs:?}\u{1f}{action:?}\u{1f}{buttons:?}", entry.title, parts.join("\u{1f}"));
    let now = s.clock();
    let words = std::iter::once(entry.title.as_str())
        .chain(parts)
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .fold(String::new(), |out, w| match out.chars().last() {
            None => w.to_owned(),
            Some('.' | ':' | '!' | '?') => format!("{out} {w}"),
            Some(_) => format!("{out}. {w}"),
        });
    s.announce(words, LiveRegion::Polite);
    step(s, ToastEvent::Shown { key, options: Shown::Status(Box::new(entry)), secs, now }, None);
    // A toast shown from a handler or a poll times out on the next poll's wake.
    let Some(front) = s.toast().stack.front().map(|f| f.id) else { return 0 };
    if let Some(e) = s.toast_mut().current_mut() {
        e.id = front;
    }
    front
}

/// Shows a message that leaves after [`DURATION`] s.
pub fn say<S: ToastHost>(s: &mut S, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    show(s, Entry::new(title, description, false), Some(DURATION));
}

/// A success (`variant: "success"`): a message that leaves after [`DURATION`] s, after Desktop's
/// `circle-check`.
pub fn succeed<S: ToastHost>(s: &mut S, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    let e = Entry { icon: Some(&rune_theme_opencode::icons::CIRCLE_CHECK), ..Entry::new(title, description, false) };
    show(s, e, Some(DURATION));
}

/// A line from the server's worker threads (`Incoming::Status`) that isn't a failure: an export
/// saved, the configuration reloaded, or the event stream's own line.
pub fn from_server<S: ToastHost>(s: &mut S, e: String) {
    if e == "Configuration reloaded" {
        s.set_status(String::new());
        return succeed(s, opencode_i18n::t!("command.config.reload.done"), String::new());
    }
    s.set_status(e);
}

/// Shows a structured failure toast (what happened, why, optional server quote and next step, plus
/// a Copy details action) until dismissed, interrupting screen-reader speech.
pub fn fail_details<S: ToastHost>(s: &mut S, notice: FailureNotice<'_>) -> u64 {
    let (title, _) = noted(s, notice.what.to_owned(), notice.status_summary.to_owned());
    let cut = |t: &str| t.trim().chars().take(240).collect::<String>();
    let entry = Entry {
        quote: notice.quote.map(cut),
        next: notice.next.map(cut),
        action: Some(ToastAction::CopyDetails(notice.details.to_owned())),
        ..Entry::new(title, Some(cut(notice.why)), true)
    };
    let id = show(s, entry, None);
    s.announce_urgent();
    id
}

/// Desktop's titles for a failed write (`prompt.toast.*`, `session.delete.failed.title`…).
pub fn write_title(method: &str, path: &str) -> &'static str {
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

/// The calm line for a server that has stayed silent: stays until [`dismiss_id`] clears it and
/// is not a failure. Returns its stack id.
pub fn waiting<S: ToastHost>(s: &mut S, title: &str) -> u64 {
    let (title, description) = noted(s, title.to_owned(), String::new());
    show(s, Entry::new(title, description, true), None)
}

/// Shows a failure toast that waits until dismissed, and records the message on the host status.
pub fn fail<S: ToastHost>(s: &mut S, title: impl Into<String>, description: impl Into<String>) {
    let (title, description) = noted(s, title.into(), description.into());
    show(s, Entry::new(title, description, true), None);
    // A failure interrupts what a reader is saying (`role="alert"`).
    s.announce_urgent();
}

/// Records a toast's words on the host status; returns its title and its description, trimmed and
/// cut at 240 characters, or `None`.
pub fn noted<S: ToastHost>(s: &mut S, title: String, desc: String) -> (String, Option<String>) {
    let trimmed = desc.trim();
    s.set_status(if trimmed.is_empty() { title.clone() } else { format!("{title}: {trimmed}") });
    let description = (!trimmed.is_empty()).then(|| trimmed.chars().take(240).collect());
    (title, description)
}

/// Shows the upload toast (when a first upload starts), in front of existing toasts.
pub fn uploads_shown<S: ToastHost>(s: &mut S) {
    if s.toast().stack.entries().iter().any(|e| matches!(e.options, Shown::Uploads)) {
        return;
    }
    let now = s.clock();
    step(s, ToastEvent::Shown { key: "\u{1}uploads".into(), options: Shown::Uploads, secs: None, now }, None);
}

/// Dismisses the newest status toast, if any.
pub fn dismiss<S: ToastHost>(s: &mut S) -> bool {
    let Some(id) = s.toast().current().map(|e| e.id) else { return false };
    let now = s.clock();
    step(s, ToastEvent::Dismissed { id: Some(id), now }, None);
    true
}

/// Dismisses toast `id` if it is still showing.
pub fn dismiss_id<S: ToastHost>(s: &mut S, id: u64) {
    if s.toast().stack.entries().iter().any(|e| e.id == id) {
        let now = s.clock();
        step(s, ToastEvent::Dismissed { id: Some(id), now }, None);
    }
}

/// Closes toasts whose time ran out, and the upload toast once no upload is left; wakes for the
/// next timer.
pub fn poll<S: ToastHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    let pulsed = match s.toast().pulse.due() {
        Some(due) if now + 1e-3 >= due => {
            if let Some(next) = s.toast_mut().pulse.step(PulseEvent::Tick { now }) {
                tx.wake_at(next);
            }
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

/// The window went off screen or came back: toasts' timers pause while hidden, as Sonner pauses
/// them while `document.hidden`.
pub fn visibility<S: ToastHost>(s: &mut S, shown: bool, tx: &mut Tx) {
    let page = if shown { ToastPage::Shown } else { ToastPage::Hidden };
    // The host's clock moves on in polls, which a hidden window may not have run.
    let now = tx.clock();
    step(s, ToastEvent::Page { page, now }, Some(tx));
}

fn toast_timers<S: ToastHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    // Keyboard focus on a toast spreads the stack and holds its timers, as the hotkey does, so
    // a toast never closes under the control a keyboard user is about to press.
    if !s.toast().stack.stack().expanded()
        && !s.toast().stack.entries().is_empty()
        && tx.focused().is_some_and(|k| k.starts_with("toast-"))
    {
        step(s, ToastEvent::Hotkey { now }, Some(tx));
        tx.motion_curve(MOVE);
    }
    let uploads = s.toast().stack.entries().iter().find(|e| matches!(e.options, Shown::Uploads)).map(|e| e.id);
    if let Some(id) = uploads.filter(|_| !s.has_uploads()) {
        tx.instant();
        step(s, ToastEvent::Dismissed { id: Some(id), now }, Some(tx));
        return true;
    }
    let Some(due) = s.toast().stack.next_due() else { return false };
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
pub fn hotkey<S: ToastHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) {
    let t = |c: char| c.eq_ignore_ascii_case(&'t');
    let on_t = tx.key_base().is_some_and(t)
        || matches!(&ev.key, Key::Command(c) if t(*c))
        || matches!(&ev.key, Key::Text(x) if x.chars().eq(['t']) || x.chars().eq(['T']));
    if !ev.mods.alt || !on_t || s.toast().stack.entries().is_empty() || s.toast().stack.stack().expanded() {
        return;
    }
    let now = s.clock();
    step(s, ToastEvent::Hotkey { now }, Some(tx));
    tx.motion_curve(MOVE);
    // The list takes focus from the field typed in, as `listRef.current.focus()` does.
    s.on_focus_taken();
    tx.focus(STACK, false);
}

/// Escape with focus in the stack puts a spread-out stack back (Sonner's `keydown`: Escape while
/// `document.activeElement` is the list or inside it), and lets whatever else hears it continue.
pub(super) fn stack_key<S: ToastHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.key == Key::Escape && s.toast().stack.stack() == ToastStackState::Expanded {
        let now = tx.clock();
        step(s, ToastEvent::Escape { now }, Some(tx));
        tx.motion_curve(MOVE);
    }
    false
}

/// Escape dismisses a persistent failure toast in front when no higher overlay owns the key.
pub fn key<S: ToastHost>(s: &mut S, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain_esc = ev.key == Key::Escape && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.shift;
    let dismissible = s.toast().current().is_some_and(|e| {
        e.persistent
            && !e.action.as_ref().is_some_and(|a| matches!(a, ToastAction::Custom(c) if s.custom_action_blocks_escape(c)))
    });
    if plain_esc && dismissible {
        tx.instant();
        dismiss(s);
        return true;
    }
    false
}
