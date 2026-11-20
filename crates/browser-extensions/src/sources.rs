//! Read-only discovery of installed Chromium browser profiles and their extensions for explicit
//! user import ("Add an extension -> From your browsers").
//!
//! Mirrors `packages/gui-plugins/src/browser/profile/sources.ts` (`chromiumProfileDirectory`,
//! `profileExtensions`). Never reads Windows Registry and never imports without user selection.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::crx::is_valid_ext_id;
use crate::manifest::{
    compare_versions, display_name, extension_access, extension_compatibility,
    extension_icon_data_url, read_manifest,
};
use crate::{BrowserExtensionCandidate, ExtensionSource, StoredExtension};

/// Injected host OS directories (avoids calling `std::env::var` inside library code).
#[derive(Clone, Debug, Default)]
pub struct PlatformDirs {
    pub home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
}

/// An installed Chromium browser and its discovered profile directories.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromiumBrowserSource {
    pub id: String,
    pub name: String,
    pub profiles: Vec<ChromiumProfileEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromiumProfileEntry {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug)]
struct BrowserDef {
    id: &'static str,
    name: &'static str,
    root: PathBuf,
    flat: bool,
}

/// Lists installed Chromium browsers on this machine that have at least one profile directory
/// containing an `Extensions/` folder.
pub fn list_chromium_sources(dirs: &PlatformDirs) -> Vec<ChromiumBrowserSource> {
    let mut out = Vec::new();
    for def in browser_definitions(dirs) {
        let profiles = locate_profiles(&def);
        if !profiles.is_empty() {
            out.push(ChromiumBrowserSource {
                id: def.id.to_owned(),
                name: def.name.to_owned(),
                profiles: profiles
                    .into_iter()
                    .map(|(id, name, _)| ChromiumProfileEntry { id, name })
                    .collect(),
            });
        }
    }
    out
}

/// Resolves a `(browser_id, profile_id)` pair to its verified profile directory on disk
/// (`sources.ts:301-306`). Rejects any unknown ID or path traversal.
pub fn chromium_profile_directory(dirs: &PlatformDirs, browser_id: &str, profile_id: &str) -> Option<PathBuf> {
    let def = browser_definitions(dirs).into_iter().find(|d| d.id == browser_id)?;
    locate_profiles(&def)
        .into_iter()
        .find(|(id, _, _)| id == profile_id)
        .map(|(_, _, path)| path)
}

/// Scans `<profile_dir>/Extensions/<id>/<newest_version>` (`sources.ts:639-655`), returning valid
/// non-theme extension directories with their newest version folder.
pub fn profile_extension_dirs(profile_dir: &Path) -> Vec<(String, PathBuf)> {
    let ext_root = profile_dir.join("Extensions");
    let Ok(root_meta) = std::fs::symlink_metadata(&ext_root) else {
        return Vec::new();
    };
    if !root_meta.is_dir() || root_meta.file_type().is_symlink() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&ext_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let Ok(id) = entry.file_name().into_string() else { continue };
        if !is_valid_ext_id(&id) {
            continue;
        }
        let id_dir = entry.path();
        let Ok(ver_entries) = std::fs::read_dir(&id_dir) else { continue };
        let mut versions: Vec<(String, PathBuf)> = ver_entries
            .flatten()
            .filter_map(|ve| {
                let vft = ve.file_type().ok()?;
                if !vft.is_dir() || vft.is_symlink() {
                    return None;
                }
                let name = ve.file_name().into_string().ok()?;
                Some((name, ve.path()))
            })
            .collect();
        versions.sort_by(|(a, _), (b, _)| {
            let ord = compare_versions(a, b);
            if ord == Ordering::Equal { a.cmp(b) } else { ord }
        });
        if let Some((_, latest_dir)) = versions.pop() {
            out.push((id, latest_dir));
        }
    }
    out
}

