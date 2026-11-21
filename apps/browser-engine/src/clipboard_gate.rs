//! Per-tab clipboard write gate matching official `site-permissions.ts:152-157` &
//! `page-prompts.ts:453,468` (READER-7 N5).
//!
//! Chromium's Chrome runtime auto-grants `clipboard-sanitized-write` on user activation without
//! invoking CEF's `on_show_permission_prompt`, so an agent's synthetic click (`Input.dispatch*`)
//! would let a page overwrite the person's OS clipboard via `navigator.clipboard.writeText()`,
//! `navigator.clipboard.write()`, or `document.execCommand("copy"|"cut")`.
//!
//! Parity contract:
//! - Agent-driven tabs (`agent_driven && !user_reclaimed`) and preview tabs (`is_preview`):
//!   every clipboard write is refused (`NotAllowedError` / `execCommand -> false` / `copy`/`cut`
//!   cancelled / `permissions.query({ name: "clipboard-write" }) -> "denied"`). Preview tabs also
//!   disable `BrowserSettings.javascript_access_clipboard` & `javascript_dom_paste` in Blink C++.
//! - Human-driven tabs (`!is_preview && !agent_owns_dialogs()`): same-origin trustworthy contexts
//!   (`https:` or loopback) work as in Chrome; cross-origin frames and insecure origins are refused.
//! - Extension popups (`is_popup`): untouched (manifest-governed `clipboardWrite` stays intact).
//! - Clipboard reads remain separately gated by `TabPermissionHandler::on_show_permission_prompt`
//!   (`CEF_PERMISSION_TYPE_CLIPBOARD` -> `PermissionKind::ClipboardRead`) and inherited-grant checks.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

/// Internal CDP message ID range owned by `TabClipboardGate` (above `TabMediaGate`'s
/// `1_550_000_000`, below `frame_authority::INTERNAL_ID_BASE = 1_600_000_000`).
pub const CLIPBOARD_GATE_ID_BASE: i32 = 1_575_000_000;

/// Builds `BrowserSettings` for a pane tab, permanently disabling Blink C++
/// `javascript_access_clipboard` & `javascript_dom_paste` on preview tabs (N5).
pub fn browser_settings(background_color: u32, is_preview: bool) -> cef::BrowserSettings {
    let clip = if is_preview {
        cef::sys::cef_state_t::STATE_DISABLED
    } else {
        cef::sys::cef_state_t::STATE_DEFAULT
    };
    cef::BrowserSettings {
        windowless_frame_rate: 60,
        background_color,
        javascript_access_clipboard: clip.into(),
        javascript_dom_paste: clip.into(),
        ..Default::default()
    }
}

