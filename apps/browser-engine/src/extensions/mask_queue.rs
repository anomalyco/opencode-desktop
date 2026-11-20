//! Per-tab serialized read & node-guard state machine for PR 1b-2 (`review-rd274.md` #311 split).
//!
//! Guarantees:
//! - Whole-tab extension surfaces (`ExtSurfaceKind::WholeTab`) refuse every CDP call.
//! - While ANY pre-commit extension sub-frame is pending (`ctx.has_pending()`), ALL guarded read
//!   and node operations fail closed (`review-rd274.md` #311 Finding 2).
//! - On web tabs with committed extension sub-frames (`ExtSurfaceKind::Subframes`):
//!   - `Input.*` and `Page.captureScreenshot` are refused unconditionally (safe standalone PR;
//!     screenshots move to PR 1b-4, and `Input.*` stays refused under Option A).
//!   - All tree/DOM reads (`Page.getFrameTree`, `Accessibility.*`, `DOM.getDocument`,
//!     `DOM.describeNode`, `DOM.querySelector*`) and node/owner calls (`DOM.resolveNode`,
//!     `DOM.focus`, `DOM.getFrameOwner`, `DOM.getBoxModel`, `DOM.getContentQuads`,
//!     `DOM.scrollIntoViewIfNeeded`) run single-flight in FIFO order (capped at 64), strip closed
//!     ShadowRoots and extension frames without leaking `backendNodeId`s, and redact all
//!     `chrome-extension:` strings.
//! - On `Clean` web tabs with extensions active (`ctx.extensions_active == true`), all DOM read
//!   and node-targeted methods (`DOM.describeNode`, `DOM.getDocument`, `DOM.querySelector*`,
//!   `DOM.resolveNode`, `DOM.focus`, `DOM.getFrameOwner`, `DOM.getBoxModel`, `DOM.getContentQuads`,
//!   `DOM.scrollIntoViewIfNeeded`) STILL run through `ReadFilterOp`/`NodeGuardOp` so sequential
//!   `backendNodeId` brute-forcing can never dump closed-ShadowRoot extension DOM before an iframe
//!   mounts (`review-rd274.md` #311 Finding 1), and queued `CleanPass` read calls run
//!   `redact_ext_strings`.

#![forbid(unsafe_code)]

use std::collections::{HashSet, VecDeque};

use serde_json::{Value, json};

use super::frame_guard::{
    ExtSurfaceKind, FrameTreeAnalysis, ProtectedDomNodes, ReadFilterKind, analyze_frame_tree,
    collect_protected_dom_nodes, is_node_guard_method, is_refused_on_subframes,
    read_filter_for_method, redact_ext_strings, strip_ax_tree, strip_dom_value, strip_frame_tree,
};

const MAX_QUEUED_CALLS: usize = 64;
const MAX_QUERY_VERIFY_NODES: usize = 32;
const WHOLE_TAB_REFUSAL: &str =
    "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";
const SUBFRAME_REFUSAL: &str =
    "Agent browser tools cannot read or target browser extension frames.";
const SUBFRAME_INPUT_SHOT_REFUSAL: &str =
    "Input and screenshot DevTools calls are not permitted on tabs with browser extension frames.";

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
}

impl TabMaskCtx {
    fn has_pending(&self) -> bool {
        matches!(&self.surface, ExtSurfaceKind::Subframes(s) if s.has_pending)
    }

    fn cef_ext_count(&self) -> usize {
        match &self.surface {
            ExtSurfaceKind::Subframes(s) => s.cef_ext_count,
            _ => 0,
        }
    }

    fn known_cdp_ext(&self) -> HashSet<String> {
        match &self.surface {
            ExtSurfaceKind::Subframes(s) => s.cdp_ext_frames.clone(),
            _ => HashSet::new(),
        }
    }
}

pub struct TabMaskQueue {
    next_job_id: u64,
    active: Option<ActiveOp>,
    queued: VecDeque<QueuedCdpCall>,
}

