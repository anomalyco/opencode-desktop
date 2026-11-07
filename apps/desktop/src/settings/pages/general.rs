//! Settings › Preferences (`settings/general/general.tsx`): Interface layout, Language, Default
//! environment, Auto-accept permissions, Show agent, Follow-up behavior, Pinch to zoom and
//! Spellcheck, this app's own rows (Terminal placement, Download browser in the background), and
//! the sections plugins put on the page (`PluginSettingsSections page="general"`), Updates
//! among them.

use rune::{Color, El, Tx, col, row, text};

use super::{Page, choice, header, page_with, select};
use crate::App;
use crate::palette::Pal;
use crate::settings::{self, Choice as _, Destination, FollowUp, setting, setting_at, switch};
use crate::view::UI;

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    // `formatKeybind("mod+enter")`: the chord the alternate send is here.
    let alternate = crate::keymap::Bind::SubmitAlternate.keybind().label();
    let mut rows = vec![
        // Desktop's first row, its control at the row's top (`align="start"`).
        ("settings-tab-layout", setting_at(p, "settings-tab-layout", "Interface layout", "Choose how you switch between sessions and keep track of what's running", super::super::interface_mode::control(s, p), false)),
        ("settings-language", setting(p, "settings-language", "Language", "Change the display language for OpenCode",
            select(s, p, "settings-language-select", "Language", settings::LANGUAGES.iter().map(|(id, l)| ((*id).to_owned(), (*l).to_owned())).collect(), &st.language, |s, id, _| {
                let id = id.to_owned();
                s.change_settings(|x| x.language = id);
            }))),
        ("settings-workspace-destination", setting(p, "settings-workspace-destination", "Default environment", "Choose where new sessions start",
            choice(s, p, "settings-workspace-destination-select", "Default environment", st.default_destination, |s, id, _| s.change_settings(|x| x.default_destination = Destination::parse(Some(id)))))),
        ("settings-auto-accept-permissions", setting(p, "settings-auto-accept-permissions", "Auto-accept permissions", "Approve permission requests without asking. Projects can override this in their settings.",
            switch(p, "auto-accept-switch", st.auto_accept).on_press(|s: &mut App| s.set_auto_accept(!s.settings.auto_accept)))),
        ("settings-show-custom-agents", setting(p, "settings-show-custom-agents", "Show agent", "Switch between agents in the composer. When hidden, defaults to Build agent.",
            switch(p, "show-agent", st.show_agent).on_press(|s: &mut App| s.set_show_agent(!s.settings.show_agent)))),
        ("settings-follow-up-behavior", setting(p, "settings-follow-up-behavior", "Follow-up behavior", &format!("Choose whether to queue follow-ups or steer the current turn. Use {alternate} to switch."),
            choice(s, p, "settings-follow-up-behavior-select", "Follow-up behavior", st.follow_up, |s, id, _| s.change_settings(|x| x.follow_up = FollowUp::parse(Some(id)))))),
        ("settings-pinch-zoom", setting(p, "settings-pinch-zoom", "Pinch to zoom", "Allow trackpad pinch and Ctrl-scroll gestures to zoom",
            switch(p, "pinch-zoom-switch", st.pinch_zoom).on_press(|s: &mut App| s.change_settings(|x| x.pinch_zoom = !x.pinch_zoom)))),
        ("settings-spellcheck", setting(p, "settings-spellcheck", "Spellcheck", "Underline misspelled words while typing",
            switch(p, "spellcheck-switch", st.spellcheck).on_press(|s: &mut App| s.change_settings(|x| x.spellcheck = !x.spellcheck)))),
        ("terminal-placement", setting(p, "terminal-placement", "Terminal placement", "Choose where the terminal opens in sessions.", placement(s, p))),
    ];
    if !s.browser_unsupported() {
        let control = switch(p, "prefetch-browser-switch", st.prefetch_browser).on_press(|s: &mut App| s.set_prefetch_browser(!s.settings.prefetch_browser));
        rows.push(("prefetch-browser", setting(p, "prefetch-browser", "Download browser in the background", &crate::browser::prefetch::describe(&s.browser.host.status()), control)));
    }
    let rows = super::rows(s, p, 20.0, rows);
    let general = super::section(p, "settings-general-section", Some("General"), settings::list(p, "settings-list", rows));
    let body = col().w_full().gap(32.0)
        .child(general)
        .children(crate::plugin_host::settings_pages(s, p, "general"))
        .children(crate::updater::settings_section(s, p));
    page_with(p, "settings-general-page", header(p, Page::General.label(), "Customize preferences and theme and default behavior"), body, true)
}

/// Side or Bottom: where the terminal opens (this app's; Desktop's terminal is a side panel).
fn placement(s: &App, p: &Pal) -> El {
    use crate::panels::DockPlacement;
    let mut r = row().key("terminal-placement-control").gap(2.0).padding(2.0).radius(6.0).fill(p.layer2);
    for (pl, label) in [(DockPlacement::Side, "Side"), (DockPlacement::Bottom, "Bottom")] {
        let on = s.settings.terminal_placement == pl;
        r = r.child(
            row().key(format!("placement-{}", pl.name())).role("button").h(24.0).px(10.0).center().radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.set_terminal_placement(pl, tx))
                .child(text(label).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}
