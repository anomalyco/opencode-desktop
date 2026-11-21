//! Frames, deep accessibility snapshot (with OOPIF `sessionId` propagation + post-capture
//! generation/target re-check), find, and bounded sensitive-field redaction executors.

use std::collections::HashMap;

use browser_pane::tools::field_value::{DESCRIBE_FIELD_JS, FieldInfo, readable_field};
use browser_pane::tools::frames::parse_frame_tree;
use browser_pane::tools::snapshot::{
    DEFAULT_MAX_CHARS, PreparedSnapshot, SnapshotOptions, prepare_snapshot,
};
use opencode_kit::browser_rpc::{Command, Outcome, TargetMeta, failure_code};
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::read::{queue_live_frame_refresh, step_live_frame_refresh};
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards, send_rpc_cdp, send_rpc_cdp_in_frame, verify_authorized_state};
use crate::App;

const MAX_SNAPSHOT_PROBES: usize = 200;

pub(super) struct FramesState {
    pub host_id: u64,
    pub pending: Vec<(u64, Option<String>)>,
    pub top_tree: Option<Value>,
}

pub(super) struct SnapshotState {
    pub host_id: u64,
    pub frame_id: Option<String>,
    pub cdp_session: Option<String>,
    pub expected_gen: u64,
    pub expected_target: Option<TargetMeta>,
    pub cmd: Command,
    pub opts: SnapshotOptions,
    pub stage: SnapshotStage,
}

impl SnapshotState {
    pub fn cancel_cleanup(&mut self, app: &mut App) {
        match &mut self.stage {
            SnapshotStage::CheckFrame { pending, .. } | SnapshotStage::VerifyPost { pending, .. } => {
                for (req_id, _) in pending.drain(..) {
                    app.workspace_panes.browser.host.cancel_cdp(req_id);
                }
            }
            SnapshotStage::FetchAx { cdp_req } | SnapshotStage::FetchBoxes { cdp_req, .. } => {
                app.workspace_panes.browser.host.cancel_cdp(*cdp_req);
            }
            SnapshotStage::ResolveFields { step, .. } => match step {
                FieldResolveStep::ResolveNode { cdp_req, .. } => {
                    app.workspace_panes.browser.host.cancel_cdp(*cdp_req);
                }
                FieldResolveStep::CallDescribe { object_id, cdp_req, .. } => {
                    app.workspace_panes.browser.host.cancel_cdp(*cdp_req);
                    let _ = send_rpc_cdp(
                        app,
                        self.host_id,
                        "Runtime.releaseObject",
                        json!({ "objectId": object_id }),
                        self.cdp_session.clone(),
                    );
                }
            },
        }
    }
}

pub(super) enum SnapshotStage {
    CheckFrame { pending: Vec<(u64, Option<String>)>, top_tree: Option<Value> },
    FetchAx { cdp_req: u64 },
    ResolveFields {
        prepared: PreparedSnapshot,
        queue: Vec<(i64, String)>,
        readable: HashMap<i64, bool>,
        boxes: HashMap<i64, String>,
        box_queue: Vec<i64>,
        step: FieldResolveStep,
    },
    FetchBoxes {
        prepared: PreparedSnapshot,
        readable: HashMap<i64, bool>,
        boxes: HashMap<i64, String>,
        box_queue: Vec<i64>,
        current_id: i64,
        cdp_req: u64,
    },
    VerifyPost {
        pending: Vec<(u64, Option<String>)>,
        prepared: PreparedSnapshot,
        readable: HashMap<i64, bool>,
        boxes: HashMap<i64, String>,
    },
}

pub(super) enum FieldResolveStep {
    ResolveNode { id: i64, name: String, cdp_req: u64 },
    CallDescribe { id: i64, name: String, object_id: String, cdp_req: u64 },
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
) -> Option<StepOutcome> {
    match cmd.action_type()? {
        "frames" => Some(start_frames(att, app, cmd)),
        "snapshot" => Some(start_snapshot(att, app, cmd, false)),
        "find" => Some(start_snapshot(att, app, cmd, true)),
        _ => None,
    }
}

