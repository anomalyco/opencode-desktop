//! A session's title changing under you (the server's rename a few seconds into a turn): the
//! old title blurs out while the new one fades in and the row's width springs (Kit picked the
//! crossfade over swapping at once, t1137). The story is Header › Title change: R replays, T
//! renames back and forth.

use rune::Play;
use rune_knobs::{Group, Preset};

use crate::stories::{Trigger, replay};
use crate::{App, Route};

pub use session_view::title::{TITLE_CHANGE, TITLE_SPEED, title};

const PRESETS: &[Preset] = &[Preset { name: "Instant", values: &[("Title change", 0.0)] }];

pub const TITLE: Group = Group::new("Header", "crumbs", &[TITLE_CHANGE, TITLE_SPEED]).presets(PRESETS);

rune_register::register!(crate::knobs::GROUP_LIST: Group = [TITLE]);

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
    let Route::Session { id, .. } = &s.route else { return None };
    s.sessions.iter().find(|x| &x.id == id).map(|i| i.title.clone())
}

fn set(s: &mut App, to: &str) {
    let Route::Session { id, .. } = s.route.clone() else { return };
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
