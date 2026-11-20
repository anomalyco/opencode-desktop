//! Per-tab frame tree, out-of-process iframe (OOPIF), extension exclusion, and main-world
//! `contextId` tracking for `experimental.browser` v4 (`chromium.ts:524, 990-1060, 2031-2085`).

use std::collections::{BTreeMap, HashMap, HashSet};

use browser_pane::policy::{extension_origin, extension_page_refusal};
use serde_json::{Value, json};

const MAX_TRACKED_FRAMES: usize = 256;

#[derive(Default)]
pub(super) struct TabFrameState {
    pub main_frame_id: Option<String>,
    pub frame_urls: BTreeMap<String, String>,
    pub frame_parents: HashMap<String, String>,
    pub frame_names: HashMap<String, String>,
    /// Flattened CDP `sessionId` for each out-of-process iframe (`frame_id -> session_id`).
    pub frame_sessions: HashMap<String, String>,
    /// Extension iframe targets (`session_id -> frame_id`), excluded from agent access.
    pub extension_sessions: HashMap<String, String>,
    /// Frame IDs showing extension documents (or descending from an extension frame).
    pub extension_frames: HashSet<String>,
    /// Default main-world execution context per frame (`frame_id -> (context_id, session_id)`).
    pub main_contexts: HashMap<String, (i64, Option<String>)>,
}

