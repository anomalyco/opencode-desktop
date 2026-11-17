//! Settings › Experimental (`settings/experimental/experimental.tsx`): Organize vertical sidebar,
//! New icons, New question UI and Smooth timeline; then this app's
//! Developer section (Rewind).

use rune::{El, col};

use super::{header, page, select};
use crate::App;
use opencode_i18n::t;
use crate::palette::Pal;
use crate::settings::{self, Choice as _, Organize, setting, switch};

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let rows = vec![
        ("settings-sidebar-organize", setting(p, "settings-sidebar-organize", t!("settings.experimental.row.sidebarOrganize.title"), t!("settings.experimental.row.sidebarOrganize.description"),
            organize(s, p))),
        ("settings-new-icons", setting(p, "settings-new-icons", t!("settings.experimental.row.newIcons.title"), t!("settings.experimental.row.newIcons.description"),
            switch(p, "new-icons-switch", st.new_icons).on_press(|s: &mut App| {
                s.change_settings(|x| x.new_icons = !x.new_icons);
                crate::appearance::apply_icons(&s.settings);
            }))),
        ("settings-new-question-ui", setting(p, "settings-new-question-ui", t!("settings.experimental.row.newQuestionUI.title"), t!("settings.experimental.row.newQuestionUI.description"),
            switch(p, "new-question-ui-switch", st.new_question_ui).on_press(|s: &mut App| s.change_settings(|x| x.new_question_ui = !x.new_question_ui)))),
        ("settings-smooth-timeline", setting(p, "settings-smooth-timeline", t!("rune.settings.experimental.row.smoothTimeline.title"), t!("rune.settings.experimental.row.smoothTimeline.description"),
            switch(p, "smooth-timeline-switch", st.smooth_timeline).on_press(|s: &mut App| s.change_settings(|x| x.smooth_timeline = !x.smooth_timeline)))),
    ];
    let body = col().w_full().gap(32.0)
        .child(settings::list(p, "experimental-list", super::rows(s, p, 20.0, rows)))
        .child(super::revealable(s, p, "record-glitches", 0.0, crate::glitch::settings_section(s, p)));
    page(p, "settings-experimental-page", header(p, t!("settings.tab.experimental"), t!("settings.experimental.description")), body)
}

/// Organize vertical sidebar's select (`experimental.tsx:32-45`): the choices this build offers.
fn organize(s: &App, p: &Pal) -> El {
    let options = Organize::options().iter().map(|o| (o.name().to_owned(), o.label().to_owned())).collect();
    let current = s.settings.sidebar_organize.effective().name();
    select(s, p, "settings-sidebar-organize-select", t!("settings.experimental.row.sidebarOrganize.title"), options, current, |s, id, _| s.change_settings(|x| x.sidebar_organize = Organize::parse(Some(id))))
}