impl Default for TabMaskQueue {
    fn default() -> Self {
        Self {
            next_job_id: 1,
            active: None,
            queued: VecDeque::new(),
        }
    }
}

impl TabMaskQueue {
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }

    pub fn fail_all(&mut self, msg: &str) -> Vec<MaskEffect> {
        let mut out = Vec::new();
        if let Some(op) = self.active.take() {
            out.push(refuse(op.request_id, msg));
        }
        while let Some(q) = self.queued.pop_front() {
            out.push(refuse(q.request_id, msg));
        }
        out
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
            return Vec::new();
        };
        if op.job_id != job_id {
            self.active = Some(op);
            return Vec::new();
        }
        let mut effects = Vec::new();
        match self.step_op(&mut op, ok, result, ctx, &mut effects) {
            StepResult::Continue => {
                self.active = Some(op);
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

    fn start_call(&mut self, call: QueuedCdpCall, ctx: &TabMaskCtx) -> Vec<MaskEffect> {
        if ctx.surface.is_whole_tab() {
            return vec![refuse(call.request_id, WHOLE_TAB_REFUSAL)];
        }
        // Fail closed on ALL calls while any extension sub-frame navigation is pre-commit
        // (`review-rd274.md` #311 Finding 2).
        if ctx.has_pending() {
            return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
        }

        if ctx.surface.is_clean() {
            if ctx.extensions_active {
                if is_node_guard_method(&call.method) {
                    return self.start_node_guard(call, 0);
                }
                if let Some(filter) = read_filter_for_method(&call.method)
                    && matches!(
                        filter,
                        ReadFilterKind::Dom
                            | ReadFilterKind::QuerySelector
                            | ReadFilterKind::QuerySelectorAll
                    )
                {
                    return self.start_read_filter(call, filter, 0);
                }
            }
            let job_id = self.alloc_job_id();
            let redact_ext =
                ctx.extensions_active && read_filter_for_method(&call.method).is_some();
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
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

        // `ExtSurfaceKind::Subframes` (committed extension sub-frame(s), none pending):
        if is_refused_on_subframes(&call.method) {
            return vec![refuse(call.request_id, SUBFRAME_INPUT_SHOT_REFUSAL)];
        }

        let baseline_cef_ext = ctx.cef_ext_count();
        if is_node_guard_method(&call.method) {
            return self.start_node_guard(call, baseline_cef_ext);
        }
        if let Some(filter) = read_filter_for_method(&call.method) {
            return self.start_read_filter(call, filter, baseline_cef_ext);
        }

        let job_id = self.alloc_job_id();
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
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
        baseline_cef_ext: usize,
    ) -> Vec<MaskEffect> {
        let job_id = self.alloc_job_id();
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
            kind: OpKind::ReadFilter(Box::new(ReadFilterOp {
                method: call.method,
                params: call.params,
                filter,
                baseline_cef_ext,
                stage: ReadStage::FrameTree,
                raw_ft: Value::Null,
                tree: FrameTreeAnalysis::default(),
                protected: ProtectedDomNodes::default(),
                query_candidates: Vec::new(),
                query_verified: Vec::new(),
                query_is_all: false,
            })),
        });
        vec![send(job_id, "Page.getFrameTree", json!({}))]
    }

    fn start_node_guard(&mut self, call: QueuedCdpCall, baseline_cef_ext: usize) -> Vec<MaskEffect> {
        let job_id = self.alloc_job_id();
        self.active = Some(ActiveOp {
            job_id,
            request_id: call.request_id,
            kind: OpKind::NodeGuard(Box::new(NodeGuardOp {
                method: call.method,
                params: call.params,
                baseline_cef_ext,
                stage: NodeStage::FrameTree,
                raw_ft: Value::Null,
                tree: FrameTreeAnalysis::default(),
                protected: ProtectedDomNodes::default(),
            })),
        });
        vec![send(job_id, "Page.getFrameTree", json!({}))]
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
            OpKind::ReadFilter(st) => step_read_filter(job_id, req_id, st, ok, result, ctx, effects),
            OpKind::NodeGuard(st) => step_node_guard(job_id, req_id, st, ok, result, ctx, effects),
        }
    }
}

