//! A page's notifications reach the person through the app's own system notifications, as
//! official's Electron shows them (its `Notification` is the system's, under the app's name).
//!
//! The engine's own Chromium shows nothing a person sees: it has no Start-menu identity on
//! Windows and no notification rights on macOS, so `new Notification()` fired `show` and vanished.
//! The page's `Notification` and `ServiceWorkerRegistration.showNotification` therefore go through
//! a hidden CDP binding (as the media gate's `getUserMedia` does) once Chromium's own permission
//! for the site is `granted`; the engine sends them to the app as extension-style toasts under
//! `page:<tab id>`, and a toast's click or close comes back as the page's `click` / `close`.
//!
//! Like the other gates, every CDP message produced inside a `DevToolsMessageObserver` callback is
//! returned for the ledger's outbox, never sent re-entrantly.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

/// Internal CDP message IDs for event answers (swallowed by the passkey-range reply check in
/// `cdp_ledger.rs`, below `frame_authority::INTERNAL_ID_BASE`).
const NOTIFICATION_GATE_ID_BASE: i32 = 1_600_000_000;
/// Toasts a tab keeps track of for their clicks; the oldest is forgotten past it.
const MAX_TRACKED: usize = 64;
const MAX_TITLE: usize = 256;
const MAX_BODY: usize = 1024;

