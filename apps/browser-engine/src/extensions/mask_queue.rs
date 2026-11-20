//! Per-tab serialized read & node-guard state machine for PR 1b-2 (`review-rd274.md` #311 & #314).
//!
//! Guarantees:
//! - Whole-tab extension surfaces (`ExtSurfaceKind::WholeTab`) refuse every CDP call.
//! - While ANY pre-commit extension sub-frame is pending (`ctx.has_pending()`), ALL guarded read
//!   and node operations fail closed (`review-rd274.md` #311 Finding 2).
//! - On web tabs with committed extension sub-frames (`ExtSurfaceKind::Subframes`):
//!   - `Input.*` and `Page.captureScreenshot` are refused unconditionally (PR 1b-4 handles
//!     screenshots; `Input.*` stays refused under Option A).
//!   - All tree/DOM/AX reads and node/owner calls run single-flight in FIFO order (capped at 64,
//!     with an engine-side active-op timeout so awaited promises cannot wedge the queue), strip
//!     closed ShadowRoots, `pseudoElements`, AX descendants, and extension frames without leaking
//!     `backendNodeId`s, and redact literal and encoded extension-scheme URLs.
//! - On `Clean` web tabs with extensions active (`ctx.extensions_active == true`), ALL
//!   `Accessibility.*`, `DOM.getDocument`, `DOM.describeNode`, `DOM.querySelector*`, and node-guard
//!   calls STILL run through `ReadFilterOp`/`NodeGuardOp` (`review-rd274.md` #314 Finding 1).
//! - Strict single-target parameter validation rejects multi-identifier combos
//!   (`{ backendNodeId, nodeId }`) and JSON floats (`12.0`) before dispatch (#314 Finding 2).
//! - Epoch-scoped protected-set caching (`Rc<CachedMaskState>`, keyed on `dom_epoch` bumped by
//!   Blink `DOM.*` & `Page.*` events plus CEF frame lifecycle) computes the pierced DOM/frame set
//!   once per layout/DOM epoch and reuses it across `browser.snapshot` and `querySelectorAll` +
//!   `getBoxModel` loops, verifying `dom_epoch` again on reply via Mojo FIFO ordering.

#![forbid(unsafe_code)]

use std::collections::{HashSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::frame_guard::{
    DomTarget, ExtSurfaceKind, FrameTreeAnalysis, ProtectedDomNodes, ReadFilterKind,
    is_node_guard_method, is_refused_on_subframes, parse_get_document_depth,
    parse_required_node_id, parse_single_dom_target, read_filter_for_method, redact_ext_strings,
    sanitize_dom_params,
};
use super::mask_ops::{
    NodeGuardOp, NodeStage, ReadFilterOp, ReadStage, step_node_guard, step_read_filter,
};

const MAX_QUEUED_CALLS: usize = 64;
pub(super) const MAX_QUERY_VERIFY_NODES: usize = 32;
/// Engine-side timeout on any active queued op so an unresolved `Runtime.evaluate({ awaitPromise })`
/// cannot wedge `TabMaskQueue` (`review-rd274.md` #314 Finding 4).
const ACTIVE_OP_TIMEOUT: Duration = Duration::from_secs(10);

const WHOLE_TAB_REFUSAL: &str =
    "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";
pub(super) const SUBFRAME_REFUSAL: &str =
    "Agent browser tools cannot read or target browser extension frames.";
const SUBFRAME_INPUT_SHOT_REFUSAL: &str =
    "Input and screenshot DevTools calls are not permitted on tabs with browser extension frames.";
const TIMEOUT_REFUSAL: &str =
    "DevTools call timed out while extension frame masking was active.";

#[derive(Clone, Debug)]
pub struct QueuedCdpCall {
    pub request_id: u64,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug)]
pub struct MaskReply {
    pub tab_id: u64,
    pub job_id: u64,
    pub ok: bool,
    pub result: Value,
}

#[derive(Clone, Debug)]
pub enum MaskEffect {
    SendInternal {
        job_id: u64,
        method: String,
        params: Value,
    },
    ReleaseObject {
        object_id: String,
    },
    Respond {
        request_id: u64,
        ok: bool,
        result: Value,
    },
}

/// Live tab snapshot passed into each state-machine transition.
pub struct TabMaskCtx {
    pub surface: ExtSurfaceKind,
    pub extensions_active: bool,
    pub dom_epoch: u64,
}

