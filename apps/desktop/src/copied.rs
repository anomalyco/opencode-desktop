//! A copy button's tick: pressing Copy turns its icon into a tick for a moment, as Desktop's
//! does, so the press shows it worked. The tick grows in on a short spring; both swaps back are
//! instant.

use rune::{El, Play, Spring, SpringPair, Transition, Tx, icon};
use rune_knobs::{Group, Knob, Knobs};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::stories::{Story, Trigger, replay};

pub const SHOW: Knob = Knob::new("Copied tick", 0.4, 3.0, 1.2).step(0.1).label("Tick shows").about("Seconds the tick stays after Copy");
pub const GROW: Knob = Knob::new("Copied grow", 0.0, 0.4, 0.15).step(0.01).label("Tick grows").about("Seconds the tick takes to grow in");
pub const FROM: Knob = Knob::new("Copied from", 0.5, 1.0, 0.7).step(0.05).label("Grows from").about("The tick's starting scale");

pub const GROUP: Group = Group::new("Copy", "copy", &[SHOW, GROW, FROM]).under("Timeline");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

/// The copy button last pressed (its key) and when, on the app clock.
#[derive(Default)]
pub struct Copied(Option<(String, f64)>);

impl App {
    /// Shows the tick on copy button `key`.
    pub fn mark_copied(&mut self, key: impl Into<String>, tx: &mut Tx) {
        self.copied.0 = Some((key.into(), self.clock));
        tx.wake_after(std::time::Duration::from_secs_f64(self.knobs.get(&SHOW) as f64));
    }

    /// Whether copy button `key` shows its tick.
    pub fn copied(&self, key: &str) -> bool {
        self.copied.0.as_ref().is_some_and(|(k, at)| k == key && self.clock - at < self.knobs.get(&SHOW) as f64)
    }
}

/// The button's glyph, `size` pt in `color`: the copy icon, or the tick growing in.
pub fn glyph(s: &App, key: &str, size: f32, color: rune::Color) -> El {
    if s.copied(key) {
        icon(&ic::CHECK, size).key("tick").color(color).transition(grow(&s.knobs))
    } else {
        icon(&ic::OUTLINE_COPY, size).key("copy-glyph").color(color).transition(Transition::none())
    }
}

fn grow(k: &Knobs) -> Transition {
    let enter = Spring::new(k.get(&GROW).max(0.01), 0.0);
    Transition::asymmetric(Transition::scale(k.get(&FROM)).combined(Transition::opacity()).allow_small_scale(), Transition::none())
        .springs(SpringPair::new(enter, Spring::NONE))
}

/// Drops the tick once its time is up.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let Some((_, at)) = &s.copied.0 else { return false };
    let left = at + s.knobs.get(&SHOW) as f64 - now;
    if left > 0.0 {
        tx.wake_after(std::time::Duration::from_secs_f64(left));
        return false;
    }
    s.copied.0 = None;
    true
}

const KEYS: &[Trigger] = &[replay(copy_last)];

/// Copies the session's last reply, as pressing its Copy does.
fn copy_last(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.copied = Copied::default();
    let id = crate::stories::sid(s);
    let last = s.transcripts.get(&id).and_then(|t| t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::Assistant(_)))).map(|m| m.id().to_owned());
    if let Some(m) = last {
        tx.after(0.4, move |s, tx| s.mark_copied(format!("copy-{m}"), tx));
    }
}

rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Timeline/Copy tick", KEYS).knobs(&[GROUP])]);

#[cfg(test)]
#[path = "copied_tests.rs"]
mod tests;