/// Lists importable extensions in `(browser_id, profile_id)` sorted by display name
/// (`profile/index.ts:525-550`), omitting themes and marking already-imported items.
pub fn list_browser_extensions(
    dirs: &PlatformDirs,
    browser_id: &str,
    profile_id: &str,
    stored: &[StoredExtension],
    changed_ids: &HashSet<String>,
) -> Vec<BrowserExtensionCandidate> {
    let Some(profile_dir) = chromium_profile_directory(dirs, browser_id, profile_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (id, version_dir) in profile_extension_dirs(&profile_dir) {
        let Some(manifest) = read_manifest(&version_dir) else {
            continue;
        };
        if manifest.is_theme {
            continue;
        }
        let origin_str = version_dir.parent().map(|p| p.to_string_lossy().into_owned());
        let installed = stored.iter().any(|rec| {
            !changed_ids.contains(&rec.id)
                && rec.source == ExtensionSource::Browser
                && rec.origin.is_some()
                && rec.origin == origin_str
        });
        out.push(BrowserExtensionCandidate {
            id,
            name: display_name(&version_dir, &manifest),
            version: manifest.version.clone(),
            icon: extension_icon_data_url(&version_dir, &manifest),
            installed,
            compatibility: extension_compatibility(&manifest),
            access: extension_access(&manifest),
        });
    }
    out.sort_by_key(|a| a.name.to_ascii_lowercase());
    out
}

fn locate_profiles(def: &BrowserDef) -> Vec<(String, String, PathBuf)> {
    if def.flat {
        return if def.root.join("Extensions").is_dir() {
            vec![("default".into(), "Default".into(), def.root.clone())]
        } else {
            Vec::new()
        };
    }
    let names = local_state_profiles(&def.root);
    let mut found = Vec::new();
    for (id, name) in names {
        let dir = def.root.join(&id);
        if let Ok(meta) = std::fs::symlink_metadata(&dir)
            && meta.is_dir()
            && !meta.file_type().is_symlink()
            && dir.join("Extensions").is_dir()
        {
            found.push((id, name, dir));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn local_state_profiles(root: &Path) -> Vec<(String, String)> {
    let mut names: HashMap<String, String> = HashMap::new();
    let state_path = root.join("Local State");
    if let Ok(meta) = std::fs::symlink_metadata(&state_path)
        && meta.is_file()
        && !meta.file_type().is_symlink()
        && meta.len() <= 8 * 1024 * 1024
        && let Ok(text) = std::fs::read_to_string(&state_path)
        && let Ok(val) = serde_json::from_str::<Value>(&text)
        && let Some(cache) = val.pointer("/profile/info_cache").and_then(Value::as_object)
    {
        for (id, info) in cache {
            if is_plain_dir_name(id) {
                let display = info
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(id);
                names.insert(id.clone(), display.to_owned());
            }
        }
    }
    names.entry("Default".into()).or_insert_with(|| "Default".into());
    names.into_iter().collect()
}

fn is_plain_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains(':')
}

fn browser_definitions(dirs: &PlatformDirs) -> Vec<BrowserDef> {
    let mut defs = Vec::new();
    #[cfg(windows)]
    {
        let local = dirs
            .local_app_data
            .clone()
            .or_else(|| dirs.home.as_ref().map(|h| h.join("AppData").join("Local")));
        let roaming = dirs
            .app_data
            .clone()
            .or_else(|| dirs.home.as_ref().map(|h| h.join("AppData").join("Roaming")));
        if let Some(l) = local {
            defs.push(BrowserDef { id: "chrome", name: "Google Chrome", root: l.join("Google/Chrome/User Data"), flat: false });
            defs.push(BrowserDef { id: "edge", name: "Microsoft Edge", root: l.join("Microsoft/Edge/User Data"), flat: false });
            defs.push(BrowserDef { id: "helium", name: "Helium", root: l.join("imput/Helium/User Data"), flat: false });
            defs.push(BrowserDef { id: "brave", name: "Brave", root: l.join("BraveSoftware/Brave-Browser/User Data"), flat: false });
            defs.push(BrowserDef {
                id: "arc",
                name: "Arc",
                root: l.join("Packages/TheBrowserCompany.Arc_ttt1ap7aakyb4/LocalCache/Local/Arc/User Data"),
                flat: false,
            });
            defs.push(BrowserDef { id: "vivaldi", name: "Vivaldi", root: l.join("Vivaldi/User Data"), flat: false });
            defs.push(BrowserDef { id: "chromium", name: "Chromium", root: l.join("Chromium/User Data"), flat: false });
        }
        if let Some(r) = roaming {
            defs.push(BrowserDef { id: "opera", name: "Opera", root: r.join("Opera Software/Opera Stable"), flat: true });
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = &dirs.home {
            let s = home.join("Library/Application Support");
            defs.push(BrowserDef { id: "chrome", name: "Google Chrome", root: s.join("Google/Chrome"), flat: false });
            defs.push(BrowserDef { id: "edge", name: "Microsoft Edge", root: s.join("Microsoft Edge"), flat: false });
            defs.push(BrowserDef { id: "helium", name: "Helium", root: s.join("net.imput.helium"), flat: false });
            defs.push(BrowserDef { id: "arc", name: "Arc", root: s.join("Arc/User Data"), flat: false });
            defs.push(BrowserDef { id: "brave", name: "Brave", root: s.join("BraveSoftware/Brave-Browser"), flat: false });
            defs.push(BrowserDef { id: "dia", name: "Dia", root: s.join("Dia/User Data"), flat: false });
            defs.push(BrowserDef { id: "comet", name: "Comet", root: s.join("Comet"), flat: false });
            defs.push(BrowserDef { id: "vivaldi", name: "Vivaldi", root: s.join("Vivaldi"), flat: false });
            defs.push(BrowserDef { id: "opera", name: "Opera", root: s.join("com.operasoftware.Opera"), flat: true });
            defs.push(BrowserDef { id: "chromium", name: "Chromium", root: s.join("Chromium"), flat: false });
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let cfg = dirs
            .xdg_config_home
            .clone()
            .or_else(|| dirs.home.as_ref().map(|h| h.join(".config")));
        if let Some(c) = cfg {
            defs.push(BrowserDef { id: "chrome", name: "Google Chrome", root: c.join("google-chrome"), flat: false });
            defs.push(BrowserDef { id: "edge", name: "Microsoft Edge", root: c.join("microsoft-edge"), flat: false });
            defs.push(BrowserDef { id: "helium", name: "Helium", root: c.join("net.imput.helium"), flat: false });
            defs.push(BrowserDef { id: "brave", name: "Brave", root: c.join("BraveSoftware/Brave-Browser"), flat: false });
            defs.push(BrowserDef { id: "vivaldi", name: "Vivaldi", root: c.join("vivaldi"), flat: false });
            defs.push(BrowserDef { id: "opera", name: "Opera", root: c.join("opera"), flat: true });
            defs.push(BrowserDef { id: "chromium", name: "Chromium", root: c.join("chromium"), flat: false });
        }
    }
    defs
}
