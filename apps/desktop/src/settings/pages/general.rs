//! Settings › Preferences (`settings/general/general.tsx`): Interface layout, Language, Default
//! environment, Auto-accept permissions, Show agent, Follow-up behavior, Pinch to zoom and
//! Spellcheck, this app's own rows (Terminal placement, Download browser in the background), and
//! the sections plugins put on the page (`PluginSettingsSections page="general"`), Updates
//! among them.

use rune::{Color, El, Tx, col, row, text};

use super::{choice, header, page_with, select};
use crate::App;
use opencode_i18n::t;
use crate::palette::Pal;
use crate::settings::{self, Choice as _, Destination, FollowUp, setting, setting_at, switch};
use crate::view::UI;

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    // `formatKeybind("mod+enter")`: the chord the alternate send is here.
    let alternate = crate::keymap::Bind::SubmitAlternate.chord().map_or_else(String::new, session_view::keymap::sentence);
    let mut rows = vec![
        // Desktop's first row, its control at the row's top (`align="start"`).
        ("settings-tab-layout", setting_at(p, "settings-tab-layout", t!("settings.general.row.interfaceMode.title"), t!("settings.general.row.interfaceMode.description"), super::super::interface_mode::control(s, p), false)),
        ("settings-language", setting(p, "settings-language", t!("settings.general.row.language.title"), t!("settings.general.row.language.description"),
            select(s, p, "settings-language-select", t!("settings.general.row.language.title"), opencode_i18n::locales().iter().map(|l| (l.id().to_owned(), l.label().to_owned())).collect(), opencode_i18n::locale().id(), |s, id, _| crate::language::set(s, id)))),
        ("settings-workspace-destination", setting(p, "settings-workspace-destination", t!("settings.workspaces.default.title"), t!("settings.workspaces.default.description"),
            choice(s, p, "settings-workspace-destination-select", t!("settings.workspaces.default.title"), st.default_destination, |s, id, _| s.change_settings(|x| x.default_destination = Destination::parse(Some(id)))))),
        ("settings-auto-accept-permissions", setting(p, "settings-auto-accept-permissions", t!("command.permissions.autoaccept.enable"), t!("settings.permissions.autoAccept.default.description"),
            switch(p, "auto-accept-switch", st.auto_accept).on_press(|s: &mut App| s.set_auto_accept(!s.settings.auto_accept)))),
        ("settings-show-custom-agents", setting(p, "settings-show-custom-agents", t!("settings.general.row.showCustomAgents.title"), t!("settings.general.row.showCustomAgents.description"),
            switch(p, "show-agent", st.show_agent).on_press(|s: &mut App| s.set_show_agent(!s.settings.show_agent)))),
        ("settings-follow-up-behavior", setting(p, "settings-follow-up-behavior", t!("settings.general.row.followUpBehavior.title"), &t!("settings.general.row.followUpBehavior.description", keybind = alternate),
            choice(s, p, "settings-follow-up-behavior-select", t!("settings.general.row.followUpBehavior.title"), st.follow_up, |s, id, _| s.change_settings(|x| x.follow_up = FollowUp::parse(Some(id)))))),
        ("settings-pinch-zoom", setting(p, "settings-pinch-zoom", t!("settings.general.row.pinchZoom.title"), t!("settings.general.row.pinchZoom.description"),
            switch(p, "pinch-zoom-switch", st.pinch_zoom).on_press_tx(|s: &mut App, tx: &mut Tx| s.set_pinch_zoom(!s.settings.pinch_zoom, tx)))),
        ("settings-spellcheck", setting(p, "settings-spellcheck", t!("settings.general.row.spellcheck.title"), t!("settings.general.row.spellcheck.description"),
            switch(p, "spellcheck-switch", st.spellcheck).on_press(|s: &mut App| s.change_settings(|x| x.spellcheck = !x.spellcheck)))),
        // The review plugin's row in this section (`review/settings.tsx`).
        ("settings-mobile-diff-wrap", setting(p, "settings-mobile-diff-wrap", t!("plugin.review.settings.wrapLines.title"), t!("plugin.review.settings.wrapLines.description"),
            switch(p, "mobile-diff-wrap-switch", st.mobile_diff_wrap).on_press(|s: &mut App| s.change_settings(|x| x.mobile_diff_wrap = !x.mobile_diff_wrap)))),
    ];
    // A narrow window's own row (`general.tsx:299`).
    if s.narrow() {
        let bottom = st.mobile_titlebar == settings::TitlebarPosition::Bottom;
        let control = switch(p, "mobile-titlebar-bottom-switch", bottom).on_press(|s: &mut App| {
            s.change_settings(|x| x.mobile_titlebar = if x.mobile_titlebar == settings::TitlebarPosition::Bottom { settings::TitlebarPosition::Top } else { settings::TitlebarPosition::Bottom })
        });
        rows.push(("settings-mobile-titlebar-bottom", setting(p, "settings-mobile-titlebar-bottom", t!("settings.general.row.mobileTitlebarBottom.title"), t!("settings.general.row.mobileTitlebarBottom.description"), control)));
    }
    rows.extend([
        ("terminal-placement", setting(p, "terminal-placement", t!("rune.settings.general.row.terminalPlacement.title"), t!("rune.settings.general.row.terminalPlacement.description"), placement(s, p))),
    ]);
    if !s.browser_unsupported() {
        let control = switch(p, "prefetch-browser-switch", st.prefetch_browser).on_press(|s: &mut App| s.set_prefetch_browser(!s.settings.prefetch_browser));
        rows.push(("prefetch-browser", setting(p, "prefetch-browser", t!("rune.settings.general.row.prefetchBrowser.title"), &crate::browser::prefetch::describe(&s.browser.host.status()), control)));
    }
    let rows = super::rows(s, p, 20.0, rows);
    let general = super::section(p, "settings-general-section", Some(t!("settings.general.section.general")), settings::list(p, "settings-list", rows));
    let body = col().w_full().gap(32.0)
        .child(general)
        .child(super::timeline::section(s, p))
        .children(crate::plugin_host::settings_pages(s, p, "general"))
        .children(crate::updater::settings_section(s, p));
    page_with(p, "settings-general-page", header(p, t!("settings.tab.preferences"), t!("settings.preferences.description")), body, true)
}

/// Side or Bottom: where the terminal opens (this app's; Desktop's terminal is a side panel).
fn placement(s: &App, p: &Pal) -> El {
    use crate::panels::DockPlacement;
    // A segmented radio group, one Tab stop whose arrows pick, as the official segmented control.
    let mut r = row().key("terminal-placement-control").role("radiogroup").focus_group().gap(2.0).padding(2.0).radius(6.0).fill(p.layer2);
    for (pl, label) in [(DockPlacement::Side, t!("rune.settings.general.row.terminalPlacement.side")), (DockPlacement::Bottom, t!("rune.settings.general.row.terminalPlacement.bottom"))] {
        let on = s.settings.terminal_placement == pl;
        r = r.child(
            row().key(format!("placement-{}", pl.name())).role("radio").focus_selected(on).h(24.0).px(10.0).center().radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.set_terminal_placement(pl, tx))
                .child(text(label).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}
