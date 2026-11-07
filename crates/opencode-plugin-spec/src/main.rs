//! `cargo run -p opencode-plugin-spec -- --write` regenerates every artifact from contract.json;
//! `-- --check` exits 1 when any of them has drifted from it.

mod codegen;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode != "--write" && mode != "--check" {
        eprintln!("usage: opencode-plugin-spec --write | --check");
        return ExitCode::from(2);
    }
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = crate_dir.join("../..");
    let contract = match std::fs::read_to_string(crate_dir.join("contract.json"))
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<codegen::model::Contract>(&text).map_err(|e| format!("contract.json: {e}")))
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let files = match codegen::render(&contract) {
        Ok(files) => files,
        Err(errors) => {
            for e in errors {
                eprintln!("contract.json: {e}");
            }
            return ExitCode::FAILURE;
        }
    };
    let drifted: Vec<&str> = files
        .iter()
        .filter(|(path, content)| std::fs::read_to_string(root.join(path)).map(|old| old.replace("\r\n", "\n")).ok().as_deref() != Some(content.as_str()))
        .map(|(path, _)| path.as_str())
        .collect();
    if mode == "--check" {
        if drifted.is_empty() {
            println!("{} generated files match contract.json {}", files.len(), contract.contract);
            return ExitCode::SUCCESS;
        }
        for path in drifted {
            eprintln!("drifted: {path}");
        }
        eprintln!("run `cargo run -p opencode-plugin-spec -- --write`");
        return ExitCode::FAILURE;
    }
    for (path, content) in files.iter().filter(|(path, _)| drifted.contains(&path.as_str())) {
        let target: PathBuf = root.join(path);
        if let Some(dir) = target.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&target, content) {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
        println!("wrote {path}");
    }
    println!("{} generated files match contract.json {}", files.len(), contract.contract);
    ExitCode::SUCCESS
}
