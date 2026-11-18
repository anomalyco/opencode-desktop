//! Settings › Models (`settings/models/models.tsx`): every model the server lists, by provider,
//! each with a switch that shows or hides it in the model list, a search over them, and each
//! provider folding shut.

use std::cell::OnceCell;
use std::collections::HashSet;

use opencode_i18n::t;
use opencode_ui::popup::PopupHost;
use opencode_ui::widgets;
use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::Switch;
use rune_theme_opencode::icons as ic;
use serde_json::{Value, json};
use session_view::palette::{Pal, kit};
use session_view::pickers::Model;

use super::{NAME, NAME_REGULAR, catalog, header_with, page};
use crate::host::SettingsHost;
use crate::stored;

/// The providers folded shut, kept per server under Desktop's global
/// `settings-v2.models.providers` (`collapsed`, by provider): read the first time Models
/// draws, written on each fold.
#[derive(Default)]
pub struct Folds(OnceCell<HashSet<String>>);

impl Folds {
    fn get(&self, s: &dyn SettingsHost) -> &HashSet<String> {
        self.0.get_or_init(|| {
            server(s)
                .map(|server| {
                    read()[server]["collapsed"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter(|(_, shut)| shut.as_bool() == Some(true))
                        .map(|(provider, _)| provider.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// Folds `provider` shut, or open again, and saves the folds.
    fn toggle(s: &mut dyn SettingsHost, provider: &str) {
        let mut shut = s.settings_pages().collapsed.get(s).clone();
        if !shut.remove(provider) {
            shut.insert(provider.to_owned());
        }
        if let Some(server) = server(s) {
            let collapsed: serde_json::Map<String, Value> = shut.iter().map(|p| (p.clone(), Value::Bool(true))).collect();
            opencode_platform::storage::update_json(stored::GLOBAL, KEY, |all| {
                all.insert(server, json!({ "collapsed": collapsed }));
            });
        }
        s.settings_pages_mut().collapsed = Folds(OnceCell::from(shut));
    }
}

const KEY: &str = "settings-v2.models.providers";

fn read() -> Value {
    opencode_platform::storage::read_json(stored::GLOBAL, KEY).unwrap_or_else(|| json!({}))
}

/// The server the folds are kept for; none while nothing persists (headless, a fixture).
fn server(s: &dyn SettingsHost) -> Option<String> {
    s.persists().then(|| s.live_server_base()).flatten()
}

const POPULAR: [&str; 8] = ["opencode-go", "opencode", "anthropic", "github-copilot", "openai", "google", "openrouter", "vercel"];

/// The models the search finds, each provider's together by name, the providers in Desktop's
/// order: the popular ones first, then by name.
fn groups(s: &dyn SettingsHost) -> Vec<(String, Vec<&Model>)> {
    let query = s.settings_pages().search.text.trim().to_owned();
    let mut models: Vec<&Model> = s
        .models()
        .iter()
        .filter(|m| query.is_empty() || session_view::pickers::search::score(&query, &[m.group.as_str(), m.name.as_str(), m.id.as_str()]).is_some())
        .collect();
    let rank = |m: &Model| match POPULAR.iter().position(|p| *p == m.provider) {
        Some(at) => (at, String::new()),
        None => (POPULAR.len(), m.group.to_lowercase()),
    };
    models.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| rune::text::compare_names(&a.name, &b.name)));
    let mut out: Vec<(String, Vec<&Model>)> = Vec::new();
    for m in models {
        match out.last_mut() {
            Some((p, list)) if *p == m.provider => list.push(m),
            _ => out.push((m.provider.clone(), vec![m])),
        }
    }
    out
}

fn toggle_provider_fold(s: &mut dyn SettingsHost, provider: &str, tx: &mut Tx) {
    tx.instant();
    Folds::toggle(s, provider);
}

fn toggle_model_visibility(s: &mut dyn SettingsHost, key: &str, visible: bool, tx: &mut Tx) {
    tx.instant();
    s.set_model_visibility(key, visible);
}

pub fn view<H: SettingsHost + PopupHost + 'static>(s: &H, p: &Pal) -> El {
    let searching = !s.settings_pages().search.text.trim().is_empty();
    let groups = groups(s);
    let mut body = col().key("settings-models").w_full().gap(12.0);
    if groups.is_empty() {
        let status = if searching {
            row().key("settings-models-empty").w_full().py(48.0).gap(4.0).center().children(super::no_results_words(p, &s.settings_pages().search.text))
        } else {
            row().key("settings-models-empty").w_full().py(48.0).center().child(text(t!("dialog.model.empty")).font(NAME_REGULAR).color(p.muted))
        };
        body = body.child(status);
    }
    for (provider, models) in &groups {
        let open = searching || !s.settings_pages().collapsed.get(s).contains(provider);
        body = body.child(group(s, p, provider, models, open, searching));
    }
    let head = col()
        .w_full()
        .gap(24.0)
        .child(header_with(p, t!("settings.models.title"), t!("settings.models.description"), super::servers::switcher(s, p)))
        .child(search(s, p));
    page(p, "settings-models-page", head, body)
}

/// `settings-filter-search`: 36 pt, `layer-02` at 60%, glass 16 pt in at 14 pt.
fn search<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let focused = s.settings_pages().search.focused;
    row()
        .key("settings-models-search")
        .w_full()
        .h(36.0)
        .mx(1.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if focused { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .child(el().pl(16.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(
            session_view::style::field(&s.settings_pages().search, NAME_REGULAR)
                .key("settings-models-input")
                .single_line()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .color(p.text)
                .placeholder(t!("dialog.model.search.placeholder"), p.faint)
                .caret(p.text, p.selection)
                .selection_style(p.field_selection)
                .label_static(t!("dialog.model.search.placeholder"))
                .on_edit(|s: &mut H| &mut s.settings_pages_mut().search)
                .on_input_change(|_: &mut H, tx: &mut Tx| tx.instant()),
        )
}

/// A provider's section: its header folds it; its models in a catalog list with their switches.
fn group<H: SettingsHost + 'static>(s: &H, p: &Pal, provider: &str, models: &[&Model], open: bool, searching: bool) -> El {
    let name = models.first().map_or_else(|| provider.to_owned(), |m| m.group.clone());
    let fold = provider.to_owned();
    let mut trigger = row()
        .key("trigger")
        .role("button")
        .label(name.clone())
        .h(28.0)
        .pt(4.0)
        .pr(8.0)
        .pb(4.0)
        .pl(4.0)
        .gap(4.0)
        .center_y()
        .radius(4.0)
        .child(
            el().size(20.0, 20.0)
                .center()
                .child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 }).mirror_in_rtl()),
        )
        .child(row().gap(8.0).center_y().child(widgets::provider_mark(provider, p.icon)).child(text(name).font(NAME).color(p.text)));
    if !searching {
        trigger = trigger.hover(p.layer2).on_press_tx(move |s: &mut H, tx: &mut Tx| toggle_provider_fold(s, &fold, tx));
    }
    let mut section = col()
        .key(format!("settings-models-provider-{provider}"))
        .w_full()
        .gap(12.0)
        .pb(if open { 12.0 } else { 0.0 })
        .child(row().key("settings-models-group-header").w_full().h(28.0).center_y().child(trigger));
    if open {
        let rows = models.iter().map(|m| model_row(s, p, m)).collect();
        section = section.child(catalog(p, "settings-models-list", rows));
    }
    section
}

fn model_row<H: SettingsHost + 'static>(s: &H, p: &Pal, m: &Model) -> El {
    let shown = s.model_visible(m);
    let key = m.key.clone();
    row()
        .key(format!("settings-model-{}", m.key))
        .w_full()
        .py(16.0)
        .gap(16.0)
        .center_y()
        .child(text(m.name.clone()).font(NAME_REGULAR).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
        .child(
            Switch::new(shown)
                .web()
                .label(m.name.clone())
                .hide_label()
                .on_toggle_tx(move |s: &mut H, tx: &mut Tx| toggle_model_visibility(s, &key, !shown, tx))
                .build(kit(p)),
        )
}
