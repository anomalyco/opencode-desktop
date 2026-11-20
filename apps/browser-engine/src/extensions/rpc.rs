//! Extension RPC method dispatcher, Rust-side permission enforcement, tab/window queries,
//! action icon resolution (`path` + `ImageData` -> PNG), and navigation URL policy.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::path::Path;

use base64::Engine as _;
use serde_json::{Value, json};

use super::manifest::{
    ExtensionManifest, closest_icon, extension_id_from_url, extension_label, extension_page_url,
    glob_match, is_extension_scheme, is_private_url, matches_host, read_icon_data_url,
};
use super::png::rgba_to_png_data_url;
use super::{ExtEffect, ExtTab, ExtensionHost, WINDOW_ID, WINDOW_ID_CURRENT};

const MAX_NAV_URL_BYTES: usize = 8192;

/// Exact finite allow-list of supported shimmed event names (prevents unbounded `subscribers` OOM;
/// `review-rd274.md` §4).
pub(super) fn is_supported_event(name: &str) -> bool {
    matches!(
        name,
        "tabs.onCreated"
            | "tabs.onRemoved"
            | "tabs.onUpdated"
            | "tabs.onActivated"
            | "windows.onCreated"
            | "windows.onRemoved"
            | "windows.onFocusChanged"
            | "action.onClicked"
            | "webNavigation.onBeforeNavigate"
            | "webNavigation.onCommitted"
            | "webNavigation.onDOMContentLoaded"
            | "webNavigation.onCompleted"
            | "contextMenus.onClicked"
            | "notifications.onClicked"
            | "notifications.onClosed"
            | "commands.onCommand"
    )
}

pub(super) fn is_event_allowed(manifest: &ExtensionManifest, name: &str) -> bool {
    if !is_supported_event(name) {
        return false;
    }
    let required: Option<&str> = if name.starts_with("webNavigation.") {
        Some("webNavigation")
    } else if name.starts_with("contextMenus.") {
        Some("contextMenus")
    } else if name.starts_with("notifications.") {
        Some("notifications")
    } else {
        None
    };
    required.is_none_or(|perm| manifest.has_permission(perm))
}

/// Exact method allow-list enforced in Rust before every shimmed RPC call (`review-rd274.md` §4).
pub(super) fn is_api_allowed(manifest: &ExtensionManifest, name: &str) -> bool {
    let required: Option<&str> = match name {
        "webNavigation.getFrame" | "webNavigation.getAllFrames" => Some("webNavigation"),
        "contextMenus.create"
        | "contextMenus.update"
        | "contextMenus.remove"
        | "contextMenus.removeAll" => Some("contextMenus"),
        "notifications.create"
        | "notifications.update"
        | "notifications.clear"
        | "notifications.getAll"
        | "notifications.getPermissionLevel" => Some("notifications"),
        "cookies.getAllCookieStores" | "cookies.getPartitionKey" => Some("cookies"),
        "sidePanel.setOptions"
        | "sidePanel.getOptions"
        | "sidePanel.setPanelBehavior"
        | "sidePanel.getPanelBehavior"
        | "sidePanel.open"
        | "sidePanel.close" => Some("sidePanel"),
        "fontSettings.getFontList"
        | "fontSettings.getFont"
        | "fontSettings.setFont"
        | "fontSettings.clearFont" => Some("fontSettings"),
        "tabs.query"
        | "tabs.get"
        | "tabs.getCurrent"
        | "tabs.create"
        | "tabs.update"
        | "tabs.remove"
        | "tabs.reload"
        | "tabs.highlight"
        | "windows.get"
        | "windows.getCurrent"
        | "windows.getLastFocused"
        | "windows.getAll"
        | "windows.create"
        | "windows.update"
        | "windows.remove"
        | "action.setTitle"
        | "action.getTitle"
        | "action.setIcon"
        | "action.setPopup"
        | "action.getPopup"
        | "action.setBadgeText"
        | "action.getBadgeText"
        | "action.setBadgeBackgroundColor"
        | "action.getBadgeBackgroundColor"
        | "action.setBadgeTextColor"
        | "action.getBadgeTextColor"
        | "action.enable"
        | "action.disable"
        | "action.isEnabled"
        | "action.getUserSettings"
        | "action.openPopup"
        | "permissions.getAll"
        | "permissions.contains"
        | "permissions.request"
        | "permissions.remove"
        | "commands.getAll"
        | "extension.isAllowedFileSchemeAccess"
        | "extension.isAllowedIncognitoAccess"
        | "extension.setUpdateUrlData"
        | "runtime.openOptionsPage" => None,
        other if other.starts_with("privacy.") && other.len() <= 96 => Some("privacy"),
        _ => return false,
    };
    required.is_none_or(|perm| manifest.has_permission(perm))
}

