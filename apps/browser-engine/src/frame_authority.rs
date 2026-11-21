//! The engine's authority over a tab's frames and child targets for typed agent CDP. The engine
//! arms auto-attach itself, prepares and resumes every child target in the observer callback, and
//! decides every frame, session, context and object rule; the app keeps only a routing cache.
//! Official: `child-targets.ts:8-29`, `chromium.ts:617-637, 1106-1188, 1391-1398, 1612-1625,
//! 2167-2198`, `policy.ts:216-336`, `diagnostics.ts:103-134` (opencode `b9a60d9d`).

use std::collections::{HashMap, HashSet, VecDeque};

use browser_policy::extension_origin;
use serde_json::{Value, json};

/// Past this many tracked frames every frame is unavailable until the main frame commits again.
const MAX_FRAMES: usize = 1024;
/// Remote objects from the engine's own verified `DOM.resolveNode` replies.
const MAX_OBJECTS: usize = 512;
/// Engine-sent message ids start here, far above the app's ids (CEF refuses negative ids).
pub const INTERNAL_ID_BASE: i32 = 1_600_000_000;

const EXTENSION_FRAME: &str = "Agent browser tools cannot use extension frames: the frame shows an extension's document, which runs with that extension's privileges. Omit frameID to use the page around it, or use another frame from browser.frames({tabID}).";
const UNAVAILABLE_FRAME: &str = "Frame is unavailable. Call browser.frames({tabID}) and use a current frameID from this tab; omit frameID for the main frame.";
const UNPREPARED: &str = "This page's frames could not be prepared for agent browser tools. Reload the page and retry.";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Frame,
    Extension,
    Unavailable,
}

/// What a typed call's reply still needs once Chromium answers.
pub enum Post {
    None,
    /// The frame must still be usable when the reply arrives (`chromium.ts:1623-1625`).
    Frame(String),
    /// As `Frame`, and the returned `objectId` becomes usable on this session.
    Resolve(String),
    /// `DOM.getNodeForLocation`: the hit frame must be usable.
    Hit,
    /// `Page.getFrameTree`: ingest, then remove excluded subtrees before the app sees it.
    Tree,
}

/// How the engine handles a verified typed call.
pub enum Dispatch {
    Send(Post),
    /// `Page.enable` on root: replay already-attached web OOPIFs to a late-attaching app without
    /// letting the app drive `Target.setAutoAttach`.
    ReplayWebTargets { replay: Vec<(Value, Option<String>)> },
    /// `Runtime.enable` the engine already sent at arm time: disable first so Chromium reports
    /// the existing contexts again to the app.
    ReEnable,
}

/// What an agent call can act in (`EngineCdpLedger::reach`), checked against inherited grants
/// when it is sent and again when its reply comes back.
#[derive(Clone, Debug)]
pub enum Reach {
    Frames(Vec<String>),
    WholeTab,
}

enum Internal {
    Resume { session: String, worker: bool },
    Prepare { session: Option<String> },
    Tree { session: Option<String> },
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ArmState {
    #[default]
    Armed,
    Unprepared,
}

struct Frame {
    parent: Option<String>,
    session: Option<String>,
    url: Option<String>,
    origin: String,
    extension: bool,
}

struct WebTarget {
    target: String,
    parent_session: Option<String>,
    attach: Value,
    /// Held `false` while `waitForDebuggerOnStart` delivered an empty pre-commit `targetInfo.url`,
    /// so an extension iframe is never announced before its URL commits.
    announced: bool,
}

/// What the observer does with one event after the ledger saw it.
#[derive(Default)]
pub struct Observed {
    pub send: Vec<String>,
    pub forward: bool,
    /// Deferred `Target.attachedToTarget` events released once a clean web OOPIF commits its URL.
    pub announce: Vec<(Value, Option<String>)>,
    /// URL-free `(frameId, sessionId)` pairs that just left the agent's reach.
    pub excluded: Vec<(String, Option<String>)>,
    /// Newly attached non-extension OOPIF session awaiting `Runtime.runIfWaitingForDebugger`
    /// (last message in `send`), so per-tab gates can attach before the iframe executes script.
    pub new_web_session: Option<String>,
}

#[derive(Default)]
pub struct EngineCdpLedger {
    state: ArmState,
    frames: HashMap<String, Frame>,
    overflow: bool,
    main_frame: Option<String>,
    web_sessions: HashMap<String, WebTarget>,
    extension_targets: HashMap<String, String>,
    extension_contexts: HashSet<String>,
    contexts: HashMap<(i64, Option<String>), String>,
    /// Each frame's main-world context origin as V8 reported it, by frame id (`on_context`).
    context_origins: HashMap<String, String>,
    objects: HashMap<String, (Option<String>, String)>,
    object_order: VecDeque<String>,
    internal: HashMap<i32, Internal>,
    next_internal: u32,
    tree_retry: bool,
    /// Engine-sent messages queued inside an observer callback, where CEF forbids sending; the
    /// engine sends them right after the same `do_message_loop_work` turn.
    pub outbox: Vec<String>,
    /// The app has spoken CDP to this tab; events are forwarded only from then on.
    pub app_attached: bool,
    pub revision: u64,
}

impl EngineCdpLedger {
    /// Messages that arm a tab: auto-attach with every child paused at start, `Page` and `Runtime`
    /// on the root session, and the frame tree (`chromium.ts:1391-1398`, `child-targets.ts:8-19`).
    pub fn arm(&mut self) -> Vec<String> {
        self.state = ArmState::Armed;
        let prepare = Some(Internal::Prepare { session: None });
        let tree = Some(Internal::Tree { session: None });
        vec![
            self.msg("Target.setAutoAttach", auto_attach(true), None, prepare),
            self.msg("Page.enable", json!({}), None, None),
            self.msg("Runtime.enable", json!({}), None, None),
            self.msg("Page.getFrameTree", json!({}), None, tree),
        ]
    }

