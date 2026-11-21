//! Acting executors for `experimental.browser` v4 (`click`, `hover`, `drag`, `fill`, `fill_form`,
//! `select`, `check`, `press`, and `scroll`).

use std::collections::VecDeque;
use std::time::Duration;

use browser_pane::tools::keys::{CdpKeySpec, parse_key_chord};
use opencode_kit::browser_rpc::{Command, Outcome, failure_code};
use rune::time::Instant;
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::point::PointResolver;
use super::{ExecPhase, StepOutcome, cdp_err_msg, check_page_guards};
use crate::App;

const DRAG_INTERCEPT_WAIT: Duration = Duration::from_secs(2);
const CHECK_DRAGGABLE_JS: &str = "function() { return Boolean(this.draggable); }";

const CHECK_EDITABLE_JS: &str = r#"function() {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  if (this.disabled || this.readOnly) return '';
  if (this instanceof HTMLTextAreaElement || this.isContentEditable) return 'text';
  if (!(this instanceof HTMLInputElement)) return '';
  if (['date','time','datetime-local','month','week'].includes(this.type)) return 'structured';
  if (!['file','checkbox','radio','button','submit','reset','image','hidden','range','color'].includes(this.type)) return 'text';
  return '';
}"#;

const SET_STRUCTURED_JS: &str = r#"function(value) {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  const previous = this.value;
  this.focus();
  this.value = value;
  if (this.value !== value) {
    this.value = previous;
    throw new Error('The ' + this.type + ' input rejected this value and keeps its previous one.');
  }
  this.dispatchEvent(new Event('input', { bubbles: true }));
  this.dispatchEvent(new Event('change', { bubbles: true }));
  return true;
}"#;

const SELECT_OPTIONS_JS: &str = r#"function(values) {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  if (!(this instanceof HTMLSelectElement) || this.disabled)
    throw new Error('Target is not an enabled HTML select. Take a fresh snapshot and choose an enabled dropdown ref.');
  if (!this.multiple && values.length !== 1)
    throw new Error('This dropdown accepts exactly one value; pass a one-item values array.');
  for (const value of values)
    if (!Array.from(this.options).some(o => o.value === value && !o.disabled))
      throw new Error('Option value was not found or is disabled. Inspect option values with browser.evaluate before retrying browser.select.');
  for (const option of this.options) option.selected = values.includes(option.value);
  this.dispatchEvent(new Event('input', { bubbles: true }));
  this.dispatchEvent(new Event('change', { bubbles: true }));
  return true;
}"#;

const CHECK_PRE_JS: &str = r#"function(checked) {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  if (!(this instanceof HTMLInputElement) || !['checkbox','radio'].includes(this.type) || this.disabled)
    throw new Error('Target is not an enabled checkbox or radio. Take a fresh snapshot and choose the correct ref.');
  if (this.type === 'radio' && this.checked && !checked)
    throw new Error('A selected radio cannot be cleared by clicking it. Select a different radio in its group instead.');
  return Boolean(this.checked);
}"#;

const CHECK_POST_JS: &str = r#"function() {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  return Boolean(this.checked);
}"#;

/// An element: its `backendNodeId`, the OOPIF CDP session that owns it (`None`: the page), and
/// the frame ID its snapshot came from (verified by `#350`'s engine frame authority).
type Target = (i64, Option<String>, Option<String>);