impl ExtensionHost {
    pub(super) fn dispatch_rpc(
        &mut self,
        sid: &str,
        ext_id: &str,
        req_id: i64,
        name: &str,
        args: &[Value],
    ) -> Vec<ExtEffect> {
        let mut effects = Vec::new();
        let Some(ext) = self.extensions.get(ext_id).cloned() else {
            self.reply_err(sid, req_id, "Extension is not loaded.");
            return effects;
        };
        if !is_api_allowed(&ext.manifest, name) {
            self.reply_err(sid, req_id, &format!("chrome.{name} is not available in this browser."));
            return effects;
        }

        let result: Result<Value, String> = match name {
            "tabs.query" => {
                let query = args.first().and_then(Value::as_object);
                Ok(Value::Array(self.answer_tabs_query(&ext.manifest, query)))
            }
            "tabs.get" => {
                let wanted = args.first().and_then(Value::as_i64).map(|n| n as i32);
                match wanted.and_then(|id| self.tabs.iter().find(|t| t.chromium_tab_id == Some(id)))
                {
                    Some(t) => Ok(self.tab_json(t, &ext.manifest, false)),
                    None => Err(format!("No tab with id: {}.", wanted.unwrap_or(-1))),
                }
            }
            "tabs.getCurrent" => Ok(Value::Null),
            "tabs.create" => {
                if !self.allow_open_tab(ext_id) {
                    Err("Too many tabs opened by this extension.".into())
                } else {
                    let opts = args.first().and_then(Value::as_object);
                    let active = opts.and_then(|o| o.get("active")).and_then(Value::as_bool).unwrap_or(true);
                    let raw_url = opts.and_then(|o| o.get("url")).and_then(Value::as_str).unwrap_or("about:blank");
                    match resolve_extension_nav_url(ext_id, raw_url) {
                        Ok(url) => {
                            effects.push(ExtEffect::OpenTab {
                                ext_id: ext_id.to_owned(),
                                url,
                                active,
                                reply_to: Some((sid.to_owned(), req_id, ext_id.to_owned())),
                            });
                            return effects;
                        }
                        Err(e) => Err(e),
                    }
                }
            }
            "tabs.update" => self.rpc_tabs_update(sid, ext_id, req_id, &ext.manifest, args, &mut effects),
            "tabs.remove" => {
                let ids: Vec<i32> = match args.first() {
                    Some(Value::Number(n)) => n.as_i64().map(|v| vec![v as i32]).unwrap_or_default(),
                    Some(Value::Array(arr)) => arr.iter().take(16).filter_map(|v| v.as_i64().map(|n| n as i32)).collect(),
                    _ => Vec::new(),
                };
                for cid in ids {
                    if let Some(t) = self.tabs.iter().find(|t| t.chromium_tab_id == Some(cid)) {
                        if t.agent_driven {
                            continue;
                        }
                        effects.push(ExtEffect::CloseTab { engine_tab_id: t.engine_tab_id });
                    }
                }
                Ok(Value::Null)
            }
            "tabs.reload" => {
                let (tid_opt, opts) = if args.first().is_some_and(Value::is_number) {
                    (args[0].as_i64().map(|n| n as i32), args.get(1).and_then(Value::as_object))
                } else {
                    (None, args.first().and_then(Value::as_object))
                };
                let ignore_cache = opts.and_then(|o| o.get("bypassCache")).and_then(Value::as_bool).unwrap_or(false);
                let tab = match tid_opt {
                    Some(cid) => self.tabs.iter().find(|t| t.chromium_tab_id == Some(cid)),
                    None => self.tabs.iter().find(|t| t.active),
                };
                if let Some(t) = tab {
                    if t.agent_driven {
                        Err("Extensions cannot reload agent-driven tabs.".into())
                    } else {
                        effects.push(ExtEffect::ReloadTab { engine_tab_id: t.engine_tab_id, ignore_cache });
                        Ok(Value::Null)
                    }
                } else {
                    Err("No tab to reload.".into())
                }
            }
            "tabs.highlight" => {
                let idx_opt = args
                    .first()
                    .and_then(|v| v.get("tabs"))
                    .and_then(|t| t.as_u64().or_else(|| t.as_array()?.first()?.as_u64()))
                    .map(|n| n as usize);
                if let Some(idx) = idx_opt
                    && let Some(t) = self.tabs.get(idx)
                {
                    effects.push(ExtEffect::FocusTab { engine_tab_id: t.engine_tab_id });
                }
                Ok(self.window_json(&ext.manifest, true))
            }
            "windows.get" => {
                let id = args.first().and_then(Value::as_i64).unwrap_or(-1) as i32;
                let populate = args.get(1).and_then(|v| v.get("populate")).and_then(Value::as_bool).unwrap_or(false);
                if id == WINDOW_ID || id == WINDOW_ID_CURRENT {
                    Ok(self.window_json(&ext.manifest, populate))
                } else {
                    Err(format!("No window with id: {id}."))
                }
            }
            "windows.getCurrent" | "windows.getLastFocused" => {
                let populate = args.first().and_then(|v| v.get("populate")).and_then(Value::as_bool).unwrap_or(false);
                Ok(self.window_json(&ext.manifest, populate))
            }
            "windows.getAll" => {
                let populate = args.first().and_then(|v| v.get("populate")).and_then(Value::as_bool).unwrap_or(false);
                Ok(Value::Array(vec![self.window_json(&ext.manifest, populate)]))
            }
            "windows.create" => {
                if !self.allow_open_tab(ext_id) {
                    Err("Too many tabs opened by this extension.".into())
                } else {
                    let url = args
                        .first()
                        .and_then(|v| v.get("url"))
                        .and_then(|u| u.as_str().or_else(|| u.as_array()?.first()?.as_str()))
                        .unwrap_or("about:blank");
                    match resolve_extension_nav_url(ext_id, url) {
                        Ok(u) => {
                            effects.push(ExtEffect::OpenTab {
                                ext_id: ext_id.to_owned(),
                                url: u,
                                active: true,
                                reply_to: None,
                            });
                            Ok(self.window_json(&ext.manifest, true))
                        }
                        Err(e) => Err(e),
                    }
                }
            }
            "windows.update" => {
                if let Some(state) = args.get(1).and_then(|v| v.get("state")).and_then(Value::as_str)
                    && state != "normal"
                {
                    Err("Extensions cannot change the size of app windows.".into())
                } else {
                    Ok(self.window_json(&ext.manifest, false))
                }
            }
            "windows.remove" => Err("Extensions cannot close app windows.".into()),

            n if n.starts_with("action.") => self.rpc_action(ext_id, &ext.dir, &ext.manifest, n, args, &mut effects),

            "runtime.openOptionsPage" => {
                if !self.allow_open_tab(ext_id) {
                    Err("Too many tabs opened by this extension.".into())
                } else if let Some(page) = ext.manifest.options_page.as_deref()
                    && let Some(url) = extension_page_url(ext_id, page)
                {
                    effects.push(ExtEffect::OpenTab {
                        ext_id: ext_id.to_owned(),
                        url,
                        active: true,
                        reply_to: None,
                    });
                    Ok(Value::Null)
                } else {
                    Err("Could not create an options page.".into())
                }
            }

            "webNavigation.getFrame" => {
                let tid = args.first().and_then(|v| v.get("tabId")).and_then(Value::as_i64).map(|n| n as i32);
                let fid = args.first().and_then(|v| v.get("frameId")).and_then(Value::as_i64).unwrap_or(0);
                if fid == 0
                    && let Some(t) = tid.and_then(|id| self.tabs.iter().find(|t| t.chromium_tab_id == Some(id)))
                    && !is_private_url(&t.url)
                    && (!is_extension_scheme(&t.url) || extension_id_from_url(&t.url) == Some(ext_id))
                {
                    Ok(json!({ "url": t.url, "errorOccurred": false, "processId": 0, "frameId": 0, "parentFrameId": -1 }))
                } else {
                    Ok(Value::Null)
                }
            }
            "webNavigation.getAllFrames" => {
                let tid = args.first().and_then(|v| v.get("tabId")).and_then(Value::as_i64).map(|n| n as i32);
                if let Some(t) = tid.and_then(|id| self.tabs.iter().find(|t| t.chromium_tab_id == Some(id)))
                    && !is_private_url(&t.url)
                    && (!is_extension_scheme(&t.url) || extension_id_from_url(&t.url) == Some(ext_id))
                {
                    Ok(json!([{ "url": t.url, "errorOccurred": false, "processId": 0, "frameId": 0, "parentFrameId": -1 }]))
                } else {
                    Ok(json!([]))
                }
            }

            "contextMenus.create" => {
                if let Some(item) = args.first() {
                    let item_bytes = item.to_string().len();
                    let list = self.context_menus.entry(ext_id.to_owned()).or_default();
                    let cur_bytes: usize = list.iter().map(|v| v.to_string().len()).sum();
                    if list.len() < 256 && cur_bytes.saturating_add(item_bytes) <= 256 * 1024 {
                        list.push(item.clone());
                    }
                }
                Ok(Value::Null)
            }
            "contextMenus.update" => Ok(Value::Null),
            "contextMenus.remove" => {
                if let Some(id_val) = args.first()
                    && let Some(list) = self.context_menus.get_mut(ext_id)
                {
                    list.retain(|item| item.get("id") != Some(id_val));
                }
                Ok(Value::Null)
            }
            "contextMenus.removeAll" => {
                self.context_menus.remove(ext_id);
                Ok(Value::Null)
            }

            "notifications.create" => {
                let id = args
                    .first()
                    .and_then(Value::as_str)
                    .map(extension_label)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("n-{}", self.alloc_id()));
                Ok(Value::String(id))
            }
            "notifications.update" | "notifications.clear" => Ok(Value::Bool(true)),
            "notifications.getAll" => Ok(json!({})),
            "notifications.getPermissionLevel" => Ok(json!("granted")),

            "permissions.getAll" => {
                let perms: Vec<&String> = ext
                    .manifest
                    .permissions
                    .iter()
                    .filter(|p| !p.contains("://") && p.as_str() != "<all_urls>")
                    .collect();
                Ok(json!({ "permissions": perms, "origins": ext.manifest.hosts }))
            }
            "permissions.contains" | "permissions.request" => {
                let req = args.first().and_then(Value::as_object);
                let ok_perms = req
                    .and_then(|o| o.get("permissions"))
                    .and_then(Value::as_array)
                    .is_none_or(|arr| {
                        arr.iter()
                            .all(|v| v.as_str().is_some_and(|p| ext.manifest.has_permission(p)))
                    });
                let ok_origins = req
                    .and_then(|o| o.get("origins"))
                    .and_then(Value::as_array)
                    .is_none_or(|arr| {
                        arr.iter().all(|v| {
                            v.as_str().is_some_and(|o| {
                                ext.manifest.hosts.iter().any(|h| h == o || h == "<all_urls>" || h == "*://*/*")
                            })
                        })
                    });
                Ok(Value::Bool(ok_perms && ok_origins))
            }
            "permissions.remove" => Ok(Value::Bool(false)),

            "cookies.getAllCookieStores" => {
                let tab_ids: Vec<i32> = self.tabs.iter().filter_map(|t| t.chromium_tab_id).collect();
                Ok(json!([{ "id": "0", "tabIds": tab_ids }]))
            }
            "cookies.getPartitionKey" => Ok(json!({ "partitionKey": {} })),

            "fontSettings.getFontList" => Ok(json!([])),
            "fontSettings.getFont" => Ok(json!({ "fontId": "", "levelOfControl": "not_controllable" })),
            "fontSettings.setFont" | "fontSettings.clearFont" => Ok(Value::Null),
            "sidePanel.setOptions" | "sidePanel.setPanelBehavior" | "sidePanel.close" => Ok(Value::Null),
            "sidePanel.getOptions" => Ok(json!({ "enabled": false })),
            "sidePanel.getPanelBehavior" => Ok(json!({ "openPanelOnActionClick": false })),
            "sidePanel.open" => Err("This browser has no side panel.".into()),
            "commands.getAll" => {
                let list: Vec<Value> = ext
                    .manifest
                    .commands
                    .iter()
                    .map(|c| json!({ "name": c.name, "description": c.description, "shortcut": "" }))
                    .collect();
                Ok(Value::Array(list))
            }
            "extension.isAllowedFileSchemeAccess" | "extension.isAllowedIncognitoAccess" => Ok(Value::Bool(false)),
            "extension.setUpdateUrlData" => Ok(Value::Null),
            other if other.starts_with("privacy.") => {
                if other.ends_with(".get") {
                    Ok(json!({ "value": true, "levelOfControl": "not_controllable" }))
                } else {
                    Ok(Value::Null)
                }
            }
            _ => Err(format!("chrome.{name} is not available in this browser.")),
        };

