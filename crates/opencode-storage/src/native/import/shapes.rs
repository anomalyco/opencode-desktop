//! Every file shape Rune kept before the database, and the rows each becomes. Each mapping is
//! lossless: a file the rows can't give back exactly (checked by rebuilding it from them) is also
//! kept verbatim under `state("rune.legacy", <file>)`, as are the shared files whose keys several
//! derived rows mix (`tabs.json`, `ui.json`, `servers.json`, `home.json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use opencode_kit::servers::migration::{global, window};
use serde_json::{Map, Value, json};

use super::ImportConfig;
use crate::model::{OutboxRow, WindowFrame};

/// One row to import (`INSERT … ON CONFLICT DO NOTHING`: an existing row wins).
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    State { name: String, key: String, value: String },
    /// Decoded text; long strings become chunks as it's written.
    Doc { key: String, value: String },
    Window { id: String, frame: WindowFrame },
    Plugin { id: String, enabled: bool },
    Trust { id: String, sha256: String },
    Outbox(Box<OutboxRow>),
}

/// The files a set of rows came from: removable together, once every row verified.
#[derive(Debug, Default)]
pub struct Source {
    pub paths: Vec<PathBuf>,
    pub rows: Vec<Row>,
    /// Attachment bytes the rows reference, by id.
    pub blobs: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub sources: Vec<Source>,
    /// Files that couldn't be read or parsed: never imported, never removed.
    pub unreadable: Vec<PathBuf>,
}

impl Plan {
    pub fn rows(&self) -> usize {
        self.sources.iter().map(|s| s.rows.len()).sum()
    }
}

const SHARED: [&str; 6] = ["opencode.settings.dat", "tabs.json", "ui.json", "servers.json", "home.json", "opencode.global.dat"];

/// Plain files: `(file, namespace, key)`, the whole file as the value.
const PLAIN: [(&str, &str, &str); 7] = [
    ("model-prefs.json", "opencode.global.dat", "model"),
    ("settings-models.json", "opencode.global.dat", "settings-v2.models.providers"),
    ("review-panel-v2.json", "opencode.global.dat", "review-panel-v2"),
    ("workspace-onboarding.json", "opencode.global.dat", "workspace-onboarding"),
    ("file-wrap.json", "plugin.file", "rune.wrap"),
    ("browser-zoom.json", "plugin.browser", "rune.siteZoom"),
    ("storybook.json", "rune.dev", "storybook"),
];