pub(super) enum InputStep {
    ResolvePoint(Target),
    ResolvePointNoScroll(Target),
    StoreFromCenter,
    DispatchMouse { kind: &'static str, button: String, click_count: u32, modifiers: u32 },
    SendInput { method: &'static str, params: Value },
    CheckDraggable,
    SetInterceptDrags(bool),
    ExpandDragMotion,
    WaitDragData,
    ExpandFinishDrag,
    ResolveNode(Target),
    CheckEditable { bid: Target, text: String },
    SetStructured { text: String },
    FocusNode(Target),
    InsertText(String),
    DispatchKey(CdpKeySpec),
    CallSelect(Vec<String>),
    CallCheckPre { bid: Target, checked: bool },
    CallCheckPost { checked: bool },
    ReleaseObject,
    GetViewportCenter,
    DispatchWheel { dx: i64, dy: i64 },
}

pub(super) struct InputState {
    pub host_id: u64,
    steps: VecDeque<InputStep>,
    current: Option<(InputStep, u64)>,
    point: Option<PointResolver>,
    saved_pos: Option<(f64, f64)>,
    saved_from: Option<(f64, f64)>,
    saved_obj: Option<String>,
    obj_session: Option<String>,
    obj_frame: Option<String>,
    html5_drag: bool,
    intercepting_drags: bool,
    drag_pressed: bool,
    drag_data: Option<Value>,
    drag_deadline: Option<Instant>,
}

impl InputState {
    pub fn next_wake(&self) -> Option<Instant> {
        self.drag_deadline
    }

    pub fn ingest_drag_intercepted(&mut self, data: Value) {
        if self.intercepting_drags {
            self.drag_data = Some(data);
            self.drag_deadline = None;
        }
    }

    pub fn cancel_cleanup(&mut self, app: &mut App) {
        if let Some(mut p) = self.point.take() {
            p.cancel_cleanup(app);
        }
        if let Some((_, req_id)) = self.current.take() {
            app.workspace_panes.browser.host.cancel_cdp(req_id);
        }
        if self.intercepting_drags {
            self.intercepting_drags = false;
            let _ = app.workspace_panes.browser.host.send_typed_cdp(
                self.host_id,
                "Input.setInterceptDrags",
                json!({ "enabled": false }),
                None,
            );
        }
        if self.drag_pressed {
            self.drag_pressed = false;
            if let Some((x, y)) = self.saved_pos.or(self.saved_from) {
                let _ = app.workspace_panes.browser.host.send_typed_cdp(
                    self.host_id,
                    "Input.dispatchMouseEvent",
                    json!({ "type": "mouseReleased", "x": x.round(), "y": y.round(), "button": "left", "clickCount": 1 }),
                    None,
                );
            }
        }
        release_saved_obj(app, self);
    }
}

pub(super) fn try_start(
    att: &mut SessionAttachment,
    app: &mut App,
    cmd: &Command,
) -> Option<StepOutcome> {
    match cmd.action_type()? {
        "click" => Some(start_click(att, app, cmd)),
        "hover" => Some(start_hover(att, app, cmd)),
        "drag" => Some(start_drag(att, app, cmd)),
        "fill" => Some(start_fill(att, app, cmd)),
        "fill_form" => Some(start_fill_form(att, app, cmd)),
        "select" => Some(start_select(att, app, cmd)),
        "check" => Some(start_check(att, app, cmd)),
        "press" => Some(start_press(att, app, cmd)),
        "scroll" => Some(start_scroll(att, app, cmd)),
        _ => None,
    }
}

fn resolve_tab(att: &SessionAttachment, app: &App, cmd: &Command, action: &str) -> Result<u64, Outcome> {
    let wire_id = cmd
        .tab_id()
        .ok_or_else(|| Outcome::fail(failure_code::OPERATION_FAILED, "Missing tabID."))?;
    let host_id = att
        .resolve_tab(app, wire_id)
        .map_err(|msg| Outcome::fail(failure_code::OPERATION_FAILED, msg))?;
    check_page_guards(app, host_id, action)?;
    if !app.workspace_panes.browser.host.engine_agent_tools() {
        return Err(Outcome::fail(failure_code::UNSUPPORTED, "Acting on pages requires an updated browser engine."));
    }
    Ok(host_id)
}

fn resolve_ref(att: &SessionAttachment, host_id: u64, cmd: &Command, field: &str) -> Result<Target, Outcome> {
    let raw_ref = cmd
        .action
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| Outcome::fail(failure_code::OPERATION_FAILED, format!("Missing {field}.")))?;
    att.refs
        .get(&host_id)
        .and_then(|r| r.resolve(raw_ref).map(|(bid, sid)| (bid, sid, r.frame_of(raw_ref))))
        .ok_or_else(|| {
            Outcome::fail(
                failure_code::STALE_REF,
                format!("Element ref {raw_ref} is stale or absent. Refresh the snapshot first."),
            )
        })
}

fn start_click(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "click") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let bid = match resolve_ref(att, host_id, cmd, "ref") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    let button = cmd.action.get("button").and_then(Value::as_str).unwrap_or("left").to_owned();
    #[cfg(target_os = "linux")]
    if button == "middle" {
        return StepOutcome::Done(Outcome::fail(
            failure_code::OPERATION_FAILED,
            "Middle-click can paste the system selection on Linux. Use fill to enter text and evaluate to read page text.",
        ));
    }
    let count = cmd.action.get("count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 2) as u32;
    let modifiers = parse_modifiers(cmd.action.get("modifiers"));
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let mut steps = VecDeque::new();
    push_click_steps(&mut steps, bid, button, count, modifiers);
    launch_steps(att, app, host_id, steps)
}

fn start_hover(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "hover") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let bid = match resolve_ref(att, host_id, cmd, "ref") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let mut steps = VecDeque::new();
    steps.push_back(InputStep::ResolvePoint(bid));
    steps.push_back(InputStep::DispatchMouse { kind: "mouseMoved", button: "none".into(), click_count: 0, modifiers: 0 });
    launch_steps(att, app, host_id, steps)
}

