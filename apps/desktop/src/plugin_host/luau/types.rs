//! `opencode-desktop --write-plugin-types <dir>`: writes what a plugin author's editor needs
//! beside their plugins (`opencode.d.luau`, `types/*.luau`, `.luaurc`, `plugin.schema.json`,
//! `opencode-selene.yml`), generated from the contract this app implements, and exits.

use std::path::Path;

/// Writes the files into `dir`; the paths written, or why it stopped.
pub fn write(dir: &Path) -> Result<Vec<String>, String> {
    let mut wrote = Vec::new();
    for (rel, content) in opencode_plugin_spec::AUTHOR_FILES {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
        wrote.push(path.display().to_string());
    }
    Ok(wrote)
}

/// Handles `--write-plugin-types <dir>` at launch: true when it ran (the app then exits).
pub fn from_args() -> bool {
    let Some(dir) = crate::arg("--write-plugin-types") else { return false };
    match write(Path::new(&dir)) {
        Ok(paths) => {
            for p in paths {
                println!("wrote {p}");
            }
            println!("contract {}", opencode_plugin_spec::CONTRACT);
        }
        Err(e) => eprintln!("--write-plugin-types: {e}"),
    }
    true
}