/// Page-side clipboard write gate injected via `Page.addScriptToEvaluateOnNewDocument`.
///
/// Security invariants:
/// - `secret` is closed inside the IIFE and never stored on `globalThis`.
/// - `answer` and `sync_fn` are defined non-writable, non-configurable, non-enumerable, and
///   require `tok === secret`, so page JS cannot forge an allow or unblock even after inspecting
///   `Object.getOwnPropertyNames(globalThis)`.
/// - Wraps `Clipboard.prototype.{writeText,write}`, `Document.prototype.execCommand`, `copy`/`cut`
///   events, and `Permissions.prototype.query("clipboard-write")` unconditionally (never fails open
///   even if `binding` is absent in a synthetic `about:blank` frame).
pub fn clipboard_gate_script(
    binding: &str,
    answer: &str,
    sync_fn: &str,
    secret: &str,
    initial_blocked: bool,
) -> String {
    let binding_json = serde_json::to_string(binding).unwrap_or_else(|_| "\"_cb\"".to_owned());
    let answer_json = serde_json::to_string(answer).unwrap_or_else(|_| "\"_ca\"".to_owned());
    let sync_json = serde_json::to_string(sync_fn).unwrap_or_else(|_| "\"_cs\"".to_owned());
    let secret_json = serde_json::to_string(secret).unwrap_or_else(|_| "\"_ck\"".to_owned());
    let init_js = if initial_blocked { "true" } else { "false" };
    format!(
        r#"(() => {{
  "use strict"
  const secret = {secret_json}
  const ask = globalThis[{binding_json}]
  if (typeof ask === "function") {{
    try {{ delete globalThis[{binding_json}] }} catch {{}}
  }}
  if (Object.getOwnPropertyDescriptor(globalThis, {sync_json})) {{
    try {{ globalThis[{sync_json}]({init_js}, secret) }} catch {{}}
    return
  }}
  const apply = Reflect.apply
  const reflectGet = Reflect.get
  const define = Object.defineProperty
  const ProxyCtor = Proxy
  const StringCtor = String
  const fnBind = Function.prototype.bind
  const strToLower = String.prototype.toLowerCase
  const MapCtor = Map
  const mapGet = Map.prototype.get
  const mapSet = Map.prototype.set
  const mapDel = Map.prototype.delete
  const PromiseCtor = Promise
  const promiseThen = Promise.prototype.then
  const promiseReject = Promise.reject.bind(Promise)
  const DomExc = DOMException
  const jsonStringify = JSON.stringify
  const addListener = EventTarget.prototype.addEventListener
  const stopImm = Event.prototype.stopImmediatePropagation
  const preventDef = Event.prototype.preventDefault
  const waiting = new MapCtor()
  let next = 0
  let blocked = {init_js}
  const denied = () => new DomExc("Write permission denied.", "NotAllowedError")

  define(globalThis, {answer_json}, {{
    value: (id, allowed, tok) => {{
      if (tok !== secret) return
      const resolve = apply(mapGet, waiting, [id])
      if (!resolve) return
      apply(mapDel, waiting, [id])
      resolve(allowed === true)
    }},
    writable: false,
    configurable: false,
    enumerable: false,
  }})

  define(globalThis, {sync_json}, {{
    value: (nextBlocked, tok) => {{
      if (tok !== secret) return
      blocked = nextBlocked === true
    }},
    writable: false,
    configurable: false,
    enumerable: false,
  }})

  const sameOriginTop = () => {{
    try {{
      if (globalThis.top === globalThis) return true
      return globalThis.top.origin === globalThis.origin && globalThis.origin !== "null"
    }} catch {{
      return false
    }}
  }}

  const trustworthy = () => {{
    try {{
      const proto = globalThis.location && globalThis.location.protocol
      if (proto === "chrome-extension:") return true
      return globalThis.isSecureContext === true
    }} catch {{
      return false
    }}
  }}

  const canWriteSync = () => !blocked && sameOriginTop() && trustworthy()

  const clipProto = globalThis.Clipboard && Clipboard.prototype
  if (clipProto) {{
    for (const name of ["writeText", "write"]) {{
      const orig = clipProto[name]
      if (typeof orig !== "function") continue
      const wrapped = new ProxyCtor(orig, {{
        apply(target, self, args) {{
          if (!canWriteSync() || typeof ask !== "function") {{
            return promiseReject(denied())
          }}
          const p = new PromiseCtor((resolve) => {{
            const id = ++next
            apply(mapSet, waiting, [id, resolve])
            ask(jsonStringify({{ id }}))
          }})
          return apply(promiseThen, p, [
            (allowed) => (allowed ? apply(target, self, args) : promiseReject(denied())),
          ])
        }},
      }})
      define(clipProto, name, {{ value: wrapped, writable: true, configurable: true, enumerable: true }})
    }}
  }}

  const docProto = globalThis.Document && Document.prototype
  if (docProto && typeof docProto.execCommand === "function") {{
    const origExec = docProto.execCommand
    const wrappedExec = new ProxyCtor(origExec, {{
      apply(target, self, args) {{
        let cmd = ""
        try {{
          cmd = apply(strToLower, StringCtor(args[0] ?? ""), [])
        }} catch {{
          return false
        }}
        if ((cmd === "copy" || cmd === "cut") && !canWriteSync()) {{
          return false
        }}
        return apply(target, self, args)
      }},
    }})
    define(docProto, "execCommand", {{ value: wrappedExec, writable: true, configurable: true, enumerable: true }})
  }}

  for (const evName of ["copy", "cut"]) {{
    apply(addListener, globalThis, [
      evName,
      (ev) => {{
        if (blocked) {{
          try {{ apply(preventDef, ev, []) }} catch {{}}
          try {{ apply(stopImm, ev, []) }} catch {{}}
        }}
      }},
      true,
    ])
  }}

  const permProto = globalThis.Permissions && Permissions.prototype
  if (permProto && typeof permProto.query === "function") {{
    const origQuery = permProto.query
    const statusNameGetter = globalThis.PermissionStatus && Object.getOwnPropertyDescriptor(PermissionStatus.prototype, "name")?.get
    const wrappedQuery = new ProxyCtor(origQuery, {{
      apply(target, self, args) {{
        let name = ""
        try {{
          name = args[0] && typeof args[0] === "object" ? StringCtor(args[0].name ?? "") : ""
        }} catch {{}}
        return apply(promiseThen, apply(target, self, args), [
          (status) => {{
            let resolvedName = name
            if (status && statusNameGetter) {{
              try {{ resolvedName = apply(statusNameGetter, status, []) }} catch {{}}
            }}
            if (resolvedName !== "clipboard-write" || canWriteSync()) return status
            return new ProxyCtor(status, {{
              get(t, prop) {{
                if (prop === "state") return "denied"
                const v = reflectGet(t, prop, t)
                return typeof v === "function" ? apply(fnBind, v, [t]) : v
              }},
            }})
          }},
        ])
      }},
    }})
    define(permProto, "query", {{ value: wrappedQuery, writable: true, configurable: true, enumerable: true }})
  }}
}})()"#
    )
}