        match result {
            Ok(val) => self.reply_ok(sid, req_id, val),
            Err(msg) => self.reply_err(sid, req_id, &msg),
        }
        effects
    }

    fn rpc_tabs_update(
        &mut self,
        _sid: &str,
        ext_id: &str,
        _req_id: i64,
        manifest: &ExtensionManifest,
        args: &[Value],
        effects: &mut Vec<ExtEffect>,
    ) -> Result<Value, String> {
        let (tid_opt, change) = if args.len() >= 2 {
            (args[0].as_i64().map(|n| n as i32), args[1].as_object())
        } else {
            (None, args.first().and_then(Value::as_object))
        };
        let target_idx = match tid_opt {
            Some(cid) => self.tabs.iter().position(|t| t.chromium_tab_id == Some(cid)),
            None => self.tabs.iter().position(|t| t.active),
        };
        let Some(idx) = target_idx else {
            return Err("No tab to update.".into());
        };
        if self.tabs[idx].agent_driven {
            return Err("Extensions cannot modify agent-driven tabs.".into());
        }
        if is_extension_scheme(&self.tabs[idx].url)
            && extension_id_from_url(&self.tabs[idx].url) != Some(ext_id)
        {
            return Err("Extensions cannot navigate another extension's page.".into());
        }
        let engine_tab_id = self.tabs[idx].engine_tab_id;
        if let Some(ch) = change {
            if let Some(raw_url) = ch.get("url").and_then(Value::as_str) {
                let url = resolve_extension_nav_url(ext_id, raw_url)?;
                effects.push(ExtEffect::NavigateTab { engine_tab_id, url });
            }
            if ch.get("active").and_then(Value::as_bool) == Some(true)
                || ch.get("highlighted").and_then(Value::as_bool) == Some(true)
            {
                effects.push(ExtEffect::FocusTab { engine_tab_id });
            }
            if let Some(muted) = ch.get("muted").and_then(Value::as_bool) {
                self.tabs[idx].muted = muted;
            }
        }
        Ok(self.tab_json(&self.tabs[idx], manifest, false))
    }

    fn rpc_action(
        &mut self,
        ext_id: &str,
        dir: &Path,
        manifest: &ExtensionManifest,
        name: &str,
        args: &[Value],
        effects: &mut Vec<ExtEffect>,
    ) -> Result<Value, String> {
        match name {
            "action.setTitle" => {
                if !self.allow_action_update(ext_id) {
                    return Ok(Value::Null);
                }
                let title: String = args
                    .first()
                    .and_then(|v| v.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(256)
                    .collect();
                let tid = arg_tab_id(args.first());
                self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.title = Some(title));
                if let Some(snap) = self.action_snapshot(ext_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                Ok(Value::Null)
            }
            "action.getTitle" => {
                let tid = arg_tab_id(args.first());
                let title = self
                    .actions
                    .get(ext_id)
                    .and_then(|s| s.for_tab(tid).title)
                    .or_else(|| manifest.action.as_ref().and_then(|a| a.default_title.clone()))
                    .unwrap_or_else(|| manifest.name.clone());
                Ok(Value::String(title))
            }
            "action.setIcon" => {
                if !self.allow_action_update(ext_id) {
                    return Ok(Value::Null);
                }
                let tid = arg_tab_id(args.first());
                if let Some(url) = args.first().and_then(|d| resolve_action_icon(dir, d)) {
                    self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.icon = Some(url));
                    if let Some(snap) = self.action_snapshot(ext_id) {
                        effects.push(ExtEffect::ActionUpdated(snap));
                    }
                }
                Ok(Value::Null)
            }
            "action.setPopup" => {
                let popup: String = args
                    .first()
                    .and_then(|v| v.get("popup"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(1024)
                    .collect();
                if !popup.is_empty() && extension_page_url(ext_id, &popup).is_none() {
                    return Err("The popup must be a page of this extension.".into());
                }
                let tid = arg_tab_id(args.first());
                self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.popup = Some(popup));
                if let Some(snap) = self.action_snapshot(ext_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                Ok(Value::Null)
            }
            "action.getPopup" => {
                let tid = arg_tab_id(args.first());
                let rel = self
                    .actions
                    .get(ext_id)
                    .and_then(|s| s.for_tab(tid).popup)
                    .or_else(|| manifest.action.as_ref().and_then(|a| a.default_popup.clone()))
                    .unwrap_or_default();
                Ok(Value::String(extension_page_url(ext_id, &rel).unwrap_or_default()))
            }
            "action.setBadgeText" => {
                if !self.allow_action_update(ext_id) {
                    return Ok(Value::Null);
                }
                let text: String = args
                    .first()
                    .and_then(|v| v.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .chars()
                    .take(32)
                    .collect();
                let tid = arg_tab_id(args.first());
                self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.badge_text = Some(text));
                if let Some(snap) = self.action_snapshot(ext_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                Ok(Value::Null)
            }
            "action.getBadgeText" => {
                let tid = arg_tab_id(args.first());
                Ok(Value::String(self.actions.get(ext_id).and_then(|s| s.for_tab(tid).badge_text).unwrap_or_default()))
            }
            "action.setBadgeBackgroundColor" => {
                if let Some(c) = args.first().and_then(|v| v.get("color")).and_then(parse_badge_color) {
                    let tid = arg_tab_id(args.first());
                    self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.badge_color = Some(c));
                    if let Some(snap) = self.action_snapshot(ext_id) {
                        effects.push(ExtEffect::ActionUpdated(snap));
                    }
                }
                Ok(Value::Null)
            }
            "action.getBadgeBackgroundColor" => {
                let tid = arg_tab_id(args.first());
                let c = self.actions.get(ext_id).and_then(|s| s.for_tab(tid).badge_color).unwrap_or_else(|| "rgba(0, 0, 0, 0)".into());
                Ok(Value::String(c))
            }
            "action.setBadgeTextColor" => {
                if let Some(c) = args.first().and_then(|v| v.get("color")).and_then(parse_badge_color) {
                    let tid = arg_tab_id(args.first());
                    self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.badge_text_color = Some(c));
                    if let Some(snap) = self.action_snapshot(ext_id) {
                        effects.push(ExtEffect::ActionUpdated(snap));
                    }
                }
                Ok(Value::Null)
            }
            "action.getBadgeTextColor" => {
                let tid = arg_tab_id(args.first());
                let c = self.actions.get(ext_id).and_then(|s| s.for_tab(tid).badge_text_color).unwrap_or_else(|| "#ffffff".into());
                Ok(Value::String(c))
            }
            "action.enable" => {
                let tid = args.first().and_then(Value::as_i64).map(|n| n as i32);
                self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.enabled = Some(true));
                if let Some(snap) = self.action_snapshot(ext_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                Ok(Value::Null)
            }
            "action.disable" => {
                let tid = args.first().and_then(Value::as_i64).map(|n| n as i32);
                self.actions.entry(ext_id.to_owned()).or_default().apply(tid, |a| a.enabled = Some(false));
                if let Some(snap) = self.action_snapshot(ext_id) {
                    effects.push(ExtEffect::ActionUpdated(snap));
                }
                Ok(Value::Null)
            }
            "action.isEnabled" => {
                let tid = args.first().and_then(Value::as_i64).map(|n| n as i32);
                Ok(Value::Bool(self.actions.get(ext_id).and_then(|s| s.for_tab(tid).enabled).unwrap_or(true)))
            }
            "action.getUserSettings" => Ok(json!({ "isOnToolbar": true })),
            "action.openPopup" => Err("A popup opens only when the user clicks the extension.".into()),
            _ => Err(format!("chrome.{name} is not available in this browser.")),
        }
    }

    pub(super) fn answer_tabs_query(
        &self,
        manifest: &ExtensionManifest,
        query: Option<&serde_json::Map<String, Value>>,
    ) -> Vec<Value> {
        let active_q = query.and_then(|q| q.get("active")).and_then(Value::as_bool);
        let highlighted_q = query.and_then(|q| q.get("highlighted")).and_then(Value::as_bool);
        let cur_win_q = query.and_then(|q| q.get("currentWindow")).and_then(Value::as_bool);
        let last_win_q = query.and_then(|q| q.get("lastFocusedWindow")).and_then(Value::as_bool);
        let win_id_q = query.and_then(|q| q.get("windowId")).and_then(Value::as_i64).map(|n| n as i32);
        let index_q = query.and_then(|q| q.get("index")).and_then(Value::as_u64);
        let title_q = query.and_then(|q| q.get("title")).and_then(Value::as_str);
        let status_q = query.and_then(|q| q.get("status")).and_then(Value::as_str);
        let url_patterns: Vec<String> = match query.and_then(|q| q.get("url")) {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(arr)) => arr.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => Vec::new(),
        };

        let mut out = Vec::new();
        for (idx, tab) in self.tabs.iter().enumerate() {
            let revealed = manifest.reveals_tab(&tab.url);
            if !revealed && (!url_patterns.is_empty() || title_q.is_some()) {
                continue;
            }
            if active_q.is_some_and(|a| a != tab.active) || highlighted_q.is_some_and(|h| h != tab.active) {
                continue;
            }
            if cur_win_q == Some(false) || last_win_q == Some(false) {
                continue;
            }
            if let Some(wid) = win_id_q
                && wid != WINDOW_ID
                && wid != WINDOW_ID_CURRENT
            {
                continue;
            }
            if index_q.is_some_and(|i| i as usize != idx) {
                continue;
            }
            if !url_patterns.is_empty() && !matches_host(&url_patterns, &tab.url) {
                continue;
            }
            if let Some(tp) = title_q
                && !glob_match(tp, &tab.title)
            {
                continue;
            }
            let status = if tab.loading { "loading" } else { "complete" };
            if status_q.is_some_and(|s| s != status) {
                continue;
            }
            out.push(self.tab_json(tab, manifest, false));
        }
        out
    }

    pub(super) fn tab_json(&self, tab: &ExtTab, manifest: &ExtensionManifest, force_reveal: bool) -> Value {
        let idx = self.tabs.iter().position(|t| t.engine_tab_id == tab.engine_tab_id).unwrap_or(0);
        let id = tab.chromium_tab_id.unwrap_or(tab.engine_tab_id as i32);
        let status = if tab.loading { "loading" } else { "complete" };
        let mut obj = json!({
            "id": id,
            "index": idx,
            "windowId": WINDOW_ID,
            "active": tab.active,
            "highlighted": tab.active,
            "selected": tab.active,
            "pinned": false,
            "incognito": false,
            "discarded": false,
            "autoDiscardable": true,
            "frozen": false,
            "groupId": -1,
            "status": status,
            "audible": tab.audible,
            "mutedInfo": { "muted": tab.muted },
            "width": tab.width,
            "height": tab.height,
        });
        let revealed = !is_private_url(&tab.url) && (force_reveal || manifest.reveals_tab(&tab.url));
        if revealed && let Some(m) = obj.as_object_mut() {
            m.insert("url".into(), Value::String(tab.url.clone()));
            m.insert("title".into(), Value::String(tab.title.clone()));
        }
        obj
    }

    pub(super) fn window_json(&self, manifest: &ExtensionManifest, populate: bool) -> Value {
        let mut obj = json!({
            "id": WINDOW_ID,
            "focused": true,
            "top": 0,
            "left": 0,
            "width": 1280,
            "height": 800,
            "incognito": false,
            "type": "normal",
            "state": "normal",
            "alwaysOnTop": false,
        });
        if populate && let Some(m) = obj.as_object_mut() {
            let tabs: Vec<Value> = self.tabs.iter().map(|t| self.tab_json(t, manifest, false)).collect();
            m.insert("tabs".into(), Value::Array(tabs));
        }
        obj
    }
}

