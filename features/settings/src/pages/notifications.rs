//! Settings › Notifications (`settings/notifications/notifications.tsx`): Desktop notifications
//! (Agent, Permissions, Questions, Errors) and Sound effects (a sound or None for each, and the
//! interface's sounds).

use opencode_i18n::t;
use opencode_ui::popup::PopupHost;
use rune::{El, Tx, col};
use opencode_ui::palette::Pal;

use super::{header, page_with, select_preview};
use crate::host::SettingsHost;
use crate::prefs;
use crate::types::NotifyKind;
use crate::{change_settings, list, set_notify, setting, switch};

fn toggle_notify(s: &mut dyn SettingsHost, kind: NotifyKind) {
    let next = !s.settings().notify.get(kind);
    set_notify(s, kind, next);
}

fn toggle_interface_sound(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.sounds.interface = !x.sounds.interface);
}

fn preview_sound(s: &mut dyn SettingsHost, id: &str) {
    if id != "none" {
        s.play_sound(id);
    }
}

fn pick_agent_sound(s: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    set_sound(s, |x| &mut x.agent, id, tx);
}

fn pick_permissions_sound(s: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    set_sound(s, |x| &mut x.permissions, id, tx);
}

fn pick_questions_sound(s: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    set_sound(s, |x| &mut x.questions, id, tx);
}

fn pick_errors_sound(s: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    set_sound(s, |x| &mut x.errors, id, tx);
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let st = s.settings();
    let notify = [
        (NotifyKind::Agent, "settings-notifications-agent", t!("settings.general.notifications.agent.title"), t!("settings.general.notifications.agent.description")),
        (NotifyKind::Permission, "settings-notifications-permissions", t!("settings.general.notifications.permissions.title"), t!("settings.general.notifications.permissions.description")),
        (NotifyKind::Question, "settings-notifications-questions", t!("settings.general.notifications.questions.title"), t!("settings.general.notifications.questions.description")),
        (NotifyKind::Error, "settings-notifications-errors", t!("settings.general.notifications.errors.title"), t!("settings.general.notifications.errors.description")),
    ];
    let notify = notify
        .into_iter()
        .map(|(kind, target, title, about)| {
            let control = switch(p, &format!("notify-{}", kind.name()), st.notify.get(kind)).on_press(move |s: &mut H| toggle_notify(s, kind));
            (target, setting(p, target, title, about, control))
        })
        .collect();
    let options = || {
        std::iter::once(("none".to_owned(), t!("sound.option.none").to_owned()))
            .chain(prefs::SOUNDS.iter().map(|(id, l)| ((*id).to_owned(), opencode_i18n::I18N.text(*l).to_owned())))
            .collect::<Vec<_>>()
    };
    let current = |id: &Option<String>| id.clone().unwrap_or_else(|| "none".to_owned());
    let sounds = vec![
        (
            "settings-sounds-agent",
            setting(
                p,
                "settings-sounds-agent",
                t!("settings.general.sounds.agent.title"),
                t!("settings.general.sounds.agent.description"),
                select_preview(s, p, "settings-sounds-agent-select", t!("settings.general.sounds.agent.title"), options(), &current(&st.sounds.agent), pick_agent_sound, preview_sound),
            ),
        ),
        (
            "settings-sounds-permissions",
            setting(
                p,
                "settings-sounds-permissions",
                t!("settings.general.sounds.permissions.title"),
                t!("settings.general.sounds.permissions.description"),
                select_preview(
                    s,
                    p,
                    "settings-sounds-permissions-select",
                    t!("settings.general.sounds.permissions.title"),
                    options(),
                    &current(&st.sounds.permissions),
                    pick_permissions_sound,
                    preview_sound,
                ),
            ),
        ),
        (
            "settings-sounds-questions",
            setting(
                p,
                "settings-sounds-questions",
                t!("settings.general.sounds.questions.title"),
                t!("settings.general.sounds.questions.description"),
                select_preview(
                    s,
                    p,
                    "settings-sounds-questions-select",
                    t!("settings.general.sounds.questions.title"),
                    options(),
                    &current(&st.sounds.questions),
                    pick_questions_sound,
                    preview_sound,
                ),
            ),
        ),
        (
            "settings-sounds-errors",
            setting(
                p,
                "settings-sounds-errors",
                t!("settings.general.sounds.errors.title"),
                t!("settings.general.sounds.errors.description"),
                select_preview(s, p, "settings-sounds-errors-select", t!("settings.general.sounds.errors.title"), options(), &current(&st.sounds.errors), pick_errors_sound, preview_sound),
            ),
        ),
        (
            "settings-sounds-interface",
            setting(
                p,
                "settings-sounds-interface",
                t!("settings.general.sounds.interface.title"),
                t!("settings.general.sounds.interface.description"),
                switch(p, "sounds-interface-switch", st.sounds.interface).on_press(|s: &mut H| toggle_interface_sound(s)),
            ),
        ),
    ];
    let body = col()
        .w_full()
        .gap(32.0)
        .child(super::section(p, "settings-notifications-section", Some(t!("settings.general.section.notifications")), list(p, "notifications-list", super::rows(s, p, 20.0, notify))))
        .child(super::section(p, "settings-sounds-section", Some(t!("settings.general.section.sounds")), list(p, "sounds-list", super::rows(s, p, 20.0, sounds))));
    page_with(p, "settings-notifications-page", header(p, t!("settings.tab.notifications"), t!("settings.notifications.description")), body, true)
}

/// Picks sound `id` (or none) for an event and plays it, as Desktop previews a pick.
fn set_sound(s: &mut dyn SettingsHost, which: fn(&mut prefs::Sounds) -> &mut Option<String>, id: &str, _tx: &mut Tx) {
    let value = (id != "none").then(|| id.to_owned());
    change_settings(s, |x| *which(&mut x.sounds) = value.clone());
    if let Some(id) = value {
        s.play_sound(&id);
    }
}