fn start_drag(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "drag") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let from_bid = match resolve_ref(att, host_id, cmd, "from") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    let to_bid = match resolve_ref(att, host_id, cmd, "to") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    // Match official `chromium.ts:1812-1815`: scroll destination first, then scroll source and
    // save `from`, then re-measure destination WITHOUT scrolling so `from` stays in place.
    let mut steps = VecDeque::new();
    steps.push_back(InputStep::ResolvePoint(to_bid.clone()));
    steps.push_back(InputStep::ResolvePoint(from_bid.clone()));
    steps.push_back(InputStep::StoreFromCenter);
    steps.push_back(InputStep::ResolvePointNoScroll(to_bid));
    steps.push_back(InputStep::ResolveNode(from_bid));
    steps.push_back(InputStep::CheckDraggable);
    steps.push_back(InputStep::ReleaseObject);
    steps.push_back(InputStep::SetInterceptDrags(true));
    steps.push_back(InputStep::ExpandDragMotion);
    steps.push_back(InputStep::WaitDragData);
    steps.push_back(InputStep::ExpandFinishDrag);
    launch_steps(att, app, host_id, steps)
}

fn start_fill(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "fill") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let bid = match resolve_ref(att, host_id, cmd, "ref") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    let Some(text) = cmd.action.get("text").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing text."));
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let mut steps = VecDeque::new();
    push_fill_steps(&mut steps, bid, text.to_owned());
    launch_steps(att, app, host_id, steps)
}

fn start_select(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "select") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let bid = match resolve_ref(att, host_id, cmd, "ref") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    let values = str_array(cmd.action.get("values"));
    if values.is_empty() {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "browser.select requires values."));
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let mut steps = VecDeque::new();
    push_select_steps(&mut steps, bid, values);
    launch_steps(att, app, host_id, steps)
}

fn start_check(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "check") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let bid = match resolve_ref(att, host_id, cmd, "ref") {
        Ok(b) => b,
        Err(o) => return StepOutcome::Done(o),
    };
    let Some(checked) = cmd.action.get("checked").and_then(Value::as_bool) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing checked boolean."));
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    let mut steps = VecDeque::new();
    push_check_steps(&mut steps, bid, checked);
    launch_steps(att, app, host_id, steps)
}