fn arg_tab_id(arg: Option<&Value>) -> Option<i32> {
    arg?.get("tabId")?.as_i64().map(|n| n as i32)
}

fn parse_badge_color(val: &Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.len() > 32 {
            return None;
        }
        if let Some(hex) = trimmed.strip_prefix('#') {
            if matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Some(trimmed.to_owned());
            }
            return None;
        }
        let lower = trimmed.to_ascii_lowercase();
        if (lower.starts_with("rgb(") || lower.starts_with("rgba("))
            && lower.ends_with(')')
            && lower.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'(' | b')' | b',' | b'.' | b' ' | b'%'))
        {
            return Some(lower);
        }
        if lower.bytes().all(|b| b.is_ascii_lowercase()) {
            return Some(lower);
        }
        return None;
    }
    let arr = val.as_array()?;
    let r = arr.first().and_then(Value::as_u64).unwrap_or(0).min(255);
    let g = arr.get(1).and_then(Value::as_u64).unwrap_or(0).min(255);
    let b = arr.get(2).and_then(Value::as_u64).unwrap_or(0).min(255);
    let a = arr.get(3).and_then(Value::as_u64).unwrap_or(255).min(255);
    let alpha = ((a as f64 / 255.0) * 100.0).round() / 100.0;
    Some(format!("rgba({r}, {g}, {b}, {alpha})"))
}

