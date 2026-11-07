//! Settings › Extensions (`settings/providers/extensions.tsx`): the server's MCPs (each with its
//! status, switch or sign-in, and a failed one's error), its plugins and its skills, under pill
//! tabs. MCPs list the default location's servers, as available to all projects.

use rune::{El, TextStyle, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{ExtensionsTab, LINE, NAME, catalog, header_with, page};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use opencode_i18n::t;

/// A tab's label: 13 on 16 at 440.
const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);

/// Reads the server's plugins and skills for their tabs.
pub fn load(s: &App) {
    s.job(|srv| {
        let plugins = srv.get("/api/plugin").ok().map(|v| {
            v["data"].as_array().into_iter().flatten()
                .filter(|p| p["source"]["type"].as_str() != Some("builtin"))
                .filter_map(|p| p["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        });
        let skills = srv.get("/api/skill").ok().map(|v| {
            v["data"].as_array().into_iter().flatten().filter_map(|p| p["name"].as_str().map(str::to_owned)).collect::<Vec<_>>()
        });
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if plugins.is_some() {
                s.settings_pages.plugins = plugins;
            }
            if skills.is_some() {
                s.settings_pages.skills = skills;
            }
        }) as Apply
    });
}

pub fn view(s: &App, p: &Pal) -> El {
    let tab = s.settings_pages.tab;
    let (link, content) = match tab {
        ExtensionsTab::Mcps => (manage(p), mcps(s, p)),
        ExtensionsTab::Plugins => (manage(p), named_rows(p, "settings-plugins", &ic::CUBE, s.settings_pages.plugins.as_deref().unwrap_or_default())),
        ExtensionsTab::Skills => (skills_link(p), named_rows(p, "settings-skills", &ic::POST_SKILL, s.settings_pages.skills.as_deref().unwrap_or_default())),
    };
    let heading = row().w_full().h(24.0).center_y()
        .child(text(t!("settings.extensions.availableAll")).font(LINE).color(p.text).grow(1.0))
        .child(link);
    let body = col().key("settings-extensions").w_full()
        .child(tabs(p, tab))
        .child(col().w_full().pt(24.0).gap(16.0).child(heading).children(content))
        .children((tab == ExtensionsTab::Plugins).then(|| crate::plugin_host::settings_section(s, p)).flatten().map(|e| e.mt(32.0)));
    page(p, "settings-extensions-page", header_with(p, t!("settings.tab.extensions"), t!("settings.extensions.description"), super::servers::switcher(s, p)), body)
}

/// Pill tabs (`tabs-v2` pill in `.settings-subtabs`): 24 pt pills 6 apart, the picked one on
/// `layer-03` with a hairline ring.
fn tabs(p: &Pal, on: ExtensionsTab) -> El {
    let mut list = row().key("settings-extensions-tabs").w(280.0).gap(6.0).pb(8.0);
    for (tab, id, label) in [(ExtensionsTab::Mcps, "mcps", t!("settings.extensions.tab.mcps")), (ExtensionsTab::Plugins, "plugins", t!("settings.extensions.tab.plugins")), (ExtensionsTab::Skills, "skills", t!("settings.extensions.tab.skills"))] {
        let picked = tab == on;
        let mut pill = row().key(format!("settings-extensions-tab-{id}")).role("tab").label(label).h(24.0).px(8.0).center().radius(6.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.settings_pages.tab = tab;
            })
            .child(text(label).font(TAB).color(if picked { p.text } else { p.muted }));
        pill = if picked { pill.fill(p.layer3).box_shadow(Pal::ring_in(p.border_muted)) } else { pill.hover(p.hover) };
        list = list.child(pill);
    }
    list
}

fn manage(p: &Pal) -> El {
    text(t!("settings.extensions.manageConfig")).font(LINE).color(p.muted)
}

/// `.settings-extension-link`: underlined in `text-accent`, `text-accent-hover` under the pointer.
fn skills_link(p: &Pal) -> El {
    let words = t!("settings.extensions.addSkills");
    const URL: &str = "https://opencode.ai/v2/docs/skills";
    text(words).key("settings-extension-link").font(LINE).color(p.accent)
        .spans(vec![rune::Span::new(0, words.len()).underline(p.accent)])
        .link(rune::Link::new(0..words.len(), URL).role("link").label(words).hover(rune::LinkPaint::new().color(p.accent_hover).underline(p.accent_hover)))
        .on_link(|_: &mut App, url: &str, tx: &mut Tx| {
            let _ = tx.open_url(url);
        })
}

/// `.settings-extension-row`: 56 pt tall, the 16 pt mark or dot, the name, and what ends it.
fn extension_row(key: String, lead: El, name: El, trailing: Option<El>) -> El {
    row().key(key).w_full().min_h(56.0).py(16.0).gap(16.0).center_y()
        .child(row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(lead).child(name))
        .children(trailing)
}

fn mcps(s: &App, p: &Pal) -> Option<El> {
    let mut list: Vec<_> = s.mcp_servers("").unwrap_or_default().to_vec();
    list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let rows: Vec<El> = list.iter().map(|m| {
        let disabled = m.status == opencode_kit::integration::McpStatus::Disabled;
        extension_row(format!("settings-mcp-{}", m.name), crate::widgets::status_dot(p, m.status.name()),
            text(m.name.clone()).font(NAME).color(if disabled { p.muted } else { p.text }).shrink().truncate(),
            Some(crate::mcp::view::trailing(s, p, "", m)))
    }).collect();
    (!rows.is_empty()).then(|| catalog(p, "settings-mcps", rows))
}

fn named_rows(p: &Pal, key: &str, glyph: &'static rune::Icon, names: &[String]) -> Option<El> {
    let rows: Vec<El> = names.iter().map(|n| {
        extension_row(format!("{key}-{n}"), icon(glyph, 16.0).color(p.icon_muted), text(n.clone()).font(NAME).color(p.text).shrink().truncate(), None)
    }).collect();
    (!rows.is_empty()).then(|| catalog(p, key, rows))
}

