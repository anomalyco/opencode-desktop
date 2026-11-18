//! Desktop Studio console adapter over `opencode_plugin_runtime::console`.

pub use opencode_plugin_runtime::console::{clear, eval_here, push, reloaded, version};

use rune_luau::Data;

use crate::App;

/// Every line as `ConsoleLine` records, oldest first.
pub fn lines(app: &App) -> Data {
    opencode_plugin_runtime::console::lines_with(|plugin| {
        app.plugins.list.iter().find(|p| p.manifest.id == plugin)?.origin.dir()
    })
}

/// Runs `source` in plugin `plugin`'s window VM, under its watchdog: as an expression first
/// (`return <source>`), else as a chunk.
pub fn eval(app: &App, plugin: &str, source: &str) -> Result<String, String> {
    let script = app
        .plugins
        .list
        .iter()
        .find(|p| p.manifest.id == plugin)
        .and_then(crate::plugin_host::Plugin::script)
        .ok_or_else(|| format!("{plugin} isn't running in this window"))?;
    super::cx::with_app(app, || opencode_plugin_runtime::console::eval_script(script, source))
}