/// The page-side wrapper. `binding` is taken from the page and deleted before any page script
/// runs; `answer` is the function the engine delivers `click` / `close` through. The permission is
/// read through the original getter, captured before any page script can replace it.
pub fn notification_gate_script(binding: &str, answer: &str) -> String {
    let binding_json = serde_json::to_string(binding).unwrap_or_else(|_| "\"_b\"".to_owned());
    let answer_json = serde_json::to_string(answer).unwrap_or_else(|_| "\"_a\"".to_owned());
    format!(
        r#"(() => {{
  const gate = globalThis[{binding_json}]
  if (typeof gate !== "function") return
  delete globalThis[{binding_json}]
  const Original = globalThis.Notification
  if (typeof Original !== "function" || Object.getOwnPropertyDescriptor(globalThis, {answer_json})) return
  const permission = Object.getOwnPropertyDescriptor(Original, "permission").get.bind(Original)
  const live = new Map()
  let next = 0
  Object.defineProperty(globalThis, {answer_json}, {{
    value: (id, type) => {{
      const n = live.get(id)
      if (!n) return
      if (type === "close") live.delete(id)
      n.dispatchEvent(new Event(type))
    }},
  }})
  const send = (n, title, options) => {{
    const id = ++next
    live.set(id, n)
    if (live.size > {MAX_TRACKED}) live.delete(live.keys().next().value)
    gate(JSON.stringify({{ op: "show", id, title: String(title).slice(0, {MAX_TITLE}), body: String((options && options.body) || "").slice(0, {MAX_BODY}) }}))
    return id
  }}
  class Notification extends EventTarget {{
    #id = 0
    constructor(title, options = {{}}) {{
      if (!new.target) throw new TypeError("Failed to construct 'Notification': Please use the 'new' operator.")
      if (arguments.length === 0) throw new TypeError("Failed to construct 'Notification': 1 argument required, but only 0 present.")
      super()
      this.title = String(title)
      this.body = String(options.body ?? "")
      this.tag = String(options.tag ?? "")
      this.icon = String(options.icon ?? "")
      this.data = options.data ?? null
      this.onclick = this.onshow = this.onclose = this.onerror = null
      for (const type of ["click", "show", "close", "error"]) this.addEventListener(type, (e) => {{ const f = this["on" + type]; if (typeof f === "function") f.call(this, e) }})
      if (permission() !== "granted") {{
        queueMicrotask(() => this.dispatchEvent(new Event("error")))
        return
      }}
      this.#id = send(this, title, options)
      queueMicrotask(() => this.dispatchEvent(new Event("show")))
    }}
    close() {{
      if (!this.#id || !live.delete(this.#id)) return
      gate(JSON.stringify({{ op: "close", id: this.#id }}))
      this.dispatchEvent(new Event("close"))
    }}
    static get permission() {{ return permission() }}
    static get maxActions() {{ return Original.maxActions }}
    static requestPermission(callback) {{ return Original.requestPermission(callback) }}
  }}
  Object.defineProperty(globalThis, "Notification", {{ value: Notification, writable: true, configurable: true }})
  const reg = globalThis.ServiceWorkerRegistration && ServiceWorkerRegistration.prototype
  if (reg && typeof reg.showNotification === "function") {{
    const showNotification = function showNotification(title, options = {{}}) {{
      if (permission() !== "granted") return Promise.reject(new TypeError("No notification permission has been granted for this origin."))
      send(new EventTarget(), title, options)
      return Promise.resolve()
    }}
    Object.defineProperty(reg, "showNotification", {{ value: showNotification, writable: true, configurable: true, enumerable: true }})
    Object.defineProperty(reg, "getNotifications", {{ value: () => Promise.resolve([]), writable: true, configurable: true, enumerable: true }})
  }}
}})()"#
    )
}

#[derive(Deserialize)]
struct Call {
    op: String,
    id: u32,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

/// What the app is asked to do for a page's call.
pub enum NotificationEffect {
    Show { notif_id: String, origin: String, title: String, body: String },
    Close { notif_id: String },
}

#[derive(Default)]
struct State {
    /// `(session_id, execution_context_id)` -> origin.
    contexts: HashMap<(String, i64), String>,
    /// Engine notification id -> where its page object lives: `(session_id, context id, page id)`.
    shown: Vec<(String, (String, i64, u32))>,
}

pub struct TabNotificationGate {
    binding: String,
    answer: String,
    next_msg: AtomicI32,
    next_notif: AtomicU64,
    state: Mutex<State>,
}

impl TabNotificationGate {
    pub fn new() -> Self {
        Self {
            binding: crate::passkeys::random_token(),
            answer: crate::passkeys::random_token(),
            next_msg: AtomicI32::new(0),
            next_notif: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }
    }

    fn build_cdp(&self, method: &str, params: Value, session_id: Option<&str>) -> String {
        let id = NOTIFICATION_GATE_ID_BASE + self.next_msg.fetch_add(1, Ordering::Relaxed).rem_euclid(25_000_000);
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
                json!({ "source": notification_gate_script(&self.binding, &self.answer), "runImmediately": true }),
                session_id,
            ),
        ]
    }

    /// `Runtime.addBinding` for the wrapper's channel, sent again after each `Runtime.disable`.
    pub fn binding_msg(&self, session_id: Option<&str>) -> String {
        self.build_cdp("Runtime.addBinding", json!({ "name": self.binding }), session_id)
    }

    /// The page's `click` (or `close`, when the person dismissed it or the app could not show it)
    /// for notification `notif_id`, as a CDP message to send; `None` once its page is gone.
    pub fn event_msg(&self, notif_id: &str, clicked: bool) -> Option<String> {
        let mut state = self.state.lock().unwrap();
        let at = state.shown.iter().position(|(id, _)| id == notif_id)?;
        let (sid, ctx, page_id) = if clicked { state.shown[at].1.clone() } else { state.shown.remove(at).1 };
        if !state.contexts.contains_key(&(sid.clone(), ctx)) {
            return None;
        }
        drop(state);
        let answer_json = serde_json::to_string(&self.answer).unwrap_or_else(|_| "\"\"".to_owned());
        let kind = if clicked { "click" } else { "close" };
        Some(self.build_cdp(
            "Runtime.evaluate",
            json!({ "expression": format!("globalThis[{answer_json}]?.({page_id}, \"{kind}\")"), "contextId": ctx }),
            Some(sid.as_str()).filter(|s| !s.is_empty()),
        ))
    }

    /// Observes a CDP event. Returns `(consumed, effect)`: `consumed` marks our private binding
    /// call (never forwarded to the app); `effect` is what the app should show or take down.
    pub fn observe_event(&self, method: &str, params: &Value, session_id: Option<&str>) -> (bool, Option<NotificationEffect>) {
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
                let (Some(ctx), Ok(call)) = (params["executionContextId"].as_i64(), serde_json::from_str::<Call>(payload)) else {
                    return (true, None);
                };
                if payload.len() > 4096 || call.id == 0 {
                    return (true, None);
                }
                let mut state = self.state.lock().unwrap();
                // A call from a context the gate never saw has no origin and shows nothing.
                let Some(origin) = state.contexts.get(&(sid.clone(), ctx)).cloned().filter(|o| !o.is_empty() && o != "null") else {
                    return (true, None);
                };
                let place = (sid, ctx, call.id);
                match call.op.as_str() {
                    "show" => {
                        let notif_id = (self.next_notif.fetch_add(1, Ordering::Relaxed) + 1).to_string();
                        if state.shown.len() >= MAX_TRACKED {
                            state.shown.remove(0);
                        }
                        state.shown.push((notif_id.clone(), place));
                        let title: String = call.title.chars().take(MAX_TITLE).collect();
                        let body: String = call.body.chars().take(MAX_BODY).collect();
                        (true, Some(NotificationEffect::Show { notif_id, origin, title, body }))
                    }
                    "close" => {
                        let Some(at) = state.shown.iter().position(|(_, p)| *p == place) else {
                            return (true, None);
                        };
                        let (notif_id, _) = state.shown.remove(at);
                        (true, Some(NotificationEffect::Close { notif_id }))
                    }
                    _ => (true, None),
                }
            }
            _ => (false, None),
        }
    }
}
