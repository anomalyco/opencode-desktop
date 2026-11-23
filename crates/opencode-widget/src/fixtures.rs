//! Copies of `@opencode/widget`'s preludes and built-in widget templates (anomalyco/opencode
//! `packages/widget/src/prelude.js`, `prelude-live.js` and `widget-templates/*.js`), and the diagram built-ins' example
//! data (`packages/widget/test/fixtures/diagrams.ts`, as JSON), for stories, fixtures and local
//! runs without a server. A live client always runs the program the server serves, never these;
//! refresh them when the canonical ones change.

/// The prelude (`ui`, `submit`, `dismiss`, `send`, `__widget_native_step`): every program starts
/// with it.
pub const PRELUDE: &str = include_str!("../fixtures/prelude.js");

/// The live prelude (`widget`, with `widget.gpu`, and `__widget_live`): after the prelude in a
/// live program only.
pub const LIVE_PRELUDE: &str = include_str!("../fixtures/prelude-live.js");

/// The program the server serves for `source` in `mode` (`widgets.md`, Prelude and step ABI):
/// `prelude + "\n" + source` for a step widget template, `prelude + "\n" + livePrelude + "\n" +
/// source` for a live one.
pub fn served(mode: &str, source: &str) -> String {
    if mode == "live" { format!("{PRELUDE}\n{LIVE_PRELUDE}\n{source}") } else { format!("{PRELUDE}\n{source}") }
}

/// The diagram built-ins, in the skill's order.
pub const DIAGRAMS: [&str; 17] = [
    "pie", "scatter", "histogram", "area", "heatmap", "treemap", "sankey", "graph", "tree", "timeline", "funnel", "radar", "gauge", "boxplot", "flowchart", "sequence", "table",
];

/// The built-in widget templates' names.
pub const BUILTINS: [&str; 22] = [
    "suggestions", "form", "confirm", "chart", "backdrop", "pie", "scatter", "histogram", "area", "heatmap", "treemap", "sankey", "graph", "tree", "timeline", "funnel", "radar", "gauge", "boxplot", "flowchart", "sequence", "table",
];

/// How built-in widget template `name` runs: every built-in steps (`"step"`); live widget templates are the
/// agent's own.
pub fn mode(_name: &str) -> &'static str {
    "step"
}

/// Built-in widget template `name`'s source.
pub fn source(name: &str) -> Option<&'static str> {
    Some(match name {
        "suggestions" => include_str!("../fixtures/widget-templates/suggestions.js"),
        "form" => include_str!("../fixtures/widget-templates/form.js"),
        "confirm" => include_str!("../fixtures/widget-templates/confirm.js"),
        "chart" => include_str!("../fixtures/widget-templates/chart.js"),
        "backdrop" => include_str!("../fixtures/widget-templates/backdrop.js"),
        "pie" => include_str!("../fixtures/widget-templates/pie.js"),
        "scatter" => include_str!("../fixtures/widget-templates/scatter.js"),
        "histogram" => include_str!("../fixtures/widget-templates/histogram.js"),
        "area" => include_str!("../fixtures/widget-templates/area.js"),
        "heatmap" => include_str!("../fixtures/widget-templates/heatmap.js"),
        "treemap" => include_str!("../fixtures/widget-templates/treemap.js"),
        "sankey" => include_str!("../fixtures/widget-templates/sankey.js"),
        "graph" => include_str!("../fixtures/widget-templates/graph.js"),
        "tree" => include_str!("../fixtures/widget-templates/tree.js"),
        "timeline" => include_str!("../fixtures/widget-templates/timeline.js"),
        "funnel" => include_str!("../fixtures/widget-templates/funnel.js"),
        "radar" => include_str!("../fixtures/widget-templates/radar.js"),
        "gauge" => include_str!("../fixtures/widget-templates/gauge.js"),
        "boxplot" => include_str!("../fixtures/widget-templates/boxplot.js"),
        "flowchart" => include_str!("../fixtures/widget-templates/flowchart.js"),
        "sequence" => include_str!("../fixtures/widget-templates/sequence.js"),
        "table" => include_str!("../fixtures/widget-templates/table.js"),
        _ => return None,
    })
}

/// Built-in widget template `name`'s program, as the server serves it ([`served`]).
pub fn program(name: &str) -> Option<String> {
    source(name).map(|s| served(mode(name), s))
}

/// Diagram built-in `name`'s example data.
pub fn example(name: &str) -> Option<serde_json::Value> {
    static EXAMPLES: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let all = EXAMPLES.get_or_init(|| serde_json::from_str(include_str!("../fixtures/diagrams.json")).unwrap_or_default());
    all.get(name).cloned()
}
