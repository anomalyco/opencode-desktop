//! Pointer, keyboard, and masked-screenshot operation state machines executed sequentially
//! inside `TabMaskQueue` (`mask_queue.rs`).

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::frame_guard::{
    CssRect, FrameTreeAnalysis, POINTER_PAD_CSS, ProtectedDomNodes, RECT_SHIFT_TOLERANCE_CSS,
    ViewportMetrics, analyze_frame_tree, collect_protected_dom_nodes, extract_box_rect,
    extract_viewport_metrics, is_no_layout_box_error, is_no_node_at_location_error,
    metrics_shifted, point_hits_any_rect, rects_shifted,
};
use super::mask_queue::{
    MaskEffect, SUBFRAME_REFUSAL, StepResult, TabMaskCtx, record_owner_ids, refuse, send,
};
use super::screenshot_mask::{
    ParsedScreenshotParams, mask_screenshot_b64, validate_clip_in_viewport,
};

const MOVING_FRAME_WINDOW: Duration = Duration::from_millis(250);

// ============================================================================
// 1. PointerOp (`Input.dispatchMouseEvent`, `dispatchTouchEvent`, `emulateTouchFromMouseEvent`)
// ============================================================================

pub(super) struct PointerOp {
    pub method: String,
    pub params: Value,
    pub points: Vec<(f64, f64)>,
    pub is_press: bool,
    pub start_epoch: u64,
    pub baseline_cef_ext: usize,
    pub stage: PointerStage,
    pub raw_ft: Value,
    pub tree: FrameTreeAnalysis,
    pub protected: ProtectedDomNodes,
    pub owner_bids: Vec<i64>,
    pub rects: Vec<CssRect>,
}

pub(super) enum PointerStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    Boxes(usize),
    NodeAtLocation(usize),
    Exec,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn step_pointer(
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
// 2. KeyboardOp (`Input.dispatchKeyEvent`, `Input.insertText`)
// ============================================================================

pub(super) struct KeyboardOp {
    pub method: String,
    pub params: Value,
    pub stage: KeyStage,
    pub raw_ft: Value,
    pub tree: FrameTreeAnalysis,
    pub protected: ProtectedDomNodes,
}

pub(super) enum KeyStage {
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    QueryFocus,
    DescribeFocus,
    Exec,
}

pub(super) fn step_keyboard(
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
// 3. ScreenshotOp (`Page.captureScreenshot` with pre/post verify + paint-out)
// ============================================================================

pub(super) struct ScreenshotOp {
    pub parsed: ParsedScreenshotParams,
    pub start_epoch: u64,
    pub stage: ShotStage,
    pub raw_ft: Value,
    pub pre_ext_ids: HashSet<String>,
    pub pre_host_ids: Vec<String>,
    pub pre_metrics: Option<ViewportMetrics>,
    pub pre_owners: Vec<(String, i64)>,
    pub pre_rects: HashMap<String, Option<CssRect>>,
    pub captured_png_b64: String,
    pub post_host_ids: Vec<String>,
    pub post_owners: Vec<(String, i64)>,
    pub post_rects: HashMap<String, Option<CssRect>>,
}

pub(super) enum ShotStage {
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
pub(super) fn step_screenshot(
    job_id: u64,
    req_id: u64,
    st: &mut ScreenshotOp,
    ok: bool,
    result: &mut Value,
    ctx: &TabMaskCtx,
    layout_epoch: u64,
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
