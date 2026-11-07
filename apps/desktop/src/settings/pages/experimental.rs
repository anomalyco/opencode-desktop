//! Settings › Experimental (`settings/experimental/experimental.tsx`): Organize vertical sidebar,
//! New icons, Fold long messages and New question UI; then this app's Developer section (Rewind).

use rune::{El, col};

use super::{choice, header, page};
use crate::App;
use crate::palette::Pal;
use crate::settings::{self, Choice as _, Organize, setting, switch};

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let rows = vec![
        ("settings-sidebar-organize", setting(p, "settings-sidebar-organize", "Organize vertical sidebar", "Group side tabs by project or show one list. Only applies to side tabs.",
            choice(s, p, "settings-sidebar-organize-select", "Organize vertical sidebar", st.sidebar_organize, |s, id, _| s.change_settings(|x| x.sidebar_organize = Organize::parse(Some(id)))))),
        ("settings-new-icons", setting(p, "settings-new-icons", "New icons", "Preview the updated icon set",
            switch(p, "new-icons-switch", st.new_icons).on_press(|s: &mut App| {
                s.change_settings(|x| x.new_icons = !x.new_icons);
                crate::appearance::apply_icons(&s.settings);
            }))),
        ("settings-fold-user-messages", setting(p, "settings-fold-user-messages", "Fold long messages", "Fold your messages past 12 lines behind a Read more button",
            switch(p, "fold-user-switch", st.fold_user).on_press(|s: &mut App| s.set_fold_user(!s.settings.fold_user)))),
        ("settings-new-question-ui", setting(p, "settings-new-question-ui", "New question UI", "Preview the keyboard-first question dock",
            switch(p, "new-question-ui-switch", st.new_question_ui).on_press(|s: &mut App| s.change_settings(|x| x.new_question_ui = !x.new_question_ui)))),
    ];
    let body = col().w_full().gap(32.0)
        .child(settings::list(p, "experimental-list", super::rows(s, p, 20.0, rows)))
        .child(super::revealable(s, p, "record-glitches", 0.0, crate::glitch::settings_section(s, p)));
    page(p, "settings-experimental-page", header(p, "Experimental", "Try experimental features"), body)
}
