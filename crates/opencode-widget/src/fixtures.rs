//! Stand-ins for `@opencode/widget`'s prelude and built-in templates (`suggestions`, `form`,
//! `confirm`), written from the contract, for stories, fixtures and local runs without a server.
//! A live client always runs the program the server serves, never these.

/// The prelude (`ui`, `submit`, `dismiss`, `__widget_step`).
pub const PRELUDE: &str = include_str!("../fixtures/prelude.js");

/// The built-in templates' names.
pub const BUILTINS: [&str; 3] = ["suggestions", "form", "confirm"];

/// Built-in template `name`'s source.
pub fn source(name: &str) -> Option<&'static str> {
    Some(match name {
        "suggestions" => include_str!("../fixtures/suggestions.js"),
        "form" => include_str!("../fixtures/form.js"),
        "confirm" => include_str!("../fixtures/confirm.js"),
        _ => return None,
    })
}

/// Built-in template `name`'s program, as the server serves it: `prelude + "\n" + source`.
pub fn program(name: &str) -> Option<String> {
    source(name).map(|s| format!("{PRELUDE}\n{s}"))
}
