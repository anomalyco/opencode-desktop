//! Where plugins come from, and their files.
//!
//! | Tier | Where | Trust |
//! | --- | --- | --- |
//! | Dev | `--plugin-dir <path>`: a plugin folder, or a folder of them | Trusted |
//! | Override | `<config>/plugins-override/<id>/` | Asks once |
//! | Bundled | Built into the binary (`build.rs`); a debug build reads the repository's `plugins/` | Trusted |
//! | User | `<data>/plugins/<id>/` | Asks once; can't take a built-in's id |
//!
//! A release build reads no folder for its built-ins: the install folder holds the app alone. To
//! change a built-in, copy it into `plugins-override/`. A debug build's repository copy wins over
//! the embedded one, so saving a built-in's `.luau` file reloads it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// A folder, with its files as they were read: what runs, and what a trust OK was for.
    Dir { dir: PathBuf, files: Arc<[(String, String)]> },
    /// Its files (no `locales/`) and its catalogs' table.
    Embedded(&'static [(&'static str, &'static str)], &'static [u8]),
}

/// A plugin folder, or why it doesn't read as one.
pub struct Found {
    pub manifest: Manifest,
    pub origin: Origin,
    pub tier: Tier,
    pub error: Option<String>,
    /// The SHA-256 of its files, when it asks for trust.
    pub hash: Option<String>,
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

/// The built-ins: the copies built into the binary, after (in a debug build) the repository's
/// folders, so saving one reloads it. A release build reads no file here.
pub fn builtins() -> Vec<Found> {
    let embedded = embedded::BUILTINS.iter().filter_map(|(_, files, catalogs)| read_embedded(files, catalogs));
    #[cfg(debug_assertions)]
    let repo = scan(Tier::Bundled, Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins")));
    #[cfg(not(debug_assertions))]
    let repo = Vec::new();
    repo.into_iter().chain(embedded).collect()
}

/// The plugins in folders, highest tier first: `--plugin-dir`, then the overrides, then the
/// installed ones; each that asks for trust with its files' SHA-256. Reads the disk, so it runs
/// off the app's thread.
pub fn folders() -> Vec<Found> {
    let mut tiers = Vec::new();
    if let Some(dir) = crate::arg("--plugin-dir") {
        tiers.push((Tier::Dev, PathBuf::from(dir)));
    }
    if let Some(config) = crate::dirs::config() {
        tiers.push((Tier::Override, config.join("plugins-override")));
    }
    if let Some(data) = crate::dirs::data() {
        tiers.push((Tier::User, data.join("plugins")));
    }
    let mut found: Vec<Found> = tiers.iter().flat_map(|(tier, dir)| scan(*tier, dir)).collect();
    for f in found.iter_mut().filter(|f| f.tier.needs_trust()) {
        f.hash = Some(content_hash(&f.origin.files()));
    }
    found
}
/// What built-in `id` contributes (`Slot`, `Panel`, `ComposerPill`, ...: its `ctx:Add…` calls, read at
/// build time); empty for a plugin that isn't built in.
pub fn contributions(id: &str) -> &'static [&'static str] {
    embedded::CONTRIBUTIONS.iter().find(|(p, _)| *p == id).map_or(&[], |(_, kinds)| kinds)
}

/// What a built-in's record (`dormant`) is keyed by: the hash of the files this build carries for
/// it, or (a debug build, which runs them from the repository) of the files as read. None for a
/// plugin that isn't built in: those start as they always have.
pub fn files_hash(id: &str, origin: &Origin, tier: Tier) -> Option<String> {
    let base = match origin {
        Origin::Embedded(..) => embedded::HASHES.iter().find(|(p, _)| *p == id).map(|(_, h)| (*h).to_owned()),
        Origin::Dir { files, .. } if tier == Tier::Bundled => Some(content_hash(files)),
        Origin::Dir { .. } => None,
    }?;
    // The browser plugin registers its MCP tools only on builds with an engine, so an update that
    // adds or removes the engine source invalidates its dormant contribution record.
    if id == "browser" && !browser_pane::BrowserHost::engine_configured() {
        return Some(format!("{base}:no-engine"));
    }
    Some(base)
}

/// The ids of the built-ins this binary carries.
pub fn builtin_ids() -> impl Iterator<Item = &'static str> {
    embedded::BUILTINS.iter().map(|(id, _, _)| *id)
}