#[derive(Deserialize)]
struct Ask {
    id: u32,
}

/// A pending async `navigator.clipboard.write{Text}` call awaiting the browser-process verdict.
pub struct ClipboardAsk {
    pub origin: Option<String>,
    pub allow_msg: String,
    pub deny_msg: String,
}

#[derive(Default)]
struct ContextState {
    /// Live `(session_id, execution_context_id)` -> origin (default V8 worlds only).
    contexts: HashMap<(String, i64), String>,
    /// Live attached sessions (`""` for root + every non-extension web OOPIF `session_id`).
    sessions: HashSet<String>,
    /// Current `Page.addScriptToEvaluateOnNewDocument` identifier per session.
    script_ids: HashMap<String, String>,
    /// In-flight `Page.addScriptToEvaluateOnNewDocument` CDP msg id -> `(session_id, generation)`.
    pending_adds: HashMap<i32, (String, u64)>,
}

pub struct TabClipboardGate {
    is_popup: bool,
    is_preview: bool,
    blocked: AtomicBool,
    generation: AtomicU64,
    binding: String,
    answer: String,
    sync_fn: String,
    secret: String,
    next_id: AtomicI32,
    state: Mutex<ContextState>,
}

impl TabClipboardGate {
    pub fn new(is_popup: bool, is_preview: bool, agent_driven: bool) -> Self {
        Self {
            is_popup,
            is_preview,
            blocked: AtomicBool::new(!is_popup && (is_preview || agent_driven)),
            generation: AtomicU64::new(0),
            binding: crate::passkeys::random_token(),
            answer: crate::passkeys::random_token(),
            sync_fn: crate::passkeys::random_token(),
            secret: crate::passkeys::random_token(),
            next_id: AtomicI32::new(0),
            state: Mutex::new(ContextState::default()),
        }
    }