fn start_fill_form(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "fill_form") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let Some(fields) = cmd.action.get("fields").and_then(Value::as_array) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing fields."));
    };
    let mut steps = VecDeque::new();
    for f in fields {
        let Some(r) = f.get("ref").and_then(Value::as_str) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Form field missing ref."));
        };
        let Some(bid) = att.refs.get(&host_id).and_then(|t| t.resolve(r).map(|(b, s)| (b, s, t.frame_of(r)))) else {
            return StepOutcome::Done(Outcome::fail(
                failure_code::STALE_REF,
                format!("Element ref {r} is stale or absent. Refresh the snapshot first."),
            ));
        };
        match f.get("type").and_then(Value::as_str) {
            Some("text") => push_fill_steps(&mut steps, bid, f.get("value").and_then(Value::as_str).unwrap_or("").to_owned()),
            Some("select") => push_select_steps(&mut steps, bid, str_array(f.get("values"))),
            Some("check") => push_check_steps(&mut steps, bid, f.get("checked").and_then(Value::as_bool).unwrap_or(false)),
            _ => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Unknown fill_form field type.")),
        }
    }
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    if steps.is_empty() {
        let Some(tab) = att.wire_tab(app, host_id) else {
            return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
        };
        return StepOutcome::Done(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
    }
    launch_steps(att, app, host_id, steps)
}

fn start_press(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "press") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let Some(raw_key) = cmd.action.get("key").and_then(Value::as_str) else {
        return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, "Missing key."));
    };
    let spec = match parse_key_chord(raw_key) {
        Ok(s) => s,
        Err(msg) => return StepOutcome::Done(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    };
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    launch_steps(att, app, host_id, VecDeque::from([InputStep::DispatchKey(spec)]))
}

fn start_scroll(att: &mut SessionAttachment, app: &mut App, cmd: &Command) -> StepOutcome {
    let host_id = match resolve_tab(att, app, cmd, "scroll") {
        Ok(id) => id,
        Err(o) => return StepOutcome::Done(o),
    };
    let dx = cmd.action.get("deltaX").and_then(Value::as_i64).unwrap_or(0).clamp(-10_000, 10_000);
    let dy = cmd.action.get("deltaY").and_then(Value::as_i64).unwrap_or(0).clamp(-10_000, 10_000);
    app.workspace_panes.browser.host.set_agent_driven(host_id, true);
    launch_steps(att, app, host_id, VecDeque::from([InputStep::GetViewportCenter, InputStep::DispatchWheel { dx, dy }]))
}

fn push_click_steps(steps: &mut VecDeque<InputStep>, bid: Target, button: String, count: u32, modifiers: u32) {
    steps.push_back(InputStep::ResolvePoint(bid));
    steps.push_back(InputStep::DispatchMouse { kind: "mouseMoved", button: "none".into(), click_count: 0, modifiers });
    for click_count in 1..=count {
        steps.push_back(InputStep::DispatchMouse { kind: "mousePressed", button: button.clone(), click_count, modifiers });
        steps.push_back(InputStep::DispatchMouse { kind: "mouseReleased", button: button.clone(), click_count, modifiers });
    }
}

fn push_fill_steps(steps: &mut VecDeque<InputStep>, bid: Target, text: String) {
    steps.push_back(InputStep::ResolveNode(bid.clone()));
    steps.push_back(InputStep::CheckEditable { bid, text });
}

fn push_select_steps(steps: &mut VecDeque<InputStep>, bid: Target, values: Vec<String>) {
    steps.push_back(InputStep::ResolveNode(bid));
    steps.push_back(InputStep::CallSelect(values));
    steps.push_back(InputStep::ReleaseObject);
}

fn push_check_steps(steps: &mut VecDeque<InputStep>, bid: Target, checked: bool) {
    steps.push_back(InputStep::ResolveNode(bid.clone()));
    steps.push_back(InputStep::CallCheckPre { bid, checked });
}

