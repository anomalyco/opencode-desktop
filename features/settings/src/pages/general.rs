//! Settings › Preferences (`settings/general/general.tsx`): Interface layout, Language, Default
//! environment, Auto-accept permissions, Show agent, Follow-up behavior, Pinch to zoom and
//! Spellcheck, this app's own rows (Terminal placement, macOS Option-as-Meta, Windows shell
//! prompt marks), and the sections plugins put on the page (`PluginSettingsSections page="general"`).

use opencode_i18n::t;
use opencode_ui::keymap::Bind;
use opencode_ui::popup::PopupHost;
use rune::{Color, El, Tx, col, row, text};
use opencode_ui::palette::Pal;
use opencode_ui::style::UI;

use super::{choice, header, page_with, select};
use crate::host::SettingsHost;
use crate::types::{DockPlacement, TerminalMarkShell};
use crate::{Choice as _, Destination, FollowUp, TitlebarPosition, change_settings, list, set_show_agent, set_terminal_placement, setting, setting_at, switch};

fn toggle_shared_side_panel(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.shared_side_panel = !x.shared_side_panel);
}

fn pick_language(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    s.set_language(id);
}

fn pick_destination(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    change_settings(s, |x| x.default_destination = Destination::parse(Some(id)));
}

fn toggle_auto_accept(s: &mut dyn SettingsHost) {
    let on = !s.settings().auto_accept;
    s.set_auto_accept(on);
}

fn toggle_show_agent(s: &mut dyn SettingsHost) {
    let on = !s.settings().show_agent;
    set_show_agent(s, on);
}

fn pick_follow_up(s: &mut dyn SettingsHost, id: &str, _tx: &mut Tx) {
    change_settings(s, |x| x.follow_up = FollowUp::parse(Some(id)));
}

fn toggle_pinch_zoom(s: &mut dyn SettingsHost, tx: &mut Tx) {
    let on = !s.settings().pinch_zoom;
    s.set_pinch_zoom(on, tx);
}

fn toggle_spellcheck(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.spellcheck = !x.spellcheck);
}

fn toggle_mobile_diff_wrap(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.mobile_diff_wrap = !x.mobile_diff_wrap);
}

fn toggle_mobile_titlebar(s: &mut dyn SettingsHost) {
    change_settings(s, |x| {
        x.mobile_titlebar = if x.mobile_titlebar == TitlebarPosition::Bottom { TitlebarPosition::Top } else { TitlebarPosition::Bottom };
    });
}

fn toggle_terminal_option_meta(s: &mut dyn SettingsHost) {
    change_settings(s, |x| x.terminal_option_meta = !x.terminal_option_meta);
}

