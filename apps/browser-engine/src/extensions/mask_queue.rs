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
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::frame_guard::{
    CssRect, ExtSurfaceKind, FrameTreeAnalysis, POINTER_PAD_CSS, ProtectedDomNodes,
    RECT_SHIFT_TOLERANCE_CSS, ReadFilterKind, ViewportMetrics, analyze_frame_tree,
    collect_protected_dom_nodes, extract_box_rect, extract_input_points, extract_viewport_metrics,
    is_keyboard_input_method, is_no_layout_box_error, is_no_node_at_location_error,
    is_node_guard_method, is_pointer_input_method, is_pointer_press_event, metrics_shifted,
    point_hits_any_rect, read_filter_for_method, rects_shifted, redact_ext_strings, strip_ax_tree,
    strip_dom_value, strip_frame_tree,
};
use super::screenshot_mask::{
    ParsedScreenshotParams, mask_screenshot_b64, parse_screenshot_params, validate_clip_in_viewport,
};

const MAX_QUEUED_CALLS: usize = 64;
const MAX_QUERY_VERIFY_NODES: usize = 32;
const MOVING_FRAME_WINDOW: Duration = Duration::from_millis(250);
const WHOLE_TAB_REFUSAL: &str =
    "Agent browser tools cannot read or act on extension pages or tabs with extension frames.";
const SUBFRAME_REFUSAL: &str =
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
                let last_rects = &self.last_rects;
                step_screenshot(job_id, req_id, st, ok, &mut result, ctx, epoch, last_rects, effects)
            }
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

// ============================================================================
// 3. PointerOp (`Input.dispatchMouseEvent`, `dispatchTouchEvent`, `emulateTouchFromMouseEvent`)
// ============================================================================

struct PointerOp {
    method: String,
    params: Value,
    points: Vec<(f64, f64)>,
    is_press: bool,
    start_epoch: u64,
    baseline_cef_ext: usize,
    stage: PointerStage,
    raw_ft: Value,
    tree: FrameTreeAnalysis,
    protected: ProtectedDomNodes,
    owner_bids: Vec<i64>,
    rects: Vec<CssRect>,
}

enum PointerStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    Boxes(usize),
    NodeAtLocation(usize),
    Exec,
}

