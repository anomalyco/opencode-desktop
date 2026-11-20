//! Settings › Extensions (`settings/providers/extensions.tsx`): the server's MCPs (each with its
//! status, switch or sign-in, and a failed one's error), its plugins and its skills, under pill
//! tabs. MCPs list the default location's servers, as available to all projects.

use opencode_i18n::t;
use opencode_kit::integration::McpStatus;
use opencode_ui::popup::PopupHost;
use opencode_ui::widgets;
use rune::{El, Icon, TextStyle, Tx, col, icon, row, rule, text};
use rune_theme_opencode::icons as ic;
use opencode_ui::palette::{Pal, kit};
use rune_kit::ButtonVariant;

use super::{ExtensionsTab, LINE, NAME, catalog_with, header_with, page};
use super::plugin_updates;
use crate::host::SettingsHost;
use crate::jobs::{SettingsJob, SettingsRequest};

/// The open book Desktop draws for a skill (`additional-icons.ts` `post-skill`, on a 16 grid).
static SKILL: Icon = Icon {
    name: "post-skill-book",
    view: 16.0,
    body: r#"<path d="M8 4.5c-1.5-1-3.5-1.5-5.5-1.5v9c2 0 4 .5 5.5 1.5 1.5-1 3.5-1.5 5.5-1.5V3c-2 0-4 .5-5.5 1.5zM8 4.5v9" stroke="currentColor" stroke-linecap="square" stroke-linejoin="round"/>"#,
};

/// A tab's label: 13 on 16 at 440.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

/// Reads the server's plugins and skills for their tabs.
pub fn load(s: &mut dyn SettingsHost) {
    s.spawn_job(SettingsJob::ActiveServer(SettingsRequest::LoadExtensions));
}

fn select_tab(s: &mut dyn SettingsHost, tab: ExtensionsTab, tx: &mut Tx) {
    tx.instant();
    s.settings_pages_mut().tab = tab;
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let tab = s.settings_pages().tab;
    let (link, content) = match tab {
        ExtensionsTab::Mcps => (manage(p), mcps(s, p)),
        ExtensionsTab::Plugins => (plugins_heading::<H>(s, p), plugins::<H>(s, p)),
        ExtensionsTab::Skills => {
            let names: Vec<(String, Option<El>)> = s.settings_pages().skills.iter().flatten().map(|n| (n.clone(), None)).collect();
            (skills_link::<H>(p), named_rows(p, "settings-skills", &SKILL, names))
        }
    };
    let heading = row()
        .w_full()
        .h(24.0)
        .center_y()
        .child(text(t!("settings.extensions.availableAll")).font(LINE).color(p.text).grow(1.0))
        .child(link);
    let body = col()
        .key("settings-extensions")
        .w_full()
        .child(tabs::<H>(p, tab))
        .child(col().w_full().pt(24.0).gap(16.0).child(heading).children(content))
        .children((tab == ExtensionsTab::Plugins).then(|| s.plugin_extensions_section(p)).flatten().map(|e| e.mt(32.0)));
    page(p, "settings-extensions-page", header_with(p, t!("settings.tab.extensions"), t!("settings.extensions.description"), super::servers::switcher(s, p)), body)
}

/// Pill tabs (`tabs-v2` pill in `.settings-subtabs`).
fn tabs<H: SettingsHost + 'static>(p: &Pal, on: ExtensionsTab) -> El {
    let mut list = row().key("settings-extensions-tabs").w(280.0).gap(6.0).pb(8.0);
    for (tab, id, label) in [
        (ExtensionsTab::Mcps, "mcps", t!("settings.extensions.tab.mcps")),
        (ExtensionsTab::Plugins, "plugins", t!("settings.extensions.tab.plugins")),
        (ExtensionsTab::Skills, "skills", t!("settings.extensions.tab.skills")),
    ] {
        let picked = tab == on;
        let edge = if picked { p.border_muted } else { rune::Color::TRANSPARENT };
        let mut pill = row()
            .key(format!("settings-extensions-tab-{id}"))
            .role("tab")
            .label(label)
            .h(24.0)
            .center()
            .radius(6.0)
            .border(0.5, edge)
            .border_layout()
            .on_press_tx(move |s: &mut H, tx: &mut Tx| select_tab(s, tab, tx))
            .child(row().px(8.0).border(0.5, rune::Color::TRANSPARENT).border_layout().center().child(text(label).font(TAB).color(if picked { p.text } else { p.muted })));
        pill = if picked { pill.fill(p.layer3) } else { pill.hover(p.layer3) };
        list = list.child(pill);
    }
    list
}

fn manage(p: &Pal) -> El {
    text(t!("settings.extensions.manageConfig")).font(LINE).color(p.muted)
}

