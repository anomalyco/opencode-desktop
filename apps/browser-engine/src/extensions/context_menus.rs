//! Extension `chrome.contextMenus` item catalog, context matching, and click payload builder,
//! mirroring official `packages/gui-plugins/src/browser/profile/extension-api/context-menus.ts`
//! (`cfbf3f71`).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extensions::integrity::read_regular_file_bounded;
use crate::extensions::manifest::{
    extension_label, is_extension_scheme, is_private_url, is_valid_ext_id, matches_host,
};
use crate::wire::{ExtensionMenuGroup, ExtensionMenuItem, ExtensionMenuItemKind};

const MAX_ITEMS_PER_EXT: usize = 256;
const MAX_SERIALIZED_BYTES: usize = 256 * 1024;
const MAX_MENU_DEPTH: usize = 6;
const MAX_GROUPS: usize = 32;
const MAX_TOTAL_ITEMS: usize = 512;

/// What was right-clicked on a page (`context-menus.ts:45-51`).
#[derive(Clone, Debug, Default)]
pub struct MenuTarget {
    pub page_url: String,
    pub frame_url: String,
    pub link_url: Option<String>,
    pub src_url: Option<String>,
    pub media_type: Option<String>,
    pub is_editable: bool,
    pub selection_text: String,
}

impl MenuTarget {
    fn contexts(&self) -> Vec<&'static str> {
        let mut ctxs = Vec::new();
        if self.link_url.as_deref().is_some_and(|s| !s.is_empty()) {
            ctxs.push("link");
        }
        if !self.selection_text.is_empty() {
            ctxs.push("selection");
        }
        if self.is_editable {
            ctxs.push("editable");
        }
        if let Some(m) = self.media_type.as_deref() {
            match m {
                "image" => ctxs.push("image"),
                "video" => ctxs.push("video"),
                "audio" => ctxs.push("audio"),
                _ => {}
            }
        }
        if !self.frame_url.is_empty() && self.frame_url != self.page_url {
            ctxs.push("frame");
        }
        if ctxs.is_empty() {
            ctxs.push("page");
        }
        ctxs
    }

    /// Returns `true` if any target URL is private (`file:`, `oc-preview:`, `artifact:`,
    /// `workspace:`) or `chrome-extension://` so cross-extension pages/frames/resources never
    /// leak into another extension's `contextMenus.onClicked` (`review-rd294.md` #447 SF #1).
    pub fn has_private_url(&self) -> bool {
        let excluded = |u: &str| is_private_url(u) || is_extension_scheme(u);
        excluded(&self.page_url)
            || excluded(&self.frame_url)
            || self.link_url.as_deref().is_some_and(excluded)
            || self.src_url.as_deref().is_some_and(excluded)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredMenuItem {
    id: String,
    id_is_num: bool,
    title: String,
    kind: ExtensionMenuItemKind,
    contexts: Vec<String>,
    parent_id: Option<String>,
    parent_is_num: bool,
    checked: bool,
    enabled: bool,
    visible: bool,
    document_url_patterns: Option<Vec<String>>,
    target_url_patterns: Option<Vec<String>>,
}

/// Persisted archive matching official `context-menus.ts:37-42` (`MenuArchive`).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MenuArchive {
    version: String,
    items: Vec<StoredMenuItem>,
}

#[derive(Clone, Debug, Default)]
struct ExtMenus {
    name: String,
    version: String,
    items: Vec<StoredMenuItem>,
}

/// Shared catalog of registered extension context-menu items across loaded extensions with the
/// `"contextMenus"` permission, persisted per extension version under `<profile>/opencode-ext-menus/`.
#[derive(Clone, Debug, Default)]
pub struct ExtensionContextMenuCatalog {
    by_ext: BTreeMap<String, ExtMenus>,
    storage_dir: Option<PathBuf>,
}

impl ExtensionContextMenuCatalog {
    pub fn with_storage_dir(storage_dir: PathBuf) -> Self {
        Self { by_ext: BTreeMap::new(), storage_dir: Some(storage_dir) }
    }

