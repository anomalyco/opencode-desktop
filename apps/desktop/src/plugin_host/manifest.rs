//! `plugin.toml` and discovery: what's installed is known without loading any library, so a
//! disabled plugin costs nothing at launch.
//!
//! | Tier | Where | Role |
//! | --- | --- | --- |
//! | Dev | `--plugin-dir <path>` | Highest; a folder with `plugin.toml`, or folders of them. |
//! | Override | `<config>/plugins-override/<id>/` | Replaces a built-in without touching the app. |
//! | Bundled | `<exe dir>/plugins/<id>/` (and `--features static-builtins`) | The built-ins. |
//! | User | `<data>/plugins/<id>/` | Third-party; can't take a built-in's id. |

use std::path::{Path, PathBuf};

use opencode_plugin::abi::API_VERSION;

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub api: String,
    /// `windows`, `macos`, `linux`; empty for every desktop OS.
    pub os: Vec<String>,
    /// `opencode_plugin_review` → `opencode_plugin_review.dll`, `libopencode_plugin_review.dylib`, `.so`.
    pub library: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Dev,
    Override,
    Bundled,
    User,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dev => "Dev",
            Self::Override => "Override",
            Self::Bundled => "Built-in",
            Self::User => "Installed",
        }
    }
}

/// A plugin folder, or the error that kept it from reading as one.
pub struct Found {
    pub manifest: Manifest,
    pub dir: PathBuf,
    pub tier: Tier,
    pub error: Option<String>,
}

pub fn parse(src: &str) -> Result<Manifest, String> {
    let doc: toml_edit::DocumentMut = src.parse().map_err(|e: toml_edit::TomlError| e.message().to_owned())?;
    let s = |key: &str| doc.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_owned();
    let m = Manifest {
        id: s("id"),
        name: s("name"),
        version: s("version"),
        description: s("description"),
        api: s("api"),
        os: doc.get("os").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
        library: s("library"),
    };
    if m.id.is_empty() || m.id.contains([':', '/', '\\']) {
        return Err(format!("`id` must be set and contain no `:`, `/` or `\\` (got {:?})", m.id));
    }
    if m.api != API_VERSION {
        return Err(format!("targets plugin API {:?}; this app speaks {API_VERSION:?}", m.api));
    }
    if m.library.is_empty() {
        return Err("`library` must name the plugin's library".to_owned());
    }
    Ok(Manifest { name: if m.name.is_empty() { m.id.clone() } else { m.name.clone() }, ..m })
}

impl Manifest {
    pub fn runs_here(&self) -> bool {
        let here = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        self.os.is_empty() || self.os.iter().any(|o| o == here)
    }

    /// The library's path in `dir`.
    pub fn library_path(&self, dir: &Path) -> PathBuf {
        let file = if cfg!(target_os = "windows") {
            format!("{}.dll", self.library)
        } else if cfg!(target_os = "macos") {
            format!("lib{}.dylib", self.library)
        } else {
            format!("lib{}.so", self.library)
        };
        dir.join(file)
    }
}

/// Every plugin folder in the tiers, highest tier first.
pub fn scan(tiers: &[(Tier, PathBuf)]) -> Vec<Found> {
    tiers.iter().flat_map(|(tier, dir)| scan_tier(*tier, dir)).collect()
}

fn scan_tier(tier: Tier, dir: &Path) -> Vec<Found> {
    if tier == Tier::Dev && dir.join("plugin.toml").is_file() {
        return vec![read(tier, dir)];
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.join("plugin.toml").is_file()).collect();
    dirs.sort();
    dirs.iter().map(|d| read(tier, d)).collect()
}

fn read(tier: Tier, dir: &Path) -> Found {
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match std::fs::read_to_string(dir.join("plugin.toml")).map_err(|e| e.to_string()).and_then(|src| parse(&src)) {
        Ok(manifest) => Found { manifest, dir: dir.to_owned(), tier, error: None },
        Err(e) => Found {
            manifest: Manifest { id: folder.clone(), name: folder, ..Manifest::default() },
            dir: dir.to_owned(),
            tier,
            error: Some(format!("plugin.toml: {e}")),
        },
    }
}
