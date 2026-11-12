//! A project's Extensions (`workspaces/project-extensions.tsx`): MCPs, Plugins, Skills and LSPs
//! under pill tabs. Each of the first three lists what the project's folder adds to the server's
//! own ("Added to this project", or No MCPs yet and what will show there), with Manage in
//! opencode.json (or How to add skills) beside it, then "Shared with all projects" folded under
//! its count. LSPs lists the language servers the folder's configuration names, each with its
//! file types and whether it is on.

use opencode_kit::model::Project;
use rune::{El, TextStyle, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use super::{catalog, header, page};
use crate::App;
use crate::jobs::Apply;
use crate::palette::Pal;
use opencode_i18n::t;

const TAB: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
/// A section's title (`project-settings-extension-section-header`): 13 on 16 at 530.
const HEAD: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);
const BODY: TextStyle = TextStyle::new(13.0, 440.0).lh(20.0);
const NAME: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0).tracking(-0.04);
const STATUS: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0);
const COUNT: TextStyle = TextStyle::new(11.0, 530.0).lh(16.0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Mcps,
    Plugins,
    Skills,
    Lsps,
}

/// A language server the configuration names (`configuredLanguageServers`).
#[derive(Clone, Debug)]
pub struct Lsp {
    name: String,
    disabled: bool,
    extensions: Vec<String>,
}

/// What the tabs read, for the folder they read it for.
#[derive(Default)]
pub struct State {
    pub dir: String,
    pub tab: Tab,
    /// "Shared with all projects" unfolded.
    pub shared: bool,
    /// (the server's, the folder's)
    plugins: Option<(Vec<String>, Vec<String>)>,
    /// By name and path: (the server's, the folder's).
    skills: Option<(Vec<(String, String)>, Vec<(String, String)>)>,
    /// The folder's configured servers, whether LSP is off, or a failed read.
    lsps: Option<Result<(bool, Vec<Lsp>), ()>>,
}

fn loc(dir: &str) -> String {
    format!("location%5Bdirectory%5D={}", crate::switcher::encode(dir))
}

/// Reads the folder's and the server's extensions.
pub fn load(s: &mut App, dir: &str) {
    if s.settings_pages.project.extensions.dir != dir {
        s.settings_pages.project.extensions = State { dir: dir.to_owned(), ..State::default() };
    }
    s.sync_mcp("");
    s.sync_mcp(dir);
    let dir = dir.to_owned();
    s.job(move |srv| {
        let plugins = |q: &str| srv.get(&format!("/api/plugin{q}")).ok().map(|v| {
            v["data"].as_array().into_iter().flatten()
                .filter(|p| p["source"]["type"].as_str() != Some("builtin"))
                .filter_map(|p| p["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()
        });
        // 503 until the server's plugins load (`get_ready` waits that out).
        let skills = |q: &str| srv.get_ready(&format!("/api/skill{q}")).ok().map(|v| {
            v["data"].as_array().into_iter().flatten()
                .filter_map(|p| Some((p["name"].as_str()?.to_owned(), p["location"].as_str().or(p["path"].as_str()).unwrap_or("").to_owned())))
                .collect::<Vec<_>>()
        });
        let q = format!("?{}", loc(&dir));
        let plugins = plugins("").zip(plugins(&q));
        let skills = skills("").zip(skills(&q));
        let lsps = srv.get(&format!("/api/config?{}", loc(&dir))).map(|v| configured(&v)).map_err(|_| ());
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let st = &mut s.settings_pages.project.extensions;
            if st.dir != dir {
                return;
            }
            st.plugins = plugins;
            // A failed read keeps the lists already read, as Desktop's location data does.
            st.skills = skills.or(st.skills.take());
            st.lsps = Some(lsps);
        }) as Apply
    });
}

