//! Camera / microphone gate matching official `packages/gui-plugins/src/browser/media-gate.ts`.
//!
//! Blink reads the capabilities of every microphone before it asks the browser for permission
//! (`UserMediaProcessor::SetupAudioInput`), and on macOS reading them opens the device, so the
//! system would ask whether OpenCode may use it before the site's bar appears. The page's
//! `getUserMedia` therefore asks the pane first through a hidden CDP binding and reaches Blink
//! only after the user's Allow. The engine then lets the same origin's following CEF media
//! request through without a second bar (`TabPrompts::take_media_gate_allow`).
//!
//! Like the passkey gate, every CDP message produced inside a `DevToolsMessageObserver` callback
//! is returned for the ledger's outbox, never sent re-entrantly.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::wire::PermissionKind;

/// Internal CDP message IDs for gate answers (swallowed by the passkey-range reply check in
/// `cdp_ledger.rs`, below `frame_authority::INTERNAL_ID_BASE`).
const MEDIA_GATE_ID_BASE: i32 = 1_550_000_000;

/// The page-side wrapper (`media-gate.ts:31-66`). `binding` is taken from the page and deleted
/// before any page script runs; `answer` is the function the engine releases held calls through.
pub fn media_gate_script(binding: &str, answer: &str) -> String {
    let binding_json = serde_json::to_string(binding).unwrap_or_else(|_| "\"_b\"".to_owned());
    let answer_json = serde_json::to_string(answer).unwrap_or_else(|_| "\"_a\"".to_owned());
    format!(
        r#"(() => {{
  const gate = globalThis[{binding_json}]
  if (typeof gate !== "function") return
  delete globalThis[{binding_json}]
  const proto = globalThis.MediaDevices && MediaDevices.prototype
  if (!proto || Object.getOwnPropertyDescriptor(globalThis, {answer_json})) return
  const original = proto.getUserMedia
  const waiting = new Map()
  let next = 0
  Object.defineProperty(globalThis, {answer_json}, {{
    value: (id, allowed) => {{
      const resolve = waiting.get(id)
      waiting.delete(id)
      if (resolve) resolve(allowed === true)
    }},
  }})
  const getUserMedia = function getUserMedia(constraints) {{
    const audio = !!(constraints && constraints.audio)
    const video = !!(constraints && constraints.video)
    if (!audio && !video) return original.call(this, constraints)
    return new Promise((resolve) => {{
      const id = ++next
      waiting.set(id, resolve)
      gate(JSON.stringify({{ id, audio, video }}))
    }}).then((allowed) =>
      allowed ? original.call(this, constraints) : Promise.reject(new DOMException("Permission denied", "NotAllowedError")),
    )
  }}
  Object.defineProperty(proto, "getUserMedia", {{ value: getUserMedia, writable: true, configurable: true, enumerable: true }})
  for (const name of ["getUserMedia", "webkitGetUserMedia"]) {{
    if (typeof Navigator.prototype[name] !== "function") continue
    const legacy = function (constraints, success, failure) {{
      navigator.mediaDevices.getUserMedia(constraints).then(success, failure)
    }}
    Object.defineProperty(Navigator.prototype, name, {{ value: legacy, writable: true, configurable: true, enumerable: true }})
  }}
}})()"#
    )
}

#[derive(Deserialize)]
struct Ask {
    id: u32,
    audio: bool,
    video: bool,
}

/// A held `getUserMedia` the pane must decide: who asks, for which devices, and the two CDP
/// messages that answer it (allow, deny) in the asking execution context.
pub struct MediaAsk {
    pub origin: Option<String>,
    pub kinds: Vec<PermissionKind>,
    pub allow_msg: String,
    pub deny_msg: String,
}

#[derive(Default)]
struct ContextState {
    /// `(session_id, execution_context_id)` -> origin.
    contexts: HashMap<(String, i64), String>,
}

pub struct TabMediaGate {
    binding: String,
    answer: String,
    next_id: AtomicI32,
    state: Mutex<ContextState>,
}

impl TabMediaGate {
    pub fn new() -> Self {
        Self {
            binding: crate::passkeys::random_token(),
            answer: crate::passkeys::random_token(),
            next_id: AtomicI32::new(0),
            state: Mutex::new(ContextState::default()),
        }
    }

