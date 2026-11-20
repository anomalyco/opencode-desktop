//! Per-tab serialized masking state machine for web tabs hosting extension sub-frames
//! (`review-rd274.md` Rounds 4 & 6).
//!
//! Guarantees while any extension sub-frame is present on a web tab:
//! - Strictly ONE guarded CDP operation runs at a time (FIFO queue capped at 64; overflow fails
//!   closed), preventing `Tab` + `Enter`/`insertText` pipelining and `Runtime.evaluate`/wheel/
//!   scroll/resize races against pointer hit-tests and masked screenshots.
//! - `layout_epoch` invalidates in-flight pointer hit-tests and masked screenshots on any
//!   `Resize`, `SetZoom`, `Emulate`, or user input event.
//! - Every tree read (`Page.getFrameTree`, `Accessibility.*`, `DOM.getDocument`,
//!   `DOM.describeNode`, `DOM.querySelector*`) strips closed ShadowRoots and extension frames
//!   without leaking `backendNodeId`s and redacts all `chrome-extension:` strings.
//! - Every node-targeted / frame-owner call (`DOM.resolveNode`, `DOM.focus`, `DOM.getFrameOwner`,
//!   `DOM.getBoxModel`, `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`) refuses any
//!   `backendNodeId`/`nodeId`/`objectId`/`frameId` belonging to an extension frame, cross-origin
//!   ancestor iframe hosting an extension frame, or closed ShadowRoot.
//! - Every keyboard/text call checks BOTH Chromium `browser.focused_frame()` AND native C++
//!   Blink `DOM.querySelector(root, ":focus")` against protected nodes and closed-ShadowRoot hosts.
//! - Every pointer/touch call verifies `DOM.getFrameOwner` + `DOM.getBoxModel` (8 px pad +
//!   moving-iframe detection) AND `DOM.getNodeForLocation` before dispatching.
//! - Every `Page.captureScreenshot` captures lossless PNG, re-verifies frame tree, scroll metrics,
//!   and frame boxes post-capture (failing closed if any new extension frame appeared or any box
//!   moved), paints `#18181B` over all protected rects, and re-encodes to `png` or `jpeg`.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use serde_json::{Value, json};

use super::frame_guard::{
    CssRect, ExtSurfaceKind, FrameTreeAnalysis, ProtectedDomNodes, ReadFilterKind,
    analyze_frame_tree, collect_protected_dom_nodes, extract_input_points,
    is_keyboard_input_method, is_node_guard_method, is_pointer_input_method,
    is_pointer_press_event, read_filter_for_method, redact_ext_strings, strip_ax_tree,
    strip_dom_value, strip_frame_tree,
};
use super::mask_ops::{
    KeyStage, KeyboardOp, PointerOp, PointerStage, ScreenshotOp, ShotStage, step_keyboard,
    step_pointer, step_screenshot,
};
use super::screenshot_mask::parse_screenshot_params;

const MAX_QUEUED_CALLS: usize = 64;
const MAX_QUERY_VERIFY_NODES: usize = 32;
const WHOLE_TAB_REFUSAL: &str =
    "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";
pub(super) const SUBFRAME_REFUSAL: &str =
    "Agent browser tools cannot read, target, click, or type into browser extension frames.";

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
    pub focused_is_ext: bool,
    pub extensions_active: bool,
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

pub struct TabMaskQueue {
    next_job_id: u64,
    layout_epoch: u64,
    active: Option<ActiveOp>,
    queued: VecDeque<QueuedCdpCall>,
    last_rects: Option<(Instant, Vec<CssRect>)>,
}

impl Default for TabMaskQueue {
    fn default() -> Self {
        Self {
            next_job_id: 1,
            layout_epoch: 0,
            active: None,
            queued: VecDeque::new(),
            last_rects: None,
        }
    }
}

impl TabMaskQueue {
    pub fn is_idle(&self) -> bool {
        self.active.is_none() && self.queued.is_empty()
    }

    pub fn bump_layout_epoch(&mut self) {
        self.layout_epoch = self.layout_epoch.wrapping_add(1);
    }

    pub fn fail_all(&mut self, msg: &str) -> Vec<MaskEffect> {
        let mut out = Vec::new();
        if let Some(op) = self.active.take() {
            out.push(refuse(op.request_id(), msg));
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
        if ctx.surface.is_clean() {
            // Even before any extension iframe mounts, guard node-targeted calls when extensions
            // are active so a caller cannot brute-force `backendNodeId` into a closed ShadowRoot
            // created by an extension content script (`review-rd274.md` Round-4 #1).
            if ctx.extensions_active && is_node_guard_method(&call.method) {
                let job_id = self.alloc_job_id();
                self.active = Some(ActiveOp {
                    job_id,
                    request_id: call.request_id,
                    kind: OpKind::NodeGuard(Box::new(NodeGuardOp {
                        method: call.method,
                        params: call.params,
                        baseline_cef_ext: 0,
                        stage: NodeStage::FrameTree,
                        raw_ft: Value::Null,
                        tree: FrameTreeAnalysis::default(),
                        protected: ProtectedDomNodes::default(),
                    })),
                });
                return vec![send(job_id, "Page.getFrameTree", json!({}))];
            }
            let job_id = self.alloc_job_id();
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                kind: OpKind::CleanPass {
                    method: call.method.clone(),
                },
            });
            return vec![MaskEffect::SendInternal {
                job_id,
                method: call.method,
                params: call.params,
            }];
        }

        let job_id = self.alloc_job_id();
        let baseline_cef_ext = ctx.cef_ext_count();
        let started_pending = ctx.has_pending();