#[allow(clippy::too_many_arguments)]
fn step_pointer(
    job_id: u64,
    req_id: u64,
    st: &mut PointerOp,
    ok: bool,
    result: Value,
    ctx: &TabMaskCtx,
    layout_epoch: u64,
    last_rects: &mut Option<(Instant, Vec<CssRect>)>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if !matches!(st.stage, PointerStage::Exec)
        && (layout_epoch != st.start_epoch || ctx.has_pending() || ctx.cef_ext_count() > st.baseline_cef_ext)
    {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    }
    match st.stage {
        PointerStage::FrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result;
            st.stage = PointerStage::Targets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        PointerStage::Targets => {
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
            st.tree = tree;
            st.stage = PointerStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        PointerStage::InternalDoc => {
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
                st.stage = PointerStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            begin_pointer_location_checks(job_id, req_id, st, last_rects, effects)
        }
        PointerStage::Owners(idx) => {
            let Some(bid) = result.get("backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            record_owner_ids(&result, &mut st.protected);
            st.owner_bids.push(bid);
            let next = idx + 1;
            if let Some(fid) = st.tree.host_child_cdp_frame_ids.get(next) {
                st.stage = PointerStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            if let Some(&first_bid) = st.owner_bids.first() {
                st.stage = PointerStage::Boxes(0);
                effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": first_bid })));
                return StepResult::Continue;
            }
            begin_pointer_location_checks(job_id, req_id, st, last_rects, effects)
        }
        PointerStage::Boxes(idx) => {
            if ok {
                let Some(rect) = extract_box_rect(&result) else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                st.rects.push(rect);
            } else if !is_no_layout_box_error(&result) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(&bid) = st.owner_bids.get(next) {
                st.stage = PointerStage::Boxes(next);
                effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": bid })));
                return StepResult::Continue;
            }
            begin_pointer_location_checks(job_id, req_id, st, last_rects, effects)
        }
        PointerStage::NodeAtLocation(idx) => {
            if ok {
                if let Some(bid) = result.get("backendNodeId").and_then(Value::as_i64)
                    && st.protected.backend_ids.contains(&bid)
                {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
            } else if !is_no_node_at_location_error(&result) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(&(x, y)) = st.points.get(next) {
                st.stage = PointerStage::NodeAtLocation(next);
                effects.push(send_node_for_location(job_id, x, y));
                return StepResult::Continue;
            }
            st.stage = PointerStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        PointerStage::Exec => {
            effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
            StepResult::Done
        }
    }
}

fn begin_pointer_location_checks(
    job_id: u64,
    req_id: u64,
    st: &mut PointerOp,
    last_rects: &mut Option<(Instant, Vec<CssRect>)>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if point_hits_any_rect(&st.points, &st.rects, POINTER_PAD_CSS) {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    }
    let now = Instant::now();
    if st.is_press
        && let Some((prev_at, prev_rects)) = last_rects.as_ref()
        && now.saturating_duration_since(*prev_at) < MOVING_FRAME_WINDOW
        && rects_shifted(prev_rects, &st.rects, RECT_SHIFT_TOLERANCE_CSS)
    {
        *last_rects = Some((now, st.rects.clone()));
        effects.push(refuse(
            req_id,
            "Extension frame moved during pointer verification; click refused.",
        ));
        return StepResult::Done;
    }
    *last_rects = Some((now, st.rects.clone()));
    if let Some(&(x, y)) = st.points.first() {
        st.stage = PointerStage::NodeAtLocation(0);
        effects.push(send_node_for_location(job_id, x, y));
        return StepResult::Continue;
    }
    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
    StepResult::Done
}

fn send_node_for_location(job_id: u64, x: f64, y: f64) -> MaskEffect {
    send(
        job_id,
        "DOM.getNodeForLocation",
        json!({
            "x": x.round() as i64,
            "y": y.round() as i64,
            "includeUserAgentShadowDOM": true,
            "ignorePointerEventsNone": false,
        }),
    )
}

// ============================================================================
// 4. KeyboardOp (`Input.dispatchKeyEvent`, `Input.insertText`)
// ============================================================================

struct KeyboardOp {
    method: String,
    params: Value,
    stage: KeyStage,
    raw_ft: Value,
    tree: FrameTreeAnalysis,
    protected: ProtectedDomNodes,
}

enum KeyStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    QueryFocus,
    DescribeFocus,
    Exec,
}

fn step_keyboard(
    job_id: u64,
    req_id: u64,
    st: &mut KeyboardOp,
    ok: bool,
    result: Value,
    ctx: &TabMaskCtx,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if !matches!(st.stage, KeyStage::Exec) && (ctx.has_pending() || ctx.focused_is_ext) {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    }
    match st.stage {
        KeyStage::FrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result;
            st.stage = KeyStage::Targets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        KeyStage::Targets => {
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
            st.tree = tree;
            st.stage = KeyStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        KeyStage::InternalDoc => {
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
                st.stage = KeyStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            query_focused_element(job_id, req_id, st, effects)
        }
        KeyStage::Owners(idx) => {
            if !ok || !record_owner_ids(&result, &mut st.protected) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(fid) = st.tree.host_child_cdp_frame_ids.get(next) {
                st.stage = KeyStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            query_focused_element(job_id, req_id, st, effects)
        }
        KeyStage::QueryFocus => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let nid = result.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
            if nid <= 0 {
                st.stage = KeyStage::Exec;
                effects.push(send(job_id, &st.method, st.params.clone()));
                return StepResult::Continue;
            }
            if st.protected.node_ids.contains(&nid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = KeyStage::DescribeFocus;
            effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
            StepResult::Continue
        }
        KeyStage::DescribeFocus => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if st.protected.backend_ids.contains(&bid)
                || st.protected.shadow_host_backend_ids.contains(&bid)
            {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = KeyStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        KeyStage::Exec => {
            effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
            StepResult::Done
        }
    }
}

fn query_focused_element(
    job_id: u64,
    req_id: u64,
    st: &mut KeyboardOp,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    let Some(root_nid) = st.protected.root_node_id else {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    };
    st.stage = KeyStage::QueryFocus;
    effects.push(send(
        job_id,
        "DOM.querySelector",
        json!({ "nodeId": root_nid, "selector": ":focus" }),
    ));
    StepResult::Continue
}

// ============================================================================
// 5. ScreenshotOp (`Page.captureScreenshot` with pre/post verify + paint-out)
// ============================================================================

struct ScreenshotOp {
    parsed: ParsedScreenshotParams,
    start_epoch: u64,
    stage: ShotStage,
    raw_ft: Value,
    pre_ext_ids: HashSet<String>,
    pre_host_ids: Vec<String>,
    pre_metrics: Option<ViewportMetrics>,
    pre_owners: Vec<(String, i64)>,
    pre_rects: HashMap<String, Option<CssRect>>,
    captured_png_b64: String,
    post_host_ids: Vec<String>,
    post_owners: Vec<(String, i64)>,
    post_rects: HashMap<String, Option<CssRect>>,
}

enum ShotStage {
    PreFrameTree,
    PreTargets,
    PreMetrics,
    PreOwners(usize),
    PreBoxes(usize),
    Capture,
    PostFrameTree,
    PostTargets,
    PostMetrics,
    PostOwners(usize),
    PostBoxes(usize),
}

#[allow(clippy::too_many_arguments)]
fn step_screenshot(
    job_id: u64,
    req_id: u64,
    st: &mut ScreenshotOp,
    ok: bool,
    result: &mut Value,
    ctx: &TabMaskCtx,
    layout_epoch: u64,
    _last_rects: &Option<(Instant, Vec<CssRect>)>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    if layout_epoch != st.start_epoch || ctx.has_pending() {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    }
    match st.stage {
        ShotStage::PreFrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result.clone();
            st.stage = ShotStage::PreTargets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        ShotStage::PreTargets => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let tree = match analyze_frame_tree(&st.raw_ft, result, &ctx.known_cdp_ext()) {
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
            st.pre_ext_ids = tree.ext_cdp_frame_ids;
            st.pre_host_ids = tree.host_child_cdp_frame_ids;
            st.stage = ShotStage::PreMetrics;
            effects.push(send(job_id, "Page.getLayoutMetrics", json!({})));
            StepResult::Continue
        }
        ShotStage::PreMetrics => {
            let Some(m) = ok.then(|| extract_viewport_metrics(result)).flatten() else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if let Err(msg) = validate_clip_in_viewport(st.parsed.clip, m) {
                effects.push(refuse(req_id, msg));
                return StepResult::Done;
            }
            st.pre_metrics = Some(m);
            if let Some(first) = st.pre_host_ids.first() {
                st.stage = ShotStage::PreOwners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            st.stage = ShotStage::Capture;
            effects.push(send(job_id, "Page.captureScreenshot", st.parsed.internal_png_params.clone()));
            StepResult::Continue
        }
        ShotStage::PreOwners(idx) => {
            let Some(bid) = ok.then(|| result.get("backendNodeId").and_then(Value::as_i64)).flatten() else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            st.pre_owners.push((st.pre_host_ids[idx].clone(), bid));
            let next = idx + 1;
            if let Some(fid) = st.pre_host_ids.get(next) {
                st.stage = ShotStage::PreOwners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            let first_bid = st.pre_owners[0].1;
            st.stage = ShotStage::PreBoxes(0);
            effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": first_bid })));
            StepResult::Continue
        }
        ShotStage::PreBoxes(idx) => {
            let fid = st.pre_owners[idx].0.clone();
            if ok {
                let Some(rect) = extract_box_rect(result) else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                st.pre_rects.insert(fid, Some(rect));
            } else if is_no_layout_box_error(result) {
                st.pre_rects.insert(fid, None);
            } else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some((_, bid)) = st.pre_owners.get(next) {
                st.stage = ShotStage::PreBoxes(next);
                effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": bid })));
                return StepResult::Continue;
            }
            st.stage = ShotStage::Capture;
            effects.push(send(job_id, "Page.captureScreenshot", st.parsed.internal_png_params.clone()));
            StepResult::Continue
        }
        ShotStage::Capture => {
            let Some(b64) = ok.then(|| result.get("data").and_then(Value::as_str)).flatten() else {
                effects.push(refuse(req_id, "Screenshot capture failed."));
                return StepResult::Done;
            };
            st.captured_png_b64 = b64.to_owned();
            st.stage = ShotStage::PostFrameTree;
            effects.push(send(job_id, "Page.getFrameTree", json!({})));
            StepResult::Continue
        }
        ShotStage::PostFrameTree => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.raw_ft = result.clone();
            st.stage = ShotStage::PostTargets;
            effects.push(send(job_id, "Target.getTargets", json!({})));
            StepResult::Continue
        }
        ShotStage::PostTargets => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let tree = match analyze_frame_tree(&st.raw_ft, result, &ctx.known_cdp_ext()) {
                Ok(t) => t,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            // Fail closed if ANY new extension frame appeared mid-capture.
            if tree.ext_cdp_frame_ids.len() < ctx.cef_ext_count()
                || !tree.ext_cdp_frame_ids.is_subset(&st.pre_ext_ids)
                || !tree.host_child_cdp_frame_ids.iter().all(|id| st.pre_rects.contains_key(id))
            {
                effects.push(refuse(
                    req_id,
                    "Extension frame appeared during screenshot capture; screenshot refused.",
                ));
                return StepResult::Done;
            }
            st.post_host_ids = tree.host_child_cdp_frame_ids;
            st.stage = ShotStage::PostMetrics;
            effects.push(send(job_id, "Page.getLayoutMetrics", json!({})));
            StepResult::Continue
        }
        ShotStage::PostMetrics => {
            let Some(post_m) = ok.then(|| extract_viewport_metrics(result)).flatten() else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            let Some(pre_m) = st.pre_metrics else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if metrics_shifted(pre_m, post_m, RECT_SHIFT_TOLERANCE_CSS) {
                effects.push(refuse(
                    req_id,
                    "Viewport scrolled or resized during screenshot capture; screenshot refused.",
                ));
                return StepResult::Done;
            }
            if let Some(first) = st.post_host_ids.first() {
                st.stage = ShotStage::PostOwners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            finish_masked_screenshot(req_id, st, effects)
        }
        ShotStage::PostOwners(idx) => {
            let Some(bid) = ok.then(|| result.get("backendNodeId").and_then(Value::as_i64)).flatten() else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            st.post_owners.push((st.post_host_ids[idx].clone(), bid));
            let next = idx + 1;
            if let Some(fid) = st.post_host_ids.get(next) {
                st.stage = ShotStage::PostOwners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            let first_bid = st.post_owners[0].1;
            st.stage = ShotStage::PostBoxes(0);
            effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": first_bid })));
            StepResult::Continue
        }
        ShotStage::PostBoxes(idx) => {
            let fid = st.post_owners[idx].0.clone();
            if ok {
                let Some(rect) = extract_box_rect(result) else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                st.post_rects.insert(fid, Some(rect));
            } else if is_no_layout_box_error(result) {
                st.post_rects.insert(fid, None);
            } else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some((_, bid)) = st.post_owners.get(next) {
                st.stage = ShotStage::PostBoxes(next);
                effects.push(send(job_id, "DOM.getBoxModel", json!({ "backendNodeId": bid })));
                return StepResult::Continue;
            }
            finish_masked_screenshot(req_id, st, effects)
        }
    }
}

fn finish_masked_screenshot(
    req_id: u64,
    st: &ScreenshotOp,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    let Some(pre_m) = st.pre_metrics else {
        effects.push(refuse(req_id, SUBFRAME_REFUSAL));
        return StepResult::Done;
    };
    // Verify every surviving post-capture frame had an identical pre-capture box (neither became
    // visible mid-capture nor moved/resized mid-capture).
    for (fid, post_rect) in &st.post_rects {
        let Some(pre_rect) = st.pre_rects.get(fid) else {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            return StepResult::Done;
        };
        match (pre_rect, post_rect) {
            (None, Some(_)) => {
                effects.push(refuse(
                    req_id,
                    "Extension frame became visible during screenshot capture; screenshot refused.",
                ));
                return StepResult::Done;
            }
            (Some(a), Some(b)) if rects_shifted(&[*a], &[*b], RECT_SHIFT_TOLERANCE_CSS) => {
                effects.push(refuse(
                    req_id,
                    "Extension frame moved during screenshot capture; screenshot refused.",
                ));
                return StepResult::Done;
            }
            _ => {}
        }
    }
    let visible_rects: Vec<CssRect> = st.pre_rects.values().filter_map(|r| *r).collect();
    match mask_screenshot_b64(&st.captured_png_b64, &st.parsed, pre_m, &visible_rects) {
        Ok(masked_b64) => {
            effects.push(MaskEffect::Respond {
                request_id: req_id,
                ok: true,
                result: json!({ "data": masked_b64 }),
            });
        }
        Err(msg) => {
            effects.push(refuse(req_id, msg));
        }
    }
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
