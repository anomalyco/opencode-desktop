//! A widget's keyboard focus and pointer capture (`specs/v2/widgets-input.md`), one widget at a
//! time, as an explicit machine:
//!
//! ```text
//!        click / Tab / a key inside                     the next click, capture asked for
//! Free ──────────────────────────▶ Focused { Off } ───────────────────────────▶ Focused { On }
//!   ▲                                  │   ▲ releasePointer / root prop dropped ─────┘
//!   └── Esc, a click outside, the window losing focus, leaving full screen,
//!       another session, the widget closing: releases both at once ──┘
//! ```
//!
//! Focused, the widget's keys go to its template (`key` events), except Cmd chords (Ctrl off
//! macOS), which the app keeps; Esc is never delivered. Captured, the cursor hides, presses
//! arrive as `down`/`up` with their button and moves as relative `dx`, `dy`; the cursor is put
//! back where capture began as it drifts (`WidgetsHost::warp_pointer`), so it stays over the
//! widget. Releasing sends `up` for keys still held, `focus: false`, and `capture: false` if it
//! held the pointer.

use opencode_widget::{Event, Key, PointerKind, WidgetEvent};
use opencode_widget_view::Input;
use rune::Tx;

use crate::{WidgetsHost, advance, live};

/// How far the hidden cursor may drift from where capture began before it is put back.
const DRIFT: f32 = 24.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Focus {
    #[default]
    Free,
    Focused { id: String, keys: Vec<(String, String)>, capture: Capture },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Capture {
    Off,
    /// Held: the cursor's place on screen when granted, and where the element that hears it saw
    /// it then and last.
    On { home: Option<[f64; 2]>, at_home: Option<[f32; 2]>, last: Option<[f32; 2]> },
}

/// Sends `event` to widget `id`'s template: its `update`, or its live program.
pub(crate) fn tell(host: &mut dyn WidgetsHost, id: &str, event: WidgetEvent) {
    let Some(e) = host.widgets().entries.get(id) else { return };
    if e.machine.running() {
        live::deliver(host, id, &event);
    } else if matches!(e.machine.phase, opencode_widget::Phase::Ready { .. }) {
        advance(host, id, Event::Input(event));
    }
}

/// Shows the machine's state in widget `id`'s view.
fn show(host: &mut dyn WidgetsHost, id: &str) {
    let (focused, captured) = match &host.widgets().focus {
        Focus::Focused { id: f, capture, .. } if f == id => (true, matches!(capture, Capture::On { .. })),
        _ => (false, false),
    };
    if let Some(e) = host.widgets_mut().entries.get_mut(id) {
        e.local.set_input(focused, captured);
    }
}

/// Widget `id` takes the keyboard (a click, Tab or a key inside it); another that had it lets go.
fn focus(host: &mut dyn WidgetsHost, id: &str) {
    if matches!(&host.widgets().focus, Focus::Focused { id: f, .. } if f == id) {
        return;
    }
    release(host);
    host.widgets_mut().focus = Focus::Focused { id: id.to_owned(), keys: Vec::new(), capture: Capture::Off };
    show(host, id);
    tell(host, id, WidgetEvent::focus(true));
}

/// Lets go of the keyboard and the pointer at once.
pub fn release(host: &mut dyn WidgetsHost) {
    let Focus::Focused { id, keys, capture } = std::mem::take(&mut host.widgets_mut().focus) else { return };
    for (key, code) in keys {
        tell(host, &id, WidgetEvent::key(&Key { kind: "up".into(), key, code, repeat: false, shift: false, alt: false, ctrl: false, meta: false }));
    }
    show(host, &id);
    tell(host, &id, WidgetEvent::focus(false));
    if matches!(capture, Capture::On { .. }) {
        tell(host, &id, WidgetEvent::capture(false));
    }
}

/// Gives the pointer back while keeping the keyboard (`releasePointer`, the root prop dropped).
fn uncapture(host: &mut dyn WidgetsHost, id: &str) {
    let Focus::Focused { id: f, capture, .. } = &mut host.widgets_mut().focus else { return };
    if f != id || matches!(capture, Capture::Off) {
        return;
    }
    *capture = Capture::Off;
    show(host, id);
    tell(host, id, WidgetEvent::capture(false));
}

/// What widget `id`'s view heard; true when it was taken.
pub fn take(host: &mut dyn WidgetsHost, id: &str, input: Input<'_>, tx: &mut Tx) -> bool {
    match input {
        Input::Key { event, up } => {
            // Esc is never delivered: it always lets go of both at once.
            if event.key == rune::Key::Escape {
                if !up {
                    release(host);
                    tx.blur();
                }
                return true;
            }
            // The app's chords (Cmd, Ctrl off macOS) still reach it.
            if event.mods.cmd {
                return false;
            }
            if !up {
                focus(host, id);
            }
            let key = Key {
                kind: if up { "up" } else { "down" }.into(),
                key: dom_key(&event.key),
                code: tx.physical_key().map(|c| format!("{c:?}")).unwrap_or_default(),
                repeat: tx.key_repeat(),
                shift: event.mods.shift,
                alt: event.mods.alt,
                ctrl: event.mods.ctrl,
                meta: event.mods.logo,
            };
            let Focus::Focused { id: f, keys, .. } = &mut host.widgets_mut().focus else { return false };
            if f != id {
                return false;
            }
            let pair = (key.key.clone(), key.code.clone());
            if up {
                keys.retain(|k| *k != pair);
            } else if !keys.contains(&pair) {
                keys.push(pair);
            }
            tell(host, id, WidgetEvent::key(&key));
            true
        }
        Input::Press => {
            focus(host, id);
            let key = if host.widgets().screen.is(id) { "widget-screen".to_owned() } else { format!("widget-{id}") };
            tx.focus(key, false);
            let wants = host.widgets().entries.get(id).is_some_and(|e| e.wants_capture());
            let home = host.pointer_at();
            if let Focus::Focused { capture: c @ Capture::Off, .. } = &mut host.widgets_mut().focus
                && wants
            {
                *c = Capture::On { home, at_home: None, last: None };
                show(host, id);
                tell(host, id, WidgetEvent::capture(true));
            }
            true
        }
        Input::Captured { id: node, kind, at, button } => {
            let node = node.unwrap_or("").to_owned();
            let mut warp = None;
            let event = {
                let Focus::Focused { id: f, capture: Capture::On { home, at_home, last }, .. } = &mut host.widgets_mut().focus else { return false };
                if f != id {
                    return false;
                }
                match kind {
                    PointerKind::Move => {
                        let base = at_home.get_or_insert(at);
                        let from = last.replace(at).unwrap_or(at);
                        // Drifted off: back where capture began, which the next move counts from.
                        if (at[0] - base[0]).abs() > DRIFT || (at[1] - base[1]).abs() > DRIFT {
                            warp = *home;
                            if warp.is_some() {
                                *last = Some(*base);
                            }
                        }
                        let (dx, dy) = (at[0] - from[0], at[1] - from[1]);
                        if dx == 0.0 && dy == 0.0 {
                            None
                        } else {
                            Some(WidgetEvent::captured_move(node, f64::from(dx), f64::from(dy)))
                        }
                    }
                    PointerKind::Down => Some(WidgetEvent::captured_button(node, true, button)),
                    PointerKind::Up => Some(WidgetEvent::captured_button(node, false, button)),
                }
            };
            if let Some(to) = warp {
                host.warp_pointer(to);
            }
            if let Some(event) = event {
                tell(host, id, event);
            }
            true
        }
    }
}

/// Lets go when the focused widget closed, went away, or its session isn't shown (unless it is
/// full screen), when focus moved outside it, and gives the pointer back when the widget no
/// longer asks for it.
pub(crate) fn poll(host: &mut dyn WidgetsHost, tx: &mut Tx) {
    let Focus::Focused { id, capture, .. } = host.widgets().focus.clone() else { return };
    let shown = host.widget_session().map(|s| s.0);
    let Some(e) = host.widgets().entries.get(&id) else {
        host.widgets_mut().focus = Focus::Free;
        return;
    };
    let full = host.widgets().screen.is(&id);
    let here = e.machine.open_here() && (full || shown.as_deref() == Some(e.session.as_str()));
    // Focus on the widget's frame, or a control in it (keyed by its node path).
    let inside = tx.focused().is_some_and(|k| k == format!("widget-{id}") || (full && k == "widget-screen") || k == "w" || k.starts_with("w."));
    if !here || !inside {
        release(host);
        return;
    }
    if matches!(capture, Capture::On { .. }) && !e.wants_capture() {
        uncapture(host, &id);
    }
}

/// The window lost focus: the widget lets go of both.
pub fn window_focus(host: &mut dyn WidgetsHost, focused: bool) {
    if !focused {
        release(host);
    }
}

/// The DOM `KeyboardEvent.key` for Rune's key.
fn dom_key(key: &rune::Key) -> String {
    use rune::Key as K;
    match key {
        K::Text(t) => t.clone(),
        K::Command(c) => c.to_string(),
        K::Enter => "Enter".into(),
        K::Backspace => "Backspace".into(),
        K::Delete => "Delete".into(),
        K::Left => "ArrowLeft".into(),
        K::Right => "ArrowRight".into(),
        K::Up => "ArrowUp".into(),
        K::Down => "ArrowDown".into(),
        K::Home => "Home".into(),
        K::End => "End".into(),
        K::PageUp => "PageUp".into(),
        K::PageDown => "PageDown".into(),
        K::Insert => "Insert".into(),
        K::Escape => "Escape".into(),
        K::Tab => "Tab".into(),
        K::Function(n) => format!("F{n}"),
        other => format!("{other:?}"),
    }
}