/// The language servers a configuration's documents name, a later document's entry over an
/// earlier one's and `lsp: false` (or `true`) replacing them all.
fn configured(v: &Value) -> (bool, Vec<Lsp>) {
    let mut disabled = false;
    let mut servers: Vec<Lsp> = Vec::new();
    let entries = v["data"].as_array().or(v.as_array()).cloned().unwrap_or_default();
    for e in entries.iter().filter(|e| e["type"] == "document") {
        let lsp = &e["info"]["lsp"];
        if let Some(on) = lsp.as_bool() {
            disabled = !on;
            servers.clear();
            continue;
        }
        let Some(map) = lsp.as_object() else { continue };
        disabled = false;
        for (name, server) in map {
            let before = servers.iter().position(|x| &x.name == name).map(|i| servers.remove(i));
            servers.push(Lsp {
                name: name.clone(),
                disabled: server["disabled"].as_bool().or(before.as_ref().map(|b| b.disabled)).unwrap_or(false),
                extensions: server["extensions"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                    .or(before.map(|b| b.extensions)).unwrap_or_default(),
            });
        }
    }
    servers.sort_by(|a, b| rune::text::compare_names(&a.name, &b.name));
    (disabled, servers)
}

pub fn view(s: &App, p: &Pal, x: &Project) -> El {
    let st = &s.settings_pages.project.extensions;
    let body = match st.tab {
        Tab::Mcps => {
            let shared: Vec<String> = names(s.mcp_servers("").unwrap_or_default().iter().map(|m| m.name.clone()).collect());
            let own: Vec<String> = names(s.mcp_servers(&x.directory).unwrap_or_default().iter().map(|m| m.name.clone()).filter(|n| !shared.contains(n)).collect());
            section(s, p, "mcps", manage(p), &own, &shared, &ic::MCP)
        }
        Tab::Plugins => {
            let (shared, own) = st.plugins.clone().unwrap_or_default();
            let own: Vec<String> = own.into_iter().filter(|n| !shared.contains(n)).collect();
            section(s, p, "plugins", manage(p), &own, &shared, &ic::CUBE)
        }
        Tab::Skills => {
            let (shared, own) = st.skills.clone().unwrap_or_default();
            let own: Vec<String> = own.into_iter().filter(|k| !shared.contains(k)).map(|(n, _)| n).collect();
            let shared: Vec<String> = shared.into_iter().map(|(n, _)| n).collect();
            section(s, p, "skills", skills_link(p), &own, &shared, &ic::POST_SKILL)
        }
        Tab::Lsps => lsps(p, st.lsps.as_ref()),
    };
    page(p, "settings-project-extensions-page", header(p, t!("settings.tab.extensions"), t!("project.settings.extensions.description")),
        col().key("settings-project-extensions").w_full().child(tabs(p, st.tab)).child(col().w_full().pt(24.0).child(body)))
}

fn names(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v.dedup();
    v
}

/// Pill tabs, as Extensions' (`tabs-v2` pill).
fn tabs(p: &Pal, on: Tab) -> El {
    let mut list = row().key("settings-project-extensions-tabs").gap(6.0).pb(8.0);
    for (tab, id, label) in [(Tab::Mcps, "mcps", t!("settings.extensions.tab.mcps")), (Tab::Plugins, "plugins", t!("settings.extensions.tab.plugins")), (Tab::Skills, "skills", t!("settings.extensions.tab.skills")), (Tab::Lsps, "lsps", t!("project.settings.extensions.tab.lsps"))] {
        let picked = tab == on;
        let pill = row().key(format!("settings-project-extensions-tab-{id}")).role("tab").label(label).h(24.0).px(8.0).center().radius(6.0)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.settings_pages.project.extensions.tab = tab;
            })
            .child(text(label).font(TAB).color(if picked { p.text } else { p.muted }));
        list = list.child(if picked { pill.fill(p.layer3).box_shadow(Pal::ring_in(p.border_muted)) } else { pill.hover(p.hover) });
    }
    list
}

/// `ProjectSectionHeader` and its rows, then the shared ones folded under their count.
#[allow(clippy::too_many_arguments)]
fn section(s: &App, p: &Pal, kind: &str, action: El, own: &[String], shared: &[String], glyph: &'static rune::Icon) -> El {
    let empty = own.is_empty();
    let (none, none_about) = match kind {
        "mcps" => (t!("project.settings.extensions.empty.mcps.title"), t!("project.settings.extensions.empty.mcps.description")),
        "plugins" => (t!("project.settings.extensions.empty.plugins.title"), t!("project.settings.extensions.empty.plugins.description")),
        _ => (t!("project.settings.extensions.empty.skills.title"), t!("project.settings.extensions.empty.skills.description")),
    };
    let title = if empty { none } else { t!("project.settings.extensions.added") };
    let copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0)
        .child(text(title).font(HEAD).color(p.text))
        .children(empty.then(|| text(none_about).font(BODY).color(p.muted)));
    let head = row().key("project-settings-extension-section-header").w_full().gap(16.0).items_start().child(copy).child(action);
    let rows = |list: &[String], key: &str| catalog(p, key, list.iter().map(|n| row_of(p, &format!("{key}-{n}"), glyph, n)).collect());
    let mut c = col().key(format!("project-settings-extension-{kind}")).w_full().gap(16.0).child(head);
    if !empty {
        c = c.child(rows(own, &format!("settings-project-{kind}")));
    }
    if !shared.is_empty() {
        let open = s.settings_pages.project.extensions.shared;
        let trigger = row().key("project-settings-shared-trigger").role("button").label(t!("project.settings.extensions.shared")).expanded(open).h(28.0).pl(4.0).pr(8.0).gap(4.0).center_y().radius(4.0).self_start()
            .hover(p.layer2)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                let st = &mut s.settings_pages.project.extensions;
                st.shared = !st.shared;
            })
            .child(rune::el().size(20.0, 20.0).center().child(icon(&ic::FILL_TRIANGLE_DOWN, 16.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl()))
            .child(row().gap(6.0).center_y()
                .child(text(t!("project.settings.extensions.shared")).font(HEAD).color(p.text))
                .child(row().h(16.0).min_w(16.0).px(4.0).center().radius(3.0).fill(p.layer2).child(text(shared.len().to_string()).font(COUNT).color(p.muted))));
        c = c.child(col().w_full().gap(12.0).mt(16.0).child(trigger).children(open.then(|| rows(shared, &format!("settings-project-shared-{kind}")))));
    }
    c
}