    /// Configures `(ext_id, name, version)` and restores persisted menu items when `has_permission`
    /// is true and the on-disk archive matches `version` (`context-menus.ts:40-42`).
    pub fn configure_extension(
        &mut self,
        ext_id: &str,
        name: &str,
        version: &str,
        has_permission: bool,
    ) {
        if !has_permission {
            self.by_ext.remove(ext_id);
            self.delete_archive(ext_id);
            return;
        }
        let clean_name = extension_label(name);
        let already_loaded_same = self
            .by_ext
            .get(ext_id)
            .is_some_and(|e| e.version == version && !e.items.is_empty());
        if already_loaded_same {
            if let Some(e) = self.by_ext.get_mut(ext_id) {
                e.name = clean_name;
            }
            return;
        }
        let restored = self.load_archive(ext_id, version);
        let entry = self.by_ext.entry(ext_id.to_owned()).or_default();
        entry.name = clean_name;
        entry.version = version.to_owned();
        entry.items = restored;
    }

    pub fn remove_extension(&mut self, ext_id: &str) {
        self.by_ext.remove(ext_id);
        self.delete_archive(ext_id);
    }

    pub fn clear_extension(&mut self, ext_id: &str) {
        if let Some(entry) = self.by_ext.get_mut(ext_id) {
            entry.items.clear();
        }
        self.delete_archive(ext_id);
    }

    pub fn create(&mut self, ext_id: &str, ext_name: &str, props: &Value) -> Result<(), String> {
        let Some(obj) = props.as_object() else {
            return Err("Invalid context menu properties.".into());
        };
        let (id, id_is_num) = parse_id(obj.get("id")).unwrap_or_else(|| (String::new(), false));
        let entry = self.by_ext.entry(ext_id.to_owned()).or_insert_with(|| ExtMenus {
            name: extension_label(ext_name),
            version: String::new(),
            items: Vec::new(),
        });
        if entry.name.is_empty() {
            entry.name = extension_label(ext_name);
        }
        if entry.items.iter().any(|i| i.id == id) {
            return Err(format!("Cannot create item with duplicate id {id}."));
        }
        let (parent_id, parent_is_num) = match obj.get("parentId").and_then(|v| parse_id(Some(v))) {
            Some((pid, is_num)) => {
                if !entry.items.iter().any(|i| i.id == pid) {
                    return Err(format!("Cannot find menu item with id {pid}."));
                }
                (Some(pid), is_num)
            }
            None => (None, false),
        };
        let item = StoredMenuItem {
            id,
            id_is_num,
            title: obj.get("title").and_then(Value::as_str).unwrap_or("").chars().take(256).collect(),
            kind: parse_kind(obj.get("type")),
            contexts: parse_str_list(obj.get("contexts")).unwrap_or_else(|| vec!["page".into()]),
            parent_id,
            parent_is_num,
            checked: obj.get("checked").and_then(Value::as_bool).unwrap_or(false),
            enabled: obj.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            visible: obj.get("visible").and_then(Value::as_bool).unwrap_or(true),
            document_url_patterns: parse_str_list(obj.get("documentUrlPatterns")),
            target_url_patterns: parse_str_list(obj.get("targetUrlPatterns")),
        };
        if entry.items.len() >= MAX_ITEMS_PER_EXT
            || approx_bytes(&entry.items).saturating_add(approx_item_bytes(&item)) > MAX_SERIALIZED_BYTES
        {
            return Err("Context menu storage limit exceeded.".into());
        }
        entry.items.push(item);
        self.persist_extension(ext_id);
        Ok(())
    }