    pub fn has_excluded(&self) -> bool {
        !self.extension_targets.is_empty()
            || !self.extension_contexts.is_empty()
            || self.frames.values().any(|f| f.extension)
    }

    /// Returns currently attached web OOPIF session IDs so per-tab renderer gates (e.g.
    /// `Page.setInterceptFileChooserDialog`) can update every live child target.
    pub fn web_session_ids(&self) -> Vec<String> {
        self.web_sessions.keys().cloned().collect()
    }

    /// Every frame an agent call can act in, so a controlled `Default` tab refuses calls into a
    /// frame whose origin holds an inherited grant (READER-9 #411 C3): the app's ref frame, a
    /// `frameId` parameter, the frames of `contextId` / `executionContextId`, of `objectId` and of
    /// each `arguments[*].objectId`, and the session's own root. Input lands wherever the pointer
    /// or focus is, and a context or object the engine cannot place could be in any frame, so
    /// those reach the whole tab.
    ///
    /// Same-site frames share one renderer process and so one session, and node ids
    /// (`backendNodeId`, `nodeId`) are that process's, not a frame's: a call carrying one, or
    /// reading the renderer as a whole (`DOM.getDocument`, the heap and CPU profilers, tracing),
    /// reaches every frame of its session (READER-12 #451 critical 1).
    pub fn reach(&self, method: &str, session: Option<&str>, frame_id: Option<&str>, params: &Value) -> Reach {
        if method.starts_with("Input.") || params.get("uniqueContextId").is_some() {
            return Reach::WholeTab;
        }
        let sess = session.map(str::to_owned);
        let mut frames: Vec<String> = frame_id.map(str::to_owned).into_iter().collect();
        frames.extend(params.get("frameId").and_then(Value::as_str).map(str::to_owned));
        for key in ["contextId", "executionContextId"] {
            if let Some(cid) = params.get(key).and_then(Value::as_i64) {
                let Some(fid) = self.contexts.get(&(cid, sess.clone())) else { return Reach::WholeTab };
                frames.push(fid.clone());
            }
        }
        let args = params.get("arguments").and_then(Value::as_array).into_iter().flatten();
        let object_ids = params.get("objectId").into_iter().chain(args.filter_map(|a| a.get("objectId")));
        for oid in object_ids {
            let Some(oid) = oid.as_str() else { return Reach::WholeTab };
            // The object's own context decides, whatever frame the engine tagged it with.
            let Some(fid) = self.object_context_frame(oid, session) else { return Reach::WholeTab };
            frames.push(fid);
            frames.extend(self.objects.get(oid).map(|(_, f)| f.clone()));
        }
        let renderer_wide = ["DOM.getDocument", "DOM.getFlattenedDocument", "HeapProfiler.", "Profiler.", "Tracing."]
            .iter()
            .any(|m| method.starts_with(m));
        let node_ids = ["backendNodeId", "nodeId", "nodeIds", "backendNodeIds"].iter().any(|k| params.get(k).is_some());
        if renderer_wide || node_ids {
            frames.extend(self.frames.iter().filter(|(_, f)| f.session == sess).map(|(id, _)| id.clone()));
        }
        frames.extend(self.session_root(session));
        Reach::Frames(frames)
    }

    /// The frame of the context a V8 remote object id names (`<isolate>.<context>.<id>`).
    fn object_context_frame(&self, object_id: &str, session: Option<&str>) -> Option<String> {
        let cid = object_id.split('.').nth(1)?.parse::<i64>().ok()?;
        self.contexts.get(&(cid, session.map(str::to_owned))).cloned()
    }

    /// The first origin in `reach` that `granted` says holds an inherited grant (`Some(None)` when
    /// the engine cannot tell: the frame table overflowed, or a frame's origin cannot be worked
    /// out). An empty reach, or a frame the engine does not know, widens the check to the whole tab.
    pub fn granted_origin(&self, reach: &Reach, granted: &dyn Fn(&str) -> bool) -> Option<Option<String>> {
        if self.overflow {
            return Some(None);
        }
        let ids: Vec<&String> = match reach {
            Reach::Frames(ids) if !ids.is_empty() && ids.iter().all(|id| self.frames.contains_key(id)) => ids.iter().collect(),
            _ => self.frames.keys().collect(),
        };
        ids.into_iter().find_map(|id| match self.frame_origins(id) {
            None => Some(None),
            Some(origins) => origins.into_iter().find(|o| granted(o)).map(Some),
        })
    }

