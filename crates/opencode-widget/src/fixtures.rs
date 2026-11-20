//! Copies of `@opencode/widget`'s prelude and built-in templates (`suggestions`, `form`,
//! `confirm`, `chart`, `backdrop`, the live `minigame`; anomalyco/opencode
//! `packages/widget/src/prelude.js` and `templates/*.js`), for
//! stories, fixtures and local runs without a server. A live client always runs the program the
//! server serves, never these; refresh them when the canonical ones change.

/// The prelude (`ui`, `submit`, `dismiss`, `send`, `__widget_step`, and live mode's `widget`
/// and `__widget_live`).
pub const PRELUDE: &str = include_str!("../fixtures/prelude.js");

/// The built-in templates' names.
pub const BUILTINS: [&str; 6] = ["suggestions", "form", "confirm", "chart", "backdrop", "minigame"];

/// How built-in template `name` runs: `"live"` or `"step"`.
pub fn mode(name: &str) -> &'static str {
    if name == "minigame" { "live" } else { "step" }
}

/// Built-in template `name`'s source.
pub fn source(name: &str) -> Option<&'static str> {
    Some(match name {
        "suggestions" => include_str!("../fixtures/suggestions.js"),
        "form" => include_str!("../fixtures/form.js"),
        "confirm" => include_str!("../fixtures/confirm.js"),
        "chart" => include_str!("../fixtures/chart.js"),
        "backdrop" => include_str!("../fixtures/backdrop.js"),
        "minigame" => include_str!("../fixtures/minigame.js"),
        _ => return None,
    })
}

/// Built-in template `name`'s program, as the server serves it: `prelude + "\n" + source`.
pub fn program(name: &str) -> Option<String> {
    source(name).map(|s| format!("{PRELUDE}\n{s}"))
}
