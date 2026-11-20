//! Read-filter and node-guard state machines for `TabMaskQueue` (`review-rd274.md` #311 & #314).

#![forbid(unsafe_code)]

use std::rc::Rc;

use serde_json::{Value, json};

use super::frame_guard::{
    DomTarget, FrameTreeAnalysis, ProtectedDomNodes, ReadFilterKind, analyze_frame_tree,
    clone_dom_to_depth, collect_protected_dom_nodes, redact_ext_strings, strip_ax_tree,
    strip_dom_value, strip_frame_tree,
};
use super::mask_queue::{
    CachedMaskState, MAX_QUERY_VERIFY_NODES, MaskEffect, QueuedCdpCall, SUBFRAME_REFUSAL,
    StepResult, TabMaskCtx, refuse, send,
};

// ============================================================================
// 1. ReadFilterOp (`Page.getFrameTree`, `Accessibility.*`, `DOM.getDocument`,
//    `DOM.describeNode`, `DOM.querySelector`, `DOM.querySelectorAll`)
// ============================================================================

pub(super) struct ReadFilterOp {
    pub(super) method: String,
    pub(super) params: Value,
    pub(super) filter: ReadFilterKind,
    pub(super) target: DomTarget,
    pub(super) get_doc_depth: i64,
    pub(super) baseline_cef_ext: usize,
    pub(super) start_dom_epoch: u64,
    pub(super) build_start_epoch: u64,
    pub(super) used_cache: bool,
    pub(super) retried: bool,
    pub(super) resolved_target_bid: Option<i64>,
    pub(super) stage: ReadStage,
    pub(super) raw_ft: Value,
    pub(super) raw_doc: Option<Value>,
    pub(super) building_tree: FrameTreeAnalysis,
    pub(super) building_protected: ProtectedDomNodes,
    pub(super) state: Option<Rc<CachedMaskState>>,
    pub(super) query_verified: Vec<i64>,
    pub(super) query_unverified: Vec<i64>,
    pub(super) query_is_all: bool,
}

pub(super) enum ReadStage {
    EnableDom,
    ResolveNodeBeforeDoc,
    CachedGetDocFence,
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    ResolveDescribeTarget,
    Exec,
    VerifyQueryNode(usize),
}

