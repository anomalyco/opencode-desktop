//! Settings › Notifications (`settings/notifications/notifications.tsx`): Desktop notifications
//! (Agent, Permissions, Questions, Errors) and Sound effects (a sound or None for each, and the
//! interface's sounds).

use rune::{El, Tx, col};

use super::{header, page_with, select};
use crate::App;
use crate::notifications::Kind;
use crate::palette::Pal;
use crate::settings::{self, prefs, setting, switch};

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let notify = [
        (Kind::Agent, "settings-notifications-agent", "Agent", "Show system notification when the agent is complete or needs attention"),
        (Kind::Permission, "settings-notifications-permissions", "Permissions", "Show system notification when a permission is required"),
        (Kind::Question, "settings-notifications-questions", "Questions", "Show system notification when the agent asks a question"),
        (Kind::Error, "settings-notifications-errors", "Errors", "Show system notification when an error occurs"),
    ];
    let notify = notify.into_iter().map(|(kind, target, title, about)| {
        let control = switch(p, &format!("notify-{}", kind.name()), st.notify.get(kind)).on_press(move |s: &mut App| s.set_notify(kind, !s.settings.notify.get(kind)));
        (target, setting(p, target, title, about, control))
    }).collect();
    let options = || std::iter::once(("none".to_owned(), "None".to_owned())).chain(prefs::SOUNDS.iter().map(|(id, l)| ((*id).to_owned(), (*l).to_owned()))).collect::<Vec<_>>();
    let current = |id: &Option<String>| id.clone().unwrap_or_else(|| "none".to_owned());
    let sounds = vec![
        ("settings-sounds-agent", setting(p, "settings-sounds-agent", "Agent", "Play sound when the agent is complete or needs attention",
            select(s, p, "settings-sounds-agent-select", "Agent", options(), &current(&st.sounds.agent), |s, id, tx| set_sound(s, |x| &mut x.agent, id, tx)))),
        ("settings-sounds-permissions", setting(p, "settings-sounds-permissions", "Permissions", "Play sound when a permission is required",
            select(s, p, "settings-sounds-permissions-select", "Permissions", options(), &current(&st.sounds.permissions), |s, id, tx| set_sound(s, |x| &mut x.permissions, id, tx)))),
        ("settings-sounds-questions", setting(p, "settings-sounds-questions", "Questions", "Play sound when the agent asks a question",
            select(s, p, "settings-sounds-questions-select", "Questions", options(), &current(&st.sounds.questions), |s, id, tx| set_sound(s, |x| &mut x.questions, id, tx)))),
        ("settings-sounds-errors", setting(p, "settings-sounds-errors", "Errors", "Play sound when an error occurs",
            select(s, p, "settings-sounds-errors-select", "Errors", options(), &current(&st.sounds.errors), |s, id, tx| set_sound(s, |x| &mut x.errors, id, tx)))),
        ("settings-sounds-interface", setting(p, "settings-sounds-interface", "Interface", "Play subtle sounds when the wordmark appears and when you play with it",
            switch(p, "sounds-interface-switch", st.sounds.interface).on_press(|s: &mut App| s.change_settings(|x| x.sounds.interface = !x.sounds.interface)))),
    ];
    let body = col().w_full().gap(32.0)
        .child(super::section(p, "settings-notifications-section", Some("Desktop notifications"), settings::list(p, "notifications-list", super::rows(s, p, 20.0, notify))))
        .child(super::section(p, "settings-sounds-section", Some("Sound effects"), settings::list(p, "sounds-list", super::rows(s, p, 20.0, sounds))));
    page_with(p, "settings-notifications-page", header(p, "Notifications", "Choose when to receive notifications and hear sounds"), body, true)
}

/// Picks sound `id` (or none) for an event and plays it, as Desktop previews a pick.
fn set_sound(s: &mut App, which: fn(&mut prefs::Sounds) -> &mut Option<String>, id: &str, _tx: &mut Tx) {
    let value = (id != "none").then(|| id.to_owned());
    s.change_settings(|x| *which(&mut x.sounds) = value.clone());
    if let Some(id) = value {
        crate::sounds::play(&id);
    }
}