    /// The origins a frame's document can run as: the origin Chromium reports for it and for its
    /// main-world context, its own URL, and for a document that inherits its creator's origin
    /// (`about:blank`, `about:srcdoc`, or not yet committed) every ancestor's too, so a blank or
    /// `srcdoc` child of a granted frame is decided as the granted frame. `None` (unknown: refuse)
    /// when the walk meets a parent the engine does not know, runs in a cycle, or ends on a
    /// sub-frame with no origin at all (READER-12 #451 critical 2). A top-level document with none
    /// is opaque and holds no grant.
    fn frame_origins(&self, frame_id: &str) -> Option<Vec<String>> {
        let usable = |o: &str| !o.is_empty() && o != "null" && !o.starts_with("://");
        let mut out = Vec::new();
        let mut id = frame_id;
        for _ in 0..=self.frames.len() {
            let f = self.frames.get(id)?;
            out.extend([f.origin.as_str()].into_iter().chain(self.context_origins.get(id).map(String::as_str)).filter(|o| usable(o)).map(str::to_owned));
            match f.url.as_deref() {
                Some(u) if !inherits_origin(u) => {
                    out.push(u.to_owned());
                    return Some(out);
                }
                _ => {}
            }
            match &f.parent {
                Some(p) => id = p,
                None if out.is_empty() && id != frame_id => return None,
                None => return Some(out),
            }
        }
        None
    }

    fn msg(&mut self, method: &str, params: Value, session: Option<&str>, track: Option<Internal>) -> String {
        self.next_internal = (self.next_internal + 1) % 500_000_000;
        let id = INTERNAL_ID_BASE + self.next_internal as i32;
        if let Some(t) = track {
            self.internal.insert(id, t);
        }
        let mut m = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            m["sessionId"] = json!(s);
        }
        m.to_string()
    }

    /// Official's ancestor walk, bounded by the number of frames (`policy.ts:279-293`).
    pub fn access(&self, frame_id: &str) -> Access {
        if self.overflow {
            return Access::Unavailable;
        }
        let mut id = frame_id;
        for _ in 0..=self.frames.len() {
            if self.extension_contexts.contains(id) {
                return Access::Extension;
            }
            let Some(f) = self.frames.get(id) else { return Access::Unavailable };
            if f.extension {
                return Access::Extension;
            }
            if !f.url.as_deref().is_some_and(|u| input_document(u, &f.origin)) {
                return Access::Unavailable;
            }
            match &f.parent {
                None => return Access::Frame,
                Some(p) => id = p,
            }
        }
        Access::Unavailable
    }

    fn excluded(&self, frame_id: &str) -> bool {
        self.access(frame_id) == Access::Extension
    }

    fn session_root(&self, session: Option<&str>) -> Option<String> {
        match session {
            None => self.main_frame.clone(),
            Some(s) => self.web_sessions.get(s).map(|w| w.target.clone()),
        }
    }

    fn check_frame(&self, frame_id: &str, session: Option<&str>) -> Result<(), String> {
        if self.frames.get(frame_id).is_none_or(|f| f.session.as_deref() != session) && !self.excluded(frame_id) {
            return Err(UNAVAILABLE_FRAME.into());
        }
        match self.access(frame_id) {
            Access::Frame => Ok(()),
            Access::Extension => Err(EXTENSION_FRAME.into()),
            Access::Unavailable => Err(UNAVAILABLE_FRAME.into()),
        }
    }

    /// Verifies an OOPIF or frame target before `DOM.setFileInputFiles` (`UploadFiles`).
    pub fn check_upload_target(&self, session: Option<&str>, frame_id: Option<&str>) -> Result<(), String> {
        if let Some(sid) = session
            && !self.web_sessions.contains_key(sid)
        {
            return Err("UploadFiles sessionId is not an engine-verified web iframe session.".into());
        }
        let target_frame = frame_id.map(str::to_owned).or_else(|| self.session_root(session));
        if let Some(fid) = target_frame {
            self.check_frame(&fid, session)?;
        }
        Ok(())
    }

    fn post_check(&self, frame_id: &str) -> Result<(), String> {
        match self.access(frame_id) {
            Access::Frame => Ok(()),
            Access::Extension => Err(EXTENSION_FRAME.into()),
            Access::Unavailable => Err(UNAVAILABLE_FRAME.into()),
        }
    }

