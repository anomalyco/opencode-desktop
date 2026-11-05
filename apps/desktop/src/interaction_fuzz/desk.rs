//! Kit at the keyboard: the ops that are keys and taps, played the same way against the sim
//! ([`super::world`]) and a real server ([`super::real`]).

use rune::{Key, KeyEvent, Mods, Runtime, TextEdit};

use super::ops::Op;
use super::oracle::{Kind, Oracle};
use super::trace::Trace;
use crate::App;

pub trait Desk {
    fn rt(&mut self) -> &mut Runtime<App>;
    fn now(&self) -> f64;
    /// Lets `dt` pass: one frame.
    fn tick(&mut self, dt: f64);
    /// Puts what the app wrote on the wire.
    fn flush(&mut self);
    /// Interrupts the app has written so far.
    fn interrupts(&self) -> usize;
    fn trace(&mut self) -> &mut Trace;
    fn oracle(&mut self) -> &mut Oracle;
    /// The next prompt's words: `p1`, `p2`…
    fn next_prompt(&mut self) -> String;
}

pub fn tap(rt: &mut Runtime<App>, key: &str) -> bool {
    let Some(r) = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.rect) else { return false };
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    true
}

fn press(rt: &mut Runtime<App>, key: Key, cmd: bool) {
    rt.key(KeyEvent { key, mods: Mods { cmd, ..Default::default() } });
}

/// Clears the composer and types `text` into it, as keys.
fn type_text(rt: &mut Runtime<App>, text: &str) {
    if !rt.state.composer.focused {
        tap(rt, "prompt");
    }
    rt.update(|s, _| {
        s.composer = TextEdit::new("");
        s.composer.focused = true;
    });
    if !text.is_empty() {
        rt.key(KeyEvent { key: Key::Text(text.into()), mods: Mods::default() });
    }
}

pub fn button(rt: &Runtime<App>) -> String {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("submit")).and_then(|e| e.label).unwrap_or_default()
}

/// Plays `op` if it is a key or a tap, and says whether it was.
pub fn play(d: &mut impl Desk, op: Op) -> bool {
    let t = d.now();
    match op {
        Op::Send | Op::Queue | Op::HoldEnter => {
            let text = d.next_prompt();
            d.trace().note(t, format!("> {op:?} {text}"));
            type_text(d.rt(), &text);
            press(d.rt(), Key::Enter, op == Op::Queue);
            if op == Op::HoldEnter {
                d.flush();
                let before = d.interrupts();
                for _ in 0..8 {
                    d.tick(1.0 / 30.0);
                    d.rt().repeat_key(KeyEvent { key: Key::Enter, mods: Mods::default() });
                }
                d.flush();
                if d.interrupts() > before {
                    let now = d.now();
                    d.oracle().intent(Kind::HeldEnterStopped, now, "holding ↵ after sending stopped the run".into());
                }
            }
        }
        Op::Stop => {
            let label = button(d.rt());
            d.trace().note(t, format!("> Stop (button says {label})"));
            type_text(d.rt(), "");
            tap(d.rt(), "submit");
        }
        Op::Esc => {
            d.trace().note(t, "> Esc");
            press(d.rt(), Key::Escape, false);
        }
        Op::EnterEmpty => {
            type_text(d.rt(), "");
            d.tick(1.0 / 60.0);
            let label = button(d.rt());
            let now = d.now();
            d.trace().note(now, format!("> ↵ in an empty composer (button says {label})"));
            let before = d.interrupts();
            press(d.rt(), Key::Enter, false);
            // The key is handled on the next frame.
            d.tick(1.0 / 60.0);
            d.flush();
            if label == "Stop" && d.interrupts() == before {
                let now = d.now();
                d.oracle().intent(Kind::EnterDidntStop, now, "↵ with nothing typed while Stop showed sent no interrupt".into());
            }
        }
        _ => return false,
    }
    true
}
