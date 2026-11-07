//! Settings › Experimental (`settings/experimental/experimental.tsx`): Organize vertical sidebar,
//! New icons, Fold long messages and New question UI; then this app's Developer section (Rewind).

use rune::{El, col};

use super::{choice, header, page};
use crate::App;
use opencode_i18n::t;
use crate::palette::Pal;
use crate::settings::{self, Choice as _, Organize, setting, switch};

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let rows = vec![
        ("settings-sidebar-organize", setting(p, "settings-sidebar-organize", t!("settings.experimental.row.sidebarOrganize.title"), t!("settings.experimental.row.sidebarOrganize.description"),
            choice(s, p, "settings-sidebar-organize-select", t!("settings.experimental.row.sidebarOrganize.title"), st.sidebar_organize, |s, id, _| s.change_settings(|x| x.sidebar_organize = Organize::parse(Some(id)))))),
        ("settings-new-icons", setting(p, "settings-new-icons", t!("settings.experimental.row.newIcons.title"), t!("settings.experimental.row.newIcons.description"),
            switch(p, "new-icons-switch", st.new_icons).on_press(|s: &mut App| {
                s.change_settings(|x| x.new_icons = !x.new_icons);
                crate::appearance::apply_icons(&s.settings);
            }))),
        ("settings-fold-user-messages", setting(p, "settings-fold-user-messages", t!("settings.experimental.row.foldUserMessages.title"), t!("settings.experimental.row.foldUserMessages.description"),
            switch(p, "fold-user-switch", st.fold_user).on_press(|s: &mut App| s.set_fold_user(!s.settings.fold_user)))),
        ("settings-new-question-ui", setting(p, "settings-new-question-ui", t!("settings.experimental.row.newQuestionUI.title"), t!("settings.experimental.row.newQuestionUI.description"),
            switch(p, "new-question-ui-switch", st.new_question_ui).on_press(|s: &mut App| s.change_settings(|x| x.new_question_ui = !x.new_question_ui)))),
    ];
    let body = col().w_full().gap(32.0)
        .child(settings::list(p, "experimental-list", super::rows(s, p, 20.0, rows)))
        .child(super::revealable(s, p, "record-glitches", 0.0, crate::glitch::settings_section(s, p)));
    page(p, "settings-experimental-page", header(p, t!("settings.tab.experimental"), t!("settings.experimental.description")), body)
}
