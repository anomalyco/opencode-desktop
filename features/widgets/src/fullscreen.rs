//! A widget full screen (`specs/v2/widgets-fullscreen.md`), as an explicit machine: one widget
//! at a time fills the window, and the window goes OS full screen with it (macOS native full
//! screen; borderless elsewhere). It is the same widget: its state, its live program and its
//! timers carry on, and the timeline shows a placeholder in its place. Esc, the close button,
//! or leaving OS full screen brings it back; so does it closing (answered here or elsewhere).
//!
//! ```text
//!          Enter ──▶ Full { os: Asked } ──window full──▶ Full { os: On }
//! Inline ◀─────────── Exit / Gone ─────────────────────── │ ◀── window left full screen
//!          Enter (window already full) ──▶ Full { os: Kept }   (left as it was on exit)
//!          Enter (no window to ask, or it refused) ──▶ Full { os: Off }
//! ```
//!
//! The widget hears `{ id: "", type: "fullscreen", value }` on entering and leaving (its
//! `update`, or a live program's `widget.event()`), and its frames say `fullscreen`.

use opencode_widget::{Event, WidgetEvent};
use rune::Tx;

use crate::{WidgetsHost, advance, live};

/// How long the window may take to report OS full screen before the widget fills it without.
const ASK_S: f64 = 3.0;

/// Where the window's own full screen stands while a widget fills it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Os {
    /// Asked at this clock; not full screen yet.
    Asked(f64),
    /// Full screen because the widget asked: leaving it exits, exiting leaves it.
    On,
    /// Already full screen when the widget entered: left so on exit.
    Kept,
    /// No OS full screen (a headless run, or the window didn't go): the widget fills the window.
    Off,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Screen {
    #[default]
    Inline,
    Full { id: String, os: Os },
}

impl Screen {
    pub fn is(&self, id: &str) -> bool {
        matches!(self, Screen::Full { id: i, .. } if i == id)
    }
}

pub enum ScreenEvent {
    Enter(String),
    /// Esc or the close button.
    Exit,
    /// The window's OS full screen as read now (`None`: no window to read).
    Window(Option<bool>),
    /// The full-screen widget closed or went away.
    Gone,
}

/// What a transition asks of the host.
#[derive(Debug, PartialEq)]
enum Effect {
    /// Toggle the window's OS full screen.
    ToggleOs,
    /// Tell widget `id` it entered (`true`) or left full screen.
    Tell(String, bool),
}

fn step(screen: &Screen, event: ScreenEvent, window: Option<bool>, now: f64) -> (Screen, Vec<Effect>) {
    use ScreenEvent as E;
    let leave = |id: &String, os: Os| {
        let mut fx = vec![Effect::Tell(id.clone(), false)];
        if matches!(os, Os::On | Os::Asked(_)) {
            fx.push(Effect::ToggleOs);
        }
        (Screen::Inline, fx)
    };
    match (screen, event) {
        (Screen::Inline, E::Enter(id)) => match window {
            Some(true) => (Screen::Full { id: id.clone(), os: Os::Kept }, vec![Effect::Tell(id, true)]),
            Some(false) => (Screen::Full { id: id.clone(), os: Os::Asked(now) }, vec![Effect::ToggleOs, Effect::Tell(id, true)]),
            None => (Screen::Full { id: id.clone(), os: Os::Off }, vec![Effect::Tell(id, true)]),
        },
        // Another widget asks: the one full screen goes back first, the window staying as it is.
        (Screen::Full { id: old, os }, E::Enter(id)) if *old != id => (Screen::Full { id: id.clone(), os: *os }, vec![Effect::Tell(old.clone(), false), Effect::Tell(id, true)]),
        (Screen::Full { id, os }, E::Exit | E::Gone) => leave(id, *os),
        (Screen::Full { id, os: Os::Asked(at) }, E::Window(w)) => match w {
            Some(true) => (Screen::Full { id: id.clone(), os: Os::On }, vec![]),
            _ if now - at > ASK_S => (Screen::Full { id: id.clone(), os: Os::Off }, vec![]),
            _ => (screen.clone(), vec![]),
        },
        // The user left OS full screen (the green button, a gesture): the widget follows.
        (Screen::Full { id, os: Os::On }, E::Window(Some(false))) => (Screen::Inline, vec![Effect::Tell(id.clone(), false)]),
        (s, _) => (s.clone(), vec![]),
    }
}

/// Moves the full-screen machine by `event` and runs what it asks.
pub fn send(host: &mut dyn WidgetsHost, event: ScreenEvent, now: f64, tx: &mut Tx) {
    let window = host.window_full_screen();
    let (next, effects) = step(&host.widgets().screen, event, window, now);
    host.widgets_mut().screen = next;
    for effect in effects {
        match effect {
            Effect::ToggleOs => tx.toggle_full_screen(),
            Effect::Tell(id, on) => tell(host, &id, on),
        }
    }
}

/// Widget `id` entered or left full screen: its frames say so, and its template hears it.
fn tell(host: &mut dyn WidgetsHost, id: &str, on: bool) {
    let Some(e) = host.widgets_mut().entries.get_mut(id) else { return };
    e.local.set_fullscreen(on);
    let event = WidgetEvent::fullscreen(on);
    if e.machine.running() {
        live::deliver(host, id, &event);
    } else if matches!(e.machine.phase, opencode_widget::Phase::Ready { .. }) {
        advance(host, id, Event::Input(event));
    }
}

/// Follows the window's OS full screen, and lets go of a widget that closed while full screen.
pub(crate) fn poll(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    let Screen::Full { id, os } = host.widgets().screen.clone() else { return false };
    let open = host.widgets().entries.get(&id).is_some_and(|e| e.machine.open_here());
    if !open {
        send(host, ScreenEvent::Gone, now, tx);
        return true;
    }
    if matches!(os, Os::Asked(_) | Os::On) {
        let window = host.window_full_screen();
        send(host, ScreenEvent::Window(window), now, tx);
        if matches!(os, Os::Asked(_)) {
            tx.wake_at(now + 0.1);
        }
        return host.widgets().screen != Screen::Full { id, os };
    }
    false
}
