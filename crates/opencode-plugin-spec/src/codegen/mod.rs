//! Turns contract.json into every artifact a plugin author, an editor or the host reads.

mod check;
mod docs;
mod luau;
pub mod model;
mod rust;
mod schema;
mod selene;
mod sig;

pub const GENERATED: &str = "Generated from crates/opencode-plugin-spec/contract.json by `cargo run -p opencode-plugin-spec -- --write`. Do not edit.";

/// Every generated file: its path from the repository root, and its content.
pub fn render(c: &model::Contract) -> Result<Vec<(String, String)>, Vec<String>> {
    check::validate(c)?;
    let one = |r: Result<String, String>| r.map_err(|e| vec![e]);
    let mut files = vec![
        ("plugins/opencode.d.luau".to_owned(), one(luau::definitions(c))?),
        ("plugins/types/ui.luau".to_owned(), luau::ui_stub(c)),
        ("plugins/types/plugin.luau".to_owned(), luau::plugin_stub()),
        ("plugins/.luaurc".to_owned(), luau::luaurc()),
        ("plugins/plugin.schema.json".to_owned(), one(schema::manifest(c))?),
        ("plugins/opencode-selene.yml".to_owned(), one(selene::std(c))?),
        ("crates/opencode-plugin-spec/docs/api.md".to_owned(), docs::api(c)),
    ];
    for (name, content) in rust::files(c).map_err(|e| vec![e])? {
        files.push((format!("crates/opencode-plugin-spec/src/generated/{name}"), content));
    }
    Ok(files)
}

/// A text buffer with `\n` line endings.
#[derive(Default)]
pub struct Out(String);

impl Out {
    pub fn line(&mut self, s: &str) {
        self.0.push_str(s);
        self.0.push('\n');
    }

    pub fn blank(&mut self) {
        self.0.push('\n');
    }

    /// A banner comment between groups of Luau or YAML declarations.
    pub fn section(&mut self, title: &str) {
        self.line(&format!("-- {title} {}", "-".repeat(96 - title.len().min(90))));
        self.blank();
    }

    pub fn finish(self) -> String {
        let mut s = self.0;
        while s.ends_with("\n\n") {
            s.pop();
        }
        s
    }
}