fn resolve_action_icon(dir: &Path, details: &Value) -> Option<String> {
    if let Some(img_val) = details.get("imageData") {
        let chosen = if img_val.get("__imageData").and_then(Value::as_bool) == Some(true) {
            Some(img_val)
        } else if let Some(map) = img_val.as_object() {
            let mut sizes: Vec<(u32, &Value)> = map
                .iter()
                .filter_map(|(k, v)| Some((k.parse::<u32>().ok()?, v)))
                .collect();
            sizes.sort_by_key(|(sz, _)| *sz);
            sizes
                .iter()
                .find(|(sz, _)| *sz >= 32)
                .or_else(|| sizes.last())
                .map(|(_, v)| *v)
        } else {
            None
        };
        if let Some(px) = chosen
            && let (Some(w), Some(h), Some(b64)) = (
                px.get("width").and_then(Value::as_u64),
                px.get("height").and_then(Value::as_u64),
                px.get("rgba_b64").and_then(Value::as_str),
            )
            && w <= 128
            && h <= 128
            && b64.len() <= 96 * 1024
            && let Ok(rgba) = base64::engine::general_purpose::STANDARD.decode(b64)
        {
            return rgba_to_png_data_url(w as u32, h as u32, &rgba);
        }
    }
    if let Some(path_val) = details.get("path") {
        let rel = match path_val {
            Value::String(s) => Some(s.clone()),
            Value::Object(m) => {
                let map: HashMap<String, String> = m
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                    .collect();
                closest_icon(&map)
            }
            _ => None,
        };
        if let Some(r) = rel {
            return read_icon_data_url(dir, &r);
        }
    }
    None
}