impl TabMaskCtx {
    pub(super) fn has_pending(&self) -> bool {
        matches!(&self.surface, ExtSurfaceKind::Subframes(s) if s.has_pending)
    }

    pub(super) fn cef_ext_count(&self) -> usize {
        match &self.surface {
            ExtSurfaceKind::Subframes(s) => s.cef_ext_count,
            _ => 0,
        }
    }

    pub(super) fn known_cdp_ext(&self) -> HashSet<String> {
        match &self.surface {
            ExtSurfaceKind::Subframes(s) => s.cdp_ext_frames.clone(),
            _ => HashSet::new(),
        }
    }
}

pub(super) struct CachedMaskState {
    pub(super) dom_epoch: u64,
    pub(super) cef_ext_count: usize,
    pub(super) cdp_ext_frames: HashSet<String>,
    pub(super) tree: FrameTreeAnalysis,
    pub(super) protected: ProtectedDomNodes,
    pub(super) root_node_id: Option<i64>,
    pub(super) stripped_doc: Option<Value>,
}

pub struct TabMaskQueue {
    next_job_id: u64,
    dom_events_enabled: bool,
    cache: Option<Rc<CachedMaskState>>,
    active: Option<ActiveOp>,
    queued: VecDeque<QueuedCdpCall>,
}

impl Default for TabMaskQueue {
    fn default() -> Self {
        Self {
            next_job_id: 1,
            dom_events_enabled: false,
            cache: None,
            active: None,
            queued: VecDeque::new(),
        }
    }
}