    pub fn update(&mut self, ext_id: &str, id_val: &Value, props: &Value) -> Result<(), String> {
        let Some((id, _)) = parse_id(Some(id_val)) else {
            return Err("Invalid context menu item id.".into());
        };
        let Some(obj) = props.as_object() else {
            return Err("Invalid context menu properties.".into());
        };
        let Some(entry) = self.by_ext.get_mut(ext_id) else {
            return Err(format!("Cannot find menu item with id {id}."));
        };
        let Some(pos) = entry.items.iter().position(|i| i.id == id) else {
            return Err(format!("Cannot find menu item with id {id}."));
        };
        let mut updated = entry.items[pos].clone();
        if let Some((pid, is_num)) = obj.get("parentId").and_then(|v| parse_id(Some(v))) {
            if !entry.items.iter().any(|i| i.id == pid) {
                return Err(format!("Cannot find menu item with id {pid}."));
            }
            if is_ancestor(&entry.items, &pid, &id) {
                return Err(format!("Cannot make menu item {id} a descendant of itself."));
            }
            updated.parent_id = Some(pid);
            updated.parent_is_num = is_num;
        }
        if let Some(t) = obj.get("title").and_then(Value::as_str) {
            updated.title = t.chars().take(256).collect();
        }
        if obj.get("type").is_some() {
            updated.kind = parse_kind(obj.get("type"));
        }
        if let Some(c) = parse_str_list(obj.get("contexts")) {
            updated.contexts = c;
        }
        if let Some(v) = obj.get("checked").and_then(Value::as_bool) {
            updated.checked = v;
        }
        if let Some(v) = obj.get("enabled").and_then(Value::as_bool) {
            updated.enabled = v;
        }
        if let Some(v) = obj.get("visible").and_then(Value::as_bool) {
            updated.visible = v;
        }
        if let Some(v) = parse_str_list(obj.get("documentUrlPatterns")) {
            updated.document_url_patterns = Some(v);
        }
        if let Some(v) = parse_str_list(obj.get("targetUrlPatterns")) {
            updated.target_url_patterns = Some(v);
        }
        let next_bytes = approx_bytes(&entry.items)
            .saturating_sub(approx_item_bytes(&entry.items[pos]))
            .saturating_add(approx_item_bytes(&updated));
        if next_bytes > MAX_SERIALIZED_BYTES {
            return Err("Context menu storage limit exceeded.".into());
        }
        entry.items[pos] = updated;
        self.persist_extension(ext_id);
        Ok(())
    }

    pub fn remove(&mut self, ext_id: &str, id_val: &Value) {
        let Some((id, _)) = parse_id(Some(id_val)) else {
            return;
        };
        let Some(entry) = self.by_ext.get_mut(ext_id) else {
            return;
        };
        let mut to_remove = HashSet::new();
        to_remove.insert(id);
        loop {
            let before = to_remove.len();
            for item in &entry.items {
                if item.parent_id.as_ref().is_some_and(|p| to_remove.contains(p)) {
                    to_remove.insert(item.id.clone());
                }
            }
            if to_remove.len() == before {
                break;
            }
        }
        entry.items.retain(|i| !to_remove.contains(&i.id));
        self.persist_extension(ext_id);
    }

    /// Builds sanitized, context-matched extension menu groups for a right-click on a regular web
    /// tab (`context-menus.ts:235-268`). Returns empty on `file://` or `chrome-extension://`
    /// targets and bounds total emitted items to `MAX_TOTAL_ITEMS`.
    pub fn build_groups(&self, target: &MenuTarget) -> Vec<ExtensionMenuGroup> {
        if target.has_private_url() {
            return Vec::new();
        }
        let ctxs = target.contexts();
        let mut groups = Vec::new();
        let mut remaining = MAX_TOTAL_ITEMS;
        for (ext_id, entry) in &self.by_ext {
            if groups.len() >= MAX_GROUPS || remaining == 0 {
                break;
            }
            let top = build_level(&entry.items, target, &ctxs, None, 0, &mut remaining);
            if !top.is_empty() {
                let title = if entry.name.is_empty() { ext_id.clone() } else { entry.name.clone() };
                groups.push(ExtensionMenuGroup { ext_id: ext_id.clone(), title, items: top });
            }
        }
        groups
    }

    /// Records a user click on `(ext_id, item_id)` (updating checkbox/radio state) and constructs
    /// the `contextMenus.onClicked` `OnClickData` JSON object (`context-menus.ts:206-226`).
    pub fn choose_and_build_click_info(
        &mut self,
        ext_id: &str,
        item_id: &str,
        target: &MenuTarget,
    ) -> Option<Value> {
        if target.has_private_url() {
            return None;
        }
        let entry = self.by_ext.get_mut(ext_id)?;
        let idx = entry.items.iter().position(|i| i.id == item_id && i.visible && i.enabled)?;
        let chosen = entry.items[idx].clone();
        let is_choice = matches!(chosen.kind, ExtensionMenuItemKind::Checkbox | ExtensionMenuItemKind::Radio);
        let was_checked = chosen.checked;
        let next_checked = match chosen.kind {
            ExtensionMenuItemKind::Checkbox => {
                entry.items[idx].checked = !was_checked;
                !was_checked
            }
            ExtensionMenuItemKind::Radio => {
                for item in &mut entry.items {
                    if item.kind == ExtensionMenuItemKind::Radio && item.parent_id == chosen.parent_id {
                        item.checked = item.id == chosen.id;
                    }
                }
                true
            }
            _ => false,
        };
        let mut info = json!({
            "menuItemId": id_to_json(&chosen.id, chosen.id_is_num),
            "pageUrl": target.page_url,
            "frameId": 0,
            "editable": target.is_editable,
        });
        if let Some(m) = info.as_object_mut() {
            if let Some(pid) = &chosen.parent_id {
                m.insert("parentMenuItemId".into(), id_to_json(pid, chosen.parent_is_num));
            }
            if let Some(link) = &target.link_url {
                m.insert("linkUrl".into(), Value::String(link.clone()));
            }
            if let Some(src) = &target.src_url {
                m.insert("srcUrl".into(), Value::String(src.clone()));
                if let Some(media) = &target.media_type {
                    m.insert("mediaType".into(), Value::String(media.clone()));
                }
            }
            if !target.selection_text.is_empty() {
                m.insert("selectionText".into(), Value::String(target.selection_text.clone()));
            }
            if is_choice {
                m.insert("wasChecked".into(), Value::Bool(was_checked));
                m.insert("checked".into(), Value::Bool(next_checked));
            }
        }
        if is_choice {
            self.persist_extension(ext_id);
        }
        Some(info)
    }