/// The legacy files in `dir` (one directory read, plus `plugins-state\` and the updater's
/// markers when present). Empty means nothing to import.
pub fn scan(cfg: &ImportConfig) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(&cfg.dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file() && is_legacy_name(p)).collect();
    found.extend(std::fs::read_dir(cfg.dir.join("plugins-state")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")));
    if let Some(updates) = &cfg.updates_dir {
        found.extend(["ready", "rollout-bucket"].into_iter().map(|n| updates.join(n)).filter(|p| p.is_file()));
    }
    found.sort();
    found
}

/// Which group a legacy file belongs to.
pub fn group_of(cfg: &ImportConfig, p: &Path) -> Option<super::ImportGroup> {
    use super::ImportGroup as G;
    if p.parent().and_then(Path::file_name).is_some_and(|d| d == "plugins-state") {
        return Some(G::Plugins);
    }
    if cfg.updates_dir.as_deref().is_some_and(|u| p.parent() == Some(u)) {
        return Some(G::Updater);
    }
    let name = p.file_name()?.to_str()?;
    Some(match name {
        "plugins-state.json" => G::Plugins,
        "terminals.json" => G::Terminal,
        "browser-zoom.json" => G::Browser,
        n if n.starts_with("outbox-") => G::Outbox,
        _ if is_legacy_name(p) => G::App,
        _ => return None,
    })
}

fn is_legacy_name(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else { return false };
    SHARED.contains(&name)
        || PLAIN.iter().any(|(f, _, _)| *f == name)
        || matches!(name, "settings.json" | "prompt-history.json" | "open-in.json" | "plugins-state.json" | "terminals.json")
        || (name.starts_with("opencode.window.") && name.ends_with(".dat"))
        || (name.starts_with("window-state-") && name.ends_with(".json"))
        || (name.starts_with("outbox-") && name.ends_with(".json"))
}

struct Read {
    path: PathBuf,
    raw: String,
    value: Value,
}

/// Builds every row from `files`.
pub fn plan(cfg: &ImportConfig, files: &[PathBuf]) -> Plan {
    let mut plan = Plan::default();
    let mut read: BTreeMap<String, Read> = BTreeMap::new();
    let mut plugin_stores = Vec::new();
    let mut markers = Vec::new();
    for path in files {
        if path.parent().and_then(Path::file_name).is_some_and(|d| d == "plugins-state") {
            plugin_stores.push(path.clone());
            continue;
        }
        if cfg.updates_dir.as_deref().is_some_and(|u| path.parent() == Some(u)) {
            markers.push(path.clone());
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
        match std::fs::read_to_string(path).ok().and_then(|raw| serde_json::from_str::<Value>(&raw).ok().map(|value| (raw, value))) {
            Some((raw, value)) => {
                read.insert(name, Read { path: path.clone(), raw, value });
            }
            None => plan.unreadable.push(path.clone()),
        }
    }
    // The windows an older build's per-window outbox journal may be named after.
    let mut windows: Vec<String> = read.keys().filter_map(|n| n.strip_prefix("opencode.window.")?.strip_suffix(".dat").map(str::to_owned)).collect();
    if let Some(s) = read.get(SHARED[0]) {
        windows.extend(s.value["windowIds"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)));
    }
    let shared = shared(cfg, &mut read);
    plan.sources.push(shared);
    for (name, r) in std::mem::take(&mut read) {
        let source = match name.as_str() {
            n if n.starts_with("opencode.window.") => Some(window_file(cfg, n, &r)),
            n if n.starts_with("window-state-") => Some(window_state(n, &r)),
            n if n.starts_with("outbox-") => super::outbox::source(cfg, &windows, &r.path, &r.raw, &r.value),
            "settings.json" => Some(one(&r, Row::State { name: "opencode.global.dat".into(), key: "rune.settings".into(), value: r.value.to_string() })),
            "prompt-history.json" => Some(one(&r, Row::Doc { key: crate::PROMPT_HISTORY_DOCUMENT.into(), value: r.value.to_string() })),
            "open-in.json" => Some(open_in(&r)),
            "plugins-state.json" => Some(plugins_state(&r)),
            "terminals.json" => Some(terminals(&r)),
            n => PLAIN.iter().find(|(f, _, _)| *f == n).map(|(_, ns, key)| one(&r, Row::State { name: (*ns).into(), key: (*key).into(), value: r.value.to_string() })),
        };
        match source {
            Some(s) => plan.sources.push(s),
            None => plan.unreadable.push(r.path),
        }
    }
    for path in plugin_stores {
        match plugin_store(&path) {
            Some(s) => plan.sources.push(s),
            None => plan.unreadable.push(path),
        }
    }
    for path in markers {
        match marker(&path) {
            Some(s) => plan.sources.push(s),
            None => plan.unreadable.push(path),
        }
    }
    plan.sources.retain(|s| !s.paths.is_empty());
    plan
}

fn state(name: &str, key: &str, value: &Value) -> Row {
    Row::State { name: name.into(), key: key.into(), value: value.to_string() }
}

fn archive(file: &str, raw: &str) -> Row {
    Row::State { name: "rune.legacy".into(), key: file.into(), value: json!(raw).to_string() }
}

fn paths_of(r: &Read) -> Source {
    Source { paths: vec![r.path.clone()], ..Source::default() }
}

fn one(r: &Read, row: Row) -> Source {
    Source { paths: vec![r.path.clone()], rows: vec![row], blobs: Vec::new() }
}

/// The files every window shared: the window list, the app-wide keys (MS-1's
/// `migration::global`, with `home.json`'s selection, collapsed and hidden servers), and, for a
/// profile from before windows had files, its first window (`migration::window`, `first`).
fn shared(cfg: &ImportConfig, read: &mut BTreeMap<String, Read>) -> Source {
    let mut take = |n: &str| read.remove(n);
    let (settings, tabs, ui, servers, home, global_dat) = (take(SHARED[0]), take(SHARED[1]), take(SHARED[2]), take(SHARED[3]), take(SHARED[4]), take(SHARED[5]));
    let mut source = Source::default();
    for r in [&settings, &tabs, &ui, &servers, &home, &global_dat].into_iter().flatten() {
        source.paths.push(r.path.clone());
    }
    let settings_v = settings.as_ref().map(|r| &r.value);
    for (k, v) in settings_v.and_then(Value::as_object).into_iter().flatten() {
        source.rows.push(state("opencode.settings", k, v));
    }
    let has_ids = settings_v.is_some_and(|s| s.get("windowIds").is_some());
    let (tabs_v, ui_v, servers_v) = (tabs.as_ref().map(|r| &r.value), ui.as_ref().map(|r| &r.value), servers.as_ref().map(|r| &r.value));
    let mut keys: Map<String, Value> = global_dat.as_ref().and_then(|r| r.value.as_object().cloned()).unwrap_or_default();
    if tabs.is_some() || ui.is_some() || servers.is_some() || home.is_some() {
        let mut derived = global::migrate(&global::Legacy { tabs_json: tabs_v, ui_json: ui_v, servers_json: servers_v }, &cfg.context, None);
        if let Some(h) = home.as_ref().map(|r| &r.value) {
            derived["server"]["hidden"] = h.get("hidden").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
            derived["home.servers"] = json!({ "collapsed": h.get("collapsed").cloned().filter(Value::is_object).unwrap_or_else(|| json!({})) });
            if let Some(sel) = h.get("selection").filter(|s| s.is_object()) {
                if !derived["layout"].is_object() {
                    derived["layout"] = json!({});
                }
                derived["layout"]["home"] = json!({ "selection": sel });
            }
        }
        for (k, v) in derived.as_object().into_iter().flatten() {
            keys.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    for (k, v) in &keys {
        source.rows.push(state("opencode.global.dat", k, v));
    }
    for r in [&tabs, &ui, &servers, &home].into_iter().flatten() {
        source.rows.push(archive(&file_name(&r.path), &r.raw));
    }
    let first_has_tabs = [tabs_v, ui_v].into_iter().flatten().any(|v| v.as_object().is_some_and(|m| m.keys().any(|k| opencode_kit::servers::migration::is_server_url(k))));
    if !has_ids && first_has_tabs {
        // A profile from before windows had files: its first window takes the shared lists. The
        // id comes from the files, so a second run makes the same window, not another.
        let id = format!("legacy-{}", &crate::chunk::blob_id(format!("{}\n{}", tabs.as_ref().map_or("", |r| &r.raw), ui.as_ref().map_or("", |r| &r.raw)).as_bytes())[..12]);
        let doc = window::migrate(&window::Legacy { file: None, first: true, tabs_json: tabs_v, ui_json: ui_v }, &cfg.context);
        source.rows.extend(window_rows(&crate::window_namespace(&id), &doc));
        source.rows.push(state("opencode.settings", "windowIds", &json!([id])));
    }
    source
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A window's document as rows: one `state` row per key, and each draft its own document.
fn window_rows(ns: &str, doc: &Value) -> Vec<Row> {
    let mut rows = Vec::new();
    for (k, v) in doc.as_object().into_iter().flatten() {
        match (k.as_str(), v) {
            ("drafts", Value::Object(drafts)) => {
                for (tab, d) in drafts {
                    rows.push(Row::Doc { key: format!("{ns}:draft:{tab}"), value: d.to_string() });
                }
            }
            _ => rows.push(state(ns, k, v)),
        }
    }
    rows
}

/// `opencode.window.<id>.dat`: migrated once from the per-server shape (MS-1's
/// `migration::window`), then split.
fn window_file(cfg: &ImportConfig, name: &str, r: &Read) -> Source {
    let ns = name.to_owned();
    let doc = if window::is_legacy(Some(&r.value)) { window::migrate(&window::Legacy { file: Some(&r.value), first: false, tabs_json: None, ui_json: None }, &cfg.context) } else { r.value.clone() };
    let mut source = paths_of(r);
    source.rows = window_rows(&ns, &doc);
    if window::is_legacy(Some(&r.value)) {
        source.rows.push(archive(name, &r.raw));
    }
    source
}

fn window_state(name: &str, r: &Read) -> Source {
    let id = name.trim_start_matches("window-state-").trim_end_matches(".json").to_owned();
    let mut source = paths_of(r);
    if let Some(frame) = WindowFrame::from_json(&r.value) {
        source.rows.push(Row::Window { id, frame });
        if frame.to_json() == r.value {
            return source;
        }
    }
    source.rows.push(archive(name, &r.raw));
    source
}

fn open_in(r: &Read) -> Source {
    match r.value.as_object().filter(|m| m.len() == 1).and_then(|m| m.get("app")).filter(|a| a.is_string()) {
        Some(app) => one(r, state("plugin.file", "openInApp", app)),
        None => one(r, state("plugin.file", "rune.openIn", &r.value)),
    }
}

/// `plugins-state.json`: the disabled plugins become `plugin` rows, the trusted libraries
/// `plugin_trust` rows.
fn plugins_state(r: &Read) -> Source {
    let mut source = paths_of(r);
    let disabled: Vec<String> = r.value["disabled"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
    let trusted: Vec<(String, String)> = r.value["trusted"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
    for id in &disabled {
        source.rows.push(Row::Plugin { id: id.clone(), enabled: false });
    }
    for (id, sha256) in &trusted {
        source.rows.push(Row::Trust { id: id.clone(), sha256: sha256.clone() });
    }
    let mut sorted = disabled.clone();
    sorted.sort();
    sorted.dedup();
    let mut original = r.value.clone();
    if let Some(list) = original.get_mut("disabled").and_then(Value::as_array_mut) {
        list.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        list.dedup();
    }
    let trusted: Map<String, Value> = trusted.into_iter().map(|(k, v)| (k, json!(v))).collect();
    let rebuilt = json!({ "disabled": sorted, "trusted": trusted });
    if rebuilt != original {
        source.rows.push(archive("plugins-state.json", &r.raw));
    }
    source
}

/// `terminals.json` (`{ server: { directory: workspace } }`): one row per workspace, keyed
/// `workspace:<server>:<base64url(directory)>`.
fn terminals(r: &Read) -> Source {
    let mut source = paths_of(r);
    let mut rebuilt = Map::new();
    for (server, dirs) in r.value.as_object().into_iter().flatten() {
        for (dir, ws) in dirs.as_object().into_iter().flatten() {
            source.rows.push(state("plugin.terminal", &terminal_key(server, dir), ws));
            rebuilt.entry(server.clone()).or_insert_with(|| json!({}))[dir] = ws.clone();
        }
    }
    let original: Map<String, Value> = r.value.as_object().into_iter().flatten().filter(|(_, d)| d.as_object().is_none_or(|m| !m.is_empty())).map(|(k, v)| (k.clone(), v.clone())).collect();
    if Value::Object(rebuilt) != Value::Object(original) {
        source.rows.push(archive("terminals.json", &r.raw));
    }
    source
}

pub fn terminal_key(server: &str, dir: &str) -> String {
    format!("workspace:{server}:{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(dir))
}

/// `plugins-state/<id>.json`: each Luau store value (JSON text) its own row in `plugin.<id>`.
fn plugin_store(path: &Path) -> Option<Source> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let id = path.file_stem()?.to_str()?;
    let ns = crate::plugin_namespace(id);
    let map = value.as_object()?;
    let mut source = Source { paths: vec![path.to_path_buf()], ..Source::default() };
    for (k, v) in map {
        if let Some(text) = v.as_str() {
            source.rows.push(Row::State { name: ns.clone(), key: k.clone(), value: text.to_owned() });
        }
    }
    if map.values().any(|v| !v.is_string()) {
        source.rows.push(archive(&format!("plugins-state/{id}.json"), &raw));
    }
    Some(source)
}

/// The updater's `ready` (the staged version) and `rollout-bucket` markers.
fn marker(path: &Path) -> Option<Source> {
    let raw = std::fs::read_to_string(path).ok()?;
    let text = raw.trim();
    let (key, value, rebuilt) = match path.file_name()?.to_str()? {
        "ready" => ("ready", json!(text), format!("{text}\n")),
        "rollout-bucket" => ("rune.rolloutBucket", json!(text.parse::<u64>().ok()?), format!("{text}\n")),
        _ => return None,
    };
    let mut source = Source { paths: vec![path.to_path_buf()], rows: vec![state("plugin.updater", key, &value)], blobs: Vec::new() };
    if rebuilt != raw {
        source.rows.push(archive(&format!("Updates/{}", file_name(path)), &raw));
    }
    Some(source)
}