fn start_frames(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, "frames") {
        return StepOutcome::Done(o);
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let pending = queue_live_frame_refresh(att, app, host_id);
    if pending.is_empty() {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Failed to query live frame tree on this tab.",
        ));
    }
    StepOutcome::Pending(ExecPhase::Frames(FramesState {
        host_id,
        pending,
        top_tree: None,
    }))
}

fn start_snapshot(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
    is_find: bool,
) -> StepOutcome {
    let Some(wire_id) = cmd.tab_id() else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."));
    };
    let host_id = match att.resolve_tab(app, wire_id) {
        Ok(id) => id,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    if let Err(o) = check_page_guards(app, host_id, if is_find { "find" } else { "snapshot" }) {
        return StepOutcome::Done(o);
    }
    let (root, ref_session, ref_frame) = if let Some(r) = cmd.action.get("ref").and_then(Value::as_str) {
        match att.refs.get(&host_id).and_then(|t| t.resolve(r).map(|(b, s)| (b, s, t.frame_of(r)))) {
            Some((bid, sid, rf)) => (Some(bid), sid, rf),
            None => {
                return StepOutcome::Done(Outcome::fail(
                    failure_code::STALE_REF,
                    format!("Element ref {r} is stale or absent. Refresh the snapshot without ref."),
                ));
            }
        }
    } else {
        (None, None, None)
    };
    let frame_id = cmd.action.get("frameID").and_then(Value::as_str).map(str::to_owned).or(ref_frame);
    let opts = SnapshotOptions {
        root,
        depth: cmd.action.get("depth").and_then(Value::as_u64).map(|v| v as usize),
        text: if is_find {
            cmd.action.get("text").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        },
        interactive: cmd.action.get("interactive").and_then(Value::as_bool).unwrap_or(false),
        boxes: cmd.action.get("boxes").and_then(Value::as_bool).unwrap_or(false),
        max_lines: cmd.action.get("maxLines").and_then(Value::as_u64).map(|v| v as usize),
        max_chars: cmd
            .action
            .get("maxChars")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_CHARS, |v| (v as usize).clamp(1_000, 100_000)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let expected_gen = cmd.generation.unwrap_or_else(|| att.generation_of(host_id));
    let expected_target = cmd.target.clone();
    if frame_id.is_some() {
        let pending = queue_live_frame_refresh(att, app, host_id);
        if pending.is_empty() {
            return StepOutcome::Done(Outcome::fail(
                failure_code::OPERATION_FAILED,
                "Failed to query live frame tree on this tab.",
            ));
        }
        StepOutcome::Pending(ExecPhase::Snapshot(SnapshotState {
            host_id,
            frame_id,
            cdp_session: None,
            expected_gen,
            expected_target,
            cmd: cmd.clone(),
            opts,
            stage: SnapshotStage::CheckFrame { pending, top_tree: None },
        }))
    } else {
        match send_rpc_cdp(
            app,
            host_id,
            "Accessibility.getFullAXTree",
            json!({}),
            ref_session.clone(),
        ) {
            Ok(cdp_req) => StepOutcome::Pending(ExecPhase::Snapshot(SnapshotState {
                host_id,
                frame_id: None,
                cdp_session: ref_session,
                expected_gen,
                expected_target,
                cmd: cmd.clone(),
                opts,
                stage: SnapshotStage::FetchAx { cdp_req },
            })),
            Err(e) => StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, e)),
        }
    }
}

pub(super) fn step_frames(att: &mut SessionAttachment, app: &mut App, st: &mut FramesState) -> Option<Outcome> {
    match step_live_frame_refresh(att, app, st.host_id, &mut st.pending, Some(&mut st.top_tree)) {
        Ok(false) => return None,
        Ok(true) => {}
        Err(msg) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    }
    let Some(tab) = att.wire_tab(app, st.host_id) else {
        return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
    };
    let frames = att
        .frames
        .get(&st.host_id)
        .map(|f| f.agent_frames(&tab.url))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            st.top_tree
                .as_ref()
                .and_then(|val| serde_json::to_value(parse_frame_tree(val).frames).ok())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
        });
    Some(Outcome::ok(json!({ "tab": tab, "frames": frames }), Vec::new()))
}