fn row_of(p: &Pal, key: &str, glyph: &'static rune::Icon, name: &str) -> El {
    row().key(key.to_owned()).w_full().min_h(56.0).py(16.0).gap(8.0).center_y()
        .child(icon(glyph, 16.0).color(p.icon_muted))
        .child(text(name.to_owned()).font(NAME).color(p.text).truncate().shrink().min_w(0.0))
}

fn manage(p: &Pal) -> El {
    text(t!("settings.extensions.manageConfig")).font(BODY).color(p.muted)
}

fn skills_link(p: &Pal) -> El {
    let words = t!("settings.extensions.addSkills");
    const URL: &str = "https://opencode.ai/docs/skills";
    text(words).key("settings-extension-link").font(BODY).color(p.accent)
        .spans(vec![rune::Span::new(0, words.len()).underline(p.accent)])
        .link(rune::Link::new(0..words.len(), URL).role("link").label(words).hover(rune::LinkPaint::new().color(p.accent_hover).underline(p.accent_hover)))
        .on_link(|_: &mut App, url: &str, tx: &mut Tx| {
            let _ = tx.open_url(url);
        })
}

/// LSPs: the configured servers, LSP turned off, none, or a read that failed with Retry.
fn lsps(p: &Pal, read: Option<&Result<(bool, Vec<Lsp>), ()>>) -> El {
    let (disabled, servers) = match read {
        Some(Ok((d, list))) => (*d, list.as_slice()),
        _ => (false, &[][..]),
    };
    let empty = servers.is_empty();
    let (title, about) = match (empty, disabled) {
        (true, true) => (t!("project.settings.extensions.lsp.disabled.title"), Some(t!("project.settings.extensions.lsp.disabled.description"))),
        (true, false) => (t!("project.settings.extensions.lsp.empty.title"), Some(t!("project.settings.extensions.lsp.empty.description"))),
        _ => (t!("project.settings.extensions.lsp.configured"), None),
    };
    let copy = col().grow(1.0).shrink().min_w(0.0).gap(6.0)
        .child(text(title).font(HEAD).color(p.text))
        .children(about.map(|a| text(a).font(BODY).color(p.muted)));
    let head = row().w_full().gap(16.0).items_start().child(copy).child(text(t!("project.settings.extensions.lsp.description")).font(BODY).color(p.muted));
    let mut c = col().key("project-settings-extension-lsps").w_full().gap(16.0).child(head);
    match read {
        None => c = c.child(text(t!("common.loading")).font(BODY).color(p.muted)),
        Some(Err(())) => {
            c = c.child(row().gap(8.0).center_y()
                .child(text(t!("project.settings.extensions.lsp.loadFailed")).font(BODY).color(p.danger))
                .child(text(t!("project.settings.extensions.lsp.retry")).key("project-lsp-retry").role("button").font(BODY).color(p.text)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        let dir = s.settings_pages.project.extensions.dir.clone();
                        s.settings_pages.project.extensions.dir.clear();
                        load(s, &dir);
                    })));
        }
        Some(Ok(_)) if !empty => {
            let rows = servers.iter().map(|l| {
                let copy = col().grow(1.0).shrink().min_w(0.0).gap(4.0)
                    .child(text(l.name.clone()).font(NAME).color(p.text).truncate())
                    .children((!l.extensions.is_empty()).then(|| text(l.extensions.join(", ")).font(BODY).color(p.muted).wrap()));
                row().key(format!("settings-project-lsp-{}", l.name)).w_full().min_h(56.0).py(16.0).gap(16.0).center_y()
                    .child(row().grow(1.0).shrink().min_w(0.0).gap(8.0).center_y().child(icon(&ic::CODE, 16.0).color(p.icon_muted)).child(copy))
                    .child(row().gap(6.0).center_y().child(text(if l.disabled { t!("project.settings.extensions.lsp.status.disabled") } else { t!("project.settings.extensions.lsp.status.enabled") }).font(STATUS).color(p.muted)))
            }).collect();
            c = c.child(catalog(p, "settings-project-lsps", rows));
        }
        _ => {}
    }
    c
}