enum StepResult {
    Continue,
    Done,
}

struct ActiveOp {
    job_id: u64,
    request_id: u64,
    kind: OpKind,
}

enum OpKind {
    CleanPass { method: String, redact_ext: bool },
    SerializedPass,
    ReadFilter(Box<ReadFilterOp>),
    NodeGuard(Box<NodeGuardOp>),
}

// ============================================================================
// 1. ReadFilterOp (`Page.getFrameTree`, `Accessibility.*`, `DOM.getDocument`,
//    `DOM.describeNode`, `DOM.querySelector`, `DOM.querySelectorAll`)
// ============================================================================

struct ReadFilterOp {
    method: String,
    params: Value,
    filter: ReadFilterKind,
    baseline_cef_ext: usize,
    stage: ReadStage,
    raw_ft: Value,
    tree: FrameTreeAnalysis,
    protected: ProtectedDomNodes,
    query_candidates: Vec<i64>,
    query_verified: Vec<i64>,
    query_is_all: bool,
}

enum ReadStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    ResolveDescribeTarget,
    Exec,
    VerifyQueryNode(usize),
}

fn step_read_filter(
    job_id: u64,
    req_id: u64,
    st: &mut ReadFilterOp,
    ok: bool,
    mut result: Value,
    ctx: &TabMaskCtx,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    match st.stage {
        ReadStage::FrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result;
            st.stage = ReadStage::Targets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        ReadStage::Targets => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let tree = match analyze_frame_tree(&st.raw_ft, &result, &ctx.known_cdp_ext()) {
                Ok(t) => t,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            if tree.ext_cdp_frame_ids.len() < ctx.cef_ext_count() {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            if let Some(fid) = st.params.get("frameId").and_then(Value::as_str)
                && !tree.clean_frame_ids.contains(fid)
            {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.tree = tree;
            st.stage = ReadStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        ReadStage::InternalDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.protected = match collect_protected_dom_nodes(&result, &st.tree) {
                Ok(p) => p,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            if let Some(first) = st.tree.host_child_cdp_frame_ids.first() {
                st.stage = ReadStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            dispatch_read_after_owners(job_id, req_id, st, effects)
        }
        ReadStage::Owners(idx) => {
            if !ok || !record_owner_ids(&result, &mut st.protected) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(fid) = st.tree.host_child_cdp_frame_ids.get(next) {
                st.stage = ReadStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            dispatch_read_after_owners(job_id, req_id, st, effects)
        }
        ReadStage::ResolveDescribeTarget => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if st.protected.backend_ids.contains(&bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = ReadStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        ReadStage::Exec => {
            if ctx.cef_ext_count() > st.baseline_cef_ext {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            if !ok {
                redact_ext_strings(&mut result);
                effects.push(MaskEffect::Respond { request_id: req_id, ok: false, result });
                return StepResult::Done;
            }
            match st.filter {
                ReadFilterKind::FrameTree => {
                    strip_frame_tree(&mut result, &st.tree.ext_cdp_frame_ids);
                    effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result });
                    StepResult::Done
                }
                ReadFilterKind::AxTree => {
                    strip_ax_tree(&mut result, &st.tree.ext_cdp_frame_ids, &st.protected.backend_ids);
                    effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result });
                    StepResult::Done
                }
                ReadFilterKind::Dom => {
                    if !strip_dom_value(
                        &mut result,
                        &st.tree.protected_frame_ids,
                        &st.protected.backend_ids,
                    ) {
                        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    } else {
                        effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result });
                    }
                    StepResult::Done
                }
                ReadFilterKind::QuerySelector => {
                    let nid = result.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
                    if nid <= 0 || st.protected.node_ids.contains(&nid) {
                        effects.push(MaskEffect::Respond {
                            request_id: req_id,
                            ok: true,
                            result: json!({ "nodeId": 0 }),
                        });
                        return StepResult::Done;
                    }
                    st.query_candidates = vec![nid];
                    st.query_verified.clear();
                    st.query_is_all = false;
                    st.stage = ReadStage::VerifyQueryNode(0);
                    effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
                    StepResult::Continue
                }
                ReadFilterKind::QuerySelectorAll => {
                    let cands: Vec<i64> = result
                        .get("nodeIds")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(Value::as_i64)
                                .filter(|nid| *nid > 0 && !st.protected.node_ids.contains(nid))
                                .collect()
                        })
                        .unwrap_or_default();
                    if cands.is_empty() {
                        effects.push(MaskEffect::Respond {
                            request_id: req_id,
                            ok: true,
                            result: json!({ "nodeIds": [] }),
                        });
                        return StepResult::Done;
                    }
                    if cands.len() > MAX_QUERY_VERIFY_NODES {
                        effects.push(refuse(
                            req_id,
                            "DOM.querySelectorAll returned too many nodes to verify safely.",
                        ));
                        return StepResult::Done;
                    }
                    let first = cands[0];
                    st.query_candidates = cands;
                    st.query_verified.clear();
                    st.query_is_all = true;
                    st.stage = ReadStage::VerifyQueryNode(0);
                    effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": first, "depth": 0 })));
                    StepResult::Continue
                }
            }
        }
        ReadStage::VerifyQueryNode(idx) => {
            if ok
                && let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64)
                && !st.protected.backend_ids.contains(&bid)
                && let Some(&nid) = st.query_candidates.get(idx)
            {
                st.query_verified.push(nid);
            }
            let next = idx + 1;
            if let Some(&nid) = st.query_candidates.get(next) {
                st.stage = ReadStage::VerifyQueryNode(next);
                effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
                return StepResult::Continue;
            }
            let out = if st.query_is_all {
                json!({ "nodeIds": st.query_verified })
            } else {
                json!({ "nodeId": st.query_verified.first().copied().unwrap_or(0) })
            };
            effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result: out });
            StepResult::Done
        }
    }
}

