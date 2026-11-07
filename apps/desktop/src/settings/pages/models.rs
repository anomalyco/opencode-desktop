//! Settings › Models (`settings/models/models.tsx`): every model the server lists, by provider,
//! each with a switch that shows or hides it in the model list, a search over them, and each
//! provider folding shut. The switches are the model list's own (`ModelPrefs` visibility), as
//! Manage models sets them.

use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::Switch;
use rune_theme_opencode::icons as ic;

use super::{NAME, NAME_REGULAR, catalog, header, page};
use crate::App;
use crate::palette::{Pal, kit};
use crate::pickers::Model;
use crate::widgets;

const POPULAR: [&str; 8] = ["opencode-go", "opencode", "anthropic", "github-copilot", "openai", "google", "openrouter", "vercel"];

/// The models the search finds, each provider's together by name, the providers in Desktop's
/// order: the popular ones first, then by name.
fn groups(s: &App) -> Vec<(String, Vec<&Model>)> {
    let query = s.settings_pages.search.text.trim().to_owned();
    let mut models: Vec<&Model> = s.pickers.models.iter()
        .filter(|m| query.is_empty() || session_view::pickers::search::score(&query, &[m.group.as_str(), m.name.as_str(), m.id.as_str()]).is_some())
        .collect();
    let rank = |m: &Model| match POPULAR.iter().position(|p| *p == m.provider) {
        Some(at) => (at, String::new()),
        None => (POPULAR.len(), m.group.to_lowercase()),
    };
    models.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let mut out: Vec<(String, Vec<&Model>)> = Vec::new();
    for m in models {
        match out.last_mut() {
            Some((p, list)) if *p == m.provider => list.push(m),
            _ => out.push((m.provider.clone(), vec![m])),
        }
    }
    out
}

pub fn view(s: &App, p: &Pal) -> El {
    let searching = !s.settings_pages.search.text.trim().is_empty();
    let groups = groups(s);
    let mut body = col().key("settings-models").w_full().gap(12.0);
    if groups.is_empty() {
        let status = if searching {
            row().key("settings-models-empty").w_full().py(48.0).gap(4.0).center()
                .child(text("No results for").font(NAME_REGULAR).color(p.muted))
                .child(text(format!("\u{201c}{}\u{201d}", s.settings_pages.search.text.trim())).font(NAME_REGULAR).color(p.text))
        } else {
            row().key("settings-models-empty").w_full().py(48.0).center().child(text("No model results").font(NAME_REGULAR).color(p.muted))
        };
        body = body.child(status);
    }
    for (provider, models) in &groups {
        let open = searching || !s.settings_pages.collapsed.contains(provider);
        body = body.child(group(s, p, provider, models, open, searching));
    }
    let head = col().w_full().gap(24.0)
        .child(header(p, "Models", "Choose which models appear in model picker"))
        .child(search(s, p));
    page(p, "settings-models-page", head, body)
}

/// `settings-filter-search`: 36 pt, `layer-02` at 60% (full under the pointer or while typed
/// in), the glass 16 pt in at 14 pt.
fn search(s: &App, p: &Pal) -> El {
    let focused = s.settings_pages.search.focused;
    row().key("settings-models-search").w_full().h(36.0).mx(1.0).pr(8.0).gap(8.0).center_y().radius(6.0)
        .fill(if focused { p.layer2 } else { p.search_bg })
        .hover(p.layer2)
        .child(el().pl(16.0).child(icon(&ic::MAGNIFYING_GLASS, 14.0).color(p.icon_muted)))
        .child(rune::text_input(&s.settings_pages.search, NAME_REGULAR).key("settings-models-input").single_line().grow(1.0).shrink().min_w(0.0)
            .color(p.text).placeholder("Search models", p.faint).caret(p.text, p.selection).label("Search models")
            .on_edit(|s: &mut App| &mut s.settings_pages.search)
            .on_input_change(|_: &mut App, tx: &mut Tx| tx.instant()))
}

/// A provider's section: its header folds it; its models in a catalog list with their switches.
fn group(s: &App, p: &Pal, provider: &str, models: &[&Model], open: bool, searching: bool) -> El {
    let name = models.first().map_or(provider.to_owned(), |m| m.group.clone());
    let fold = provider.to_owned();
    let mut trigger = row().key("trigger").role("button").label(name.clone()).h(28.0).pt(4.0).pr(8.0).pb(4.0).pl(4.0).gap(4.0).center_y().radius(4.0)
        .child(el().size(20.0, 20.0).center().child(icon(&ic::CHEVRON_DOWN, 14.0).color(p.icon_muted).rotate(if open { 0.0 } else { -std::f32::consts::FRAC_PI_2 })))
        .child(row().gap(8.0).center_y().child(widgets::provider_mark(provider, p.icon)).child(text(name).font(NAME).color(p.text)));
    if !searching {
        trigger = trigger.hover(p.layer2).on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if !s.settings_pages.collapsed.remove(&fold) {
                s.settings_pages.collapsed.insert(fold.clone());
            }
        });
    }
    let mut section = col().key(format!("settings-models-provider-{provider}")).w_full().gap(12.0).pb(if open { 12.0 } else { 0.0 })
        .child(row().key("settings-models-group-header").w_full().h(28.0).center_y().child(trigger));
    if open {
        let rows = models.iter().map(|m| model_row(s, p, m)).collect();
        section = section.child(catalog(p, "settings-models-list", rows));
    }
    section
}

fn model_row(s: &App, p: &Pal, m: &Model) -> El {
    let shown = s.pickers.visible(m);
    let key = m.key.clone();
    row().key(format!("settings-model-{}", m.key)).w_full().py(16.0).gap(16.0).center_y()
        .child(text(m.name.clone()).font(NAME_REGULAR).color(p.text).grow(1.0).shrink().min_w(0.0).truncate())
        .child(Switch::new(shown).web().label(m.name.clone()).hide_label()
            .on_toggle_tx(move |s: &mut App, tx: &mut Tx| {
                tx.instant();
                s.pickers.prefs.set_visibility([key.as_str()], !shown);
            })
            .build(kit(p)))
}