    /// Verifies one typed call. `frame_id` is the app's claim for a ref's frame; it must belong to
    /// the call's session. Without it, a node call names the session's own root frame.
    pub fn verify_typed_call(
        &mut self,
        method: &str,
        params: &mut Value,
        session: Option<&str>,
        frame_id: Option<&str>,
    ) -> Result<Dispatch, String> {
        self.app_attached = true;
        if self.state == ArmState::Unprepared {
            return Err(UNPREPARED.into());
        }
        if let Some(s) = session
            && !self.web_sessions.contains_key(s)
        {
            return Err("TypedCdpCall sessionId is not an engine-verified non-extension iframe session.".into());
        }
        let node_frame = || frame_id.map(str::to_owned).or_else(|| self.session_root(session));
        match method {
            "Target.setAutoAttach"
            | "Target.attachToTarget"
            | "Target.detachFromTarget"
            | "Target.setDiscoverTargets"
            | "Runtime.runIfWaitingForDebugger" => {
                Err(format!("DevTools method '{method}' is owned by the browser engine."))
            }
            "Page.enable" if session.is_none() => Ok(Dispatch::ReplayWebTargets {
                replay: self.ordered_web_targets(),
            }),
            "Runtime.enable" => Ok(Dispatch::ReEnable),
            "Accessibility.getFullAXTree" => {
                let fid = params
                    .get("frameId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(node_frame)
                    .ok_or(UNAVAILABLE_FRAME)?;
                self.check_frame(&fid, session)?;
                params["frameId"] = json!(fid);
                Ok(Dispatch::Send(Post::Frame(fid)))
            }
            "DOM.getFrameOwner" => {
                let fid = params.get("frameId").and_then(Value::as_str).ok_or(UNAVAILABLE_FRAME)?;
                self.post_check(fid)?;
                Ok(Dispatch::Send(Post::None))
            }
            "DOM.resolveNode"
            | "DOM.describeNode"
            | "DOM.getBoxModel"
            | "DOM.getContentQuads"
            | "DOM.scrollIntoViewIfNeeded"
            | "DOM.focus"
            | "DOM.querySelector"
            | "DOM.querySelectorAll"
            | "Accessibility.getPartialAXTree" => {
                let fid = if let Some(oid) = params.get("objectId").and_then(Value::as_str) {
                    let Some((osid, ofid)) = self.objects.get(oid).cloned() else {
                        return Err("TypedCdpCall objectId was not produced by an engine-verified DOM.resolveNode.".into());
                    };
                    if osid.as_deref() != session {
                        return Err("TypedCdpCall objectId belongs to another session.".into());
                    }
                    if frame_id.is_some_and(|claimed| claimed != ofid) {
                        return Err(UNAVAILABLE_FRAME.into());
                    }
                    ofid
                } else {
                    node_frame().ok_or(UNAVAILABLE_FRAME)?
                };
                self.check_frame(&fid, session)?;
                Ok(Dispatch::Send(if method == "DOM.resolveNode" { Post::Resolve(fid) } else { Post::Frame(fid) }))
            }
            "Runtime.callFunctionOn" | "Runtime.releaseObject" => {
                let oid = params.get("objectId").and_then(Value::as_str).unwrap_or("");
                let Some((osid, fid)) = self.objects.get(oid).cloned() else {
                    return Err("TypedCdpCall objectId was not produced by an engine-verified DOM.resolveNode.".into());
                };
                if osid.as_deref() != session {
                    return Err("TypedCdpCall objectId belongs to another session.".into());
                }
                if method == "Runtime.releaseObject" {
                    self.objects.remove(oid);
                    return Ok(Dispatch::Send(Post::None));
                }
                let foreign = params["arguments"].as_array().is_some_and(|args| {
                    args.iter().filter_map(|a| a.get("objectId").and_then(Value::as_str)).any(|a| {
                        self.objects.get(a).is_none_or(|(s, f)| s.as_deref() != session || self.post_check(f).is_err())
                    })
                });
                if foreign {
                    return Err("TypedCdpCall argument objectId was not produced by an engine-verified DOM.resolveNode.".into());
                }
                self.post_check(&fid)?;
                Ok(Dispatch::Send(Post::Frame(fid)))
            }
            "Runtime.evaluate" => {
                let Some(cid) = params.get("contextId").and_then(Value::as_i64) else {
                    return Err("TypedCdpCall Runtime.evaluate requires a verified integer contextId.".into());
                };
                let Some(fid) = self.contexts.get(&(cid, session.map(str::to_owned))).cloned() else {
                    return Err("TypedCdpCall contextId is not an engine-verified main-world execution context.".into());
                };
                self.post_check(&fid)?;
                Ok(Dispatch::Send(Post::Frame(fid)))
            }
            "DOM.getNodeForLocation" => Ok(Dispatch::Send(Post::Hit)),
            "Page.getFrameTree" => Ok(Dispatch::Send(Post::Tree)),
            _ => Ok(Dispatch::Send(Post::None)),
        }
    }

    /// Checks a successful typed reply again and rewrites what the app may see.
    pub fn finish_typed(&mut self, post: &Post, session: Option<&str>, result: &mut Value) -> Result<(), String> {
        match post {
            Post::None => Ok(()),
            Post::Frame(fid) => self.post_check(fid),
            Post::Resolve(fid) => {
                let oid = result.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str);
                if let Err(err) = self.post_check(fid) {
                    if let Some(oid) = oid {
                        let release = self.msg("Runtime.releaseObject", json!({ "objectId": oid }), session, None);
                        self.outbox.push(release);
                    }
                    return Err(err);
                }
                if let Some(oid) = oid {
                    if self.objects.len() >= MAX_OBJECTS
                        && let Some(old) = self.object_order.pop_front()
                    {
                        self.objects.remove(&old);
                    }
                    // Tagged with the frame of the context the object really lives in: a node id is
                    // the renderer's, so the resolved node can be in another same-process frame.
                    let tag = self.object_context_frame(oid, session).unwrap_or_else(|| fid.clone());
                    self.objects.insert(oid.to_owned(), (session.map(str::to_owned), tag));
                    self.object_order.push_back(oid.to_owned());
                }
                Ok(())
            }
            Post::Hit => match result.get("frameId").and_then(Value::as_str) {
                Some(fid) => self.post_check(fid),
                None => Err(UNAVAILABLE_FRAME.into()),
            },
            Post::Tree => {
                self.ingest_tree(result, session);
                if let Some(root) = result.get_mut("frameTree") {
                    self.prune(root);
                }
                Ok(())
            }
        }
    }

