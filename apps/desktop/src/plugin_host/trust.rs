//! The one-time prompt before a plugin from outside the app first runs (one from
//! `plugins-override/` or `<data>/plugins/`). Its OK is for the plugin's files as they are: their
//! SHA-256 is remembered, and a changed plugin asks again. Built-ins and `--plugin-dir` don't ask.
//! What the user chose (the plugins turned off, the hashes trusted) is kept in
//! `<data>/plugins-state.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rune::{El, TextStyle, Tx, col, el, row, spacer, text};

use super::{Life, Trust, TrustEvent};
use crate::App;
use crate::palette::Pal;
use crate::view::{UI, UI_MED};

/// The plugin whose prompt shows now: the first waiting one the user hasn't put off.
fn shown(app: &App) -> Option<usize> {
    app.plugins.list.iter().position(|p| matches!(p.life, Life::AwaitingTrust) && matches!(p.trust, Trust::Unknown { .. }))
}

pub fn dialog(s: &App, p: &Pal) -> Vec<El> {
    let Some(i) = shown(s) else { return Vec::new() };
    let plugin = &s.plugins.list[i];
    let (id, name) = (plugin.manifest.id.clone(), plugin.manifest.name.clone());
    let button = |key: &str, label: &str, ink: rune::Color, fill: Option<rune::Color>| {
        let mut b = row().key(key.to_owned()).role("button").label(label.to_owned()).h(28.0).px(12.0).radius(6.0).center().hover(p.hover);
        if let Some(f) = fill {
            b = b.fill(f).hover(f.alpha(0.9));
        }
        b.child(text(label.to_owned()).font(UI_MED).color(ink))
    };
    let mut card = col()
        .key("luau-trust-card")
        .w(420.0)
        .padding(20.0)
        .gap(6.0)
        .radius(12.0)
        .fill(p.bg_base)
        .box_shadow(Pal::ring_out(p.ring))
        .shadow(8.0, 32.0, p.shadow_far)
        .child(text(format!("Enable plugin \"{name}\"?")).font(TextStyle::new(15.0, 600.0).lh(24.0)).color(p.text))
        .child(text(format!("{name} {} can read and change your files, run programs and reach the network. Only enable plugins you trust.", plugin.manifest.version)).font(UI).color(p.muted).wrap());
    if let Some(d) = plugin.manifest.description.as_ref().filter(|d| !d.is_empty()) {
        card = card.child(text(d.clone()).font(UI).color(p.muted).wrap().mt(6.0));
    }
    card = card.child(text(plugin.origin.describe()).font(crate::view::UI_SMALL).color(p.faint).wrap().selectable(true).mt(6.0));
    let (later, ok) = (id.clone(), id);
    card = card.child(
        row()
            .key("luau-trust-footer")
            .gap(8.0)
            .mt(14.0)
            .child(spacer())
            .child(button("luau-trust-later", "Not now", p.text, None).on_press_tx(move |s: &mut App, tx: &mut Tx| answer(s, &later, TrustEvent::Later, tx)))
            .child(button("luau-trust-enable", "Enable", p.on_fill, Some(p.bg_accent)).on_press_tx(move |s: &mut App, tx: &mut Tx| answer(s, &ok, TrustEvent::Accept, tx))),
    );
    vec![
        el().key("luau-trust-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.dim),
        el().key("luau-trust-modal").absolute(0.0, 0.0).w_full().h_full().center().child(card).raise(),
    ]
}

fn answer(app: &mut App, id: &str, event: TrustEvent, tx: &mut Tx) {
    tx.instant();
    if let Some(i) = app.plugins.find(id) {
        super::step_trust(app, i, event);
    }
}

fn state_file() -> Option<PathBuf> {
    crate::dirs::data().map(|d| d.join("plugins-state.json"))
}

/// The plugins turned off, and the plugins from outside the app trusted.
pub(super) fn load_state() -> (BTreeSet<String>, BTreeMap<String, String>) {
    let Some(text) = state_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return Default::default() };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let disabled = v["disabled"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let trusted = v["trusted"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
    (disabled, trusted)
}

pub fn save_state(app: &App) {
    if !app.persists() {
        return;
    }
    if let Some(file) = state_file() {
        let v = serde_json::json!({ "disabled": app.plugins.disabled, "trusted": app.plugins.trusted });
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(file, v.to_string()).ok();
    }
}