fn resolve_extension_nav_url(ext_id: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.len() > MAX_NAV_URL_BYTES {
        return Err("URL exceeds maximum allowed length.".into());
    }
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
        return Ok("about:blank".into());
    }
    if browser_policy::is_file_scheme(trimmed) {
        return Err("Extensions do not have file access.".into());
    }
    if crate::extensions::manifest::is_extension_scheme(trimmed) {
        if crate::extensions::manifest::extension_id_from_url(trimmed) != Some(ext_id) {
            return Err("Extensions can open only their own extension pages.".into());
        }
        return Ok(trimmed.to_owned());
    }
    if trimmed.starts_with("//") || trimmed.starts_with("\\\\") {
        return Err(format!("This browser does not open {trimmed}."));
    }
    if !trimmed.contains("://") {
        // Reject opaque schemes (`javascript:`, `data:`, `blob:`, `vbscript:`, `about:`, ...).
        let before_delim = trimmed.split(['/', '?', '#']).next().unwrap_or("");
        if before_delim.contains(':') {
            return Err(format!("This browser does not open {trimmed}."));
        }
        return extension_page_url(ext_id, trimmed)
            .ok_or_else(|| format!("This browser does not open {trimmed}."));
    }
    if browser_policy::destination_origin(trimmed).is_some() {
        return Ok(trimmed.to_owned());
    }
    Err(format!("This browser does not open {trimmed}."))
}
