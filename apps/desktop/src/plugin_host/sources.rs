//! Where Luau plugins come from, and their files.
//!
//! | Tier | Where | Trust |
//! | --- | --- | --- |
//! | Dev | `--plugin-dir <path>`: a plugin folder, or a folder of them | Trusted |
//! | Override | `<config>/plugins-override/<id>/` | Asks once |
//! | Bundled | `<exe dir>/plugins/<id>/`; in a debug build, the repository's `plugins/` | Trusted |
//! | Embedded | Built into the binary (`build.rs`): a built-in no folder has | Trusted |
//! | User | `<data>/plugins/<id>/` | Asks once; can't take a built-in's id |
//!
//! Files on disk win over the embedded copy, so a built-in's `.luau` files can be
//! read and edited where they're installed, and save-to-reload works on them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use opencode_plugin_spec::{Compat, ContractVersion, Manifest};
use sha2::{Digest, Sha256};

/// Where a plugin comes from, highest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Dev,
    Override,
    Bundled,
    User,
}

impl Tier {
    /// Whether it waits for the user's OK before it first runs: anything that isn't shipped with
    /// the app or named on the command line.
    pub fn needs_trust(self) -> bool {
        matches!(self, Self::Override | Self::User)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Dev => "Dev",
            Self::Override => "Override",
            Self::Bundled => "Built-in",
            Self::User => "Installed",
        }
    }
}

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/luau_builtins.rs"));
}

/// A plugin's files: a folder, or the copy built into the binary.
#[derive(Clone, Debug)]
pub enum Origin {
    Dir(PathBuf),
    Embedded(&'static [(&'static str, &'static str)]),
}

/// A plugin folder, or why it doesn't read as one.
pub struct Found {
    pub manifest: Manifest,
    pub origin: Origin,
    pub tier: Tier,
    pub error: Option<String>,
}

/// Reads and checks a manifest against the contract this host implements.
pub fn parse(src: &str) -> Result<Manifest, String> {
    let m: Manifest = toml_edit::de::from_str(src).map_err(|e| e.message().to_owned())?;
    let id_ok = !m.id.is_empty() && m.id.len() <= 64 && m.id.starts_with(|c: char| c.is_ascii_lowercase()) && m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !id_ok {
        return Err(format!("`id` must be lowercase letters, digits and hyphens, starting with a letter (got {:?})", m.id));
    }
    let version = ContractVersion::parse(&m.contract).ok_or_else(|| format!("`contract` must be MAJOR.MINOR (got {:?})", m.contract))?;
    if ContractVersion::HOST.runs(version) == Compat::Unsupported {
        return Err(format!("targets contract {version}; this app implements {}", ContractVersion::HOST));
    }
    for path in std::iter::once(&m.entry).chain(&m.app) {
        if path.contains("..") || path.starts_with(['/', '\\']) {
            return Err(format!("{path:?} must be a path inside the plugin folder"));
        }
    }
    Ok(m)
}

/// Every Luau plugin in tier order, highest first.
pub fn discover() -> Vec<Found> {
    let embedded = || embedded::BUILTINS.iter().filter_map(|(_, files)| read_embedded(files));
    let mut first = Vec::new();
    if let Some(dir) = crate::arg("--plugin-dir") {
        first.push((Tier::Dev, PathBuf::from(dir)));
    }
    if let Some(config) = crate::dirs::config() {
        first.push((Tier::Override, config.join("plugins-override")));
    }
    if let Some(exe) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_owned)) {
        first.push((Tier::Bundled, exe.join("plugins")));
    }
    // A debug build runs its built-ins from the repository, so saving one reloads it.
    #[cfg(debug_assertions)]
    first.push((Tier::Bundled, PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins"))));
    let user = crate::dirs::data().map(|d| (Tier::User, d.join("plugins")));
    let scanned = first.iter().flat_map(|(tier, dir)| scan(*tier, dir));
    let users = user.iter().flat_map(|(tier, dir)| scan(*tier, dir));
    scanned.chain(embedded()).chain(users).collect()
}

/// The ids of the built-ins this binary carries.
pub fn builtin_ids() -> impl Iterator<Item = &'static str> {
    embedded::BUILTINS.iter().map(|(id, _)| *id)
}

fn read_embedded(files: &'static [(&'static str, &'static str)]) -> Option<Found> {
    let toml = files.iter().find(|(p, _)| *p == "plugin.toml")?.1;
    Some(match parse(toml) {
        Ok(manifest) => Found { manifest, origin: Origin::Embedded(files), tier: Tier::Bundled, error: None },
        Err(e) => Found { manifest: placeholder("embedded"), origin: Origin::Embedded(files), tier: Tier::Bundled, error: Some(format!("plugin.toml: {e}")) },
    })
}