    fn archive_path(&self, ext_id: &str) -> Option<PathBuf> {
        if !is_valid_ext_id(ext_id) {
            return None;
        }
        Some(self.storage_dir.as_ref()?.join(format!("{ext_id}.json")))
    }

    fn delete_archive(&self, ext_id: &str) {
        let Some(path) = self.archive_path(ext_id) else { return };
        if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn persist_extension(&self, ext_id: &str) {
        let Some(path) = self.archive_path(ext_id) else { return };
        let Some(entry) = self.by_ext.get(ext_id) else {
            self.delete_archive(ext_id);
            return;
        };
        if entry.items.is_empty() || entry.version.is_empty() {
            self.delete_archive(ext_id);
            return;
        }
        let Some(dir) = path.parent() else { return };
        if !ensure_regular_dir(dir) {
            return;
        }
        let archive = MenuArchive { version: entry.version.clone(), items: entry.items.clone() };
        let Ok(bytes) = serde_json::to_vec(&archive) else { return };
        if bytes.len() > MAX_SERIALIZED_BYTES {
            return;
        }
        let tmp = dir.join(format!("{ext_id}.json.tmp.{}", std::process::id()));
        if std::fs::symlink_metadata(&tmp).is_ok_and(|m| m.file_type().is_symlink()) {
            return;
        }
        if std::fs::write(&tmp, &bytes).is_ok() {
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = std::fs::remove_file(&tmp);
                return;
            }
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    fn load_archive(&self, ext_id: &str, expected_version: &str) -> Vec<StoredMenuItem> {
        let Some(path) = self.archive_path(ext_id) else {
            return Vec::new();
        };
        let Some(bytes) = read_regular_file_bounded(&path, MAX_SERIALIZED_BYTES as u64) else {
            return Vec::new();
        };
        let Ok(archive) = serde_json::from_slice::<MenuArchive>(&bytes) else {
            self.delete_archive(ext_id);
            return Vec::new();
        };
        if archive.version != expected_version {
            self.delete_archive(ext_id);
            return Vec::new();
        }
        validate_restored_items(archive.items)
    }
}

fn ensure_regular_dir(dir: &Path) -> bool {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        return !meta.file_type().is_symlink() && meta.is_dir();
    }
    std::fs::create_dir_all(dir).is_ok()
        && std::fs::symlink_metadata(dir).is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir())
}

fn validate_restored_items(raw: Vec<StoredMenuItem>) -> Vec<StoredMenuItem> {
    if raw.len() > MAX_ITEMS_PER_EXT {
        return Vec::new();
    }
    let mut seen = HashSet::with_capacity(raw.len());
    let mut cleaned = Vec::with_capacity(raw.len());
    for mut item in raw {
        item.id = item.id.chars().take(128).collect();
        if !seen.insert(item.id.clone()) {
            return Vec::new();
        }
        item.title = item.title.chars().take(256).collect();
        item.contexts = item
            .contexts
            .into_iter()
            .take(64)
            .map(|s| s.chars().take(64).collect())
            .collect();
        if item.contexts.is_empty() {
            item.contexts.push("page".into());
        }
        if let Some(pid) = item.parent_id {
            item.parent_id = Some(pid.chars().take(128).collect());
        }
        if let Some(pats) = item.document_url_patterns {
            item.document_url_patterns = Some(pats.into_iter().take(64).map(|s| s.chars().take(256).collect()).collect());
        }
        if let Some(pats) = item.target_url_patterns {
            item.target_url_patterns = Some(pats.into_iter().take(64).map(|s| s.chars().take(256).collect()).collect());
        }
        cleaned.push(item);
    }
    if approx_bytes(&cleaned) > MAX_SERIALIZED_BYTES {
        return Vec::new();
    }
    for item in &cleaned {
        if let Some(pid) = item.parent_id.as_deref() {
            if !seen.contains(pid) || is_ancestor(&cleaned, pid, &item.id) {
                return Vec::new();
            }
        }
    }
    cleaned
}