    fn build_cdp(&self, method: &str, params: Value, session_id: Option<&str>) -> String {
        let id = MEDIA_GATE_ID_BASE + self.next_id.fetch_add(1, Ordering::Relaxed).rem_euclid(25_000_000);
        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            payload["sessionId"] = Value::String(sid.to_owned());
        }
        payload.to_string()
    }

    /// Installs the binding and wrapper on the root session (`None`) or a paused OOPIF session.
    pub fn attach_messages(&self, session_id: Option<&str>) -> Vec<String> {
        vec![
            self.binding_msg(session_id),
            self.build_cdp(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({ "source": media_gate_script(&self.binding, &self.answer), "runImmediately": true }),
                session_id,
            ),
        ]
    }

    /// `Runtime.addBinding` for the wrapper's channel. A `Runtime.disable` on the session drops it
    /// (as for the passkey gate, #438), so the engine sends it again after each one it sends.
    pub fn binding_msg(&self, session_id: Option<&str>) -> String {
        self.build_cdp("Runtime.addBinding", json!({ "name": self.binding }), session_id)
    }

    fn answer_msg(&self, ask_id: u32, allowed: bool, context_id: i64, session_id: Option<&str>) -> String {
        let answer_json = serde_json::to_string(&self.answer).unwrap_or_else(|_| "\"\"".to_owned());
        self.build_cdp(
            "Runtime.evaluate",
            json!({ "expression": format!("globalThis[{answer_json}]?.({ask_id}, {allowed})"), "contextId": context_id }),
            session_id,
        )
    }

    /// Observes a CDP event. Returns `(consumed, ask)`: `consumed` marks our private binding call
    /// (never forwarded to the app); `ask` is a held call the pane must decide.
    pub fn observe_event(&self, method: &str, params: &Value, session_id: Option<&str>) -> (bool, Option<MediaAsk>) {
        let sid = session_id.unwrap_or("").to_owned();
        match method {
            "Target.detachedFromTarget" => {
                if let Some(detached) = params.get("sessionId").and_then(Value::as_str) {
                    self.state.lock().unwrap().contexts.retain(|(s, _), _| s != detached);
                }
                (false, None)
            }
            "Runtime.executionContextCreated" => {
                let ctx = &params["context"];
                if ctx["auxData"]["isDefault"].as_bool() == Some(true)
                    && let Some(id) = ctx["id"].as_i64()
                {
                    let origin = ctx["origin"].as_str().unwrap_or("").to_owned();
                    self.state.lock().unwrap().contexts.insert((sid, id), origin);
                }
                (false, None)
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = params["executionContextId"].as_i64() {
                    self.state.lock().unwrap().contexts.remove(&(sid, id));
                }
                (false, None)
            }
            "Runtime.executionContextsCleared" => {
                self.state.lock().unwrap().contexts.retain(|(s, _), _| s != &sid);
                (false, None)
            }
            "Runtime.bindingCalled" => {
                if params["name"].as_str() != Some(self.binding.as_str()) {
                    return (false, None);
                }
                let payload = params["payload"].as_str().unwrap_or("");
                let (Some(ctx_id), Ok(ask)) = (params["executionContextId"].as_i64(), serde_json::from_str::<Ask>(payload)) else {
                    return (true, None);
                };
                if payload.len() > 256 || ask.id == 0 || (!ask.audio && !ask.video) {
                    return (true, None);
                }
                // A call from a context the gate never saw has no origin and is refused.
                let origin = self
                    .state
                    .lock()
                    .unwrap()
                    .contexts
                    .get(&(sid.clone(), ctx_id))
                    .cloned()
                    .filter(|o| !o.is_empty() && o != "null");
                let kinds = [(ask.video, PermissionKind::Camera), (ask.audio, PermissionKind::Microphone)]
                    .into_iter()
                    .filter_map(|(on, k)| on.then_some(k))
                    .collect();
                let session = session_id.filter(|s| !s.is_empty());
                (
                    true,
                    Some(MediaAsk {
                        origin,
                        kinds,
                        allow_msg: self.answer_msg(ask.id, true, ctx_id, session),
                        deny_msg: self.answer_msg(ask.id, false, ctx_id, session),
                    }),
                )
            }
            _ => (false, None),
        }
    }
}
