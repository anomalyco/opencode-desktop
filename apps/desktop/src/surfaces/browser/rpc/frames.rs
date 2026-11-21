//! Per-tab frame tree, out-of-process iframe (OOPIF), extension exclusion, and main-world
//! `contextId` tracking for `experimental.browser` v4 (`chromium.ts:524, 990-1060, 2031-2085`).

use std::collections::{BTreeMap, HashMap, HashSet};

use browser_pane::policy::{extension_origin, extension_page_refusal};
use serde_json::{Value, json};

pub const MAX_TRACKED_FRAMES: usize = 256;

#[derive(Default)]
pub(super) struct FrameEventEffects {
    /// Newly attached non-extension OOPIF `sessionId`s to bootstrap (`Page`/`Runtime`/`Target`).
    pub enable_sessions: Vec<String>,
    /// Newly detected extension OOPIF `sessionId`s to immediately detach via `Target.detachFromTarget`.
    pub detach_sessions: Vec<String>,
    /// True when the top-level main frame navigated or reloaded (signals a generation bump).
    pub main_navigated: bool,
}

pub(super) enum EvalTargetError {
    DocumentChanged(String),
    Failed(String),
}

#[derive(Default)]
pub(super) struct TabFrameState {
    pub main_frame_id: Option<String>,
    /// Last observed `loaderId` of the top-level main frame so same-URL reloads bump `generation`
    /// even when `#274` drops `Page.*` events while extensions are active.
    pub main_loader_id: Option<String>,
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
    /// Targets (`None` = top-level, `Some(sid)` = OOPIF) whose current live frame set exceeded
    /// [`MAX_TRACKED_FRAMES`]; recomputed on every `ingest_frame_tree` rebuild so navigating away
    /// from a 257-frame page recovers cleanly instead of permanently bricking the tab.
    overflow_targets: HashSet<Option<String>>,
    /// Fail-closed flag (`!overflow_targets.is_empty()` or live overflow during event ingestion).
    pub frame_cap_exceeded: bool,
}

impl TabFrameState {
    fn record_frame_url(&mut self, fid: &str, url: &str, target: Option<&str>) -> bool {
        if let Some(existing) = self.frame_urls.get_mut(fid) {
            // Never overwrite a committed URL with Chromium's empty `""` cross-process stub
            // from an ancestor target's `Page.getFrameTree` (real blank frames report `"about:blank"`).
            if !url.is_empty() || existing.is_empty() {
                *existing = url.to_owned();
            }
            return true;
        }
        if self.frame_urls.len() >= MAX_TRACKED_FRAMES {
            self.overflow_targets.insert(target.map(str::to_owned));
            self.frame_cap_exceeded = true;
            return false;
        }
        self.frame_urls.insert(fid.to_owned(), url.to_owned());
        true
    }

