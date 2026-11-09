fn main() {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_WINDOW_TITLE");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUNDLE_ID");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_URL_SCHEME");
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    embed_luau_plugins(std::path::Path::new(&env("CARGO_MANIFEST_DIR").unwrap_or_default()), std::path::Path::new(&env("OUT_DIR").unwrap_or_default()));
    if env("CARGO_CFG_TARGET_OS").as_deref() != Some("windows") {
        return;
    }
    // A release build is a windowed program: no console window opens beside it from the Start
    // menu, a link or a toast (`launch::attach_console` still prints to a terminal that starts
    // it). Development builds keep their console for logs.
    if env("OPENCODE_RUNE_BUILD_CHANNEL").is_some_and(|c| c != "local") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/ENTRY:mainCRTStartup");
    }
    // The exe's icon (Explorer, the taskbar, Alt+Tab, a pinned shortcut), version and manifest.
    let title = env("OPENCODE_RUNE_WINDOW_TITLE").unwrap_or_else(|| "OpenCode (Rune)".into());
    let version = env("OPENCODE_RUNE_VERSION").or_else(|| env("CARGO_PKG_VERSION")).unwrap_or_default();
    let dir = std::path::PathBuf::from(env("CARGO_MANIFEST_DIR").unwrap_or_default());
    let (icon, manifest) = (dir.join("assets/icon.ico"), dir.join("assets/windows/app.manifest"));
    let resources = win_install::resources::Resources::new(&icon, &manifest, win_install::EXE, &version).names(&title, &title);
    if let Err(e) = win_install::resources::embed(&resources) {
        panic!("{e}");
    }
}

/// The built-in Luau plugins (`plugins/<id>/` whose plugin.toml names a `contract`), embedded
/// file by file as `luau_builtins.rs`: the fallback when no folder on disk has them.
fn embed_luau_plugins(manifest_dir: &std::path::Path, out: &std::path::Path) {
    use std::fmt::Write as _;
    let root = manifest_dir.join("../../plugins");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut code = String::from("pub static BUILTINS: &[(&str, &[(&str, &str)])] = &[\n");
    let mut dirs: Vec<_> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let toml = dir.join("plugin.toml");
        let Ok(text) = std::fs::read_to_string(&toml) else { continue };
        if !text.lines().any(|l| l.trim_start().starts_with("contract")) {
            continue;
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        files.sort();
        writeln!(code, "    ({id:?}, &[").ok();
        for (rel, path) in files {
            println!("cargo:rerun-if-changed={}", path.display());
            writeln!(code, "        ({rel:?}, include_str!({:?})),", path.display().to_string()).ok();
        }
        code.push_str("    ]),\n");
    }
    code.push_str("];\n");
    let luaurc = root.join(".luaurc");
    println!("cargo:rerun-if-changed={}", luaurc.display());
    writeln!(code, "pub static LUAURC: &str = include_str!({:?});", luaurc.display().to_string()).ok();
    std::fs::write(out.join("luau_builtins.rs"), code).ok();

    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
        for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if path.is_dir() {
                if name != "src" && name != "impl" && name != "target" {
                    walk(root, &path, out);
                }
                continue;
            }
            let keep = name.ends_with(".luau") || name == "plugin.toml" || name == ".luaurc" || (name.ends_with(".json") && path.parent().is_some_and(|p| p.ends_with("locales")));
            if let (true, Ok(rel)) = (keep, path.strip_prefix(root)) {
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.push((rel.join("/"), path));
            }
        }
    }
}