pub(super) fn step_snapshot(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut SnapshotState,
    ref_counter: &mut u64,
) -> Option<Outcome> {
    match &mut st.stage {
        SnapshotStage::CheckFrame { pending, top_tree } => {
            match step_live_frame_refresh(att, app, st.host_id, pending, Some(top_tree)) {
                Ok(false) => return None,
                Ok(true) => {}
                Err(msg) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
            }
            if let Err(o) = verify_authorized_state(
                att,
                app,
                &st.cmd,
                Some(st.expected_gen),
                st.expected_target.as_ref(),
            ) {
                return Some(o);
            }
            let val = top_tree.clone().unwrap_or(Value::Null);
            let tree = parse_frame_tree(&val);
            let want = st.frame_id.as_deref().unwrap_or("");
            let is_tracked_ext = att
                .frames
                .get(&st.host_id)
                .is_some_and(|f| f.extension_frames.contains(want));
            if tree.extension_frame_ids.contains(want) || is_tracked_ext {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Agent browser tools cannot inspect extension frames.",
                ));
            }
            let (cap_exceeded, tracked_known, oopif_session) = att
                .frames
                .get(&st.host_id)
                .map(|f| (f.frame_cap_exceeded, f.frame_urls.contains_key(want), f.frame_sessions.get(want).cloned()))
                .unwrap_or((false, false, None));
            if cap_exceeded {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Tab exceeded tracked frame cap; cannot verify frame authorization safely.",
                ));
            }
            if !tracked_known || (!tree.frames.iter().any(|f| f.id == want) && oopif_session.is_none()) {
                return Some(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "Frame ID not found in tracked frames. Call browser.frames({tabID}).",
                ));
            }
            st.cdp_session = oopif_session.clone();
            let ax_params = json!({ "frameId": want });
            match send_rpc_cdp(app, st.host_id, "Accessibility.getFullAXTree", ax_params, oopif_session) {
                Ok(req) => {
                    st.stage = SnapshotStage::FetchAx { cdp_req: req };
                    None
                }
                Err(e) => Some(Outcome::fail(failure_code::OPERATION_FAILED, e)),
            }
        }
        SnapshotStage::FetchAx { cdp_req } => {
            let resp = app.workspace_panes.browser.host.take_cdp_response(*cdp_req)?;
            let val = match resp {
                Ok(v) => v,
                Err(e) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e))),
            };
            let nodes = val.get("nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            let empty_map = HashMap::new();
            let prev = att
                .refs
                .get(&st.host_id)
                .map(|r| r.previous_for_session(st.cdp_session.as_deref()))
                .unwrap_or_default();
            let picked = if st.cdp_session.is_none() {
                att.refs.get(&st.host_id).map(|r| &r.picked).unwrap_or(&empty_map)
            } else {
                &empty_map
            };
            let prepared = match prepare_snapshot(nodes, st.opts.clone(), &prev, picked) {
                Ok(p) => p,
                Err(e) => return Some(Outcome::fail(failure_code::STALE_REF, e)),
            };
            let mut val_queue = prepared.value_targets();
            val_queue.truncate(MAX_SNAPSHOT_PROBES);
            let mut box_queue = prepared.box_targets();
            box_queue.truncate(MAX_SNAPSHOT_PROBES);
            if let Some((first_id, first_name)) = val_queue.pop() {
                match send_rpc_cdp_in_frame(
                    app,
                    st.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": first_id }),
                    st.cdp_session.clone(),
                    st.frame_id.clone(),
                ) {
                    Ok(req) => {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue: val_queue,
                            readable: HashMap::new(),
                            boxes: HashMap::new(),
                            box_queue,
                            step: FieldResolveStep::ResolveNode { id: first_id, name: first_name, cdp_req: req },
                        };
                        None
                    }
                    Err(_) => Some(finish_snapshot(att, app, st, prepared, &HashMap::new(), &HashMap::new(), ref_counter)),
                }
            } else if !box_queue.is_empty() {
                advance_boxes_or_finish(att, app, st, prepared, HashMap::new(), HashMap::new(), box_queue, ref_counter)
            } else {
                Some(finish_snapshot(att, app, st, prepared, &HashMap::new(), &HashMap::new(), ref_counter))
            }
        }
        SnapshotStage::ResolveFields { .. } => {
            let SnapshotStage::ResolveFields {
                prepared,
                mut queue,
                mut readable,
                boxes,
                box_queue,
                step,
            } = std::mem::replace(&mut st.stage, SnapshotStage::FetchAx { cdp_req: 0 })
            else {
                return None;
            };
            match step {
                FieldResolveStep::ResolveNode { id, name, cdp_req } => {
                    let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::ResolveNode { id, name, cdp_req },
                        };
                        return None;
                    };
                    let object_id = resp.ok().and_then(|v| {
                        v.get("object")
                            .and_then(|o| o.get("objectId"))
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    });
                    if let Some(oid) = object_id
                        && let Ok(next_req) = send_rpc_cdp(
                            app,
                            st.host_id,
                            "Runtime.callFunctionOn",
                            json!({
                                "objectId": oid,
                                "functionDeclaration": DESCRIBE_FIELD_JS,
                                "returnByValue": true,
                                "userGesture": false,
                            }),
                            st.cdp_session.clone(),
                        )
                    {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::CallDescribe { id, name, object_id: oid, cdp_req: next_req },
                        };
                        return None;
                    }
                    readable.insert(id, false);
                }
                FieldResolveStep::CallDescribe { id, name, object_id, cdp_req } => {
                    let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                        st.stage = SnapshotStage::ResolveFields {
                            prepared,
                            queue,
                            readable,
                            boxes,
                            box_queue,
                            step: FieldResolveStep::CallDescribe { id, name, object_id, cdp_req },
                        };
                        return None;
                    };
                    let _ = send_rpc_cdp(
                        app,
                        st.host_id,
                        "Runtime.releaseObject",
                        json!({ "objectId": object_id }),
                        st.cdp_session.clone(),
                    );
                    let ok_read = resp
                        .ok()
                        .and_then(|v| v.get("result").and_then(|r| r.get("value")).and_then(FieldInfo::from_value))
                        .is_some_and(|fi| readable_field(&fi, &name));
                    readable.insert(id, ok_read);
                }
            }
            if let Some((next_id, next_name)) = queue.pop()
                && let Ok(req) = send_rpc_cdp_in_frame(
                    app,
                    st.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": next_id }),
                    st.cdp_session.clone(),
                    st.frame_id.clone(),
                )
            {
                st.stage = SnapshotStage::ResolveFields {
                    prepared,
                    queue,
                    readable,
                    boxes,
                    box_queue,
                    step: FieldResolveStep::ResolveNode { id: next_id, name: next_name, cdp_req: req },
                };
                return None;
            }
            advance_boxes_or_finish(att, app, st, prepared, readable, boxes, box_queue, ref_counter)
        }
        SnapshotStage::FetchBoxes { .. } => {
            let SnapshotStage::FetchBoxes {
                prepared,
                readable,
                mut boxes,
                box_queue,
                current_id,
                cdp_req,
            } = std::mem::replace(&mut st.stage, SnapshotStage::FetchAx { cdp_req: 0 })
            else {
                return None;
            };
            let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                st.stage = SnapshotStage::FetchBoxes {
                    prepared,
                    readable,
                    boxes,
                    box_queue,
                    current_id,
                    cdp_req,
                };
                return None;
            };
            if let Ok(v) = resp
                && let Some(rect_str) = format_quad_box(&v)
            {
                boxes.insert(current_id, rect_str);
            }
            advance_boxes_or_finish(att, app, st, prepared, readable, boxes, box_queue, ref_counter)
        }
        SnapshotStage::VerifyPost { .. } => {
            let SnapshotStage::VerifyPost { mut pending, prepared, readable, boxes } =
                std::mem::replace(&mut st.stage, SnapshotStage::FetchAx { cdp_req: 0 })
            else {
                return None;
            };
            match step_live_frame_refresh(att, app, st.host_id, &mut pending, None) {
                Ok(false) => {
                    st.stage = SnapshotStage::VerifyPost { pending, prepared, readable, boxes };
                    return None;
                }
                Ok(true) => {}
                Err(msg) => return Some(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
            }
            Some(finish_snapshot(att, app, st, prepared, &readable, &boxes, ref_counter))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn advance_boxes_or_finish(
    att: &mut SessionAttachment,
    app: &mut App,
    st: &mut SnapshotState,
    prepared: PreparedSnapshot,
    readable: HashMap<i64, bool>,
    boxes: HashMap<i64, String>,
    mut box_queue: Vec<i64>,
    ref_counter: &mut u64,
) -> Option<Outcome> {
    if let Some(next_id) = box_queue.pop()
        && let Ok(req) = send_rpc_cdp_in_frame(
            app,
            st.host_id,
            "DOM.getContentQuads",
            json!({ "backendNodeId": next_id }),
            st.cdp_session.clone(),
            st.frame_id.clone(),
        )
    {
        st.stage = SnapshotStage::FetchBoxes {
            prepared,
            readable,
            boxes,
            box_queue,
            current_id: next_id,
            cdp_req: req,
        };
        return None;
    }
    if st.expected_target.is_some() && app.workspace_panes.browser.host.tab_open_in_engine(st.host_id) {
        let pending = queue_live_frame_refresh(att, app, st.host_id);
        if !pending.is_empty() {
            st.stage = SnapshotStage::VerifyPost { pending, prepared, readable, boxes };
            return None;
        }
    }
    Some(finish_snapshot(att, app, st, prepared, &readable, &boxes, ref_counter))
}

fn finish_snapshot(
    att: &mut SessionAttachment,
    app: &App,
    st: &SnapshotState,
    prepared: PreparedSnapshot,
    readable: &HashMap<i64, bool>,
    boxes: &HashMap<i64, String>,
    ref_counter: &mut u64,
) -> Outcome {
    // Re-verify generation and target before returning snapshot output so a redirect mid-snapshot
    // cannot leak content from an unauthorized origin.
    if let Err(o) = verify_authorized_state(
        att,
        app,
        &st.cmd,
        Some(st.expected_gen),
        st.expected_target.as_ref(),
    ) {
        return o;
    }
    let host_id = st.host_id;
    let empty_map = HashMap::new();
    let prev = att
        .refs
        .get(&host_id)
        .map(|r| r.previous_for_session(st.cdp_session.as_deref()))
        .unwrap_or_default();
    let picked = if st.cdp_session.is_none() {
        att.refs.get(&host_id).map(|r| &r.picked).unwrap_or(&empty_map)
    } else {
        &empty_map
    };
    let out = prepared.finish(readable, boxes, &prev, picked, &mut || {
        *ref_counter = ref_counter.saturating_add(1);
        format!("e{ref_counter}")
    });
    att.refs
        .entry(host_id)
        .or_default()
        .update_from_snapshot(out.refs, st.cdp_session.clone(), st.frame_id.clone());
    let Some(tab) = att.wire_tab(app, host_id) else {
        return Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable.");
    };
    Outcome::ok(json!({ "tab": tab, "content": out.content, "truncated": out.truncated }), Vec::new())
}

fn format_quad_box(v: &Value) -> Option<String> {
    let quad = v.get("quads")?.as_array()?.first()?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let xs = [0, 2, 4, 6].map(|i| quad[i].as_f64().unwrap_or(0.0));
    let ys = [1, 3, 5, 7].map(|i| quad[i].as_f64().unwrap_or(0.0));
    let min_x = xs.into_iter().fold(f64::INFINITY, f64::min).round() as i64;
    let min_y = ys.into_iter().fold(f64::INFINITY, f64::min).round() as i64;
    let max_x = xs.into_iter().fold(f64::NEG_INFINITY, f64::max).round() as i64;
    let max_y = ys.into_iter().fold(f64::NEG_INFINITY, f64::max).round() as i64;
    let w = (max_x - min_x).max(0);
    let h = (max_y - min_y).max(0);
    Some(format!("{min_x},{min_y} {w}x{h}"))
}