    fn prune(&self, tree: &mut Value) {
        if let Some(children) = tree.get_mut("childFrames").and_then(Value::as_array_mut) {
            children.retain(|c| {
                c.get("frame").and_then(|f| f.get("id")).and_then(Value::as_str).is_some_and(|id| !self.excluded(id))
            });
            for c in children {
                self.prune(c);
            }
        }
    }

    fn ingest_tree(&mut self, result: &Value, session: Option<&str>) {
        let Some(root) = result.get("frameTree") else { return };
        let all_excluded = session.is_some_and(|s| self.extension_targets.contains_key(s));
        let keep_parent = root
            .get("frame")
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .and_then(|id| self.frames.get(id))
            .and_then(|f| f.parent.clone());
        self.frames.retain(|_, f| f.session.as_deref() != session);
        let mut stack = vec![(root, keep_parent)];
        while let Some((node, parent)) = stack.pop() {
            let Some(frame) = node.get("frame") else { continue };
            let Some(id) = frame.get("id").and_then(Value::as_str) else { continue };
            let parent = frame.get("parentId").and_then(Value::as_str).map(str::to_owned).or(parent);
            if session.is_none() && parent.is_none() {
                self.main_frame = Some(id.to_owned());
            }
            let url = frame.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).map(str::to_owned);
            let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("").to_owned();
            self.upsert(id, parent, session, url, origin, all_excluded);
            if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
                stack.extend(children.iter().map(|c| (c, Some(id.to_owned()))));
            }
        }
    }

    fn upsert(&mut self, id: &str, parent: Option<String>, session: Option<&str>, url: Option<String>, origin: String, force_ext: bool) {
        let extension = force_ext || url.as_deref().is_some_and(extension_origin) || extension_origin(&origin);
        if let Some(f) = self.frames.get_mut(id) {
            if parent.is_some() {
                f.parent = parent;
            }
            if url.is_some() {
                f.url = url;
                f.origin = origin;
            }
            f.extension |= extension;
            return;
        }
        if self.frames.len() >= MAX_FRAMES {
            self.overflow = true;
            return;
        }
        self.frames.insert(id.to_owned(), Frame { parent, session: session.map(str::to_owned), url, origin, extension });
    }

    /// Moves a web OOPIF whose document became an extension's into `extension_targets` (out of
    /// every agent path, diagnostics disabled, while staying attached so nested iframes inside it
    /// are still auto-attached and excluded, matching `chromium.ts:1146-1157`).
    fn exclude_web_session(&mut self, sid: &str, obs: &mut Observed) {
        if let Some(w) = self.web_sessions.remove(sid) {
            self.drop_session_state(sid);
            if let Some(f) = self.frames.get_mut(&w.target) {
                f.extension = true;
            }
            self.extension_targets.insert(sid.to_owned(), w.target.clone());
            obs.excluded.push((w.target, w.announced.then(|| sid.to_owned())));
            obs.send.push(self.msg("Runtime.disable", json!({}), Some(sid), None));
            obs.send.push(self.msg("Page.disable", json!({}), Some(sid), None));
            let tree = Some(Internal::Tree { session: Some(sid.to_owned()) });
            obs.send.push(self.msg("Page.getFrameTree", json!({}), Some(sid), tree));
        }
    }

    fn announce_web_session(&mut self, sid: &str, url: &str, obs: &mut Observed) {
        if url.is_empty() || extension_origin(url) {
            return;
        }
        if let Some(w) = self.web_sessions.get_mut(sid) {
            if let Some(info) = w.attach.get_mut("targetInfo").and_then(Value::as_object_mut) {
                info.insert("url".into(), json!(url));
            }
            if !w.announced {
                w.announced = true;
                obs.announce.push((w.attach.clone(), w.parent_session.clone()));
            }
        }
    }

    fn drop_session_state(&mut self, sid: &str) {
        self.contexts.retain(|(_, s), _| s.as_deref() != Some(sid));
        self.objects.retain(|_, (s, _)| s.as_deref() != Some(sid));
    }

    /// Updates the ledger from one event and returns what to send and whether to forward it. The
    /// resume of a paused child goes out here, before any filter, with no app round trip.
    pub fn observe_event(&mut self, method: &str, params: &Value, session: Option<&str>) -> Observed {
        let mut obs = Observed::default();
        let from_extension = session.is_some_and(|s| self.extension_targets.contains_key(s));
        let known = session.is_none_or(|s| self.web_sessions.get(s).is_some_and(|w| w.announced));
        match method {
            "Target.attachedToTarget" => {
                self.on_attached(params, session, &mut obs);
                return obs;
            }
            "Target.detachedFromTarget" => {
                if let Some(sid) = params.get("sessionId").and_then(Value::as_str) {
                    if self.web_sessions.remove(sid).is_some_and(|w| w.announced) {
                        obs.forward = true;
                    }
                    self.extension_targets.remove(sid);
                    self.frames.retain(|_, f| f.session.as_deref() != Some(sid));
                    self.drop_session_state(sid);
                }
                return obs;
            }
            "Target.targetInfoChanged" => {
                let info = &params["targetInfo"];
                let target = info.get("targetId").and_then(Value::as_str).unwrap_or("");
                let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                let sid = self.web_sessions.iter().find(|(_, w)| w.target == target).map(|(s, _)| s.clone());
                if let Some(sid) = sid {
                    if extension_origin(url) {
                        self.exclude_web_session(&sid, &mut obs);
                    } else {
                        self.announce_web_session(&sid, url, &mut obs);
                        obs.forward = self.web_sessions.get(&sid).is_some_and(|w| w.announced);
                    }
                }
                return obs;
            }
            "Page.frameAttached" => {
                if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                    let parent = params.get("parentFrameId").and_then(Value::as_str).map(str::to_owned);
                    self.upsert(id, parent, session, None, String::new(), from_extension);
                }
            }
            "Page.frameNavigated" => {
                if self.on_navigated(&params["frame"], session, from_extension, &mut obs) {
                    return obs;
                }
            }
            "Page.navigatedWithinDocument" => {
                if let (Some(id), Some(url)) = (params["frameId"].as_str(), params["url"].as_str())
                    && let Some(f) = self.frames.get_mut(id)
                {
                    f.url = Some(url.to_owned());
                    f.extension |= extension_origin(url);
                }
            }
            "Page.frameDetached" => {
                if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                    self.revision += 1;
                    // A swap moves the frame to another process; keep what is known (`chromium.ts:632-637`).
                    if params.get("reason").and_then(Value::as_str) != Some("swap") {
                        self.extension_contexts.remove(id);
                        if self.frames.get(id).is_some_and(|f| f.session.as_deref() == session) {
                            self.frames.remove(id);
                        }
                        self.contexts.retain(|_, f| f != id);
                    }
                }
            }
            "Runtime.executionContextCreated" => {
                self.on_context(&params["context"], session, from_extension, &mut obs);
                return obs;
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(cid) = params.get("executionContextId").and_then(Value::as_i64) {
                    self.contexts.remove(&(cid, session.map(str::to_owned)));
                }
            }
            "Runtime.executionContextsCleared" => {
                self.contexts.retain(|(_, s), _| s.as_deref() != session);
                self.objects.retain(|_, (s, _)| s.as_deref() != session);
            }
            "Runtime.consoleAPICalled" => {
                let cid = params.get("executionContextId").and_then(Value::as_i64);
                obs.forward = known && !from_extension && self.page_context(cid, session) && !extension_stack(&params["stackTrace"]);
                return obs;
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let cid = details.get("executionContextId").and_then(Value::as_i64);
                obs.forward = known
                    && !from_extension
                    && self.page_context(cid, session)
                    && !details.get("url").and_then(Value::as_str).is_some_and(extension_origin)
                    && !extension_stack(&details["stackTrace"]);
                return obs;
            }
            "Log.entryAdded" => {
                let entry = &params["entry"];
                obs.forward = known
                    && !from_extension
                    && !entry.get("url").and_then(Value::as_str).is_some_and(extension_origin)
                    && !extension_stack(&entry["stackTrace"]);
                return obs;
            }
            _ if method.starts_with("Network.") => {
                let urls = [
                    &params["request"]["url"],
                    &params["response"]["url"],
                    &params["redirectResponse"]["url"],
                    &params["documentURL"],
                    &params["initiator"]["url"],
                ];
                obs.forward = known
                    && !from_extension
                    && !urls.iter().any(|u| u.as_str().is_some_and(extension_origin))
                    && !extension_stack(&params["initiator"]["stack"]);
                return obs;
            }
            _ => {}
        }
        obs.forward = known && !from_extension;
        obs
    }

    fn page_context(&self, cid: Option<i64>, session: Option<&str>) -> bool {
        cid.and_then(|c| self.contexts.get(&(c, session.map(str::to_owned))))
            .is_some_and(|fid| !self.excluded(fid))
    }

    fn on_attached(&mut self, params: &Value, parent: Option<&str>, obs: &mut Observed) {
        let info = &params["targetInfo"];
        let Some(child) = params.get("sessionId").and_then(Value::as_str) else { return };
        let ty = info.get("type").and_then(Value::as_str).unwrap_or("");
        let target = info.get("targetId").and_then(Value::as_str).unwrap_or("").to_owned();
        let url = info.get("url").and_then(Value::as_str).unwrap_or("");
        if ty != "iframe" {
            // A worker opens no chooser and asks for no device: let it run and leave its session
            // (`child-targets.ts:22-29`). Its errors never matter.
            let worker = Some(Internal::Resume { session: child.to_owned(), worker: true });
            let resume = self.msg("Runtime.runIfWaitingForDebugger", json!({}), Some(child), worker);
            let detach = self.msg("Target.detachFromTarget", json!({ "sessionId": child }), None, None);
            obs.send.extend([resume, detach]);
            return;
        }
        let parent_frame = info
            .get("parentFrameId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| self.session_root(parent));
        let extension = extension_origin(url)
            || parent.is_some_and(|p| self.extension_targets.contains_key(p))
            || parent_frame.as_deref().is_some_and(|p| self.excluded(p));
        let page_url = (!url.is_empty()).then(|| url.to_owned());
        self.frames.remove(&target);
        self.upsert(&target, parent_frame, Some(child), page_url, String::new(), extension);
        self.contexts.retain(|_, f| *f != target);
        let prepare = Some(Internal::Prepare { session: Some(child.to_owned()) });
        obs.send.push(self.msg("Target.setAutoAttach", auto_attach(true), Some(child), prepare));
        if extension {
            // Stays attached and paused-free, out of every agent path (`chromium.ts:1146-1157`).
            // Fetch its frame tree too so same-process child frames under it are marked excluded.
            self.extension_targets.insert(child.to_owned(), target.clone());
            let tree = Some(Internal::Tree { session: Some(child.to_owned()) });
            obs.send.push(self.msg("Page.getFrameTree", json!({}), Some(child), tree));
            obs.excluded.push((target, None));
        } else {
            let announced = !url.is_empty();
            self.web_sessions.insert(
                child.to_owned(),
                WebTarget { target, parent_session: parent.map(str::to_owned), attach: params.clone(), announced },
            );
            obs.send.push(self.msg("Page.enable", json!({}), Some(child), None));
            obs.send.push(self.msg("Runtime.enable", json!({}), Some(child), None));
            let tree = Some(Internal::Tree { session: Some(child.to_owned()) });
            obs.send.push(self.msg("Page.getFrameTree", json!({}), Some(child), tree));
            obs.new_web_session = Some(child.to_owned());
            obs.forward = announced;
        }
        let resume = Some(Internal::Resume { session: child.to_owned(), worker: false });
        obs.send.push(self.msg("Runtime.runIfWaitingForDebugger", json!({}), Some(child), resume));
    }

    /// Returns `true` when the frame is excluded, so the event goes no further.
    fn on_navigated(&mut self, frame: &Value, session: Option<&str>, from_extension: bool, obs: &mut Observed) -> bool {
        let Some(id) = frame.get("id").and_then(Value::as_str) else { return false };
        let parent = frame.get("parentId").and_then(Value::as_str).map(str::to_owned);
        self.revision += 1;
        if session.is_none() && parent.is_none() {
            // The main frame committed: a fresh document, so drop stale root-owned frames and
            // root-owned extension contexts; re-arm if needed, refetch tree if the first fetch failed.
            self.extension_contexts.retain(|fid| self.frames.get(fid).is_some_and(|f| f.session.is_some()));
            self.frames.retain(|_, f| f.session.is_some());
            self.overflow = false;
            self.main_frame = Some(id.to_owned());
            if self.state == ArmState::Unprepared {
                obs.send.extend(self.arm());
            } else if std::mem::take(&mut self.tree_retry) {
                let tree = Some(Internal::Tree { session: None });
                obs.send.push(self.msg("Page.getFrameTree", json!({}), None, tree));
            }
        }
        let url = frame.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).map(str::to_owned);
        let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("").to_owned();
        let was_excluded = self.excluded(id);
        self.upsert(id, parent, session, url.clone(), origin, from_extension);
        if self.excluded(id) && !was_excluded {
            match session.filter(|s| self.web_sessions.get(*s).is_some_and(|w| w.target == id)) {
                Some(sid) => {
                    let sid = sid.to_owned();
                    self.exclude_web_session(&sid, obs);
                }
                None => obs.excluded.push((id.to_owned(), session.map(str::to_owned))),
            }
        } else if !self.excluded(id)
            && let Some(u) = url.as_deref()
            && let Some(sid) = session.filter(|s| self.web_sessions.get(*s).is_some_and(|w| w.target == id))
        {
            self.announce_web_session(sid, u, obs);
        }
        self.excluded(id)
    }

    fn on_context(&mut self, ctx: &Value, session: Option<&str>, from_extension: bool, obs: &mut Observed) {
        let aux = &ctx["auxData"];
        let (Some(cid), Some(fid)) = (ctx.get("id").and_then(Value::as_i64), aux.get("frameId").and_then(Value::as_str)) else {
            return;
        };
        if from_extension || aux.get("isDefault").and_then(Value::as_bool) != Some(true) {
            return;
        }
        if extension_origin(ctx.get("origin").and_then(Value::as_str).unwrap_or("")) {
            // A frame that becomes an extension's document is no target (`chromium.ts:1112-1126`).
            self.contexts.retain(|_, f| f != fid);
            self.extension_contexts.insert(fid.to_owned());
            match self.web_sessions.iter().find(|(_, w)| w.target == fid).map(|(s, _)| s.clone()) {
                Some(sid) => self.exclude_web_session(&sid, obs),
                None => obs.excluded.push((fid.to_owned(), session.map(str::to_owned))),
            }
            return;
        }
        self.extension_contexts.remove(fid);
        self.contexts.insert((cid, session.map(str::to_owned)), fid.to_owned());
        // The document's own origin, which a blank frame's `Page` events may not carry yet.
        if self.context_origins.len() >= 2 * MAX_FRAMES {
            let frames = &self.frames;
            self.context_origins.retain(|id, _| frames.contains_key(id));
        }
        match ctx.get("origin").and_then(Value::as_str).filter(|o| !o.is_empty() && *o != "null" && !o.starts_with("://")) {
            Some(origin) => self.context_origins.insert(fid.to_owned(), origin.to_owned()),
            None => self.context_origins.remove(fid),
        };
        obs.forward = session.is_none_or(|s| self.web_sessions.get(s).is_some_and(|w| w.announced)) && !self.excluded(fid);
    }

    /// Handles the reply to an engine-sent message. Returns `None` when the id isn't the engine's.
    /// A worker's errors, and errors for a target that is already gone, never matter; any other
    /// failure to prepare a live target refuses typed calls until the main frame commits again.
    pub fn on_internal_reply(&mut self, id: i32, ok: bool, body: Option<&Value>) -> Option<Vec<String>> {
        if id < INTERNAL_ID_BASE {
            return None;
        }
        let Some(kind) = self.internal.remove(&id) else { return Some(Vec::new()) };
        let message = body
            .and_then(|b| b.get("message").or_else(|| b.get("error").and_then(|e| e.get("message"))))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let gone = ["no session", "session with given id not found", "no target", "target closed", "not attached"]
            .iter()
            .any(|m| message.contains(m));
        let live = |s: &Option<String>| {
            s.as_deref().is_none_or(|s| self.web_sessions.contains_key(s) || self.extension_targets.contains_key(s))
        };
        match kind {
            Internal::Tree { session } => {
                if ok && let Some(b) = body {
                    self.ingest_tree(b, session.as_deref());
                } else if session.is_none() {
                    self.tree_retry = true;
                }
                Some(Vec::new())
            }
            Internal::Resume { worker: true, .. } => Some(Vec::new()),
            Internal::Resume { session, worker: false } if !ok && !gone && live(&Some(session.clone())) => Some(self.unprepare()),
            Internal::Prepare { session } if !ok && !gone && live(&session) => Some(self.unprepare()),
            _ => Some(Vec::new()),
        }
    }

    /// Disarms the tab (Chromium detaches the auto-attached sessions and releases their start
    /// throttles) and resumes every known child; re-armed on the next main-frame commit.
    fn unprepare(&mut self) -> Vec<String> {
        self.state = ArmState::Unprepared;
        self.internal.clear();
        let sessions: Vec<String> = self.web_sessions.keys().chain(self.extension_targets.keys()).cloned().collect();
        let mut out: Vec<String> = sessions
            .iter()
            .map(|s| self.msg("Runtime.runIfWaitingForDebugger", json!({}), Some(s), None))
            .collect();
        out.push(self.msg("Target.setAutoAttach", auto_attach(false), None, None));
        self.objects.clear();
        self.object_order.clear();
        out
    }

    /// An internal `Runtime.disable` on `session`, sent before the app's own `Runtime.enable`.
    pub fn runtime_disable(&mut self, session: Option<&str>) -> String {
        self.msg("Runtime.disable", json!({}), session, None)
    }

    /// Returns announced web OOPIFs ordered parents-before-children so nested clean OOPIFs attach
    /// under an already-known parent session in `TabFrameState`.
    fn ordered_web_targets(&self) -> Vec<(Value, Option<String>)> {
        let mut emitted: HashSet<&str> = HashSet::new();
        let mut out: Vec<(Value, Option<String>)> = Vec::with_capacity(self.web_sessions.len());
        loop {
            let before = out.len();
            for (sid, w) in &self.web_sessions {
                if !w.announced || emitted.contains(sid.as_str()) {
                    continue;
                }
                let parent_ready = w.parent_session.as_deref().is_none_or(|p| emitted.contains(p));
                if parent_ready {
                    emitted.insert(sid.as_str());
                    out.push((w.attach.clone(), w.parent_session.clone()));
                }
            }
            if out.len() == before {
                break;
            }
        }
        for (sid, w) in &self.web_sessions {
            if w.announced && emitted.insert(sid.as_str()) {
                out.push((w.attach.clone(), w.parent_session.clone()));
            }
        }
        out
    }
}

