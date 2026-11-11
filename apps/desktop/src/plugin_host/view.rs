//! Settings › Plugins: each plugin with where it runs from, its state, Reload and its switch,
//! then the settings rows each running plugin registered, under its name.

use rune::{El, Tx, col, row, text};

use super::icons::intern;
use super::luau::{Life, Plugin};
use crate::App;
use crate::palette::Pal;
use crate::view::UI_MED;

/// Settings › Plugins. Nothing when no plugin is installed.
pub fn settings_section(s: &App, p: &Pal) -> Option<El> {
    let plugins = &s.plugins.luau.plugins;
    if plugins.is_empty() {
        return None;
    }
    let titles: Vec<String> = plugins.iter().map(|lp| format!("{} {}", lp.manifest.name, lp.manifest.version)).collect();
    let rows = plugins.iter().zip(&titles).map(|(lp, title)| (format!("plugin-{}", lp.manifest.id), title.as_str(), status(lp), controls(p, lp)));
    let list = crate::settings::settings_card(p, "plugins-list", rows);
    let own = (0..plugins.len()).filter_map(|i| plugin_rows(s, p, i));
    Some(col().key("plugins-settings").w_full().gap(32.0).child(crate::settings::section(p, "Plugins", list)).children(own))
}

fn status(lp: &Plugin) -> String {
    let mut parts = vec![format!("{} · Luau {} · {}", lp.tier.label(), lp.manifest.contract, lp.life.label())];
    parts.extend(lp.manifest.description.clone().filter(|d| !d.is_empty()));
    parts.extend(lp.life.why().map(str::to_owned));
    parts.push(lp.origin.describe());
    parts.join("\n")
}

fn controls(p: &Pal, lp: &Plugin) -> El {
    let id = lp.manifest.id.clone();
    let switchable = matches!(lp.life, Life::Running(_) | Life::Reloading(_) | Life::Disabled | Life::Errored { .. } | Life::AwaitingTrust);
    let toggle = switchable.then(|| {
        let id = id.clone();
        crate::settings::switch(p, &format!("plugin-switch-{id}"), lp.running()).on_press_tx(move |app: &mut App, tx: &mut Tx| super::luau::toggle(app, &id, tx))
    });
    let reload = matches!(lp.life, Life::Running(_) | Life::Errored { .. }).then(|| {
        row()
            .key(format!("plugin-reload-{id}"))
            .role("button")
            .label("Reload")
            .h(28.0)
            .px(10.0)
            .center()
            .radius(6.0)
            .border(0.5, p.border)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |app: &mut App, tx: &mut Tx| {
                tx.instant();
                if let Some(i) = app.plugins.luau.find(&id) {
                    super::luau::reload(app, i, None);
                }
            })
            .child(text("Reload").font(UI_MED).color(p.text))
    });
    row().gap(12.0).center_y().children(reload).children(toggle)
}

/// The settings rows plugin `i` registered, under its name.
fn plugin_rows(s: &App, p: &Pal, i: usize) -> Option<El> {
    let lp = &s.plugins.luau.plugins[i];
    if !lp.running() {
        return None;
    }
    let rows = super::luau::surfaces::setting_rows(s, p, i);
    if rows.is_empty() {
        return None;
    }
    let card = crate::settings::settings_card(p, &format!("plugin-{}-settings", lp.manifest.id), rows.iter().map(|(k, t, d, c)| (k.clone(), t.as_str(), d.clone(), c.clone())));
    Some(crate::settings::section(p, intern(&lp.manifest.name), card))
}