fn launch_steps(att: &SessionAttachment, app: &mut App, host_id: u64, steps: VecDeque<InputStep>) -> StepOutcome {
    let mut state = InputState {
        host_id,
        steps,
        current: None,
        point: None,
        saved_pos: None,
        saved_from: None,
        saved_obj: None,
        obj_session: None,
        obj_frame: None,
        html5_drag: false,
        intercepting_drags: false,
        drag_pressed: false,
        drag_data: None,
        drag_deadline: None,
    };
    if let Some(err) = dispatch_next_step(att, app, &mut state, Instant::now()) {
        state.cancel_cleanup(app);
        return StepOutcome::Done(err);
    }
    StepOutcome::Pending(ExecPhase::Input(state))
}

pub(super) fn step_input(att: &SessionAttachment, app: &mut App, st: &mut InputState, now: Instant) -> Option<Outcome> {
    if let Some(mut resolver) = st.point.take() {
        match resolver.step(app) {
            None => {
                st.point = Some(resolver);
                return None;
            }
            Some(Ok(pt)) => st.saved_pos = Some(pt),
            Some(Err(outcome)) => {
                st.cancel_cleanup(app);
                return Some(outcome);
            }
        }
    }
    if let Some(deadline) = st.drag_deadline {
        if st.drag_data.is_some() || now >= deadline {
            st.drag_deadline = None;
        } else {
            return None;
        }
    }
    if let Some((step, req_id)) = st.current.take() {
        let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(req_id) else {
            st.current = Some((step, req_id));
            return None;
        };
        if let Err(outcome) = handle_step_response(st, step, resp) {
            st.cancel_cleanup(app);
            return Some(outcome);
        }
    }
    loop {
        if st.steps.is_empty() {
            st.cancel_cleanup(app);
            let Some(tab) = att.wire_tab(app, st.host_id) else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Tab unavailable."));
            };
            return Some(Outcome::ok(serde_json::to_value(tab).unwrap_or(Value::Null), Vec::new()));
        }
        if let Some(err) = dispatch_next_step(att, app, st, now) {
            st.cancel_cleanup(app);
            return Some(err);
        }
        if st.current.is_some() || st.point.is_some() || st.drag_deadline.is_some() {
            return None;
        }
    }
}

fn call_fn(app: &mut App, st: &InputState, decl: &str, arg: Option<Value>) -> Result<Option<u64>, String> {
    let Some(oid) = st.saved_obj.as_deref() else {
        return Err("Element ref could not be resolved.".into());
    };
    let mut params = json!({
        "objectId": oid,
        "functionDeclaration": decl,
        "returnByValue": true,
        "userGesture": false,
    });
    if let Some(a) = arg {
        params["arguments"] = json!([{ "value": a }]);
    }
    app.workspace_panes.browser.host.send_typed_cdp_in_frame(st.host_id, "Runtime.callFunctionOn", params, st.obj_session.clone(), st.obj_frame.clone()).map(Some)
}

