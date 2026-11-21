//! Resolves an element ref to page-viewport center `(x, y)` across nested same-process and
//! cross-origin OOPIF iframes, matching official `rect` (`packages/gui-plugins/src/browser/chromium.ts`
//! at `db2d889c0e24daf7b34c8dede641fb01fcd02c00`).

use std::collections::VecDeque;

use opencode_kit::browser_rpc::{Outcome, failure_code};
use serde_json::{Value, json};

use super::super::session::SessionAttachment;
use super::cdp_err_msg;
use crate::App;

const ELEMENT_RECT_JS: &str = r#"function() {
  if (!this.isConnected) throw new Error('browser.detached_ref');
  const r = this.getBoundingClientRect();
  return { x: r.x, y: r.y, width: r.width, height: r.height };
}"#;

/// Official `chromium.ts` iframe owner box: border offset (`clientLeft`/`clientTop`) scaled by CSS
/// transform scale (`r.width / offsetWidth`, `r.height / offsetHeight`).
const IFRAME_BOX_JS: &str = r#"function() {
  const r = this.getBoundingClientRect();
  const sx = this.offsetWidth ? r.width / this.offsetWidth : 1;
  const sy = this.offsetHeight ? r.height / this.offsetHeight : 1;
  return { x: r.x + this.clientLeft * sx, y: r.y + this.clientTop * sy, scaleX: sx, scaleY: sy };
}"#;

pub(super) type ElementRef = (i64, Option<String>, Option<String>);

enum Stage {
    Scroll { cdp_req: u64 },
    ResolveElem { cdp_req: u64 },
    MeasureElem { cdp_req: u64, object_id: String },
    OwnerFrame { cdp_req: u64, parent_frame: String, parent_session: Option<String> },
    ResolveOwner { cdp_req: u64, parent_session: Option<String> },
    MeasureOwner { cdp_req: u64, object_id: String, parent_session: Option<String> },
    /// Mobile device emulation visual-viewport pan offset (`chromium.ts:2487-2492`).
    MobileVisualOffset { cdp_req: u64, scale: f64 },
}

pub(super) struct PointResolver {
    host_id: u64,
    target: ElementRef,
    ancestors: VecDeque<(String, String, Option<String>)>,
    rect: (f64, f64, f64, f64),
    stage: Stage,
}

impl PointResolver {
    pub fn start(
        att: &SessionAttachment,
        app: &mut App,
        host_id: u64,
        target: ElementRef,
    ) -> Result<Self, Outcome> {
        Self::start_with_scroll(att, app, host_id, target, true)
    }

    pub fn start_with_scroll(
        att: &SessionAttachment,
        app: &mut App,
        host_id: u64,
        target: ElementRef,
        scroll: bool,
    ) -> Result<Self, Outcome> {
        let effective_frame = target.2.clone().or_else(|| {
            let sid = target.1.as_deref()?;
            att.frames.get(&host_id).and_then(|f| {
                f.frame_sessions
                    .iter()
                    .find(|(_, s)| s.as_str() == sid)
                    .map(|(fid, _)| fid.clone())
            })
        });
        let ancestors = att
            .frames
            .get(&host_id)
            .map(|f| f.ancestor_owners(effective_frame.as_deref()))
            .unwrap_or_default();
        let stage = if scroll {
            let cdp_req = app
                .workspace_panes
                .browser
                .host
                .send_typed_cdp_in_frame(
                    host_id,
                    "DOM.scrollIntoViewIfNeeded",
                    json!({ "backendNodeId": target.0 }),
                    target.1.clone(),
                    effective_frame.clone(),
                )
                .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
            Stage::Scroll { cdp_req }
        } else {
            let cdp_req = app
                .workspace_panes
                .browser
                .host
                .send_typed_cdp_in_frame(
                    host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": target.0 }),
                    target.1.clone(),
                    effective_frame.clone(),
                )
                .map_err(|e| Outcome::fail(failure_code::OPERATION_FAILED, e))?;
            Stage::ResolveElem { cdp_req }
        };
        Ok(Self {
            host_id,
            target: (target.0, target.1, effective_frame),
            ancestors: VecDeque::from(ancestors),
            rect: (0.0, 0.0, 0.0, 0.0),
            stage,
        })
    }

    pub fn cancel_cleanup(&mut self, app: &mut App) {
        let (req, obj) = match &self.stage {
            Stage::Scroll { cdp_req }
            | Stage::ResolveElem { cdp_req }
            | Stage::OwnerFrame { cdp_req, .. }
            | Stage::ResolveOwner { cdp_req, .. }
            | Stage::MobileVisualOffset { cdp_req, .. } => (*cdp_req, None),
            Stage::MeasureElem { cdp_req, object_id } => (*cdp_req, Some((object_id.clone(), self.target.1.clone()))),
            Stage::MeasureOwner { cdp_req, object_id, parent_session } => {
                (*cdp_req, Some((object_id.clone(), parent_session.clone())))
            }
        };
        app.workspace_panes.browser.host.cancel_cdp(req);
        if let Some((oid, sid)) = obj {
            let _ = app
                .workspace_panes
                .browser
                .host
                .send_typed_cdp(self.host_id, "Runtime.releaseObject", json!({ "objectId": oid }), sid);
        }
    }