fn scan(tier: Tier, dir: &Path) -> Vec<Found> {
    if tier == Tier::Dev && dir.join("plugin.toml").is_file() {
        return read(tier, dir).into_iter().collect();
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.join("plugin.toml").is_file()).collect();
    dirs.sort();
    dirs.iter().filter_map(|d| read(tier, d)).collect()
}

fn read(tier: Tier, dir: &Path) -> Option<Found> {
    let src = std::fs::read_to_string(dir.join("plugin.toml")).ok()?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let origin = Origin::Dir(dir.to_owned());
    Some(match parse(&src) {
        Ok(manifest) => Found { manifest, origin, tier, error: None },
        Err(e) => Found { manifest: placeholder(&folder), origin, tier, error: Some(format!("plugin.toml: {e}")) },
    })
}

fn placeholder(id: &str) -> Manifest {
    let toml = format!("id = {id:?}\nname = {id:?}\nversion = \"0.0.0\"\ncontract = \"{}\"\n", ContractVersion::HOST);
    toml_edit::de::from_str(&toml).unwrap_or_else(|_| unreachable_manifest())
}

#[allow(clippy::panic)]
fn unreachable_manifest() -> Manifest {
    panic!("a fixed manifest always parses")
}

impl Origin {
    /// Every file, by its path relative to the plugin folder.
    pub fn files(&self) -> Vec<(String, String)> {
        match self {
            Origin::Embedded(files) => files.iter().map(|(p, s)| ((*p).to_owned(), (*s).to_owned())).collect(),
            Origin::Dir(dir) => {
                let mut out = Vec::new();
                walk(dir, dir, &mut out);
                out.sort();
                out
            }
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        match self {
            Origin::Dir(d) => Some(d),
            Origin::Embedded(_) => None,
        }
    }

    /// Where the plugin is, for people: its folder, or "built in".
    pub fn describe(&self) -> String {
        self.dir().map_or_else(|| "Built into the app".to_owned(), |d| d.display().to_string())
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if path.is_dir() {
            walk(root, &path, out);
            continue;
        }
        if let Some(rel) = relative(root, &path)
            && kept(&rel)
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            out.push((rel, text));
        }
    }
}

/// `path` under plugin folder `root`, `/`-separated.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel: Vec<String> = path.strip_prefix(root).ok()?.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(rel.join("/"))
}

/// Whether a plugin runs from the file at `rel`: its manifest, Luau modules, `.luaurc`,
/// `locales/*.json` catalogs and `icons/*.svg` icons, outside hidden folders.
pub fn kept(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    let Some(name) = parts.pop() else { return false };
    if parts.iter().any(|d| d.starts_with('.')) {
        return false;
    }
    let under = |dir: &str, ext: &str| parts.last() == Some(&dir) && name.ends_with(ext);
    name.ends_with(".luau") || name == "plugin.toml" || name == ".luaurc" || under("locales", ".json") || under("icons", ".svg")
}

/// Interns plugin `plugin`'s `icons/<name>.svg` files as its icons, by file name.
pub fn intern_icons(plugin: &str, files: &[(String, String)]) {
    for (path, svg) in files {
        if let Some(name) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")) {
            crate::plugin_host::icons::intern_svg(plugin, name, svg);
        }
    }
}
/// The script sources of the half that starts at `entry`, with the plugins folder's `.luaurc`
/// above them as editors see it.
pub fn script_sources(files: &[(String, String)], folder: &str, entry: &str) -> rune_luau::Sources {
    let sources = files.iter().filter(|(p, _)| p.ends_with(".luau") || p.ends_with(".luaurc")).fold(rune_luau::Sources::new(), |s, (p, text)| s.with(p, text.as_str()));
    sources.parent_config(embedded::LUAURC).folder(folder).entry(entry)
}

/// The message catalogs in `locales/<id>.json`, by locale id: flat objects of strings.
pub fn catalogs(files: &[(String, String)]) -> BTreeMap<String, BTreeMap<String, String>> {
    files
        .iter()
        .filter_map(|(path, text)| {
            let lang = path.strip_prefix("locales/")?.strip_suffix(".json")?;
            let v: serde_json::Value = serde_json::from_str(text).ok()?;
            let map = v.as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
            Some((lang.to_owned(), map))
        })
        .collect()
}

/// The SHA-256 of every file the plugin runs from: what trusting it means.
pub fn content_hash(files: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    for (path, text) in files {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(text.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
