fn main() {
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_WINDOW_TITLE");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_VERSION");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_BUNDLE_ID");
    println!("cargo:rerun-if-env-changed=OPENCODE_RUNE_URL_SCHEME");
    #[allow(clippy::disallowed_methods)]
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    embed_luau_plugins(std::path::Path::new(&env("CARGO_MANIFEST_DIR").unwrap_or_default()), std::path::Path::new(&env("OUT_DIR").unwrap_or_default()));
    embed_beta_notes(std::path::Path::new(&env("CARGO_MANIFEST_DIR").unwrap_or_default()), std::path::Path::new(&env("OUT_DIR").unwrap_or_default()), env("OPENCODE_RUNE_BUILD_CHANNEL").as_deref().unwrap_or("local"));
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
            let under = |dir: &str, ext: &str| name.ends_with(ext) && path.parent().is_some_and(|p| p.ends_with(dir));
            let keep = name.ends_with(".luau") || name == "plugin.toml" || name == ".luaurc" || under("locales", ".json") || under("icons", ".svg");
            if let (true, Ok(rel)) = (keep, path.strip_prefix(root)) {
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.push((rel.join("/"), path));
            }
        }
    }
}

/// The build's commit (`OPENCODE_RUNE_COMMIT`), and in local and beta builds the changes it
/// carries (`beta_notes.json` in `OUT_DIR`, empty in other builds), as Desktop's `script/beta-notes.ts` embeds them: the
/// commit, the short shas of its newest 200 commits, and the notes of the feature and fix
/// commits among them, newest first, for the Updater plugin's What's new. A commit reads as a
/// note when its subject is conventional (`feat(scope): …`, `fix: …`, `perf: …`) or this repo's
/// own (`OpenCode (Rune): …` is a change, `Bump Rune to <sha>: …` a fix); a long subject keeps
/// its first sentence. Without git, nothing is embedded.
fn embed_beta_notes(manifest_dir: &std::path::Path, out: &std::path::Path, channel: &str) {
    std::fs::write(out.join("beta_notes.json"), "").ok();
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(manifest_dir).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = std::path::Path::new(dir.trim());
        println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
        println!("cargo:rerun-if-changed={}", dir.join("logs").join("HEAD").display());
    }
    let Some(sha) = git(&["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()) else { return };
    println!("cargo:rustc-env=OPENCODE_RUNE_COMMIT={sha}");
    if channel != "local" && channel != "beta" {
        return;
    }
    let Some(log) = git(&["log", "--topo-order", "--max-count=200", "--format=%H%x1f%P%x1f%an%x1f%s%x1e", &sha]) else { return };
    let quote = |s: &str| {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    };
    let (mut commits, mut notes) = (Vec::new(), Vec::new());
    for record in log.split('\u{1e}') {
        let fields: Vec<&str> = record.trim().split('\u{1f}').collect();
        let [commit, parents, author, subject] = fields[..] else { continue };
        let short = &commit[..commit.len().min(12)];
        commits.push(quote(short));
        if parents.split_whitespace().count() > 1 {
            continue;
        }
        let Some((kind, summary, pr)) = parse_subject(subject) else { continue };
        let mut note = format!("{{\"type\":{},\"summary\":{},\"sha\":{}", quote(kind), quote(&summary), quote(short));
        if let Some(pr) = pr {
            note.push_str(&format!(",\"pr\":{pr}"));
        }
        if !author.trim().is_empty() {
            note.push_str(&format!(",\"author\":{{\"name\":{}}}", quote(author.trim())));
        }
        note.push('}');
        notes.push(note);
    }
    let json = format!("{{\"sha\":{},\"commits\":[{}],\"notes\":[{}]}}", quote(&sha), commits.join(","), notes.join(","));
    std::fs::write(out.join("beta_notes.json"), json).ok();
}

/// A subject's note: its kind (`feat`, `fix`, `perf`), summary and pull request.
fn parse_subject(subject: &str) -> Option<(&'static str, String, Option<u64>)> {
    let subject = subject.trim();
    let (kind, rest) = if let Some(rest) = subject.strip_prefix("OpenCode (Rune):") {
        ("feat", rest)
    } else if let Some(rest) = subject.strip_prefix("Bump Rune to ").and_then(|r| r.split_once(':')).map(|(_, r)| r) {
        ("fix", rest)
    } else {
        let (head, rest) = subject.split_once(':')?;
        let ty = head.split(['(', '!']).next()?.trim().to_ascii_lowercase();
        let kind = match ty.as_str() {
            "feat" => "feat",
            "fix" => "fix",
            "perf" => "perf",
            _ => return None,
        };
        (kind, rest)
    };
    let mut rest = rest.trim();
    let mut pr = None;
    if let Some(open) = rest.rfind("(#")
        && rest.ends_with(')')
    {
        pr = rest[open + 2..rest.len() - 1].parse().ok();
        rest = rest[..open].trim_end();
    }
    // The first sentence: up to a full stop, or a colon that opens a list.
    let cut = rest.char_indices().find(|&(i, c)| (c == '.' || c == ':' || c == ';') && rest[i + 1..].starts_with(' ')).map_or(rest.len(), |(i, _)| i);
    let mut summary: String = rest[..cut].trim().to_owned();
    if summary.chars().count() > 140 {
        let cut: String = summary.chars().take(140).collect();
        summary = format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head).trim_end_matches([',', ' ']));
    }
    (!summary.is_empty()).then_some((kind, summary, pr))
}