fn build_level(
    items: &[StoredMenuItem],
    target: &MenuTarget,
    ctxs: &[&str],
    parent_id: Option<&str>,
    depth: usize,
    remaining: &mut usize,
) -> Vec<ExtensionMenuItem> {
    if depth >= MAX_MENU_DEPTH || *remaining == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for item in items {
        if *remaining == 0 {
            break;
        }
        if item.parent_id.as_deref() != parent_id || !item.visible {
            continue;
        }
        if parent_id.is_none()
            && !item.contexts.iter().any(|c| c == "all" || ctxs.contains(&c.as_str()))
        {
            continue;
        }
        if let Some(pats) = &item.document_url_patterns
            && !matches_host(pats, &target.page_url)
        {
            continue;
        }
        if let Some(pats) = &item.target_url_patterns {
            let target_ok = target.link_url.as_deref().is_some_and(|u| matches_host(pats, u))
                || target.src_url.as_deref().is_some_and(|u| matches_host(pats, u));
            if !target_ok {
                continue;
            }
        }
        *remaining -= 1;
        let children = if item.kind == ExtensionMenuItemKind::Separator || *remaining == 0 {
            Vec::new()
        } else {
            build_level(items, target, ctxs, Some(&item.id), depth + 1, remaining)
        };
        let sel = extension_label(&target.selection_text);
        let title = extension_label(&extension_label(&item.title).replace("%s", &sel));
        out.push(ExtensionMenuItem {
            id: item.id.clone(),
            title,
            kind: item.kind,
            checked: item.checked,
            enabled: item.enabled,
            children,
        });
    }
    out
}

fn parse_id(val: Option<&Value>) -> Option<(String, bool)> {
    match val? {
        Value::String(s) => Some((s.chars().take(128).collect(), false)),
        Value::Number(n) => Some((n.to_string(), true)),
        _ => None,
    }
}

fn id_to_json(id: &str, is_num: bool) -> Value {
    if is_num && let Ok(n) = id.parse::<i64>() {
        return Value::Number(n.into());
    }
    Value::String(id.to_owned())
}

fn parse_kind(val: Option<&Value>) -> ExtensionMenuItemKind {
    match val.and_then(Value::as_str) {
        Some("checkbox") => ExtensionMenuItemKind::Checkbox,
        Some("radio") => ExtensionMenuItemKind::Radio,
        Some("separator") => ExtensionMenuItemKind::Separator,
        _ => ExtensionMenuItemKind::Normal,
    }
}

fn parse_str_list(val: Option<&Value>) -> Option<Vec<String>> {
    let arr = val?.as_array()?;
    Some(
        arr.iter()
            .take(64)
            .filter_map(Value::as_str)
            .map(|s| s.chars().take(256).collect())
            .collect(),
    )
}

fn is_ancestor(items: &[StoredMenuItem], start: &str, needle: &str) -> bool {
    let mut cur = Some(start);
    let mut seen = HashSet::new();
    while let Some(id) = cur {
        if id == needle {
            return true;
        }
        if !seen.insert(id) {
            return false;
        }
        cur = items.iter().find(|i| i.id == id).and_then(|i| i.parent_id.as_deref());
    }
    false
}

fn approx_item_bytes(item: &StoredMenuItem) -> usize {
    64 + item.id.len()
        + item.title.len()
        + item.parent_id.as_ref().map_or(0, String::len)
        + item.contexts.iter().map(String::len).sum::<usize>()
        + item.document_url_patterns.as_ref().map_or(0, |v| v.iter().map(String::len).sum())
        + item.target_url_patterns.as_ref().map_or(0, |v| v.iter().map(String::len).sum())
}

fn approx_bytes(items: &[StoredMenuItem]) -> usize {
    items.iter().map(approx_item_bytes).sum()
}