        if call.method == "Page.captureScreenshot" {
            if started_pending {
                return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
            }
            let parsed = match parse_screenshot_params(&call.params) {
                Ok(p) => p,
                Err(msg) => return vec![refuse(call.request_id, msg)],
            };
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                kind: OpKind::Screenshot(Box::new(ScreenshotOp {
                    parsed,
                    start_epoch: self.layout_epoch,
                    stage: ShotStage::PreFrameTree,
                    raw_ft: Value::Null,
                    pre_ext_ids: HashSet::new(),
                    pre_host_ids: Vec::new(),
                    pre_metrics: None,
                    pre_owners: Vec::new(),
                    pre_rects: HashMap::new(),
                    captured_png_b64: String::new(),
                    post_host_ids: Vec::new(),
                    post_owners: Vec::new(),
                    post_rects: HashMap::new(),
                })),
            });
            return vec![send(job_id, "Page.getFrameTree", json!({}))];
        }

        if is_pointer_input_method(&call.method) {
            if started_pending {
                return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
            }
            let Some(points) = extract_input_points(&call.method, &call.params) else {
                return vec![refuse(call.request_id, "Invalid pointer coordinates.")];
            };
            let is_press = is_pointer_press_event(&call.method, &call.params);
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                kind: OpKind::Pointer(Box::new(PointerOp {
                    method: call.method,
                    params: call.params,
                    points,
                    is_press,
                    start_epoch: self.layout_epoch,
                    baseline_cef_ext,
                    stage: PointerStage::FrameTree,
                    raw_ft: Value::Null,
                    tree: FrameTreeAnalysis::default(),
                    protected: ProtectedDomNodes::default(),
                    owner_bids: Vec::new(),
                    rects: Vec::new(),
                })),
            });
            return vec![send(job_id, "Page.getFrameTree", json!({}))];
        }

        if is_keyboard_input_method(&call.method) {
            if started_pending || ctx.focused_is_ext {
                return vec![refuse(call.request_id, SUBFRAME_REFUSAL)];
            }
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                kind: OpKind::Keyboard(Box::new(KeyboardOp {
                    method: call.method,
                    params: call.params,
                    stage: KeyStage::FrameTree,
                    raw_ft: Value::Null,
                    tree: FrameTreeAnalysis::default(),
                    protected: ProtectedDomNodes::default(),
                })),
            });
            return vec![send(job_id, "Page.getFrameTree", json!({}))];
        }

        if is_node_guard_method(&call.method) {
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
            return vec![send(job_id, "Page.getFrameTree", json!({}))];
        }

        if let Some(filter) = read_filter_for_method(&call.method) {
            self.active = Some(ActiveOp {
                job_id,
                request_id: call.request_id,
                kind: OpKind::ReadFilter(Box::new(ReadFilterOp {
                    method: call.method,
                    params: call.params,
                    filter,
                    baseline_cef_ext,
                    started_pending,
                    stage: ReadStage::FrameTree,
                    raw_ft: Value::Null,
                    tree: FrameTreeAnalysis::default(),
                    protected: ProtectedDomNodes::default(),
                    query_candidates: Vec::new(),
                    query_verified: Vec::new(),
                    query_is_all: false,
                })),
            });
            return vec![send(job_id, "Page.getFrameTree", json!({}))];
        }

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
        if ctx.surface.is_whole_tab() {
            if let OpKind::NodeGuard(ng) = &op.kind
                && matches!(ng.stage, NodeStage::Exec)
                && ng.method == "DOM.resolveNode"
                && let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str)
            {
                effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
            }
            effects.push(refuse(req_id, WHOLE_TAB_REFUSAL));
            return StepResult::Done;
        }
        match &mut op.kind {
            OpKind::CleanPass { method } => {
                if !ctx.surface.is_clean() {
                    if method == "DOM.resolveNode"
                        && let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str)
                    {
                        effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
                    }
                    effects.push(refuse(req_id, WHOLE_TAB_REFUSAL));
                } else {
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
            OpKind::Pointer(st) => {
                let epoch = self.layout_epoch;
                step_pointer(job_id, req_id, st, ok, result, ctx, epoch, &mut self.last_rects, effects)
            }
            OpKind::Keyboard(st) => step_keyboard(job_id, req_id, st, ok, result, ctx, effects),
            OpKind::Screenshot(st) => {
                let epoch = self.layout_epoch;
                step_screenshot(job_id, req_id, st, ok, &mut result, ctx, epoch, effects)
            }
        }
    }
}

pub(super) enum StepResult {
    Continue,
    Done,
}

struct ActiveOp {
    job_id: u64,
    request_id: u64,
    kind: OpKind,
}

impl ActiveOp {
    fn request_id(&self) -> u64 {
        self.request_id
    }
}

enum OpKind {
    CleanPass { method: String },
    SerializedPass,
    ReadFilter(Box<ReadFilterOp>),
    NodeGuard(Box<NodeGuardOp>),
    Pointer(Box<PointerOp>),
    Keyboard(Box<KeyboardOp>),
    Screenshot(Box<ScreenshotOp>),
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
    started_pending: bool,
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
                && (ctx.has_pending() || !tree.clean_frame_ids.contains(fid))
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
            if ctx.cef_ext_count() > st.baseline_cef_ext || (ctx.has_pending() && !st.started_pending) {
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
                            "DOM.querySelectorAll returned too many nodes to verify safely on a tab with extension frames.",
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
                    || ctx.has_pending()
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
    // No target identifier supplied: fail closed.
    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
    StepResult::Done
}

pub(super) fn record_owner_ids(val: &Value, protected: &mut ProtectedDomNodes) -> bool {
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