impl TabMaskQueue {
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }

    pub fn invalidate_cache(&mut self) {
        self.dom_events_enabled = false;
        self.cache = None;
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.active.as_ref().map(|op| op.started_at + ACTIVE_OP_TIMEOUT)
    }

    pub fn fail_all(&mut self, msg: &str) -> Vec<MaskEffect> {
        self.dom_events_enabled = false;
        self.cache = None;
        let mut out = Vec::new();
        if let Some(op) = self.active.take() {
            out.push(refuse(op.request_id, msg));
        }
        while let Some(q) = self.queued.pop_front() {
            out.push(refuse(q.request_id, msg));
        }
        out
    }

    pub fn expire_timed_out(&mut self, now: Instant, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        let timed_out = self
            .active
            .as_ref()
            .is_some_and(|op| now.saturating_duration_since(op.started_at) >= ACTIVE_OP_TIMEOUT);
        if !timed_out {
            return Vec::new();
        }
        let Some(op) = self.active.take() else {
            return Vec::new();
        };
        self.cache = None;
        let mut effects = vec![refuse(op.request_id, TIMEOUT_REFUSAL)];
        self.pump_next(ctx, &mut effects);
        effects
    }

    pub fn enqueue(&mut self, call: QueuedCdpCall, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        if self.active.is_none() {
            return self.start_call(call, ctx);
        }
        if self.queued.len() >= MAX_QUEUED_CALLS {
            return vec![refuse(
                call.request_id,
                "Too many queued DevTools calls while extension frame masking is active.",
            )];
        }
        self.queued.push_back(call);
        Vec::new()
    }

    pub fn on_reply(&mut self, job_id: u64, ok: bool, result: Value, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        let Some(mut op) = self.active.take() else {
            return release_late_object_if_any(&result);
        };
        if op.job_id != job_id {
            self.active = Some(op);
            return release_late_object_if_any(&result);
        }
        let mut effects = Vec::new();
        match self.step_op(&mut op, ok, result, ctx, &mut effects) {
            StepResult::Continue => {
                self.active = Some(op);
            }
            StepResult::RetryFresh(call) => {
                self.cache = None;
                let mut retry_eff = self.start_retry_fresh(call, ctx);
                effects.append(&mut retry_eff);
                if self.active.is_none() {
                    self.pump_next(ctx, &mut effects);
                }
            }
            StepResult::Done => {
                self.pump_next(ctx, &mut effects);
            }
        }
        effects
    }

    fn pump_next(&mut self, ctx: &TabMaskCtx, effects: &mut Vec<MaskEffect>) {
        while self.active.is_none() {
            let Some(next) = self.queued.pop_front() else {
                break;
            };
            let mut next_eff = self.start_call(next, ctx);
            effects.append(&mut next_eff);
        }
    }

    fn alloc_job_id(&mut self) -> u64 {
        let id = self.next_job_id;
        self.next_job_id = self.next_job_id.wrapping_add(1).max(1);
        id
    }

    fn cache_if_valid(&self, ctx: &TabMaskCtx) -> Option<Rc<CachedMaskState>> {
        let c = self.cache.as_ref()?;
        if self.dom_events_enabled
            && !ctx.has_pending()
            && c.protected.clean_cache_complete
            && c.dom_epoch == ctx.dom_epoch
            && c.cef_ext_count == ctx.cef_ext_count()
            && c.cdp_ext_frames == ctx.known_cdp_ext()
        {
            Some(Rc::clone(c))
        } else {
            None
        }
    }

    fn start_retry_fresh(&mut self, call: QueuedCdpCall, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        if ctx.surface.is_whole_tab() {
            return vec![refuse(call.request_id, WHOLE_TAB_REFUSAL)];
        }
        if ctx.has_pending() {
            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
        }
        if is_node_guard_method(&call.method) {
            return self.start_node_guard(call, ctx, true, true);
        }
        if let Some(filter) = read_filter_for_method(&call.method) {
            return self.start_read_filter(call, filter, ctx, true, true);
        }
        vec![refuse(call.request_id, SUBFRAME_REFUSAL)]
    }

    fn start_call(&mut self, call: QueuedCdpCall, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        if ctx.surface.is_whole_tab() {
            return vec![refuse(call.request_id, WHOLE_TAB_REFUSAL)];
        }
        if ctx.has_pending() {
            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
        }

        if ctx.surface.is_clean() {
            if ctx.extensions_active {
                if is_node_guard_method(&call.method) {
                    return self.start_node_guard(call, ctx, false, false);
                }
                if let Some(filter) = read_filter_for_method(&call.method)
                    && matches!(
                        filter,
                        ReadFilterKind::AxTree
                            | ReadFilterKind::Dom
                            | ReadFilterKind::QuerySelector
                            | ReadFilterKind::QuerySelectorAll
                    )
                {
                    return self.start_read_filter(call, filter, ctx, false, false);
                }
            }
            let job_id = self.alloc_job_id();
            let redact_ext =
                ctx.extensions_active && read_filter_for_method(&call.method).is_some();
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                started_at: Instant::now(),
                kind: OpKind::CleanPass {
                    method: call.method.clone(),
                    redact_ext,
                },
            });
            return vec![MaskEffect::SendInternal {
                job_id,
                method: call.method,
                params: call.params,
            }];
        }

        if is_refused_on_subframes(&call.method) {
            return vec![refuse(call.request_id, SUBFRAME_INPUT_SHOT_REFUSAL)];
        }

        if is_node_guard_method(&call.method) {
            return self.start_node_guard(call, ctx, false, false);
        }
        if let Some(filter) = read_filter_for_method(&call.method) {
            return self.start_read_filter(call, filter, ctx, false, false);
        }

        let job_id = self.alloc_job_id();
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
            started_at: Instant::now(),
            kind: OpKind::SerializedPass,
        });
        vec![MaskEffect::SendInternal {
            job_id,
            method: call.method,
            params: call.params,
        }]
    }

    fn start_read_filter(
        &mut self,
        call: QueuedCdpCall,
        filter: ReadFilterKind,
        ctx: &TabMaskCtx,
        bypass_cache: bool,
        retried: bool,
    ) -> Vec<MaskEffect> {
        let mut get_doc_depth = 1i64;
        let target = match filter {
            ReadFilterKind::Dom if call.method == "DOM.getDocument" => {
                match parse_get_document_depth(&call.params) {
                    Ok(d) => get_doc_depth = d,
                    Err(msg) => return vec![refuse(call.request_id, msg)],
                }
                DomTarget::None
            }
            ReadFilterKind::Dom => match parse_single_dom_target(&call.params, false) {
                Ok(t) => t,
                Err(msg) => return vec![refuse(call.request_id, msg)],
            },
            ReadFilterKind::QuerySelector | ReadFilterKind::QuerySelectorAll => {
                match parse_required_node_id(&call.params) {
                    Ok(nid) => DomTarget::NodeId(nid),
                    Err(msg) => return vec![refuse(call.request_id, msg)],
                }
            }
            ReadFilterKind::AxTree | ReadFilterKind::FrameTree => {
                if let Some(v) = call.params.get("frameId")
                    && !v.is_null()
                    && v.as_str().is_none_or(str::is_empty)
                {
                    return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                }
                DomTarget::None
            }
        };
        let sanitized = sanitize_dom_params(&call.params);

        // Attempt O(1) epoch-cache hit for read methods.
        if !bypass_cache && let Some(cached) = self.cache_if_valid(ctx) {
            if call.method == "DOM.getDocument" {
                if let (Some(root_nid), Some(_)) = (cached.root_node_id, cached.stripped_doc.as_ref()) {
                    // Use a lightweight 1-node `DOM.describeNode` as a Mojo FIFO barrier to prove
                    // no `DOM.*` mutation occurred before answering from `cached.stripped_doc`
                    // without resetting Blink's bound `nodeId` table.
                    let job_id = self.alloc_job_id();
                    self.active = Some(ActiveOp {
                        job_id,
                        request_id: call.request_id,
                        started_at: Instant::now(),
                        kind: OpKind::ReadFilter(Box::new(ReadFilterOp {
                            method: call.method,
                            params: sanitized,
                            filter,
                            target,
                            get_doc_depth,
                            baseline_cef_ext: ctx.cef_ext_count(),
                            start_dom_epoch: ctx.dom_epoch,
                            build_start_epoch: ctx.dom_epoch,
                            used_cache: true,
                            retried,
                            resolved_target_bid: None,
                            stage: ReadStage::CachedGetDocFence,
                            raw_ft: Value::Null,
                            raw_doc: None,
                            building_tree: FrameTreeAnalysis::default(),
                            building_protected: ProtectedDomNodes::default(),
                            state: Some(cached),
                            query_verified: Vec::new(),
                            query_unverified: Vec::new(),
                            query_is_all: false,
                        })),
                    });
                    return vec![send(
                        job_id,
                        "DOM.describeNode",
                        json!({ "nodeId": root_nid, "depth": 0 }),
                    )];
                }
            } else {
                if let Some(fid) = sanitized.get("frameId").and_then(Value::as_str) {
                    if cached.tree.protected_frame_ids.contains(fid) {
                        return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                    }
                    if !cached.tree.clean_frame_ids.contains(fid) {
                        return self.start_read_filter(call, filter, ctx, true, retried);
                    }
                }
                match &target {
                    DomTarget::BackendNodeId(bid) => {
                        if cached.protected.backend_ids.contains(bid) {
                            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                        }
                        if !cached.protected.clean_backend_ids.contains(bid) {
                            return self.start_read_filter(call, filter, ctx, true, retried);
                        }
                    }
                    DomTarget::NodeId(nid) => {
                        if cached.protected.node_ids.contains(nid) {
                            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                        }
                        if !cached.protected.clean_node_ids.contains(nid) {
                            return self.start_read_filter(call, filter, ctx, true, retried);
                        }
                    }
                    DomTarget::ObjectId(_) => {
                        return self.start_read_filter(call, filter, ctx, true, retried);
                    }
                    DomTarget::None => {}
                }
                let job_id = self.alloc_job_id();
                let method = call.method.clone();
                let params_send = sanitized.clone();
                self.active = Some(ActiveOp {
                    job_id,
                    request_id: call.request_id,
                    started_at: Instant::now(),
                    kind: OpKind::ReadFilter(Box::new(ReadFilterOp {
                        method: call.method,
                        params: sanitized,
                        filter,
                        target,
                        get_doc_depth,
                        baseline_cef_ext: ctx.cef_ext_count(),
                        start_dom_epoch: ctx.dom_epoch,
                        build_start_epoch: ctx.dom_epoch,
                        used_cache: true,
                        retried,
                        resolved_target_bid: None,
                        stage: ReadStage::Exec,
                        raw_ft: Value::Null,
                        raw_doc: None,
                        building_tree: FrameTreeAnalysis::default(),
                        building_protected: ProtectedDomNodes::default(),
                        state: Some(cached),
                        query_verified: Vec::new(),
                        query_unverified: Vec::new(),
                        query_is_all: false,
                    })),
                });
                return vec![send(job_id, &method, params_send)];
            }
        }

        let job_id = self.alloc_job_id();
        let (stage, first_cmd) = if !self.dom_events_enabled {
            (ReadStage::EnableDom, send(job_id, "DOM.enable", json!({})))
        } else if let DomTarget::NodeId(nid) = target {
            // Resolve caller's `nodeId` -> permanent `backendNodeId` BEFORE internal
            // `DOM.getDocument({ pierce: true })` renumbers Blink's `nodeId` table.
            (
                ReadStage::ResolveNodeBeforeDoc,
                send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })),
            )
        } else {
            (ReadStage::FrameTree, send(job_id, "Page.getFrameTree", json!({})))
        };
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
            started_at: Instant::now(),
            kind: OpKind::ReadFilter(Box::new(ReadFilterOp {
                method: call.method,
                params: sanitized,
                filter,
                target,
                get_doc_depth,
                baseline_cef_ext: ctx.cef_ext_count(),
                start_dom_epoch: ctx.dom_epoch,
                build_start_epoch: ctx.dom_epoch,
                used_cache: false,
                retried,
                resolved_target_bid: None,
                stage,
                raw_ft: Value::Null,
                raw_doc: None,
                building_tree: FrameTreeAnalysis::default(),
                building_protected: ProtectedDomNodes::default(),
                state: None,
                query_verified: Vec::new(),
                query_unverified: Vec::new(),
                query_is_all: false,
            })),
        });
        vec![first_cmd]
    }

    fn start_node_guard(
        &mut self,
        call: QueuedCdpCall,
        ctx: &TabMaskCtx,
        bypass_cache: bool,
        retried: bool,
    ) -> Vec<MaskEffect> {
        let is_frame_owner = call.method == "DOM.getFrameOwner";
        let target = if is_frame_owner {
            if !matches!(parse_single_dom_target(&call.params, false), Ok(DomTarget::None)) {
                return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
            }
            let fid = call.params.get("frameId").and_then(Value::as_str).unwrap_or("");
            if fid.is_empty() {
                return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
            }
            DomTarget::None
        } else {
            match parse_single_dom_target(&call.params, true) {
                Ok(t) => t,
                Err(msg) => return vec![refuse(call.request_id, msg)],
            }
        };
        let sanitized = sanitize_dom_params(&call.params);

        // Side-effecting `DOM.focus` / `DOM.scrollIntoViewIfNeeded` always run a fresh build.
        let cache_eligible = !bypass_cache
            && !matches!(
                call.method.as_str(),
                "DOM.focus" | "DOM.scrollIntoViewIfNeeded"
            );
        if cache_eligible && let Some(cached) = self.cache_if_valid(ctx) {
            if is_frame_owner {
                let fid = sanitized.get("frameId").and_then(Value::as_str).unwrap_or("");
                if fid == cached.tree.main_frame_id
                    || cached.tree.protected_frame_ids.contains(fid)
                {
                    return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                }
                if !cached.tree.clean_frame_ids.contains(fid) {
                    return self.start_node_guard(call, ctx, true, retried);
                }
            } else {
                match &target {
                    DomTarget::BackendNodeId(bid) => {
                        if cached.protected.backend_ids.contains(bid) {
                            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                        }
                        if !cached.protected.clean_backend_ids.contains(bid) {
                            return self.start_node_guard(call, ctx, true, retried);
                        }
                    }
                    DomTarget::NodeId(nid) => {
                        if cached.protected.node_ids.contains(nid) {
                            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
                        }
                        if !cached.protected.clean_node_ids.contains(nid) {
                            return self.start_node_guard(call, ctx, true, retried);
                        }
                    }
                    DomTarget::ObjectId(_) | DomTarget::None => {
                        return self.start_node_guard(call, ctx, true, retried);
                    }
                }
            }
            let job_id = self.alloc_job_id();
            let method = call.method.clone();
            let params_send = sanitized.clone();
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                started_at: Instant::now(),
                kind: OpKind::NodeGuard(Box::new(NodeGuardOp {
                    method: call.method,
                    params: sanitized,
                    target,
                    baseline_cef_ext: ctx.cef_ext_count(),
                    start_dom_epoch: ctx.dom_epoch,
                    build_start_epoch: ctx.dom_epoch,
                    used_cache: true,
                    retried,
                    resolved_target_bid: None,
                    stage: NodeStage::Exec,
                    raw_ft: Value::Null,
                    building_tree: FrameTreeAnalysis::default(),
                    building_protected: ProtectedDomNodes::default(),
                    state: Some(cached),
                })),
            });
            return vec![send(job_id, &method, params_send)];
        }

        let job_id = self.alloc_job_id();
        let (stage, first_cmd) = if !self.dom_events_enabled {
            (NodeStage::EnableDom, send(job_id, "DOM.enable", json!({})))
        } else if let DomTarget::NodeId(nid) = target {
            (
                NodeStage::ResolveNodeBeforeDoc,
                send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })),
            )
        } else {
            (NodeStage::FrameTree, send(job_id, "Page.getFrameTree", json!({})))
        };
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
            started_at: Instant::now(),
            kind: OpKind::NodeGuard(Box::new(NodeGuardOp {
                method: call.method,
                params: sanitized,
                target,
                baseline_cef_ext: ctx.cef_ext_count(),
                start_dom_epoch: ctx.dom_epoch,
                build_start_epoch: ctx.dom_epoch,
                used_cache: false,
                retried,
                resolved_target_bid: None,
                stage,
                raw_ft: Value::Null,
                building_tree: FrameTreeAnalysis::default(),
                building_protected: ProtectedDomNodes::default(),
                state: None,
            })),
        });
        vec![first_cmd]
    }

    fn store_cache_if_fresh(&mut self, state: &Rc<CachedMaskState>, ctx: &TabMaskCtx) {
        if self.dom_events_enabled
            && !ctx.has_pending()
            && state.protected.clean_cache_complete
            && state.dom_epoch == ctx.dom_epoch
            && state.cef_ext_count == ctx.cef_ext_count()
        {
            self.cache = Some(Rc::clone(state));
        }
    }

    fn step_op(
        &mut self,
        op: &mut ActiveOp,
        ok: bool,
        mut result: Value,
        ctx: &TabMaskCtx,
        effects: &mut Vec<MaskEffect>,
    ) -> StepResult {
        let req_id = op.request_id;
        let job_id = op.job_id;
        if ctx.surface.is_whole_tab() || ctx.has_pending() {
            if let OpKind::NodeGuard(ng) = &op.kind
                && matches!(ng.stage, NodeStage::Exec)
                && ng.method == "DOM.resolveNode"
                && let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str)
            {
                effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
            }
            let msg = if ctx.surface.is_whole_tab() {
                WHOLE_TAB_REFUSAL
            } else {
                SUBFRAME_REFUSAL
            };
            effects.push(refuse(req_id, msg));
            return StepResult::Done;
        }
        match &mut op.kind {
            OpKind::CleanPass { method, redact_ext } => {
                if !ctx.surface.is_clean() {
                    if method == "DOM.resolveNode"
                        && let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str)
                    {
                        effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
                    }
                    effects.push(refuse(req_id, WHOLE_TAB_REFUSAL));
                } else {
                    if *redact_ext {
                        redact_ext_strings(&mut result);
                    }
                    effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
                }
                StepResult::Done
            }
            OpKind::SerializedPass => {
                effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
                StepResult::Done
            }
            OpKind::ReadFilter(st) => {
                let mut dom_enabled = self.dom_events_enabled;
                let mut built: Option<Rc<CachedMaskState>> = None;
                let res = step_read_filter(
                    job_id,
                    req_id,
                    st,
                    ok,
                    result,
                    ctx,
                    &mut dom_enabled,
                    &mut built,
                    effects,
                );
                self.dom_events_enabled = dom_enabled;
                if let Some(c) = built {
                    self.store_cache_if_fresh(&c, ctx);
                }
                res
            }
            OpKind::NodeGuard(st) => {
                let mut dom_enabled = self.dom_events_enabled;
                let mut built: Option<Rc<CachedMaskState>> = None;
                let res = step_node_guard(
                    job_id,
                    req_id,
                    st,
                    ok,
                    result,
                    ctx,
                    &mut dom_enabled,
                    &mut built,
                    effects,
                );
                self.dom_events_enabled = dom_enabled;
                if let Some(c) = built {
                    self.store_cache_if_fresh(&c, ctx);
                }
                res
            }
        }
    }
}

pub(super) enum StepResult {
    Continue,
    RetryFresh(QueuedCdpCall),
    Done,
}

struct ActiveOp {
    job_id: u64,
    request_id: u64,
    started_at: Instant,
    kind: OpKind,
}

enum OpKind {
    CleanPass { method: String, redact_ext: bool },
    SerializedPass,
    ReadFilter(Box<ReadFilterOp>),
    NodeGuard(Box<NodeGuardOp>),
}

fn release_late_object_if_any(result: &Value) -> Vec<MaskEffect> {
    if let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str) {
        vec![MaskEffect::ReleaseObject { object_id: oid.to_owned() }]
    } else {
        Vec::new()
    }
}

pub(super) fn send(job_id: u64, method: &str, params: Value) -> MaskEffect {
    MaskEffect::SendInternal {
        job_id,
        method: method.to_owned(),
        params,
    }
}

pub(super) fn refuse(request_id: u64, message: &str) -> MaskEffect {
    MaskEffect::Respond {
        request_id,
        ok: false,
        result: json!({ "message": message }),
    }
}