/// The Plugins heading's end: Manage in opencode.json, and Check for updates while the server
/// lists a package plugin (`settings/providers/extensions.tsx`).
#[allow(deprecated)]
fn plugins_heading<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let has_packages = s.settings_pages().plugins.iter().flatten().any(|r| r.package.is_some());
    let checking = s.settings_pages().updates.checking();
    let check = has_packages.then(|| {
        widgets::button(if checking { t!("settings.plugins.checking") } else { t!("settings.plugins.check") }, ButtonVariant::GhostMuted)
            .size(rune_kit::ButtonSize::Small)
            .disabled(checking)
            .on_press_tx(|s: &mut H, tx: &mut Tx| {
                tx.instant();
                plugin_updates::dispatch(s, plugin_updates::Event::CheckRequested);
            })
            .build(kit(p))
            .key("settings-plugins-check")
    });
    row().gap(12.0).center_y().child(manage(p)).children(check)
}

/// The server's plugins, each package one with Update while it has a newer version or is updating
/// (`PluginUpdateButton` in `runtime/plugin/extension-plugins.tsx`).
#[allow(deprecated)]
fn plugins<H: SettingsHost + 'static>(s: &H, p: &Pal) -> Option<El> {
    let pages = s.settings_pages();
    let rows = pages
        .plugins
        .iter()
        .flatten()
        .map(|r| {
            let button = r.package.as_ref().and_then(|pkg| {
                let (outdated, updating) = pages.updates.view(pkg);
                (outdated || updating).then(|| {
                    let target = pkg.target.clone();
                    widgets::button(if updating { t!("settings.plugins.updating") } else { t!("settings.plugins.update") }, ButtonVariant::Ghost)
                        .size(rune_kit::ButtonSize::Small)
                        .disabled(updating)
                        .on_press_tx(move |s: &mut H, tx: &mut Tx| {
                            tx.instant();
                            plugin_updates::dispatch(s, plugin_updates::Event::UpdateRequested { target: target.clone() });
                        })
                        .build(kit(p))
                        .key(format!("settings-plugins-update-{}", pkg.target))
                        .label(t!("settings.plugins.update.label", name = r.name.as_str()))
                })
            });
            (r.name.clone(), button)
        })
        .collect();
    named_rows(p, "settings-plugins", &ic::CUBE, rows)
}

/// `.settings-extension-link`: underlined in `text-accent`, `text-accent-hover` under the pointer.
fn skills_link<H: SettingsHost + 'static>(p: &Pal) -> El {
    let words = t!("settings.extensions.addSkills");
    const URL: &str = "https://opencode.ai/docs/skills";
    text(words)
        .key("settings-extension-link")
        .font(LINE)
        .color(p.accent)
        .spans(vec![rune::Span::new(0, words.len()).underline(p.accent)])
        .link(rune::Link::new(0..words.len(), URL).role("link").label(words).hover(rune::LinkPaint::new().color(p.accent_hover).underline(p.accent_hover)))
        .on_link(|s: &mut H, url: &str, tx: &mut Tx| crate::open_external(s, url, tx))
}

/// `.settings-extension-row`: 56 pt tall with its rule. The last row has no rule.
fn extension_row(p: &Pal, key: String, lead: El, name: El, trailing: Option<El>, last: bool) -> El {
    let r = row()
        .key(key)
        .w_full()
        .min_h(56.0)
        .py(16.0)
        .gap(16.0)
        .center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(lead).child(name))
        .children(trailing);
    if last { r } else { r.child(rule(0.5, p.border).pin(Some(0.0), None, Some(0.0), Some(0.0))) }
}

fn mcps(s: &dyn SettingsHost, p: &Pal) -> Option<El> {
    let mut list: Vec<_> = s.mcp_servers("").unwrap_or_default().to_vec();
    list.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    let n = list.len();
    let rows: Vec<El> = list
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let disabled = m.status == McpStatus::Disabled;
            extension_row(
                p,
                format!("settings-mcp-{}", m.name),
                widgets::status_dot(p, m.status.name()),
                text(m.name.clone()).font(NAME).color(if disabled { p.muted } else { p.text }).shrink().truncate(),
                Some(s.mcp_row_trailing(p, "", m)),
                k + 1 == n,
            )
        })
        .collect();
    (!rows.is_empty()).then(|| catalog_with(p, "settings-mcps", rows, false))
}

/// By name, as Desktop sorts them (`localeCompare`), each with what ends its row.
fn named_rows(p: &Pal, key: &str, glyph: &'static Icon, mut names: Vec<(String, Option<El>)>) -> Option<El> {
    names.sort_by(|a, b| rune::text::compare_names(&a.0, &b.0));
    let last = names.len();
    let rows: Vec<El> = names
        .into_iter()
        .enumerate()
        .map(|(k, (n, trailing))| {
            extension_row(p, format!("{key}-{n}"), icon(glyph, 16.0).color(p.icon_muted), text(n).font(NAME).color(p.text).shrink().truncate(), trailing, k + 1 == last)
        })
        .collect();
    (!rows.is_empty()).then(|| catalog_with(p, key, rows, false))
}
