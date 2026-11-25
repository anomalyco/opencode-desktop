use opencode_plugin_spec::{ThemeSeeds, ThemeVariant};

use crate::App;
use crate::themes::{
    PluginTheme,
    seed::{Seeds, resolve_variant},
};

fn variant(input: &ThemeVariant, dark: bool) -> serde_json::Map<String, serde_json::Value> {
    let ThemeSeeds {
        neutral,
        primary,
        success,
        warning,
        error,
        info,
        interactive,
        diff_add,
        diff_delete,
        ..
    } = &input.seeds;
    let seeds = Seeds {
        neutral,
        primary,
        success,
        warning,
        error,
        info,
        interactive,
        diff_add,
        diff_delete,
    };
    resolve_variant(&seeds, dark, input.overrides.as_ref())
}

pub(super) fn publish(app: &App, i: usize) {
    let plugin = &app.plugins.list[i];
    plugin.half.themes_changed.set(false);
    let contribs = plugin.half.contribs.borrow();
    let themes = contribs
        .themes
        .iter()
        .map(|entry| {
            let theme = &entry.spec;
            PluginTheme {
                id: theme.id.clone(),
                name: theme.name.clone(),
                tokens: [variant(&theme.light, false), variant(&theme.dark, true)],
            }
        })
        .collect();
    replace(app, &plugin.manifest.id, themes);
}

pub(super) fn withdraw(app: &App, id: &str) {
    replace(app, id, Vec::new());
}

fn replace(app: &App, id: &str, themes: Vec<PluginTheme>) {
    if app.plugins.themes.replace(id, themes) {
        rune::Tracked::touch(&app.plugins);
        if let Some(wake) = &app.plugins.waker {
            wake();
        }
    }
}
