//! Copies of `@opencode/widget`'s prelude and built-in templates (anomalyco/opencode
//! `packages/widget/src/prelude.js` and `templates/*.js`), and the diagram built-ins' example
//! data (`packages/widget/test/fixtures/diagrams.ts`, as JSON), for stories, fixtures and local
//! runs without a server. A live client always runs the program the server serves, never these;
//! refresh them when the canonical ones change.

/// The prelude (`ui`, `submit`, `dismiss`, `send`, `__widget_step`, and live mode's `widget`
/// and `__widget_live`).
pub const PRELUDE: &str = include_str!("../fixtures/prelude.js");

/// The diagram built-ins, in the skill's order.
pub const DIAGRAMS: [&str; 17] = [
    "pie", "scatter", "histogram", "area", "heatmap", "treemap", "sankey", "graph", "tree", "timeline", "funnel", "radar", "gauge", "boxplot", "flowchart", "sequence", "table",
];

/// The built-in templates' names.
pub const BUILTINS: [&str; 22] = [
    "suggestions", "form", "confirm", "chart", "backdrop", "pie", "scatter", "histogram", "area", "heatmap", "treemap", "sankey", "graph", "tree", "timeline", "funnel", "radar", "gauge", "boxplot", "flowchart", "sequence", "table",
];

/// How built-in template `name` runs: every built-in steps (`"step"`); live templates are the
/// agent's own.
pub fn mode(_name: &str) -> &'static str {
    "step"
}

/// Built-in template `name`'s source.
pub fn source(name: &str) -> Option<&'static str> {
    Some(match name {
        "suggestions" => include_str!("../fixtures/suggestions.js"),
        "form" => include_str!("../fixtures/form.js"),
        "confirm" => include_str!("../fixtures/confirm.js"),
        "chart" => include_str!("../fixtures/chart.js"),
        "backdrop" => include_str!("../fixtures/backdrop.js"),
        "pie" => include_str!("../fixtures/pie.js"),
        "scatter" => include_str!("../fixtures/scatter.js"),
        "histogram" => include_str!("../fixtures/histogram.js"),
        "area" => include_str!("../fixtures/area.js"),
        "heatmap" => include_str!("../fixtures/heatmap.js"),
        "treemap" => include_str!("../fixtures/treemap.js"),
        "sankey" => include_str!("../fixtures/sankey.js"),
        "graph" => include_str!("../fixtures/graph.js"),
        "tree" => include_str!("../fixtures/tree.js"),
        "timeline" => include_str!("../fixtures/timeline.js"),
        "funnel" => include_str!("../fixtures/funnel.js"),
        "radar" => include_str!("../fixtures/radar.js"),
        "gauge" => include_str!("../fixtures/gauge.js"),
        "boxplot" => include_str!("../fixtures/boxplot.js"),
        "flowchart" => include_str!("../fixtures/flowchart.js"),
        "sequence" => include_str!("../fixtures/sequence.js"),
        "table" => include_str!("../fixtures/table.js"),
        _ => return None,
    })
}

/// Built-in template `name`'s program, as the server serves it: `prelude + "\n" + source`.
pub fn program(name: &str) -> Option<String> {
    source(name).map(|s| format!("{PRELUDE}\n{s}"))
}

/// Diagram built-in `name`'s example data.
pub fn example(name: &str) -> Option<serde_json::Value> {
    static EXAMPLES: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let all = EXAMPLES.get_or_init(|| serde_json::from_str(include_str!("../fixtures/diagrams.json")).unwrap_or_default());
    all.get(name).cloned()
}