    fn build_cdp(&self, method: &str, params: Value, session_id: Option<&str>) -> (i32, String) {
        let id = CLIPBOARD_GATE_ID_BASE + self.next_id.fetch_add(1, Ordering::Relaxed).rem_euclid(20_000_000);
        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
            payload["sessionId"] = Value::String(sid.to_owned());
        }
        (id, payload.to_string())
    }

    fn add_script_msg(&self, session_id: Option<&str>, blocked: bool, epoch: u64, run_immediately: bool) -> String {
        let source = clipboard_gate_script(&self.binding, &self.answer, &self.sync_fn, &self.secret, blocked);
        let (id, msg) = self.build_cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source, "runImmediately": run_immediately }),
            session_id,
        );
        let sid = session_id.unwrap_or("").to_owned();
        let mut st = self.state.lock().unwrap();
        st.sessions.insert(sid.clone());
        st.pending_adds.insert(id, (sid, epoch));
        msg
    }

    fn remove_script_msg(&self, session_id: Option<&str>, identifier: &str) -> String {
        self.build_cdp(
            "Page.removeScriptToEvaluateOnNewDocument",
            json!({ "identifier": identifier }),
            session_id,
        )
        .1
    }

    fn sync_context_msg(&self, blocked: bool, context_id: i64, session_id: Option<&str>) -> String {
        let sync_json = serde_json::to_string(&self.sync_fn).unwrap_or_else(|_| "\"\"".to_owned());
        let secret_json = serde_json::to_string(&self.secret).unwrap_or_else(|_| "\"\"".to_owned());
        self.build_cdp(
            "Runtime.evaluate",
            json!({
                "expression": format!("globalThis[{sync_json}]?.({blocked}, {secret_json})"),
                "contextId": context_id,
            }),
            session_id,
        )
        .1
    }

    fn answer_msg(&self, ask_id: u32, allowed: bool, context_id: i64, session_id: Option<&str>) -> String {
        let answer_json = serde_json::to_string(&self.answer).unwrap_or_else(|_| "\"\"".to_owned());
        let secret_json = serde_json::to_string(&self.secret).unwrap_or_else(|_| "\"\"".to_owned());
        self.build_cdp(
            "Runtime.evaluate",
            json!({
                "expression": format!("globalThis[{answer_json}]?.({ask_id}, {allowed}, {secret_json})"),
                "contextId": context_id,
            }),
            session_id,
        )
        .1
    }

    /// Installs the hidden binding and page-side clipboard gate on root (`None`) or a paused web
    /// OOPIF session (`Some(sid)`). No-op for extension popups.
    pub fn attach_messages(&self, session_id: Option<&str>) -> Vec<String> {
        if self.is_popup {
            return Vec::new();
        }
        let blocked = self.blocked.load(Ordering::Acquire);
        let epoch = self.generation.load(Ordering::Acquire);
        vec![
            self.binding_msg(session_id).expect("non-popup binding"),
            self.add_script_msg(session_id, blocked, epoch, true),
        ]
    }

    /// `Runtime.addBinding` for the wrapper's channel. Re-sent after any `Runtime.disable`.
    pub fn binding_msg(&self, session_id: Option<&str>) -> Option<String> {
        if self.is_popup {
            return None;
        }
        Some(self.build_cdp("Runtime.addBinding", json!({ "name": self.binding }), session_id).1)
    }

    /// Updates the tab's blocked state (`is_preview || agent_owns_dialogs()`), returning CDP
    /// messages that immediately update every live execution context AND replace each session's
    /// `Page.addScriptToEvaluateOnNewDocument` script for future navigations and `about:blank`
    /// child iframes.
    pub fn set_blocked_messages(&self, next_blocked: bool) -> Vec<String> {
        if self.is_popup {
            return Vec::new();
        }
        let effective = self.is_preview || next_blocked;
        let prev = self.blocked.swap(effective, Ordering::AcqRel);
        if prev == effective {
            return Vec::new();
        }
        let epoch = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (contexts, sessions, old_scripts): (Vec<(String, i64)>, Vec<String>, Vec<(String, String)>) = {
            let mut st = self.state.lock().unwrap();
            let contexts = st.contexts.keys().cloned().collect();
            let sessions = st.sessions.iter().cloned().collect();
            let old_scripts = st.script_ids.drain().collect();
            (contexts, sessions, old_scripts)
        };
        let mut out = Vec::with_capacity(contexts.len() + sessions.len() + old_scripts.len());
        // Update every live context synchronously first (FIFO ahead of any subsequent Input.*).
        for (sid, ctx_id) in contexts {
            let sess = (!sid.is_empty()).then_some(sid.as_str());
            out.push(self.sync_context_msg(effective, ctx_id, sess));
        }
        // Replace each session's new-document script so future navigations and iframes boot with `effective`.
        for (sid, ident) in old_scripts {
            let sess = (!sid.is_empty()).then_some(sid.as_str());
            out.push(self.remove_script_msg(sess, &ident));
        }
        for sid in sessions {
            let sess = (!sid.is_empty()).then_some(sid.as_str());
            out.push(self.add_script_msg(sess, effective, epoch, false));
        }
        out
    }

    /// Handles a reply to a `CLIPBOARD_GATE_ID_BASE` CDP call, storing the script identifier or
    /// returning a `Page.removeScriptToEvaluateOnNewDocument` message if the reply belongs to a
    /// superseded generation.
    pub fn on_reply(&self, msg_id: i32, ok: bool, body: Option<&Value>) -> Option<String> {
        let (sid, epoch) = self.state.lock().unwrap().pending_adds.remove(&msg_id)?;
        if !ok {
            return None;
        }
        let ident = body
            .and_then(|v| v.get("identifier"))
            .and_then(Value::as_str)?
            .to_owned();
        if epoch < self.generation.load(Ordering::Acquire) {
            let sess = (!sid.is_empty()).then_some(sid.as_str());
            return Some(self.remove_script_msg(sess, &ident));
        }
        let mut st = self.state.lock().unwrap();
        if !st.sessions.contains(&sid) {
            return None;
        }
        let replaced = st.script_ids.insert(sid.clone(), ident);
        drop(st);
        replaced.map(|old| {
            let sess = (!sid.is_empty()).then_some(sid.as_str());
            self.remove_script_msg(sess, &old)
        })
    }

    /// Observes a CDP event. Returns `(consumed, sync_msgs, ask)`:
    /// - `consumed`: true when this event was our private binding call (never forwarded to app).
    /// - `sync_msgs`: immediate `Runtime.evaluate` sync for newly created contexts after a toggle.
    /// - `ask`: pending `navigator.clipboard.write{Text}` call awaiting browser-process verdict.
    pub fn observe_event(
        &self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
    ) -> (bool, Vec<String>, Option<ClipboardAsk>) {
        if self.is_popup {
            return (false, Vec::new(), None);
        }
        let sid = session_id.unwrap_or("").to_owned();
        match method {
            "Target.detachedFromTarget" => {
                if let Some(detached) = params.get("sessionId").and_then(Value::as_str) {
                    let mut st = self.state.lock().unwrap();
                    st.contexts.retain(|(s, _), _| s != detached);
                    st.sessions.remove(detached);
                    st.script_ids.remove(detached);
                    st.pending_adds.retain(|_, (s, _)| s != detached);
                }
                (false, Vec::new(), None)
            }
            "Runtime.executionContextCreated" => {
                let ctx = &params["context"];
                let mut sync = Vec::new();
                if ctx["auxData"]["isDefault"].as_bool() == Some(true)
                    && let Some(id) = ctx["id"].as_i64()
                {
                    let origin = ctx["origin"].as_str().unwrap_or("").to_owned();
                    self.state.lock().unwrap().contexts.insert((sid.clone(), id), origin);
                    // Close any brief race if a context spun up while a generation swap was in flight.
                    if self.generation.load(Ordering::Acquire) > 0 {
                        let sess = session_id.filter(|s| !s.is_empty());
                        sync.push(self.sync_context_msg(self.blocked.load(Ordering::Acquire), id, sess));
                    }
                }
                (false, sync, None)
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = params["executionContextId"].as_i64() {
                    self.state.lock().unwrap().contexts.remove(&(sid, id));
                }
                (false, Vec::new(), None)
            }
            "Runtime.executionContextsCleared" => {
                self.state.lock().unwrap().contexts.retain(|(s, _), _| s != &sid);
                (false, Vec::new(), None)
            }
            "Runtime.bindingCalled" => {
                if params["name"].as_str() != Some(self.binding.as_str()) {
                    return (false, Vec::new(), None);
                }
                let payload = params["payload"].as_str().unwrap_or("");
                let (Some(ctx_id), Ok(ask)) = (params["executionContextId"].as_i64(), serde_json::from_str::<Ask>(payload)) else {
                    return (true, Vec::new(), None);
                };
                if payload.len() > 128 || ask.id == 0 {
                    return (true, Vec::new(), None);
                }
                let origin = self
                    .state
                    .lock()
                    .unwrap()
                    .contexts
                    .get(&(sid, ctx_id))
                    .cloned()
                    .filter(|o| !o.is_empty() && o != "null");
                let session = session_id.filter(|s| !s.is_empty());
                (
                    true,
                    Vec::new(),
                    Some(ClipboardAsk {
                        origin,
                        allow_msg: self.answer_msg(ask.id, true, ctx_id, session),
                        deny_msg: self.answer_msg(ask.id, false, ctx_id, session),
                    }),
                )
            }
            _ => (false, Vec::new(), None),
        }
    }
}