fn auto_attach(on: bool) -> Value {
    json!({
        "autoAttach": on,
        "waitForDebuggerOnStart": on,
        "flatten": true,
        "filter": [{ "type": "iframe", "exclude": false }, { "type": "worker", "exclude": false }, { "exclude": true }],
    })
}

/// Documents that run as the origin of the frame that created them (HTML "about:blank" and
/// `srcdoc` origin inheritance), plus a frame with nothing committed yet.
fn inherits_origin(url: &str) -> bool {
    let l = url.trim().to_ascii_lowercase();
    l.is_empty()
        || ["about:blank", "about:srcdoc"]
            .iter()
            .any(|p| l.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with(['?', '#'])))
}

/// Documents that may receive agent input (`policy.ts:216-222`), without `file://` (`#303` bans it).
fn input_document(url: &str, origin: &str) -> bool {
    if extension_origin(origin) || extension_origin(url) {
        return false;
    }
    let l = url.to_ascii_lowercase();
    ["http://", "https://", "oc-preview://", "data:", "blob:http://", "blob:https://", "blob:null/", "blob:oc-preview://"]
        .iter()
        .any(|p| l.starts_with(p))
        || ["about:blank", "about:srcdoc"]
            .iter()
            .any(|p| l.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with(['?', '#'])))
}

/// Direct or async caller attributed to an extension (`diagnostics-machine.ts:344-349`).
/// Walks `stack["parent"]` up to 64 hops, failing closed if a deeper/cyclic chain appears.
fn extension_stack(stack: &Value) -> bool {
    let mut cur = stack;
    for _ in 0..64 {
        if cur
            .get("callFrames")
            .and_then(Value::as_array)
            .is_some_and(|frames| frames.iter().any(|f| f.get("url").and_then(Value::as_str).is_some_and(extension_origin)))
        {
            return true;
        }
        let Some(parent) = cur.get("parent").filter(|p| p.is_object()) else {
            return false;
        };
        cur = parent;
    }
    true
}