#[allow(clippy::too_many_arguments)]
pub(super) fn step_read_filter(
    job_id: u64,
    req_id: u64,
    st: &mut ReadFilterOp,
    ok: bool,
    mut result: Value,
    ctx: &TabMaskCtx,
    dom_enabled: &mut bool,
    built_cache: &mut Option<Rc<CachedMaskState>>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    match st.stage {
        ReadStage::EnableDom => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            *dom_enabled = true;
            if let DomTarget::NodeId(nid) = st.target {
                st.stage = ReadStage::ResolveNodeBeforeDoc;
                effects.push(send(
                    job_id,
                    "DOM.describeNode",
                    json!({ "nodeId": nid, "depth": 0 }),
                ));
                return StepResult::Continue;
            }
            st.stage = ReadStage::FrameTree;
            effects.push(send(job_id, "Page.getFrameTree", json!({})));
            StepResult::Continue
        }
        ReadStage::ResolveNodeBeforeDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            st.resolved_target_bid = Some(bid);
            st.stage = ReadStage::FrameTree;
            effects.push(send(job_id, "Page.getFrameTree", json!({})));
            StepResult::Continue
        }
        ReadStage::CachedGetDocFence => {
            if !ok
                || ctx.cef_ext_count() > st.baseline_cef_ext
                || ctx.dom_epoch != st.start_dom_epoch
            {
                if !st.retried {
                    return StepResult::RetryFresh(QueuedCdpCall {
                        request_id: req_id,
                        method: st.method.clone(),
                        params: st.params.clone(),
                    });
                }
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(doc) = st.state.as_ref().and_then(|s| s.stripped_doc.as_ref()) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            match clone_dom_to_depth(doc, st.get_doc_depth) {
                Ok(out) => {
                    effects.push(MaskEffect::Respond {
                        request_id: req_id,
                        ok: true,
                        result: out,
                    });
                }
                Err(msg) => effects.push(refuse(req_id, msg)),
            }
            StepResult::Done
        }
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
            st.building_tree = tree;
            st.build_start_epoch = ctx.dom_epoch;
            st.stage = ReadStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        ReadStage::InternalDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.building_protected = match collect_protected_dom_nodes(&result, &st.building_tree) {
                Ok(p) => p,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            st.raw_doc = Some(result);
            if let Some(first) = st.building_tree.host_child_cdp_frame_ids.first() {
                st.stage = ReadStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            finish_read_build(job_id, req_id, st, ctx, built_cache, effects)
        }
        ReadStage::Owners(idx) => {
            if !ok || !record_owner_ids(&result, &mut st.building_protected) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(fid) = st.building_tree.host_child_cdp_frame_ids.get(next) {
                st.stage = ReadStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            finish_read_build(job_id, req_id, st, ctx, built_cache, effects)
        }
        ReadStage::ResolveDescribeTarget => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(state) = &st.state else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if state.protected.backend_ids.contains(&bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = ReadStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        ReadStage::Exec => {
            if ctx.cef_ext_count() > st.baseline_cef_ext
                || (st.used_cache && ctx.dom_epoch != st.start_dom_epoch)
            {
                if st.used_cache && !st.retried {
                    return StepResult::RetryFresh(QueuedCdpCall {
                        request_id: req_id,
                        method: st.method.clone(),
                        params: st.params.clone(),
                    });
                }
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(state) = st.state.as_ref().map(Rc::clone) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if !ok {
                redact_ext_strings(&mut result);
                effects.push(MaskEffect::Respond { request_id: req_id, ok: false, result });
                return StepResult::Done;
            }
            match st.filter {
                ReadFilterKind::FrameTree => {
                    strip_frame_tree(&mut result, &state.tree.ext_cdp_frame_ids);
                    effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result });
                    StepResult::Done
                }
                ReadFilterKind::AxTree => {
                    strip_ax_tree(
                        &mut result,
                        &state.tree.ext_cdp_frame_ids,
                        &state.protected.backend_ids,
                    );
                    effects.push(MaskEffect::Respond { request_id: req_id, ok: true, result });
                    StepResult::Done
                }
                ReadFilterKind::Dom => {
                    match strip_dom_value(
                        &mut result,
                        &state.tree.protected_frame_ids,
                        &state.protected.backend_ids,
                    ) {
                        Ok(true) => {
                            effects.push(MaskEffect::Respond {
                                request_id: req_id,
                                ok: true,
                                result,
                            });
                        }
                        Ok(false) => {
                            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                        }
                        Err(msg) => {
                            effects.push(refuse(req_id, msg));
                        }
                    }
                    StepResult::Done
                }
                ReadFilterKind::QuerySelector => {
                    let nid = result.get("nodeId").and_then(Value::as_i64).unwrap_or(0);
                    if nid <= 0 || state.protected.node_ids.contains(&nid) {
                        effects.push(MaskEffect::Respond {
                            request_id: req_id,
                            ok: true,
                            result: json!({ "nodeId": 0 }),
                        });
                        return StepResult::Done;
                    }
                    if state.protected.clean_node_ids.contains(&nid) {
                        effects.push(MaskEffect::Respond {
                            request_id: req_id,
                            ok: true,
                            result: json!({ "nodeId": nid }),
                        });
                        return StepResult::Done;
                    }
                    st.query_verified.clear();
                    st.query_unverified = vec![nid];
                    st.query_is_all = false;
                    st.stage = ReadStage::VerifyQueryNode(0);
                    effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": nid, "depth": 0 })));
                    StepResult::Continue
                }
                ReadFilterKind::QuerySelectorAll => {
                    let mut verified = Vec::new();
                    let mut unverified = Vec::new();
                    if let Some(arr) = result.get("nodeIds").and_then(Value::as_array) {
                        for nid in arr.iter().filter_map(Value::as_i64) {
                            if nid <= 0 || state.protected.node_ids.contains(&nid) {
                                continue;
                            }
                            if state.protected.clean_node_ids.contains(&nid) {
                                verified.push(nid);
                            } else {
                                unverified.push(nid);
                            }
                        }
                    }
                    if unverified.is_empty() {
                        effects.push(MaskEffect::Respond {
                            request_id: req_id,
                            ok: true,
                            result: json!({ "nodeIds": verified }),
                        });
                        return StepResult::Done;
                    }
                    if unverified.len() > MAX_QUERY_VERIFY_NODES {
                        effects.push(refuse(
                            req_id,
                            "DOM.querySelectorAll returned too many nodes to verify safely.",
                        ));
                        return StepResult::Done;
                    }
                    let first = unverified[0];
                    st.query_verified = verified;
                    st.query_unverified = unverified;
                    st.query_is_all = true;
                    st.stage = ReadStage::VerifyQueryNode(0);
                    effects.push(send(job_id, "DOM.describeNode", json!({ "nodeId": first, "depth": 0 })));
                    StepResult::Continue
                }
            }
        }
        ReadStage::VerifyQueryNode(idx) => {
            let Some(state) = &st.state else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if ok
                && let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64)
                && !state.protected.backend_ids.contains(&bid)
                && let Some(&nid) = st.query_unverified.get(idx)
            {
                st.query_verified.push(nid);
            }
            let next = idx + 1;
            if let Some(&nid) = st.query_unverified.get(next) {
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

fn finish_read_build(
    job_id: u64,
    req_id: u64,
    st: &mut ReadFilterOp,
    ctx: &TabMaskCtx,
    built_cache: &mut Option<Rc<CachedMaskState>>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    let tree = std::mem::take(&mut st.building_tree);
    let protected = std::mem::take(&mut st.building_protected);
    let mut stripped_doc = None;
    let mut root_node_id = None;
    if let Some(mut doc) = st.raw_doc.take() {
        match strip_dom_value(&mut doc, &tree.protected_frame_ids, &protected.backend_ids) {
            Ok(true) => {
                root_node_id = doc.pointer("/root/nodeId").and_then(Value::as_i64).filter(|&n| n > 0);
                stripped_doc = Some(doc);
            }
            Ok(false) => {
                if st.method == "DOM.getDocument" {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
            }
            Err(msg) => {
                if st.method == "DOM.getDocument" {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            }
        }
    }
    let state = Rc::new(CachedMaskState {
        dom_epoch: st.build_start_epoch,
        cef_ext_count: ctx.cef_ext_count(),
        cdp_ext_frames: ctx.known_cdp_ext(),
        tree,
        protected,
        root_node_id,
        stripped_doc,
    });
    *built_cache = Some(Rc::clone(&state));
    st.state = Some(Rc::clone(&state));

    if st.method == "DOM.getDocument" {
        if ctx.cef_ext_count() > st.baseline_cef_ext {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            return StepResult::Done;
        }
        let Some(doc) = state.stripped_doc.as_ref() else {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            return StepResult::Done;
        };
        match clone_dom_to_depth(doc, st.get_doc_depth) {
            Ok(out) => {
                effects.push(MaskEffect::Respond {
                    request_id: req_id,
                    ok: true,
                    result: out,
                });
            }
            Err(msg) => effects.push(refuse(req_id, msg)),
        }
        return StepResult::Done;
    }

    match &st.target {
        DomTarget::BackendNodeId(bid) => {
            if state.protected.backend_ids.contains(bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
        }
        DomTarget::NodeId(nid) => {
            if let Some(bid) = st.resolved_target_bid {
                if state.protected.backend_ids.contains(&bid) {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
                let Some(&fresh_nid) = state.protected.backend_to_clean_node.get(&bid) else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                if let Some(obj) = st.params.as_object_mut() {
                    obj.insert("nodeId".into(), Value::from(fresh_nid));
                }
            } else {
                if state.protected.node_ids.contains(nid) {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
                if !state.protected.clean_node_ids.contains(nid) {
                    st.stage = ReadStage::ResolveDescribeTarget;
                    effects.push(send(
                        job_id,
                        "DOM.describeNode",
                        json!({ "nodeId": nid, "depth": 0 }),
                    ));
                    return StepResult::Continue;
                }
            }
        }
        DomTarget::ObjectId(oid) => {
            st.stage = ReadStage::ResolveDescribeTarget;
            effects.push(send(
                job_id,
                "DOM.describeNode",
                json!({ "objectId": oid, "depth": 0 }),
            ));
            return StepResult::Continue;
        }
        DomTarget::None => {}
    }

    st.stage = ReadStage::Exec;
    effects.push(send(job_id, &st.method, st.params.clone()));
    StepResult::Continue
}

// ============================================================================
// 2. NodeGuardOp (`DOM.resolveNode`, `DOM.focus`, `DOM.getFrameOwner`,
//    `DOM.getBoxModel`, `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`)
// ============================================================================

pub(super) struct NodeGuardOp {
    pub(super) method: String,
    pub(super) params: Value,
    pub(super) target: DomTarget,
    pub(super) baseline_cef_ext: usize,
    pub(super) start_dom_epoch: u64,
    pub(super) build_start_epoch: u64,
    pub(super) used_cache: bool,
    pub(super) retried: bool,
    pub(super) resolved_target_bid: Option<i64>,
    pub(super) stage: NodeStage,
    pub(super) raw_ft: Value,
    pub(super) building_tree: FrameTreeAnalysis,
    pub(super) building_protected: ProtectedDomNodes,
    pub(super) state: Option<Rc<CachedMaskState>>,
}

pub(super) enum NodeStage {
    EnableDom,
    ResolveNodeBeforeDoc,
    FrameTree,
    Targets,
    InternalDoc,
    Owners(usize),
    ResolveTarget,
    Exec,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn step_node_guard(
    job_id: u64,
    req_id: u64,
    st: &mut NodeGuardOp,
    ok: bool,
    mut result: Value,
    ctx: &TabMaskCtx,
    dom_enabled: &mut bool,
    built_cache: &mut Option<Rc<CachedMaskState>>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    match st.stage {
        NodeStage::EnableDom => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            *dom_enabled = true;
            if let DomTarget::NodeId(nid) = st.target {
                st.stage = NodeStage::ResolveNodeBeforeDoc;
                effects.push(send(
                    job_id,
                    "DOM.describeNode",
                    json!({ "nodeId": nid, "depth": 0 }),
                ));
                return StepResult::Continue;
            }
            st.stage = NodeStage::FrameTree;
            effects.push(send(job_id, "Page.getFrameTree", json!({})));
            StepResult::Continue
        }
        NodeStage::ResolveNodeBeforeDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            st.resolved_target_bid = Some(bid);
            st.stage = NodeStage::FrameTree;
            effects.push(send(job_id, "Page.getFrameTree", json!({})));
            StepResult::Continue
        }
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
            }
            // Always run `InternalDoc` (including for `DOM.getFrameOwner`, #314 Finding 4) so
            // iframes nested inside closed ShadowRoots cannot leak `{ backendNodeId, nodeId }`.
            st.building_tree = tree;
            st.build_start_epoch = ctx.dom_epoch;
            st.stage = NodeStage::InternalDoc;
            effects.push(send(job_id, "DOM.getDocument", json!({ "depth": -1, "pierce": true })));
            StepResult::Continue
        }
        NodeStage::InternalDoc => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.building_protected = match collect_protected_dom_nodes(&result, &st.building_tree) {
                Ok(p) => p,
                Err(msg) => {
                    effects.push(refuse(req_id, msg));
                    return StepResult::Done;
                }
            };
            if let Some(first) = st.building_tree.host_child_cdp_frame_ids.first() {
                st.stage = NodeStage::Owners(0);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": first })));
                return StepResult::Continue;
            }
            finish_node_build(job_id, req_id, st, ctx, built_cache, effects)
        }
        NodeStage::Owners(idx) => {
            if !ok || !record_owner_ids(&result, &mut st.building_protected) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let next = idx + 1;
            if let Some(fid) = st.building_tree.host_child_cdp_frame_ids.get(next) {
                st.stage = NodeStage::Owners(next);
                effects.push(send(job_id, "DOM.getFrameOwner", json!({ "frameId": fid })));
                return StepResult::Continue;
            }
            finish_node_build(job_id, req_id, st, ctx, built_cache, effects)
        }
        NodeStage::ResolveTarget => {
            if !ok {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            let Some(state) = &st.state else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            let Some(bid) = result.pointer("/node/backendNodeId").and_then(Value::as_i64) else {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            };
            if state.protected.backend_ids.contains(&bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = NodeStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        NodeStage::Exec => {
            let epoch_changed = st.used_cache && ctx.dom_epoch != st.start_dom_epoch;
            if ctx.cef_ext_count() > st.baseline_cef_ext || epoch_changed {
                if st.method == "DOM.resolveNode"
                    && let Some(oid) = result.pointer("/object/objectId").and_then(Value::as_str)
                {
                    effects.push(MaskEffect::ReleaseObject { object_id: oid.to_owned() });
                }
                if st.used_cache && !st.retried {
                    return StepResult::RetryFresh(QueuedCdpCall {
                        request_id: req_id,
                        method: st.method.clone(),
                        params: st.params.clone(),
                    });
                }
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            if st.method == "DOM.getFrameOwner" && ok {
                let Some(state) = &st.state else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                let bid = result.get("backendNodeId").and_then(Value::as_i64);
                let nid = result.get("nodeId").and_then(Value::as_i64).filter(|&n| n > 0);
                let owner_protected = bid.is_none_or(|b| state.protected.backend_ids.contains(&b))
                    || nid.is_some_and(|n| state.protected.node_ids.contains(&n));
                if owner_protected {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
                if st.used_cache
                    && bid.is_some_and(|b| !state.protected.clean_backend_ids.contains(&b))
                {
                    if !st.retried {
                        return StepResult::RetryFresh(QueuedCdpCall {
                            request_id: req_id,
                            method: st.method.clone(),
                            params: st.params.clone(),
                        });
                    }
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
            }
            redact_ext_strings(&mut result);
            effects.push(MaskEffect::Respond { request_id: req_id, ok, result });
            StepResult::Done
        }
    }
}

fn finish_node_build(
    job_id: u64,
    req_id: u64,
    st: &mut NodeGuardOp,
    ctx: &TabMaskCtx,
    built_cache: &mut Option<Rc<CachedMaskState>>,
    effects: &mut Vec<MaskEffect>,
) -> StepResult {
    let state = Rc::new(CachedMaskState {
        dom_epoch: st.build_start_epoch,
        cef_ext_count: ctx.cef_ext_count(),
        cdp_ext_frames: ctx.known_cdp_ext(),
        tree: std::mem::take(&mut st.building_tree),
        protected: std::mem::take(&mut st.building_protected),
        root_node_id: None,
        stripped_doc: None,
    });
    *built_cache = Some(Rc::clone(&state));
    st.state = Some(Rc::clone(&state));

    if st.method == "DOM.getFrameOwner" {
        st.stage = NodeStage::Exec;
        effects.push(send(job_id, &st.method, st.params.clone()));
        return StepResult::Continue;
    }

    match &st.target {
        DomTarget::BackendNodeId(bid) => {
            if state.protected.backend_ids.contains(bid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            st.stage = NodeStage::Exec;
            effects.push(send(job_id, &st.method, st.params.clone()));
            StepResult::Continue
        }
        DomTarget::NodeId(nid) => {
            if let Some(bid) = st.resolved_target_bid {
                if state.protected.backend_ids.contains(&bid) {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                }
                let Some(&fresh_nid) = state.protected.backend_to_clean_node.get(&bid) else {
                    effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                    return StepResult::Done;
                };
                if let Some(obj) = st.params.as_object_mut() {
                    obj.insert("nodeId".into(), Value::from(fresh_nid));
                }
                st.stage = NodeStage::Exec;
                effects.push(send(job_id, &st.method, st.params.clone()));
                return StepResult::Continue;
            }
            if state.protected.node_ids.contains(nid) {
                effects.push(refuse(req_id, SUBFRAME_REFUSAL));
                return StepResult::Done;
            }
            if state.protected.clean_node_ids.contains(nid) {
                st.stage = NodeStage::Exec;
                effects.push(send(job_id, &st.method, st.params.clone()));
                return StepResult::Continue;
            }
            st.stage = NodeStage::ResolveTarget;
            effects.push(send(
                job_id,
                "DOM.describeNode",
                json!({ "nodeId": nid, "depth": 0 }),
            ));
            StepResult::Continue
        }
        DomTarget::ObjectId(oid) => {
            st.stage = NodeStage::ResolveTarget;
            effects.push(send(
                job_id,
                "DOM.describeNode",
                json!({ "objectId": oid, "depth": 0 }),
            ));
            StepResult::Continue
        }
        DomTarget::None => {
            effects.push(refuse(req_id, SUBFRAME_REFUSAL));
            StepResult::Done
        }
    }
}

fn record_owner_ids(val: &Value, protected: &mut ProtectedDomNodes) -> bool {
    let Some(bid) = val.get("backendNodeId").and_then(Value::as_i64) else {
        return false;
    };
    protected.backend_ids.insert(bid);
    protected.clean_backend_ids.remove(&bid);
    protected.backend_to_clean_node.remove(&bid);
    if let Some(nid) = val.get("nodeId").and_then(Value::as_i64)
        && nid > 0
    {
        protected.node_ids.insert(nid);
        protected.clean_node_ids.remove(&nid);
    }
    true
}