    /// Ingests one CDP event for this tab and returns required driver effects.
    pub fn ingest_cdp_event(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> FrameEventEffects {
        let mut eff = FrameEventEffects::default();
        match method {
            // The engine took this frame out of the agent's reach (no URL is sent): drop it, its
            // descendants, its session and its contexts from this routing cache.
            "Rune.frameExcluded" => {
                if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
                    let mut drop_ids = vec![fid.to_owned()];
                    let mut i = 0;
                    while i < drop_ids.len() {
                        let parent = drop_ids[i].clone();
                        for (child, p) in &self.frame_parents {
                            if p == &parent && !drop_ids.contains(child) {
                                drop_ids.push(child.clone());
                            }
                        }
                        i += 1;
                    }
                    for id in drop_ids {
                        self.extension_frames.insert(id.clone());
                        self.frame_urls.remove(&id);
                        self.frame_parents.remove(&id);
                        self.frame_names.remove(&id);
                        self.main_contexts.remove(&id);
                        if let Some(sid) = self.frame_sessions.remove(&id) {
                            self.extension_sessions.insert(sid.clone(), id);
                            self.main_contexts.retain(|_, (_, s)| s.as_deref() != Some(sid.as_str()));
                        }
                    }
                    if let Some(sid) = params.get("sessionId").and_then(Value::as_str) {
                        self.extension_sessions.insert(sid.to_owned(), fid.to_owned());
                        self.frame_sessions.retain(|_, s| s != sid);
                        self.main_contexts.retain(|_, (_, s)| s.as_deref() != Some(sid));
                    }
                }
            }
            "Target.attachedToTarget" => {
                let Some(info) = params.get("targetInfo") else { return eff };
                if info.get("type").and_then(Value::as_str) != Some("iframe") {
                    return eff;
                }
                let Some(child_sid) = params.get("sessionId").and_then(Value::as_str) else {
                    return eff;
                };
                let Some(frame_id) = info.get("targetId").and_then(Value::as_str) else {
                    return eff;
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
                    if !url.is_empty() {
                        self.record_frame_url(frame_id, url, Some(child_sid));
                    }
                    eff.detach_sessions.push(child_sid.to_owned());
                    return eff;
                }
                if self.record_frame_url(frame_id, url, Some(child_sid)) {
                    if let Some(pid) = parent_id {
                        self.frame_parents.insert(frame_id.to_owned(), pid);
                    }
                    self.frame_sessions.insert(frame_id.to_owned(), child_sid.to_owned());
                }
                eff.enable_sessions.push(child_sid.to_owned());
            }
            "Target.targetInfoChanged" => {
                if let Some(info) = params.get("targetInfo")
                    && info.get("type").and_then(Value::as_str) == Some("iframe")
                    && let Some(fid) = info.get("targetId").and_then(Value::as_str)
                {
                    let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                    let owner = self.frame_sessions.get(fid).cloned();
                    if !url.is_empty() {
                        self.record_frame_url(fid, url, owner.as_deref());
                    }
                    if extension_origin(url) {
                        self.extension_frames.insert(fid.to_owned());
                        self.main_contexts.remove(fid);
                        // Move sessionId into `extension_sessions` AND detach so child frames
                        // spawned inside this extension OOPIF cannot fall back to `main_frame_id`.
                        if let Some(sid) = self.frame_sessions.remove(fid) {
                            self.extension_sessions.insert(sid.clone(), fid.to_owned());
                            eff.detach_sessions.push(sid);
                        }
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
                    self.overflow_targets.remove(&Some(sid.to_owned()));
                    self.frame_cap_exceeded = !self.overflow_targets.is_empty();
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
                        eff.main_navigated = true;
                        self.overflow_targets.remove(&None);
                        self.frame_cap_exceeded = !self.overflow_targets.is_empty();
                        self.main_frame_id = Some(fid.to_owned());
                        self.main_loader_id = frame
                            .get("loaderId")
                            .and_then(Value::as_str)
                            .filter(|l| !l.is_empty())
                            .map(str::to_owned);
                        self.frame_urls.retain(|k, _| k == fid || self.frame_sessions.contains_key(k));
                        self.frame_parents.retain(|k, _| self.frame_sessions.contains_key(k));
                        self.frame_names.retain(|k, _| k == fid || self.frame_sessions.contains_key(k));
                    }
                    if self.record_frame_url(fid, url, session_id) {
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
                        if let Some(sid) = self.frame_sessions.remove(fid) {
                            self.extension_sessions.insert(sid.clone(), fid.to_owned());
                            eff.detach_sessions.push(sid);
                        }
                    } else {
                        self.extension_frames.remove(fid);
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if let (Some(fid), Some(url)) = (
                    params.get("frameId").and_then(Value::as_str),
                    params.get("url").and_then(Value::as_str),
                ) {
                    self.record_frame_url(fid, url, session_id);
                }
            }
            "Page.frameDetached" => {
                // Ignore `"swap"` detachments: Chromium emits `reason: "swap"` on the parent
                // session when an iframe transitions into an out-of-process target (OOPIF).
                if params.get("reason").and_then(Value::as_str) == Some("swap") {
                    return eff;
                }
                if let Some(fid) = params.get("frameId").and_then(Value::as_str) {
                    if session_id.is_none() && self.frame_sessions.contains_key(fid) {
                        return eff;
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
                    return eff;
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
                        if let Some(sid) = self.frame_sessions.remove(fid) {
                            self.extension_sessions.insert(sid.clone(), fid.to_owned());
                            eff.detach_sessions.push(sid);
                        }
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
                if session_id.is_none() && self.main_frame_id.is_some() {
                    eff.main_navigated = true;
                    self.main_loader_id = None;
                }
                self.main_contexts.retain(|_, (_, sid)| sid.as_deref() != session_id);
            }
            _ => {}
        }
        eff
    }

    /// Rebuilds the tracked frame set for `session_id` (`None` = top-level in-process frames,
    /// `Some(sid)` = OOPIF `sid`'s frames) from a live `Page.getFrameTree` reply: prunes removed
    /// frames, recomputes `frame_cap_exceeded` from the current live tree (preventing permanent
    /// bricking or stale denied-URL poisoning when `#274` drops `Page.*` events), and returns
    /// `true` when the top-level main frame's `loaderId` changed (same-URL reload).
    pub fn ingest_frame_tree(&mut self, tree: &Value, session_id: Option<&str>) -> bool {
        let Some(root) = tree.get("frameTree") else { return false };
        let mut walked: Vec<(String, String, Option<String>, String, Option<String>, bool)> = Vec::new();
        collect_frame_tree(root, None, false, &mut walked);
        if walked.is_empty() {
            return false;
        }
        let mut main_reloaded = false;
        if session_id.is_none() {
            let (root_fid, _, _, _, root_loader, _) = &walked[0];
            if let Some(next_loader) = root_loader.as_deref().filter(|l| !l.is_empty()) {
                if self.main_loader_id.as_deref().is_some_and(|prev| prev != next_loader) {
                    main_reloaded = true;
                }
                self.main_loader_id = Some(next_loader.to_owned());
            }
            self.main_frame_id = Some(root_fid.clone());
        }

        // Prune frames owned by `session_id` that disappeared from the live tree while preserving
        // frames belonging to other CDP sessions (OOPIFs when `session_id == None`, or top-level
        // and sibling OOPIFs when `session_id == Some(sid)`).
        let live_ids: HashSet<&str> = walked.iter().map(|(fid, ..)| fid.as_str()).collect();
        let stale: Vec<String> = self
            .frame_urls
            .keys()
            .filter(|fid| {
                let owned = match session_id {
                    None => !self.frame_sessions.contains_key(*fid),
                    Some(sid) => self.frame_sessions.get(*fid).map(String::as_str) == Some(sid),
                };
                owned && !live_ids.contains(fid.as_str())
            })
            .cloned()
            .collect();
        for fid in stale {
            self.frame_urls.remove(&fid);
            self.frame_parents.remove(&fid);
            self.frame_names.remove(&fid);
            self.frame_sessions.remove(&fid);
            self.main_contexts.remove(&fid);
            self.extension_frames.remove(&fid);
        }

        let target_key = session_id.map(str::to_owned);
        self.overflow_targets.remove(&target_key);
        let mut target_overflowed = false;
        for (fid, url, pid, name, _, walked_ext) in walked {
            // Skip OOPIF stubs inside the top-level tree whose session is already owned by a child
            // OOPIF target, except preserve parent/name links and never clobber committed URLs.
            let owned_by_other_oopif = session_id.is_none() && self.frame_sessions.contains_key(&fid);
            let is_ext = walked_ext || self.extension_frames.contains(&fid);
            if !self.record_frame_url(&fid, &url, session_id) {
                target_overflowed = true;
            } else {
                if let Some(p) = pid {
                    self.frame_parents.insert(fid.clone(), p);
                }
                if !name.is_empty() {
                    self.frame_names.insert(fid.clone(), name);
                }
                if !is_ext
                    && !owned_by_other_oopif
                    && let Some(sid) = session_id
                {
                    self.frame_sessions.insert(fid.clone(), sid.to_owned());
                }
            }
            if is_ext {
                self.extension_frames.insert(fid.clone());
                self.frame_sessions.remove(&fid);
                self.main_contexts.remove(&fid);
            }
        }
        if target_overflowed {
            self.overflow_targets.insert(target_key);
        }
        self.frame_cap_exceeded = !self.overflow_targets.is_empty();
        main_reloaded
    }

    /// Returns unique active non-extension OOPIF CDP `sessionId`s tracked on this tab.
    pub fn oopif_sessions(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for sid in self.frame_sessions.values() {
            if !self.extension_sessions.contains_key(sid) && seen.insert(sid.as_str()) {
                out.push(sid.clone());
            }
        }
        out
    }

    /// Returns all known document URLs in this tab (main document + all in-process & OOPIF
    /// subframes, including `about:blank`), sorted and deduplicated. Fails closed when the tab
    /// overflowed [`MAX_TRACKED_FRAMES`] so a 257th iframe cannot evade server authorization.
    pub fn source_urls(&self, main_url: &str) -> Result<Vec<String>, String> {
        if self.frame_cap_exceeded {
            return Err(format!(
                "Tab exceeded {MAX_TRACKED_FRAMES} tracked frames; cannot verify authorization targets safely."
            ));
        }
        let mut urls: Vec<String> = Vec::with_capacity(self.frame_urls.len() + 1);
        if !main_url.is_empty() {
            urls.push(main_url.to_owned());
        }
        for url in self.frame_urls.values() {
            if !url.is_empty() {
                urls.push(url.clone());
            }
        }
        urls.sort();
        urls.dedup();
        Ok(urls)
    }

    /// Resolves an optional `frameID` into `(pinned_context_id, cdp_session_id)`.
    /// When `require_pinned` is `true` (`engine_agent_tools`), fails closed with
    /// [`EvalTargetError::DocumentChanged`] whenever the main-world `contextId` is not known
    /// (never silently falling back to an unpinned `Runtime.evaluate`).
    pub fn resolve_eval_target(
        &self,
        frame_id: Option<&str>,
        require_pinned: bool,
    ) -> Result<(Option<i64>, Option<String>), EvalTargetError> {
        let target_fid = match frame_id {
            Some(fid) => {
                if self.extension_frames.contains(fid) {
                    let url = self
                        .frame_urls
                        .get(fid)
                        .map(String::as_str)
                        .unwrap_or("chrome-extension://");
                    return Err(EvalTargetError::Failed(extension_page_refusal(url)));
                }
                if !self.frame_urls.contains_key(fid) && self.main_frame_id.as_deref() != Some(fid) {
                    return Err(EvalTargetError::Failed(
                        "Frame ID was not found in this tab. Call browser.frames({tabID}) and pass a current frameID."
                            .into(),
                    ));
                }
                fid
            }
            None => match self.main_frame_id.as_deref() {
                Some(mid) => mid,
                None if require_pinned => {
                    return Err(EvalTargetError::DocumentChanged(
                        "Main frame execution context is unavailable yet. Retry after the document settles."
                            .into(),
                    ));
                }
                None => return Ok((None, None)),
            },
        };
        if let Some((cid, sid)) = self.main_contexts.get(target_fid) {
            return Ok((Some(*cid), sid.clone()));
        }
        if require_pinned {
            return Err(EvalTargetError::DocumentChanged(
                "Frame execution context is unavailable or navigated. Refresh state and retry."
                    .into(),
            ));
        }
        if frame_id.is_some() {
            return Err(EvalTargetError::Failed(
                "Frame context is unavailable on this browser engine. Call browser.frames({tabID}) or omit frameID."
                    .into(),
            ));
        }
        Ok((None, None))
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

fn collect_frame_tree(
    node: &Value,
    parent: Option<&str>,
    parent_ext: bool,
    out: &mut Vec<(String, String, Option<String>, String, Option<String>, bool)>,
) {
    let Some(frame) = node.get("frame") else { return };
    let Some(fid) = frame.get("id").and_then(Value::as_str) else { return };
    let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
    let name = frame.get("name").and_then(Value::as_str).unwrap_or("");
    let loader = frame.get("loaderId").and_then(Value::as_str).map(str::to_owned);
    let pid = parent
        .or_else(|| frame.get("parentId").and_then(Value::as_str))
        .map(str::to_owned);
    let is_ext = parent_ext || extension_origin(url);
    out.push((fid.to_owned(), url.to_owned(), pid, name.to_owned(), loader, is_ext));
    if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
        for child in children {
            collect_frame_tree(child, Some(fid), is_ext, out);
        }
    }
}