    pub fn step(&mut self, app: &mut App) -> Option<Result<(f64, f64), Outcome>> {
        let prev = std::mem::replace(&mut self.stage, Stage::Scroll { cdp_req: 0 });
        match prev {
            Stage::Scroll { cdp_req } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::Scroll { cdp_req };
                    return None;
                };
                if let Err(e) = resp {
                    return Some(Err(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e))));
                }
                match app.workspace_panes.browser.host.send_typed_cdp_in_frame(
                    self.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": self.target.0 }),
                    self.target.1.clone(),
                    self.target.2.clone(),
                ) {
                    Ok(next) => {
                        self.stage = Stage::ResolveElem { cdp_req: next };
                        None
                    }
                    Err(e) => Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e))),
                }
            }
            Stage::ResolveElem { cdp_req } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::ResolveElem { cdp_req };
                    return None;
                };
                let val = match resp {
                    Ok(v) => v,
                    Err(e) => return Some(Err(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e)))),
                };
                let Some(oid) = val.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Element ref could not be resolved. Refresh the snapshot first.",
                    )));
                };
                let oid = oid.to_owned();
                match app.workspace_panes.browser.host.send_typed_cdp_in_frame(
                    self.host_id,
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": oid,
                        "functionDeclaration": ELEMENT_RECT_JS,
                        "returnByValue": true,
                        "userGesture": false,
                    }),
                    self.target.1.clone(),
                    self.target.2.clone(),
                ) {
                    Ok(next) => {
                        self.stage = Stage::MeasureElem { cdp_req: next, object_id: oid };
                        None
                    }
                    Err(e) => {
                        let _ = app.workspace_panes.browser.host.send_typed_cdp(
                            self.host_id,
                            "Runtime.releaseObject",
                            json!({ "objectId": oid }),
                            self.target.1.clone(),
                        );
                        Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e)))
                    }
                }
            }
            Stage::MeasureElem { cdp_req, object_id } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::MeasureElem { cdp_req, object_id };
                    return None;
                };
                let _ = app.workspace_panes.browser.host.send_typed_cdp(
                    self.host_id,
                    "Runtime.releaseObject",
                    json!({ "objectId": object_id }),
                    self.target.1.clone(),
                );
                let val = match check_call_val(resp) {
                    Ok(v) => v,
                    Err(o) => return Some(Err(o)),
                };
                let (Some(x), Some(y), Some(w), Some(h)) = (
                    finite_f64(&val, "x"),
                    finite_f64(&val, "y"),
                    finite_f64(&val, "width"),
                    finite_f64(&val, "height"),
                ) else {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Element has no visible layout box. Refresh the snapshot first.",
                    )));
                };
                if w <= 0.0 || h <= 0.0 {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Element has no visible layout box. Refresh the snapshot first.",
                    )));
                }
                self.rect = (x, y, w, h);
                self.advance_ancestor(app)
            }
            Stage::OwnerFrame { cdp_req, parent_frame, parent_session } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::OwnerFrame { cdp_req, parent_frame, parent_session };
                    return None;
                };
                let val = match resp {
                    Ok(v) => v,
                    Err(e) => return Some(Err(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e)))),
                };
                let Some(owner_bid) = val.get("backendNodeId").and_then(Value::as_i64) else {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Iframe owner element could not be resolved. Refresh the snapshot first.",
                    )));
                };
                match app.workspace_panes.browser.host.send_typed_cdp_in_frame(
                    self.host_id,
                    "DOM.resolveNode",
                    json!({ "backendNodeId": owner_bid }),
                    parent_session.clone(),
                    Some(parent_frame),
                ) {
                    Ok(next) => {
                        self.stage = Stage::ResolveOwner { cdp_req: next, parent_session };
                        None
                    }
                    Err(e) => Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e))),
                }
            }
            Stage::ResolveOwner { cdp_req, parent_session } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::ResolveOwner { cdp_req, parent_session };
                    return None;
                };
                let val = match resp {
                    Ok(v) => v,
                    Err(e) => return Some(Err(Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e)))),
                };
                let Some(oid) = val.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Iframe owner element could not be resolved. Refresh the snapshot first.",
                    )));
                };
                let oid = oid.to_owned();
                match app.workspace_panes.browser.host.send_typed_cdp(
                    self.host_id,
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": oid,
                        "functionDeclaration": IFRAME_BOX_JS,
                        "returnByValue": true,
                        "userGesture": false,
                    }),
                    parent_session.clone(),
                ) {
                    Ok(next) => {
                        self.stage = Stage::MeasureOwner { cdp_req: next, object_id: oid, parent_session };
                        None
                    }
                    Err(e) => {
                        let _ = app.workspace_panes.browser.host.send_typed_cdp(
                            self.host_id,
                            "Runtime.releaseObject",
                            json!({ "objectId": oid }),
                            parent_session,
                        );
                        Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e)))
                    }
                }
            }
            Stage::MeasureOwner { cdp_req, object_id, parent_session } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::MeasureOwner { cdp_req, object_id, parent_session };
                    return None;
                };
                let _ = app.workspace_panes.browser.host.send_typed_cdp(
                    self.host_id,
                    "Runtime.releaseObject",
                    json!({ "objectId": object_id }),
                    parent_session,
                );
                let val = match check_call_val(resp) {
                    Ok(v) => v,
                    Err(o) => return Some(Err(o)),
                };
                let (Some(bx), Some(by), Some(sx), Some(sy)) = (
                    finite_f64(&val, "x"),
                    finite_f64(&val, "y"),
                    finite_f64(&val, "scaleX"),
                    finite_f64(&val, "scaleY"),
                ) else {
                    return Some(Err(Outcome::fail(
                        failure_code::STALE_REF,
                        "Iframe layout box could not be measured. Refresh the snapshot first.",
                    )));
                };
                self.rect = (bx + self.rect.0 * sx, by + self.rect.1 * sy, self.rect.2 * sx, self.rect.3 * sy);
                self.advance_ancestor(app)
            }
            Stage::MobileVisualOffset { cdp_req, scale } => {
                let Some(resp) = app.workspace_panes.browser.host.take_cdp_response(cdp_req) else {
                    self.stage = Stage::MobileVisualOffset { cdp_req, scale };
                    return None;
                };
                let val = match resp {
                    Ok(v) => v,
                    Err(e) => return Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, cdp_err_msg(&e)))),
                };
                let vp = val.get("cssVisualViewport");
                let off_x = vp.and_then(|v| v.get("offsetX")).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
                let off_y = vp.and_then(|v| v.get("offsetY")).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
                let (x, y, w, h) = self.rect;
                Some(Ok((((x - off_x) + w / 2.0) * scale, ((y - off_y) + h / 2.0) * scale)))
            }
        }
    }

    fn advance_ancestor(&mut self, app: &mut App) -> Option<Result<(f64, f64), Outcome>> {
        let Some((child_fid, parent_fid, parent_session)) = self.ancestors.pop_front() else {
            let (scale, is_mobile) = app
                .workspace_panes
                .browser
                .host
                .tab(self.host_id)
                .map_or((1.0, false), |t| t.emulation_info());
            if is_mobile {
                match app.workspace_panes.browser.host.send_typed_cdp(
                    self.host_id,
                    "Page.getLayoutMetrics",
                    json!({}),
                    None,
                ) {
                    Ok(cdp_req) => {
                        self.stage = Stage::MobileVisualOffset { cdp_req, scale };
                        return None;
                    }
                    Err(e) => return Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e))),
                }
            }
            let (x, y, w, h) = self.rect;
            return Some(Ok(((x + w / 2.0) * scale, (y + h / 2.0) * scale)));
        };
        match app.workspace_panes.browser.host.send_typed_cdp(
            self.host_id,
            "DOM.getFrameOwner",
            json!({ "frameId": child_fid }),
            parent_session.clone(),
        ) {
            Ok(next) => {
                self.stage = Stage::OwnerFrame { cdp_req: next, parent_frame: parent_fid, parent_session };
                None
            }
            Err(e) => Some(Err(Outcome::fail(failure_code::OPERATION_FAILED, e))),
        }
    }
}

fn check_call_val(resp: Result<Value, Value>) -> Result<Value, Outcome> {
    let val = resp.map_err(|e| Outcome::fail(failure_code::STALE_REF, cdp_err_msg(&e)))?;
    if let Some(ex) = val.get("exceptionDetails") {
        let desc = ex
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(Value::as_str)
            .or_else(|| ex.get("text").and_then(Value::as_str))
            .unwrap_or("Page script error");
        if super::is_detached_ref(desc) {
            return Err(Outcome::fail(
                failure_code::STALE_REF,
                "Element is no longer attached to the page. Refresh the snapshot first.",
            ));
        }
        return Err(Outcome::fail(
            failure_code::OPERATION_FAILED,
            super::page_exception(desc, "The page's JavaScript threw while the browser tool located the element.", false),
        ));
    }
    Ok(val.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null))
}

fn finite_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}