fn dispatch_next_step(att: &SessionAttachment, app: &mut App, st: &mut InputState, now: Instant) -> Option<Outcome> {
    let step = st.steps.pop_front()?;
    let hid = st.host_id;
    let res: Result<Option<u64>, String> = match &step {
        InputStep::ResolvePoint(target) => match PointResolver::start(att, app, hid, target.clone()) {
            Ok(resolver) => {
                st.point = Some(resolver);
                return None;
            }
            Err(o) => return Some(o),
        },
        InputStep::ResolvePointNoScroll(target) => match PointResolver::start_with_scroll(att, app, hid, target.clone(), false) {
            Ok(resolver) => {
                st.point = Some(resolver);
                return None;
            }
            Err(o) => return Some(o),
        },
        InputStep::StoreFromCenter => {
            st.saved_from = st.saved_pos;
            return None;
        }
        InputStep::DispatchMouse { kind, button, click_count, modifiers } => {
            let Some((x, y)) = st.saved_pos else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Missing element coordinates."));
            };
            let mut params = json!({ "type": kind, "x": x.round(), "y": y.round(), "modifiers": modifiers });
            if *click_count > 0 {
                params["button"] = json!(button);
                params["clickCount"] = json!(click_count);
            }
            app.workspace_panes.browser.host.send_typed_cdp(hid, "Input.dispatchMouseEvent", params, None).map(Some)
        }
        InputStep::SendInput { method, params } => {
            if *method == "Input.dispatchMouseEvent" {
                match params.get("type").and_then(Value::as_str) {
                    Some("mousePressed") => st.drag_pressed = true,
                    Some("mouseReleased") => st.drag_pressed = false,
                    _ => {}
                }
            }
            app.workspace_panes.browser.host.send_typed_cdp(hid, method, params.clone(), None).map(Some)
        }
        InputStep::CheckDraggable => call_fn(app, st, CHECK_DRAGGABLE_JS, None),
        InputStep::SetInterceptDrags(enabled) => {
            st.intercepting_drags = *enabled;
            app.workspace_panes.browser.host.send_typed_cdp(
                hid,
                "Input.setInterceptDrags",
                json!({ "enabled": enabled }),
                None,
            ).map(Some)
        }
        InputStep::ExpandDragMotion => {
            let (Some((fx, fy)), Some((tx, ty))) = (st.saved_from, st.saved_pos) else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Missing drag endpoints."));
            };
            // Enqueue each step sequentially (never pipelined fire-and-forget) so any InputGuard
            // refusal aborts cleanly and releases `SetInterceptDrags(false)` (`review-rd294.md` #8).
            let mut seq = Vec::with_capacity(12);
            seq.push(InputStep::SendInput { method: "Input.dispatchMouseEvent", params: json!({ "type": "mouseMoved", "x": fx.round(), "y": fy.round() }) });
            seq.push(InputStep::SendInput { method: "Input.dispatchMouseEvent", params: json!({ "type": "mousePressed", "x": fx.round(), "y": fy.round(), "button": "left", "buttons": 1, "clickCount": 1 }) });
            for i in 1..=10 {
                let t = f64::from(i) / 10.0;
                let (mx, my) = (fx + (tx - fx) * t, fy + (ty - fy) * t);
                seq.push(InputStep::SendInput { method: "Input.dispatchMouseEvent", params: json!({ "type": "mouseMoved", "x": mx.round(), "y": my.round(), "button": "left", "buttons": 1 }) });
            }
            for item in seq.into_iter().rev() {
                st.steps.push_front(item);
            }
            return None;
        }
        InputStep::WaitDragData => {
            if st.html5_drag && st.drag_data.is_none() {
                st.drag_deadline = Some(now + DRAG_INTERCEPT_WAIT);
            }
            return None;
        }
        InputStep::ExpandFinishDrag => {
            let Some((tx, ty)) = st.saved_pos else {
                return Some(Outcome::fail(failure_code::OPERATION_FAILED, "Missing drag target coordinates."));
            };
            let mut seq = Vec::with_capacity(5);
            if let Some(data) = st.drag_data.take() {
                for kind in ["dragEnter", "dragOver", "drop"] {
                    seq.push(InputStep::SendInput {
                        method: "Input.dispatchDragEvent",
                        params: json!({ "type": kind, "x": tx.round(), "y": ty.round(), "data": data }),
                    });
                }
            }
            if st.intercepting_drags {
                seq.push(InputStep::SetInterceptDrags(false));
            }
            seq.push(InputStep::SendInput {
                method: "Input.dispatchMouseEvent",
                params: json!({ "type": "mouseReleased", "x": tx.round(), "y": ty.round(), "button": "left", "clickCount": 1 }),
            });
            for item in seq.into_iter().rev() {
                st.steps.push_front(item);
            }
            return None;
        }
        InputStep::ResolveNode((bid, sid, fid)) => {
            st.obj_session = sid.clone();
            st.obj_frame = fid.clone();
            app.workspace_panes.browser.host.send_typed_cdp_in_frame(hid, "DOM.resolveNode", json!({ "backendNodeId": bid }), sid.clone(), fid.clone()).map(Some)
        }
        InputStep::CheckEditable { .. } => call_fn(app, st, CHECK_EDITABLE_JS, None),
        InputStep::SetStructured { text } => call_fn(app, st, SET_STRUCTURED_JS, Some(json!(text))),
        InputStep::FocusNode((bid, sid, fid)) => app.workspace_panes.browser.host.send_typed_cdp_in_frame(hid, "DOM.focus", json!({ "backendNodeId": bid }), sid.clone(), fid.clone()).map(Some),
        InputStep::InsertText(text) => app.workspace_panes.browser.host.send_typed_cdp(hid, "Input.insertText", json!({ "text": text }), None).map(Some),
        InputStep::DispatchKey(spec) => {
            let mut down = json!({ "type": "keyDown", "key": spec.key, "windowsVirtualKeyCode": spec.windows_virtual_key_code, "modifiers": spec.modifiers });
            let mut up = json!({ "type": "keyUp", "key": spec.key, "windowsVirtualKeyCode": spec.windows_virtual_key_code, "modifiers": spec.modifiers });
            if !spec.code.is_empty() {
                down["code"] = json!(spec.code);
                up["code"] = json!(spec.code);
            }
            if let Some(txt) = &spec.text {
                down["text"] = json!(txt);
            }
            st.steps.push_front(InputStep::SendInput { method: "Input.dispatchKeyEvent", params: up });
            st.steps.push_front(InputStep::SendInput { method: "Input.dispatchKeyEvent", params: down });
            return None;
        }
        InputStep::CallSelect(values) => call_fn(app, st, SELECT_OPTIONS_JS, Some(json!(values))),
        InputStep::CallCheckPre { checked, .. } => call_fn(app, st, CHECK_PRE_JS, Some(json!(checked))),
        InputStep::CallCheckPost { .. } => call_fn(app, st, CHECK_POST_JS, None),
        InputStep::ReleaseObject => {
            release_saved_obj(app, st);
            return None;
        }
        InputStep::GetViewportCenter => app.workspace_panes.browser.host.send_typed_cdp(hid, "Page.getLayoutMetrics", json!({}), None).map(Some),
        InputStep::DispatchWheel { dx, dy } => {
            let (x, y) = st.saved_pos.unwrap_or((400.0, 300.0));
            app.workspace_panes.browser.host.send_typed_cdp(hid, "Input.dispatchMouseEvent", json!({ "type": "mouseWheel", "x": x.round(), "y": y.round(), "deltaX": dx, "deltaY": dy }), None).map(Some)
        }
    };
    match res {
        Ok(Some(req_id)) => {
            st.current = Some((step, req_id));
            None
        }
        Ok(None) => None,
        Err(msg) => Some(Outcome::fail(failure_code::OPERATION_FAILED, msg)),
    }
}

