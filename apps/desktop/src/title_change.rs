//! A session's title changing under you (the server's rename a few seconds into a turn): the
//! old title blurs out while the new one fades in and the row's width springs (Kit picked the
//! crossfade over swapping at once, t1137). The story is Header › Title change: R replays, T
//! renames back and forth.

use rune::{Color, El, Play, Spring, SpringPair, TextStyle, Transition, text};
use rune_knobs::{Group, Knob, Knobs, Preset};

use crate::stories::{Trigger, replay};
use crate::{App, Route};

pub const TITLE_CHANGE: Knob = Knob::choice("Title change", &["Instant", "Crossfade"], 1)
    .about("How a renamed session's title changes")
    .hints(&["Swaps at once while the width opens", "Old title fades out, new fades in"]);
/// The crossfade, as a spring's duration.
pub const TITLE_SPEED: Knob = Knob::new("Title speed", 0.15, 0.6, 0.35).step(0.01).about("Seconds a title change takes");

const PRESETS: &[Preset] = &[Preset { name: "Instant", values: &[("Title change", 0.0)] }];

pub const TITLE: Group = Group::new("Header", "crumbs", &[TITLE_CHANGE, TITLE_SPEED]).presets(PRESETS);

rune_register::register!(crate::knobs::GROUP_LIST: Group = [TITLE]);

/// `title` in the header, keyed under `key`, changing by the Title change knob.
pub fn title(k: &Knobs, key: &str, title: String, font: TextStyle, color: Color) -> El {
    let spring = Spring::new(k.get(&TITLE_SPEED), 0.0);
    match k.get(&TITLE_CHANGE) as usize {
        1 => {
            // Keyed by the words, so a new title is a new node: the old one leaves in place while
            // the row's width springs, and the new one comes in over it. Opening another session
            // settles it at once (`open_now`).
            let tr = Transition::blur_replace().springs(SpringPair::new(spring, Spring::new(spring.duration * 0.8, 0.0)));
            text(title.clone()).key(format!("{key}:{title}")).font(font).color(color).truncate_fade().transition(tr)
        }
        _ => text(title).font(font).color(color).truncate_fade(),
    }
}

const FIRST: &str = "New session";
const RENAMED: &str = "Add dark mode to the settings page";

pub const KEYS: &[Trigger] = &[replay(open), Trigger { key: 't', label: "Rename", run: toggle }];

/// "New session", renamed 0.6 s later, as the server does a few seconds into a turn.
fn open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    set(s, FIRST);
    tx.after(0.6, toggle);
}

fn toggle(s: &mut App, _: &mut Play<App>) {
    let now = current(s).unwrap_or_default();
    set(s, if now == RENAMED { FIRST } else { RENAMED });
}

fn current(s: &App) -> Option<String> {
    let Route::Session(id) = &s.route else { return None };
    s.sessions.iter().find(|x| &x.id == id).map(|i| i.title.clone())
}

fn set(s: &mut App, to: &str) {
    let Route::Session(id) = s.route.clone() else { return };
    if let Some(i) = s.sessions.iter_mut().find(|x| x.id == id) {
        i.title = to.into();
    }
}

rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
    crate::stories::Story { path: "Header/Title change", scene: |s| crate::stories::stage(s, crate::stories::header_view), knobs: &[TITLE], keys: KEYS, variants: &[] },
]);

#[cfg(test)]
#[path = "title_change_tests.rs"]
mod tests;