impl TabFrameState {
    /// Ingests one CDP event for this tab. Returns any newly attached non-extension OOPIF
    /// `sessionId`s on which the driver should enable `Page`, `Runtime`, and `Target.setAutoAttach`.
    pub fn ingest_cdp_event(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> Vec<String> {
        let mut attach_sessions = Vec::new();
        match method {
            "Target.attachedToTarget" => {
                let Some(info) = params.get("targetInfo") else { return attach_sessions };
                if info.get("type").and_then(Value::as_str) != Some("iframe") {
                    return attach_sessions;
                }
                let Some(child_sid) = params.get("sessionId").and_then(Value::as_str) else {
                    return attach_sessions;
                };
                let Some(frame_id) = info.get("targetId").and_then(Value::as_str) else {
                    return attach_sessions;
                };
                let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                let parent_id = info
                    .get("parentFrameId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| {
                        session_id.and_then(|psid| {
                            self.frame_sessions
                                .iter()
                                .find(|(_, s)| s.as_str() == psid)
                                .map(|(fid, _)| fid.clone())
                        })
                    })
                    .or_else(|| self.main_frame_id.clone());

                let parent_is_ext = session_id.is_some_and(|ps| self.extension_sessions.contains_key(ps))
                    || parent_id.as_deref().is_some_and(|p| self.extension_frames.contains(p));
                if extension_origin(url) || parent_is_ext {
                    self.extension_sessions.insert(child_sid.to_owned(), frame_id.to_owned());
                    self.extension_frames.insert(frame_id.to_owned());
                    self.frame_sessions.remove(frame_id);
                    self.main_contexts.remove(frame_id);
                    if !url.is_empty() && self.frame_urls.len() < MAX_TRACKED_FRAMES {
                        self.frame_urls.insert(frame_id.to_owned(), url.to_owned());
                    }
                    return attach_sessions;
                }
                if self.frame_urls.len() < MAX_TRACKED_FRAMES {
                    self.frame_urls.insert(frame_id.to_owned(), url.to_owned());
                    if let Some(pid) = parent_id {
                        self.frame_parents.insert(frame_id.to_owned(), pid);
                    }
                    self.frame_sessions.insert(frame_id.to_owned(), child_sid.to_owned());
                }
                attach_sessions.push(child_sid.to_owned());
            }
            "Target.targetInfoChanged" => {
                if let Some(info) = params.get("targetInfo")
                    && info.get("type").and_then(Value::as_str) == Some("iframe")
                    && let Some(fid) = info.get("targetId").and_then(Value::as_str)
                {
                    let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                    if !url.is_empty()
                        && (self.frame_urls.len() < MAX_TRACKED_FRAMES || self.frame_urls.contains_key(fid))
                    {
                        self.frame_urls.insert(fid.to_owned(), url.to_owned());
                    }
                    if extension_origin(url) {
                        self.extension_frames.insert(fid.to_owned());
                        self.main_contexts.remove(fid);
                        self.frame_sessions.remove(fid);
                    }
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(sid) = params.get("sessionId").and_then(Value::as_str) {
                    self.extension_sessions.remove(sid);
                    let gone_frames: Vec<String> = self
                        .frame_sessions
                        .iter()
                        .filter(|(_, s)| s.as_str() == sid)
                        .map(|(fid, _)| fid.clone())
                        .collect();
                    for fid in gone_frames {
                        self.frame_sessions.remove(&fid);
                        self.frame_urls.remove(&fid);
                        self.frame_parents.remove(&fid);
                        self.frame_names.remove(&fid);
                        self.main_contexts.remove(&fid);
                        self.extension_frames.remove(&fid);
                    }
                    self.main_contexts.retain(|_, (_, s)| s.as_deref() != Some(sid));
                }
            }
            "Page.frameNavigated" => {
                if let Some(frame) = params.get("frame")
                    && let Some(fid) = frame.get("id").and_then(Value::as_str)
                {
                    let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
                    let parent = frame.get("parentId").and_then(Value::as_str);
                    let name = frame.get("name").and_then(Value::as_str).unwrap_or("");
                    if parent.is_none() && session_id.is_none() {
                        // Top-level navigation clears stale child frame records from the prior document.
                        self.main_frame_id = Some(fid.to_owned());
                        self.frame_urls.retain(|k, _| k == fid || self.frame_sessions.contains_key(k));
                        self.frame_parents.retain(|k, _| self.frame_sessions.contains_key(k));
                        self.frame_names.retain(|k, _| k == fid || self.frame_sessions.contains_key(k));
                    }
                    if self.frame_urls.len() < MAX_TRACKED_FRAMES || self.frame_urls.contains_key(fid) {
                        self.frame_urls.insert(fid.to_owned(), url.to_owned());
                        if let Some(p) = parent {
                            self.frame_parents.insert(fid.to_owned(), p.to_owned());
                        }
                        if !name.is_empty() {
                            self.frame_names.insert(fid.to_owned(), name.to_owned());
                        }
                    }
                    let is_ext = extension_origin(url)
                        || session_id.is_some_and(|s| self.extension_sessions.contains_key(s))
                        || parent.is_some_and(|p| self.extension_frames.contains(p));
                    if is_ext {
                        self.extension_frames.insert(fid.to_owned());
                        self.main_contexts.remove(fid);
                    } else {
                        self.extension_frames.remove(fid);
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if let (Some(fid), Some(url)) = (
                    params.get("frameId").and_then(Value::as_str),
                    params.get("url").and_then(Value::as_str),
                ) && (self.frame_urls.len() < MAX_TRACKED_FRAMES || self.frame_urls.contains_key(fid))
                {
                    self.frame_urls.insert(fid.to_owned(), url.to_owned());
                }
            }
            "Page.frameDetached" => {
                // Ignore `"swap"` detachments: Chromium emits `reason: "swap"` on the parent
                // session when an iframe transitions into an out-of-process target (OOPIF).
                if params.get("reason").and_then(Value::as_str) == Some("swap") {
                    return attach_sessions;
                }
                if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
                    if session_id.is_none() && self.frame_sessions.contains_key(fid) {
                        return attach_sessions;
                    }
                    self.frame_urls.remove(fid);
                    self.frame_parents.remove(fid);
                    self.frame_names.remove(fid);
                    self.frame_sessions.remove(fid);
                    self.main_contexts.remove(fid);
                    self.extension_frames.remove(fid);
                }
            }
            "Runtime.executionContextCreated" => {
                if session_id.is_some_and(|s| self.extension_sessions.contains_key(s)) {
                    return attach_sessions;
                }
                if let Some(ctx) = params.get("context")
                    && let Some(cid) = ctx.get("id").and_then(Value::as_i64)
                    && let Some(aux) = ctx.get("auxData")
                    && aux.get("isDefault").and_then(Value::as_bool) == Some(true)
                    && aux.get("type").and_then(Value::as_str).unwrap_or("default") == "default"
                    && let Some(fid) = aux.get("frameId").and_then(Value::as_str)
                    && !self.extension_frames.contains(fid)
                {
                    let origin = ctx.get("origin").and_then(Value::as_str).unwrap_or("");
                    if extension_origin(origin) {
                        self.extension_frames.insert(fid.to_owned());
                        self.main_contexts.remove(fid);
                    } else {
                        self.main_contexts
                            .insert(fid.to_owned(), (cid, session_id.map(str::to_owned)));
                    }
                }
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(cid) = params.get("executionContextId").and_then(Value::as_i64) {
                    self.main_contexts
                        .retain(|_, (id, sid)| !(*id == cid && sid.as_deref() == session_id));
                }
            }
            "Runtime.executionContextsCleared" => {
                self.main_contexts.retain(|_, (_, sid)| sid.as_deref() != session_id);
            }
            _ => {}
        }
        attach_sessions
    }

    /// Merges a `Page.getFrameTree` response into tracked frames (preserving OOPIF children).
    pub fn ingest_frame_tree(&mut self, tree: &Value, session_id: Option<&str>) {
        let Some(root) = tree.get("frameTree") else { return };
        self.walk_tree(root, None, session_id, false);
    }

    fn walk_tree(
        &mut self,
        node: &Value,
        parent: Option<&str>,
        session_id: Option<&str>,
        parent_ext: bool,
    ) {
        let Some(frame) = node.get("frame") else { return };
        let Some(fid) = frame.get("id").and_then(Value::as_str) else { return };
        let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
        let name = frame.get("name").and_then(Value::as_str).unwrap_or("");
        let pid = parent.or_else(|| frame.get("parentId").and_then(Value::as_str));
        if pid.is_none() && session_id.is_none() {
            self.main_frame_id = Some(fid.to_owned());
        }
        let is_ext = parent_ext || extension_origin(url) || self.extension_frames.contains(fid);
        if self.frame_urls.len() < MAX_TRACKED_FRAMES || self.frame_urls.contains_key(fid) {
            self.frame_urls.insert(fid.to_owned(), url.to_owned());
            if let Some(p) = pid {
                self.frame_parents.insert(fid.to_owned(), p.to_owned());
            }
            if !name.is_empty() {
                self.frame_names.insert(fid.to_owned(), name.to_owned());
            }
        }
        if is_ext {
            self.extension_frames.insert(fid.to_owned());
            self.main_contexts.remove(fid);
        }
        if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
            for child in children {
                self.walk_tree(child, Some(fid), session_id, is_ext);
            }
        }
    }

    /// Returns all known document URLs in this tab (main document + all in-process & OOPIF
    /// subframes), sorted and deduplicated, for server authorization (`inspect_target` / `Target`).
    pub fn source_urls(&self, main_url: &str) -> Vec<String> {
        let mut urls: Vec<String> = Vec::with_capacity(self.frame_urls.len() + 1);
        if !main_url.is_empty() {
            urls.push(main_url.to_owned());
        }
        for url in self.frame_urls.values() {
            if !url.is_empty() && url != "about:blank" {
                urls.push(url.clone());
            }
        }
        urls.sort();
        urls.dedup();
        urls
    }

    /// Resolves an optional `frameID` into `(pinned_context_id, cdp_session_id)`, strictly
    /// refusing extension frames (and descendants) as well as unknown `frameID`s.
    pub fn resolve_eval_target(
        &self,
        frame_id: Option<&str>,
    ) -> Result<(Option<i64>, Option<String>), String> {
        let Some(fid) = frame_id else {
            let ctx = self
                .main_frame_id
                .as_deref()
                .and_then(|mid| self.main_contexts.get(mid))
                .map(|(cid, _)| *cid);
            return Ok((ctx, None));
        };
        if self.extension_frames.contains(fid) {
            let url = self.frame_urls.get(fid).map(String::as_str).unwrap_or("chrome-extension://");
            return Err(extension_page_refusal(url));
        }
        if !self.frame_urls.contains_key(fid) && self.main_frame_id.as_deref() != Some(fid) {
            return Err(
                "Frame ID was not found in this tab. Call browser.frames({tabID}) and pass a current frameID."
                    .into(),
            );
        }
        if let Some((cid, sid)) = self.main_contexts.get(fid) {
            return Ok((Some(*cid), sid.clone()));
        }
        Ok((None, self.frame_sessions.get(fid).cloned()))
    }

    /// Returns the agent-visible frame list (main frame + in-process children + cross-site OOPIFs),
    /// excluding extension frames and their descendants.
    pub fn agent_frames(&self, main_url: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let main_id = self.main_frame_id.as_deref();
        if let Some(mid) = main_id
            && !self.extension_frames.contains(mid)
        {
            out.push(json!({
                "id": mid,
                "url": main_url,
                "name": self.frame_names.get(mid).map(String::as_str).unwrap_or(""),
            }));
        }
        for (fid, url) in &self.frame_urls {
            if Some(fid.as_str()) == main_id || self.extension_frames.contains(fid) {
                continue;
            }
            if extension_origin(url) {
                continue;
            }
            let parent = self.frame_parents.get(fid).cloned().or_else(|| main_id.map(str::to_owned));
            let mut obj = json!({
                "id": fid,
                "url": url,
                "name": self.frame_names.get(fid).map(String::as_str).unwrap_or(""),
            });
            if let Some(pid) = parent {
                obj["parentID"] = json!(pid);
            }
            out.push(obj);
        }
        out
    }
}