fn handle_step_response(st: &mut InputState, step: InputStep, resp: Result<Value, Value>) -> Result<(), Outcome> {
    let val = match resp {
        Ok(v) => v,
        Err(e) => {
            let code = if matches!(step, InputStep::ResolveNode(_)) {
                failure_code::STALE_REF
            } else {
                failure_code::OPERATION_FAILED
            };
            return Err(Outcome::fail(code, cdp_err_msg(&e)));
        }
    };
    if let Some(ex) = val.get("exceptionDetails") {
        let desc = ex
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("Page script error");
        if desc.starts_with("Error: browser.detached_ref") {
            return Err(Outcome::fail(failure_code::STALE_REF, "Element is no longer attached to the page. Refresh the snapshot first."));
        }
        let cleaned = desc.strip_prefix("Error: ").unwrap_or(desc);
        return Err(Outcome::fail(failure_code::OPERATION_FAILED, cleaned.lines().next().unwrap_or(cleaned)));
    }
    match step {
        InputStep::CheckDraggable => {
            st.html5_drag = val.get("result").and_then(|r| r.get("value")).and_then(Value::as_bool).unwrap_or(false);
        }
        InputStep::ResolveNode(_) => {
            let Some(oid) = val.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
                return Err(Outcome::fail(failure_code::STALE_REF, "Element ref could not be resolved. Refresh the snapshot first."));
            };
            st.saved_obj = Some(oid.to_owned());
        }
        InputStep::CheckEditable { bid, text } => {
            let kind = val.get("result").and_then(|r| r.get("value")).and_then(Value::as_str).unwrap_or("");
            match kind {
                "structured" => {
                    st.steps.push_front(InputStep::ReleaseObject);
                    st.steps.push_front(InputStep::SetStructured { text });
                }
                "text" => {
                    let sel = parse_key_chord(if cfg!(target_os = "macos") { "Meta+A" } else { "Control+A" })
                        .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
                    let bs = parse_key_chord("Backspace")
                        .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
                    st.steps.push_front(InputStep::ReleaseObject);
                    if !text.is_empty() {
                        st.steps.push_front(InputStep::InsertText(text));
                    }
                    st.steps.push_front(InputStep::DispatchKey(bs));
                    st.steps.push_front(InputStep::DispatchKey(sel));
                    st.steps.push_front(InputStep::FocusNode(bid));
                }
                _ => {
                    return Err(Outcome::fail(
                        failure_code::OPERATION_FAILED,
                        "Target is not an enabled editable text field. Use browser.select for dropdowns, browser.check for checkboxes/radios, or browser.files.upload for file inputs.",
                    ));
                }
            }
        }
        InputStep::CallCheckPre { bid, checked } => {
            let current = val.get("result").and_then(|r| r.get("value")).and_then(Value::as_bool).unwrap_or(false);
            if current != checked {
                let mut click_seq = VecDeque::new();
                push_click_steps(&mut click_seq, bid, "left".into(), 1, 0);
                st.steps.push_front(InputStep::ReleaseObject);
                st.steps.push_front(InputStep::CallCheckPost { checked });
                while let Some(s) = click_seq.pop_back() {
                    st.steps.push_front(s);
                }
            } else {
                st.steps.push_front(InputStep::ReleaseObject);
            }
        }
        InputStep::CallCheckPost { checked } => {
            let now_checked = val.get("result").and_then(|r| r.get("value")).and_then(Value::as_bool).unwrap_or(false);
            if now_checked != checked {
                return Err(Outcome::fail(
                    failure_code::OPERATION_FAILED,
                    "The page did not keep the requested checked state. Inspect the current snapshot before retrying.",
                ));
            }
        }
        InputStep::GetViewportCenter => {
            let vp = val.get("cssVisualViewport").or_else(|| val.get("visualViewport"));
            let w = vp.and_then(|v| v.get("clientWidth")).and_then(Value::as_f64).unwrap_or(800.0);
            let h = vp.and_then(|v| v.get("clientHeight")).and_then(Value::as_f64).unwrap_or(600.0);
            st.saved_pos = Some((w / 2.0, h / 2.0));
        }
        _ => {}
    }
    Ok(())
}

fn release_saved_obj(app: &mut App, st: &mut InputState) {
    if let Some(oid) = st.saved_obj.take() {
        let _ = app.workspace_panes.browser.host.send_typed_cdp_in_frame(st.host_id, "Runtime.releaseObject", json!({ "objectId": oid }), st.obj_session.clone(), st.obj_frame.clone());
    }
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

fn parse_modifiers(raw: Option<&Value>) -> u32 {
    raw.and_then(Value::as_array).map_or(0, |arr| {
        arr.iter().fold(0u32, |m, item| match item.as_str() {
            Some("Alt") => m | 1,
            Some("Control") => m | 2,
            Some("Meta") => m | 4,
            Some("Shift") => m | 8,
            _ => m,
        })
    })
}