fn dispatch_read_after_owners(
    job_id: u64,
    req_id: u64,
    st: &mut ReadFilterOp,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if matches!(st.filter, ReadFilterKind::QuerySelector | ReadFilterKind::QuerySelectorAll)
        && let Some(nid) = st.params.get("nodeId").and_then(Value::as_i64)
        && st.protected.node_ids.contains(&nid)
    {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    }
    if st.method == "DOM.describeNode" {
        if let Some(bid) = st.params.get("backendNodeId").and_then(Value::as_i64) {
            if st.protected.backend_ids.contains(&bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
        } else if let Some(nid) = st.params.get("nodeId").and_then(Value::as_i64) {
            if st.protected.node_ids.contains(&nid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = ReadStage::ResolveDescribeTarget;
            effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
            return StepResult::Continue;
        } else if let Some(oid) = st.params.get("objectId").and_then(Value::as_str) {
            st.stage = ReadStage::ResolveDescribeTarget;
            effects.push(send(job_id, "DOM.describeNode", json!({ "objectId": oid, "depth": 0 })));
            return StepResult::Continue;
        }
    }
    st.stage = ReadStage::Exec;
    effects.push(send(job_id, &st.method, st.params.clone()));
    StepResult::Continue
}

// ============================================================================
// 2. NodeGuardOp (`DOM.resolveNode`, `DOM.focus`, `DOM.getFrameOwner`,
//    `DOM.getBoxModel`, `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`)
// ============================================================================

struct NodeGuardOp {
    method: String,
    params: Value,
    baseline_cef_ext: usize,
    stage: NodeStage,
    raw_ft: Value,
    tree: FrameTreeAnalysis,
    protected: ProtectedDomNodes,
}

enum NodeStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    ResolveTarget,
    Exec,
}

fn step_node_guard(
    job_id: u64,
    req_id: u64,
    st: &mut NodeGuardOp,
    ok: bool,
    mut result: Value,
    ctx: &TabMaskCtx,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    match st.stage {
        NodeStage::FrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result;
            st.stage = NodeStage::Targets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        NodeStage::Targets => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let tree = match analyze_frame_tree(&st.raw_ft, &result, &ctx.known_cdp_ext()) {
                Ok(t) => t,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            if tree.ext_cdp_frame_ids.len() < ctx.cef_ext_count() {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            if st.method == "DOM.getFrameOwner" {
                let fid = st.params.get("frameId").and_then(Value::as_str).unwrap_or("");
                if fid.is_empty()
                    || fid == tree.main_frame_id
                    || !tree.clean_frame_ids.contains(fid)
                {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
                st.tree = tree;
                st.stage = NodeStage::Exec;
                effects.push(send(job_id, &st.method, st.params.clone()));
                return StepResult::Continue;
            }
            st.tree = tree;
            st.stage = NodeStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        NodeStage::InternalDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.protected = match collect_protected_dom_nodes(&result, &st.tree) {
                Ok(p) => p,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            if let Some(first) = st.tree.host_child_cdp_frame_ids.first() {
                st.stage = NodeStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            check_node_guard_target(job_id, req_id, st, effects)
        }
        NodeStage::Owners(idx) => {
            if !ok || !record_owner_ids(&result, &mut st.protected) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(fid) = st.tree.host_child_cdp_frame_ids.get(next) {
                st.stage = NodeStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            check_node_guard_target(job_id, req_id, st, effects)
        }
        NodeStage::ResolveTarget => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if st.protected.backend_ids.contains(&bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = NodeStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        NodeStage::Exec => {
            if st.method == "DOM.resolveNode" && ctx.cef_ext_count() > st.baseline_cef_ext {
                if let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str) {
                    effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
                }
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            redact_ext_strings(&mut result);
            effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
            StepResult::Done
        }
    }
}

fn check_node_guard_target(
    job_id: u64,
    req_id: u64,
    st: &mut NodeGuardOp,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if let Some(bid) = st.params.get("backendNodeId").and_then(Value::as_i64) {
        if st.protected.backend_ids.contains(&bid) {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            return StepResult::Done;
        }
        st.stage = NodeStage::Exec;
        effects.push(send(job_id, &st.method, st.params.clone()));
        return StepResult::Continue;
    }
    if let Some(nid) = st.params.get("nodeId").and_then(Value::as_i64) {
        if st.protected.node_ids.contains(&nid) {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            return StepResult::Done;
        }
        st.stage = NodeStage::ResolveTarget;
        effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
        return StepResult::Continue;
    }
    if let Some(oid) = st.params.get("objectId").and_then(Value::as_str) {
        st.stage = NodeStage::ResolveTarget;
        effects.push(send(job_id, "DOM.describeNode", json!({ "objectId": oid, "depth": 0 })));
        return StepResult::Continue;
    }
    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
    StepResult::Done
}

fn record_owner_ids(val: &Value, protected: &mut ProtectedDomNodes) -> bool {
    let Some(bid) = val.get("backendNodeId").and_then(Value::as_i64) else {
        return false;
    };
    protected.backend_ids.insert(bid);
    if let Some(nid) = val.get("nodeId").and_then(Value::as_i64)
        && nid > 0
    {
        protected.node_ids.insert(nid);
    }
    true
}

fn send(job_id: u64, method: &str, params: Value) -> MaskEffect {
    MaskEffect::SendInternal {
        job_id,
        method: method.to_owned(),
        params,
    }
}

fn refuse(request_id: u64, message: &str) -> MaskEffect {
    MaskEffect::Respond {
        request_id,
        ok: false,
        result: json!({ "message": message }),
    }
}