fn toggle_terminal_mark(s: &mut dyn SettingsHost, id: &'static str) {
    change_settings(s, |x| {
        if x.terminal_marks.iter().any(|m| m == id) {
            x.terminal_marks.retain(|m| m != id);
        } else {
            x.terminal_marks.push(id.to_owned());
        }
    });
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let st = s.settings();
    let alternate = Bind::SubmitAlternate.chord().map_or_else(String::new, opencode_ui::keymap::base::sentence);
    let mut rows = vec![
        (
            "settings-tab-layout",
            setting_at(
                p,
                "settings-tab-layout",
                t!("settings.general.row.interfaceMode.title"),
                t!("settings.general.row.interfaceMode.description"),
                crate::interface_mode::control(s, p),
                false,
            ),
        ),
        (
            "settings-shared-side-panel",
            setting(
                p,
                "settings-shared-side-panel",
                t!("settings.general.row.sharedSidePanel.title"),
                t!("settings.general.row.sharedSidePanel.description"),
                switch(p, "shared-side-panel-switch", st.shared_side_panel).on_press(|s: &mut H| toggle_shared_side_panel(s)),
            ),
        ),
        (
            "settings-language",
            setting(
                p,
                "settings-language",
                t!("settings.general.row.language.title"),
                t!("settings.general.row.language.description"),
                select(
                    s,
                    p,
                    "settings-language-select",
                    t!("settings.general.row.language.title"),
                    opencode_i18n::locales().iter().map(|l| (l.id().to_owned(), l.label().to_owned())).collect(),
                    opencode_i18n::locale().id(),
                    pick_language,
                ),
            ),
        ),
        (
            "settings-workspace-destination",
            setting(
                p,
                "settings-workspace-destination",
                t!("settings.workspaces.default.title"),
                t!("settings.workspaces.default.description"),
                choice(s, p, "settings-workspace-destination-select", t!("settings.workspaces.default.title"), st.default_destination, pick_destination),
            ),
        ),
        (
            "settings-auto-accept-permissions",
            setting(
                p,
                "settings-auto-accept-permissions",
                t!("command.permissions.autoaccept.enable"),
                t!("settings.permissions.autoAccept.default.description"),
                switch(p, "auto-accept-switch", st.auto_accept).on_press(|s: &mut H| toggle_auto_accept(s)),
            ),
        ),
        (
            "settings-show-custom-agents",
            setting(
                p,
                "settings-show-custom-agents",
                t!("settings.general.row.showCustomAgents.title"),
                t!("settings.general.row.showCustomAgents.description"),
                switch(p, "show-agent", st.show_agent).on_press(|s: &mut H| toggle_show_agent(s)),
            ),
        ),
        (
            "settings-follow-up-behavior",
            setting(
                p,
                "settings-follow-up-behavior",
                t!("settings.general.row.followUpBehavior.title"),
                &t!("settings.general.row.followUpBehavior.description", keybind = alternate),
                choice(s, p, "settings-follow-up-behavior-select", t!("settings.general.row.followUpBehavior.title"), st.follow_up, pick_follow_up),
            ),
        ),
        (
            "settings-pinch-zoom",
            setting(
                p,
                "settings-pinch-zoom",
                t!("settings.general.row.pinchZoom.title"),
                t!("settings.general.row.pinchZoom.description"),
                switch(p, "pinch-zoom-switch", st.pinch_zoom).on_press_tx(|s: &mut H, tx: &mut Tx| toggle_pinch_zoom(s, tx)),
            ),
        ),
        (
            "settings-spellcheck",
            setting(
                p,
                "settings-spellcheck",
                t!("settings.general.row.spellcheck.title"),
                t!("settings.general.row.spellcheck.description"),
                switch(p, "spellcheck-switch", st.spellcheck).on_press(|s: &mut H| toggle_spellcheck(s)),
            ),
        ),
        (
            "settings-mobile-diff-wrap",
            setting(
                p,
                "settings-mobile-diff-wrap",
                t!("plugin.review.settings.wrapLines.title"),
                t!("plugin.review.settings.wrapLines.description"),
                switch(p, "mobile-diff-wrap-switch", st.mobile_diff_wrap).on_press(|s: &mut H| toggle_mobile_diff_wrap(s)),
            ),
        ),
    ];
    if s.narrow() {
        let bottom = st.mobile_titlebar == TitlebarPosition::Bottom;
        let control = switch(p, "mobile-titlebar-bottom-switch", bottom).on_press(|s: &mut H| toggle_mobile_titlebar(s));
        rows.push((
            "settings-mobile-titlebar-bottom",
            setting(p, "settings-mobile-titlebar-bottom", t!("settings.general.row.mobileTitlebarBottom.title"), t!("settings.general.row.mobileTitlebarBottom.description"), control),
        ));
    }
    let placement_help = t!("rune.settings.general.row.terminalPlacement.description");
    let placement_help = if cfg!(windows) {
        format!("{placement_help} {}", t!("rune.settings.general.row.terminalPlacement.wslFolder"))
    } else {
        placement_help.to_owned()
    };
    rows.push(("terminal-placement", setting(p, "terminal-placement", t!("rune.settings.general.row.terminalPlacement.title"), &placement_help, placement(s, p))));
    if cfg!(target_os = "macos") {
        let control = switch(p, "terminal-option-meta-switch", st.terminal_option_meta).on_press(|s: &mut H| toggle_terminal_option_meta(s));
        rows.push(("terminal-option-meta", setting(p, "terminal-option-meta", t!("rune.settings.general.row.terminalOptionMeta.title"), t!("rune.settings.general.row.terminalOptionMeta.description"), control)));
    }
    if cfg!(windows) {
        for shell in TerminalMarkShell::ALL {
            let id = shell.id();
            let on = st.terminal_marks.iter().any(|m| m == id);
            let control = switch(p, &format!("{}-switch", shell.setting_key()), on).on_press(move |s: &mut H| toggle_terminal_mark(s, id));
            let description = match shell {
                TerminalMarkShell::Cmd => t!("rune.settings.general.row.terminalMarks.descriptionCmd").to_owned(),
                TerminalMarkShell::GitBash | TerminalMarkShell::Wsl => t!("rune.settings.general.row.terminalMarks.descriptionBash", shell = shell.label()),
                TerminalMarkShell::Pwsh | TerminalMarkShell::PowerShell => t!("rune.settings.general.row.terminalMarks.description", shell = shell.label()),
            };
            let title = t!("rune.settings.general.row.terminalMarks.title", shell = shell.label());
            rows.push((shell.setting_key(), setting(p, shell.setting_key(), &title, &description, control)));
        }
    }
    let rows = super::rows(s, p, 20.0, rows);
    let general = super::section(p, "settings-general-section", Some(t!("settings.general.section.general")), list(p, "settings-list", rows));
    let body = col()
        .w_full()
        .gap(32.0)
        .child(general)
        .child(super::timeline::section(s, p))
        .children(s.plugin_sections_for_page(p, "general"));
    page_with(p, "settings-general-page", header(p, t!("settings.tab.preferences"), t!("settings.preferences.description")), body, true)
}

/// Side or Bottom: where the terminal opens.
fn placement<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let mut r = row()
        .key("terminal-placement-control")
        .role("radiogroup")
        .labelled_by("title")
        .focus_group()
        .gap(2.0)
        .padding(2.0)
        .radius(6.0)
        .fill(p.layer2);
    for (pl, label) in [
        (DockPlacement::Side, t!("rune.settings.general.row.terminalPlacement.side")),
        (DockPlacement::Bottom, t!("rune.settings.general.row.terminalPlacement.bottom")),
    ] {
        let on = s.settings().terminal_placement == pl;
        r = r.child(
            row()
                .key(format!("placement-{}", pl.name()))
                .role("radio")
                .focus_selected(on)
                .h(24.0)
                .px(10.0)
                .center()
                .radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press_tx(move |s: &mut H, tx: &mut Tx| set_terminal_placement(s, pl, tx))
                .child(text(label).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}