fn read_embedded(files: &'static [(&'static str, &'static str)], catalogs: &'static [u8]) -> Option<Found> {
    let toml = files.iter().find(|(p, _)| *p == "plugin.toml")?.1;
    Some(match parse(toml) {
        Ok(manifest) => Found { manifest, origin: Origin::Embedded(files, catalogs), tier: Tier::Bundled, error: None, hash: None },
        Err(e) => Found { manifest: placeholder("embedded"), origin: Origin::Embedded(files, catalogs), tier: Tier::Bundled, error: Some(format!("plugin.toml: {e}")), hash: None },
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
    let files = read_files(dir);
    let src = files.iter().find(|(p, _)| p == "plugin.toml").map(|(_, t)| t.clone())?;
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let origin = Origin::Dir { dir: dir.to_owned(), files: files.into() };
    Some(match parse(&src) {
        Ok(manifest) => Found { manifest, origin, tier, error: None, hash: None },
        Err(e) => Found { manifest: placeholder(&folder), origin, tier, error: Some(format!("plugin.toml: {e}")), hash: None },
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
    /// Only an embedded origin can contribute bytecode. A folder with the same id always
    /// supplies text; it cannot impersonate the built-in or send binary chunks to the VM.
    pub fn scripts(&self, files: &[(String, String)], folder: &str, entry: &str) -> rune_luau::Sources {
        let mut sources = script_sources(files, folder, entry);
        if let Self::Embedded(..) = self
            && let Some((_, modules)) = embedded::MODULES.iter().find(|(id, _)| *id == folder)
        {
            for &(path, code, native) in *modules {
                // SAFETY: private static table generated by build.rs with this VM's compiler.
                #[allow(unsafe_code)]
                { sources = unsafe { sources.with_embedded(path, code, native) }; }
            }
        }
        sources
    }

    /// Every file, by its path relative to the plugin folder: a folder's as they were read, never
    /// read again.
    pub fn files(&self) -> Vec<(String, String)> {
        match self {
            Origin::Embedded(files, _) => files.iter().map(|(p, s)| ((*p).to_owned(), (*s).to_owned())).collect(),
            Origin::Dir { files, .. } => files.to_vec(),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        match self {
            Origin::Dir { dir, .. } => Some(dir),
            Origin::Embedded(..) => None,
        }
    }

    /// Its message catalogs: a built-in's table as it lies in the binary; a folder's from its
    /// `files`, each language parsed the first time a string is read in it.
    pub fn catalogs(&self, files: &[(String, String)]) -> super::Catalogs {
        match self {
            Origin::Embedded(_, table) => super::Catalogs::Table(table),
            Origin::Dir { .. } => super::Catalogs::folder(files),
        }
    }

    /// Where the plugin is, for people: its folder, or "built in".
    pub fn describe(&self) -> String {
        self.dir().map_or_else(|| "Built into the app".to_owned(), |d| d.display().to_string())
    }
}

/// Plugin folder dir's files, by their path relative to it, sorted. Reads the disk: never on the
/// app's thread.
pub fn read_files(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
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
/// `locales/*.json` catalogs (and `locales/fill/*.json`, see [`Catalogs`]) and `icons/*.svg` icons,
/// outside hidden folders.
pub fn kept(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    let Some(name) = parts.pop() else { return false };
    if parts.iter().any(|d| d.starts_with('.')) {
        return false;
    }
    let under = |dir: &str, ext: &str| parts.last() == Some(&dir) && name.ends_with(ext);
    name.ends_with(".luau") || name == "plugin.toml" || name == ".luaurc" || under("locales", ".json") || (parts.ends_with(&["locales", "fill"]) && name.ends_with(".json")) || under("icons", ".svg")
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
