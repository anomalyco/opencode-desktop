/// Windows: the static imports loaded on first use (`/DELAYLOAD`), with what calls them.
const DELAY_LOADED: &[&str] = &[
    "comctl32.dll",
    "uiautomationcore.dll",
    "wintrust.dll",
    "crypt32.dll",
    "setupapi.dll",
    "ws2_32.dll",
    "winmm.dll",
    "oleaut32.dll",
    "ole32.dll",
    "shell32.dll",
    "dwrite.dll",
    "dxgi.dll",
    "psapi.dll",
];

fn main() {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_WINDOW_TITLE");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUNDLE_ID");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_URL_SCHEME");
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    // A build recording the profile-guided optimisation profile (`scripts/pgo.ps1`) writes it
    // before the fast exit (`launch::write_profile`).
    println!("cargo:rustc-check-cfg=cfg(pgo_generate)");
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");
    if env("CARGO_ENCODED_RUSTFLAGS").is_some_and(|flags| flags.split('\x1f').any(|f| f.contains("profile-generate"))) {
        println!("cargo:rustc-cfg=pgo_generate");
    }    embed_luau_plugins(std::path::Path::new(&env("CARGO_MANIFEST_DIR").unwrap_or_default()), std::path::Path::new(&env("OUT_DIR").unwrap_or_default()));
    embed_build_commit(std::path::Path::new(&env("CARGO_MANIFEST_DIR").unwrap_or_default()));
    if env("CARGO_CFG_TARGET_OS").as_deref() != Some("windows") {
        return;
    }
    // A release build is a windowed program: no console window opens beside it from the Start
    // menu, a link or a toast (`launch::attach_console` still prints to a terminal that starts
    // it), and no console host starts with it (about 17,000 µs per launch on blade). A local
    // `--release` build is one too, so what it measures is what ships. Debug builds keep their
    // console for logs.
    if env("OPENCODE_RUNE_BUILD_CHANNEL").is_some_and(|c| c != "local") || env("PROFILE").as_deref() == Some("release") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/ENTRY:mainCRTStartup");
    }
    // System DLLs a first frame doesn't call load on first use instead of before `main`: the
    // loader maps every static import first (43 DLLs, about 14,100 µs on blade; comctl32 alone
    // 2,800). The delay-load helper resolves each on its first call.
    for dll in DELAY_LOADED {
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg-bin=opencode-desktop=delayimp.lib");
    // The main thread reserves 8 MiB of stack, as macOS and Linux give a main thread; Windows'
    // default is 1 MiB. Layout recurses once per nesting level (about 14 KB a Markdown list level
    // in a release build), so a reply nested 60 deep overflowed 1 MiB and closed the app. Only
    // address space is reserved: pages are committed as the stack reaches them. Here rather than
    // in .cargo/config.toml so it holds for every profile and for builds that set RUSTFLAGS (the
    // PGO recording does), and only for this executable.
    if env("CARGO_CFG_TARGET_ENV").as_deref() == Some("msvc") {
        println!("cargo:rustc-link-arg-bin=opencode-desktop=/STACK:8388608");
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

/// The built-in plugins (every `plugins/<id>/` with a `plugin.toml`), embedded file by file as
/// `luau_builtins.rs`: the fallback when no folder on disk has them. Each `.luau` file's bytecode
/// replaces its text in the module table, compiled here with Rune's settings,
/// so a launch doesn't compile them: compiling was most of each plugin's start (about 120,000 µs of
/// the 175,000 µs plugins took before the first frame on blade).
fn embed_luau_plugins(manifest_dir: &std::path::Path, out: &std::path::Path) {
    use std::fmt::Write as _;
    let root = manifest_dir.join("../../plugins");
    println!("cargo:rerun-if-changed={}", root.display());
    let compiler = mlua::chunk::Compiler::new().set_optimization_level(2).set_type_info_level(1).set_debug_level(1);
    let bytecode = out.join("luau-bytecode");
    std::fs::create_dir_all(&bytecode).ok();
    let mut consts = String::new();
    let mut precompiled = String::from("pub type BuiltinModule = (&'static str, &'static [u8], bool);\npub static MODULES: &[(&str, &[BuiltinModule])] = &[\n");
    let mut code = String::from("pub type BuiltinFile = (&'static str, &'static str);\npub static BUILTINS: &[(&str, &[BuiltinFile], &[u8])] = &[\n");
    // What each plugin contributes (`ctx:AddSlot`, `ctx:AddPanel`, ...), read from its source, so the host
    // knows which plugins a first screen shows before starting any (`plugin_host::first_screen`).
    let mut contributions = String::from("pub static CONTRIBUTIONS: &[(&str, &[&str])] = &[\n");
    // Each plugin's files hashed, so a record of what it contributed (`plugin_host::dormant`) is
    // known to be this build's.
    let mut hashes = String::from("pub static HASHES: &[(&str, &str)] = &[\n");
    let mut dirs: Vec<_> = std::fs::read_dir(&root).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let toml = dir.join("plugin.toml");
        if !toml.is_file() {
            continue;
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut files = Vec::new();
        walk(&dir, &dir, &mut files);
        files.sort();
        // FNV-1a over each file's path and bytes, in order: stable across toolchains, so a record
        // matches as long as this plugin's own files are the same.
        let hash = files.iter().flat_map(|(rel, path)| [rel.as_bytes().to_vec(), vec![0], std::fs::read(path).unwrap_or_default(), vec![0]]).flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        writeln!(hashes, "    ({id:?}, {:?}),", format!("{hash:016x}")).ok();
        writeln!(code, "    ({id:?}, &[").ok();
        // Its `locales/*.json` become one table (`plugin_host::catalogs`), read in place: a start parses
        // no JSON, and only the interface language's and English's strings are ever touched.
        let locales: Vec<(String, std::path::PathBuf)> = files.iter().filter_map(|(rel, path)| Some((rel.strip_prefix("locales/")?.strip_suffix(".json")?.to_owned(), path.clone()))).collect();
        files.retain(|(rel, _)| !(rel.starts_with("locales/") && rel.ends_with(".json")));
        let catalog = out.join(format!("catalog-{id}.bin"));
        std::fs::write(&catalog, catalog_table(&locales)).ok();
        for (_, path) in &locales {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let mut kinds: Vec<String> = files
            .iter()
            .filter(|(rel, _)| rel.ends_with(".luau"))
            .filter_map(|(_, path)| std::fs::read_to_string(path).ok())
            .flat_map(|src| src.split("ctx:Add").skip(1).filter_map(|rest| rest.split_once('(').map(|(name, _)| name.to_owned())).collect::<Vec<_>>())
            .filter(|name| !name.is_empty() && name.chars().all(char::is_alphanumeric))
            .collect();
        kinds.sort();
        kinds.dedup();
        writeln!(contributions, "    ({id:?}, &{kinds:?}),").ok();
        writeln!(precompiled, "    ({id:?}, &[").ok();
        for (index, (rel, path)) in files.into_iter().enumerate() {
            println!("cargo:rerun-if-changed={}", path.display());
            if rel.ends_with(".luau") && !rel.ends_with(".config.luau") {
                let source = match std::fs::read_to_string(&path) {
                    Ok(s) => s,
                    Err(e) => panic!("read built-in Luau module {}: {e}", path.display()),
                };
                // Keep source on compile failure: the plugin console should show the original syntax error.
                if let Ok(compiled) = compiler.compile(&source) {
                    let native = source.lines().map(str::trim).take_while(|l| l.is_empty() || l.starts_with("--")).any(|l| l.starts_with("--!native"));
                    let file = bytecode.join(format!("{id}-{index}.luauc"));
                    if let Err(e) = std::fs::write(&file, compiled) {
                        panic!("write built-in bytecode {}: {e}", file.display());
                    }
                    writeln!(precompiled, "        ({rel:?}, include_bytes!({:?}), {native}),", file.display().to_string()).ok();
                    continue;
                }
            }
            let name = format!("FILE_{}", consts.lines().count());
            writeln!(consts, "const {name}: &str = include_str!({:?});", path.display().to_string()).ok();
            writeln!(code, "        ({rel:?}, {name}),").ok();
        }
        precompiled.push_str("    ]),\n");
        writeln!(code, "    ], include_bytes!({:?})),", catalog.display().to_string()).ok();
    }
    code.push_str("];\n");
    precompiled.push_str("];\n");
    contributions.push_str("];\n");
    hashes.push_str("];\n");
    code = consts + &code + &precompiled + &contributions + &hashes;
    let luaurc = root.join(".luaurc");
    println!("cargo:rerun-if-changed={}", luaurc.display());
    writeln!(code, "pub static LUAURC: &str = include_str!({:?});", luaurc.display().to_string()).ok();
    std::fs::write(out.join("luau_builtins.rs"), code).ok();

    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
        for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if path.is_dir() {
                if !name.starts_with('.') {
                    walk(root, &path, out);
                }
                continue;
            }
            let under = |dir: &str, ext: &str| name.ends_with(ext) && path.parent().is_some_and(|p| p.ends_with(dir));
            let keep = name.ends_with(".luau") || name == "plugin.toml" || name == ".luaurc" || under("locales", ".json") || under("locales/fill", ".json") || under("icons", ".svg");
            if let (true, Ok(rel)) = (keep, path.strip_prefix(root)) {
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.push((rel.join("/"), path));
            }
        }
    }
}

/// A plugin's catalogs (`locales/<lang>.json` over `locales/fill/<lang>.json`, flat objects of strings) as one table of
/// little-endian `u32`s and UTF-8, the layout `plugin_host::catalogs::Table` reads: `[langs, keys]`,
/// then per language its name (offset, length) and the offset of its entries, then every key
/// (offset, length) sorted, then per language one (offset, length) per key (`u32::MAX` length when
/// it lacks the key), then the strings.
fn catalog_table(locales: &[(String, std::path::PathBuf)]) -> Vec<u8> {
    let read = |path: &std::path::Path| -> Option<std::collections::BTreeMap<String, String>> {
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        Some(json.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
    };
    // `fill/<lang>` holds machine translations of the keys the plugin's own `<lang>` lacks; the
    // plugin's own file is laid over it, so a translation the catalog gains wins.
    let mut merged: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> = Default::default();
    for (name, path) in locales.iter().filter(|(n, _)| n.starts_with("fill/")).chain(locales.iter().filter(|(n, _)| !n.starts_with("fill/"))) {
        if let Some(strings) = read(path) {
            merged.entry(name.strip_prefix("fill/").unwrap_or(name).to_owned()).or_default().extend(strings);
        }
    }
    let catalogs: Vec<(String, std::collections::BTreeMap<String, String>)> = merged.into_iter().collect();
    let keys: std::collections::BTreeSet<&String> = catalogs.iter().flat_map(|(_, c)| c.keys()).collect();
    let (nl, nk) = (catalogs.len(), keys.len());
    let head = 2 + nl * 3 + nk * 2 + nl * nk * 2;
    let mut words: Vec<u32> = vec![nl as u32, nk as u32];
    let mut strings: Vec<u8> = Vec::new();
    let put = |s: &str, strings: &mut Vec<u8>| {
        let at = (head * 4 + strings.len()) as u32;
        strings.extend_from_slice(s.as_bytes());
        [at, s.len() as u32]
    };
    let mut lang_words = Vec::new();
    for (i, (lang, _)) in catalogs.iter().enumerate() {
        lang_words.extend(put(lang, &mut strings));
        lang_words.push(((2 + nl * 3 + nk * 2 + i * nk * 2) * 4) as u32);
    }
    words.extend(lang_words);
    for key in &keys {
        words.extend(put(key, &mut strings));
    }
    for (_, catalog) in &catalogs {
        for key in &keys {
            match catalog.get(*key) {
                Some(text) => words.extend(put(text, &mut strings)),
                None => words.extend([0, u32::MAX]),
            }
        }
    }
    words.iter().flat_map(|w| w.to_le_bytes()).chain(strings).collect()
}

/// The build's commit (`OPENCODE_RUNE_COMMIT`); without git, nothing is embedded.
fn embed_build_commit(manifest_dir: &std::path::Path) {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_COMMIT");
    #[allow(clippy::disallowed_methods)]
    let env_sha = std::env::var("OPENCODE_RUNE_COMMIT").ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(manifest_dir).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = std::path::Path::new(dir.trim());
        println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
        println!("cargo:rerun-if-changed={}", dir.join("logs").join("HEAD").display());
    }
    if let Some(sha) = env_sha.or_else(|| git(&["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())) {
        println!("cargo:rustc-env=OPENCODE_RUNE_COMMIT={sha}");
        println!("cargo:rustc-env=OPENCODE_RUNE_BUILD_STAMP={{\"sha\":\"{sha}\"}}");
    }
}
