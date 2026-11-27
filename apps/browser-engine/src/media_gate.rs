//! Camera / microphone gate matching official `packages/gui-plugins/src/browser/media-gate.ts`
//! plus passive audio-playback detection (HTMLMediaElement & Web Audio API) for tab hibernation
//! protection without hijacking speaker output via `CefAudioHandler`.
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

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::wire::PermissionKind;

/// Internal CDP message IDs for gate answers (swallowed by the passkey-range reply check in
/// `cdp_ledger.rs`, below `frame_authority::INTERNAL_ID_BASE`).
const MEDIA_GATE_ID_BASE: i32 = 1_550_000_000;

/// The page-side wrapper (`media-gate.ts:31-66`) plus passive audio playback tracking.
/// `binding` is taken from the page and deleted before any page script runs; `answer` is the
/// function the engine releases held `getUserMedia` calls through.
pub fn media_gate_script(binding: &str, answer: &str) -> String {
    let binding_json = serde_json::to_string(binding).unwrap_or_else(|_| "\"_b\"".to_owned());
    let answer_json = serde_json::to_string(answer).unwrap_or_else(|_| "\"_a\"".to_owned());
    format!(
        r#"(() => {{
  const gate = globalThis[{binding_json}]
  if (typeof gate !== "function") return
  delete globalThis[{binding_json}]
  const proto = globalThis.MediaDevices && MediaDevices.prototype
  if (proto && !Object.getOwnPropertyDescriptor(globalThis, {answer_json})) {{
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
  }}
  // Passive audio-playback monitor: reports state transitions only (zero polling, zero audio hijack).
  const mediaEls = new Set()
  const audioCtxs = new Set()
  let lastPlayback = false
  const isAudibleMedia = (el) => {{
    try {{
      return !!el && !el.paused && !el.ended && !el.muted && el.volume > 0
    }} catch (_) {{
      return false
    }}
  }}
  const recompute = () => {{
    let active = false
    for (const el of mediaEls) {{
      if (isAudibleMedia(el)) {{
        active = true
      }} else if (el.paused || el.ended) {{
        mediaEls.delete(el)
      }}
    }}
    if (!active && audioCtxs.size > 0) {{
      for (const ref of audioCtxs) {{
        const ctx = typeof WeakRef === "function" ? ref.deref() : ref
        if (!ctx || ctx.state === "closed") {{
          audioCtxs.delete(ref)
        }} else if (ctx.state === "running") {{
          active = true
        }}
      }}
    }}
    if (active !== lastPlayback) {{
      lastPlayback = active
      try {{
        gate(JSON.stringify({{ playback: active }}))
      }} catch (_) {{}}
    }}
  }}
  const trackEl = (el) => {{
    if (!el || typeof el !== "object") return
    if (!mediaEls.has(el)) {{
      mediaEls.add(el)
      for (const ev of ["pause", "ended", "emptied", "volumechange", "playing"]) {{
        try {{
          el.addEventListener(ev, recompute, {{ passive: true }})
        }} catch (_) {{}}
      }}
    }}
    recompute()
  }}
  if (globalThis.HTMLMediaElement && HTMLMediaElement.prototype && typeof HTMLMediaElement.prototype.play === "function") {{
    const origPlay = HTMLMediaElement.prototype.play
    HTMLMediaElement.prototype.play = function play() {{
      trackEl(this)
      return origPlay.apply(this, arguments)
    }}
  }}
  if (typeof document !== "undefined" && document && typeof document.addEventListener === "function") {{
    const onMediaEv = (e) => {{
      const t = e && e.target
      if (t && (t instanceof HTMLMediaElement || typeof t.paused === "boolean")) {{
        if (e.type === "play" || e.type === "playing") trackEl(t)
        else recompute()
      }}
    }}
    for (const ev of ["play", "playing", "pause", "ended", "emptied", "volumechange"]) {{
      document.addEventListener(ev, onMediaEv, {{ capture: true, passive: true }})
    }}
  }}
  for (const name of ["AudioContext", "webkitAudioContext"]) {{
    const Ctor = globalThis[name]
    if (typeof Ctor !== "function") continue
    try {{
      globalThis[name] = new Proxy(Ctor, {{
        construct(target, args, newTarget) {{
          const ctx = Reflect.construct(target, args, newTarget)
          const ref = typeof WeakRef === "function" ? new WeakRef(ctx) : ctx
          audioCtxs.add(ref)
          try {{
            ctx.addEventListener("statechange", recompute, {{ passive: true }})
          }} catch (_) {{}}
          recompute()
          return ctx
        }},
      }})
    }} catch (_) {{}}
  }}
  if (typeof globalThis.addEventListener === "function") {{
    globalThis.addEventListener("pagehide", () => {{
      if (lastPlayback) {{
        lastPlayback = false
        try {{
          gate(JSON.stringify({{ playback: false }}))
        }} catch (_) {{}}
      }}
    }}, {{ passive: true }})
  }}
}})()"#
    )
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BindingPayload {
    Playback { playback: bool },
    Ask { id: u32, audio: bool, video: bool },
}

/// A held `getUserMedia` the pane must decide: who asks, for which devices, and the two CDP
/// messages that answer it (allow, deny) in the asking execution context.
pub struct MediaAsk {
    pub origin: Option<String>,
    pub kinds: Vec<PermissionKind>,
    pub allow_msg: String,
    pub deny_msg: String,
}

/// Result of observing one CDP event in [`TabMediaGate::observe_event`].
#[derive(Default)]
pub struct MediaGateOutcome {
    pub consumed: bool,
    pub ask: Option<MediaAsk>,
    /// `Some(audible)` whenever combined media capture/playback audibility transitioned.
    pub audible_changed: Option<bool>,
}

#[derive(Default)]
struct ContextState {
    /// `(session_id, execution_context_id)` -> origin.
    contexts: HashMap<(String, i64), String>,
    /// Active camera/microphone capture from CEF `DisplayHandler::on_media_access_change`.
    capture: bool,
    /// CDP session IDs (`""` for root main frame, child `sessionId` for OOPIFs) actively playing audio.
    playback_sessions: HashSet<String>,
}

impl ContextState {
    fn is_audible(&self) -> bool {
        self.capture || !self.playback_sessions.is_empty()
    }
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

    /// Updates active camera/microphone capture state and returns `Some(audible)` when the
    /// tab's combined audible state changed.
    pub fn set_capture(&self, active: bool) -> Option<bool> {
        let mut st = self.state.lock().unwrap();
        let prev = st.is_audible();
        st.capture = active;
        let next = st.is_audible();
        (next != prev).then_some(next)
    }

    /// Clears capture and playback tracking (on main-frame navigation or renderer crash).
    /// Returns `true` if the tab was previously audible.
    pub fn reset_audible(&self) -> bool {
        let mut st = self.state.lock().unwrap();
        let was = st.is_audible();
        st.capture = false;
        st.playback_sessions.clear();
        was
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

    /// Observes a CDP event. Returns consumed status, any held `getUserMedia` ask, and whether the
    /// tab's combined audible state transitioned.
    pub fn observe_event(&self, method: &str, params: &Value, session_id: Option<&str>) -> MediaGateOutcome {
        let sid = session_id.unwrap_or("").to_owned();
        match method {
            "Target.detachedFromTarget" => {
                let mut audible_changed = None;
                if let Some(detached) = params.get("sessionId").and_then(Value::as_str) {
                    let mut st = self.state.lock().unwrap();
                    let prev = st.is_audible();
                    st.contexts.retain(|(s, _), _| s != detached);
                    st.playback_sessions.remove(detached);
                    let next = st.is_audible();
                    if next != prev {
                        audible_changed = Some(next);
                    }
                }
                MediaGateOutcome { consumed: false, ask: None, audible_changed }
            }
            "Runtime.executionContextCreated" => {
                let ctx = &params["context"];
                if ctx["auxData"]["isDefault"].as_bool() == Some(true)
                    && let Some(id) = ctx["id"].as_i64()
                {
                    let origin = ctx["origin"].as_str().unwrap_or("").to_owned();
                    self.state.lock().unwrap().contexts.insert((sid, id), origin);
                }
                MediaGateOutcome::default()
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = params["executionContextId"].as_i64() {
                    self.state.lock().unwrap().contexts.remove(&(sid, id));
                }
                MediaGateOutcome::default()
            }
            "Runtime.executionContextsCleared" => {
                let mut st = self.state.lock().unwrap();
                let prev = st.is_audible();
                st.contexts.retain(|(s, _), _| s != &sid);
                st.playback_sessions.remove(&sid);
                let next = st.is_audible();
                MediaGateOutcome {
                    consumed: false,
                    ask: None,
                    audible_changed: (next != prev).then_some(next),
                }
            }
            "Runtime.bindingCalled" => {
                if params["name"].as_str() != Some(self.binding.as_str()) {
                    return MediaGateOutcome::default();
                }
                let payload = params["payload"].as_str().unwrap_or("");
                if payload.len() > 256 {
                    return MediaGateOutcome { consumed: true, ..Default::default() };
                }
                match serde_json::from_str::<BindingPayload>(payload) {
                    Ok(BindingPayload::Playback { playback }) => {
                        let mut st = self.state.lock().unwrap();
                        let prev = st.is_audible();
                        if playback {
                            st.playback_sessions.insert(sid);
                        } else {
                            st.playback_sessions.remove(&sid);
                        }
                        let next = st.is_audible();
                        MediaGateOutcome {
                            consumed: true,
                            ask: None,
                            audible_changed: (next != prev).then_some(next),
                        }
                    }
                    Ok(BindingPayload::Ask { id, audio, video }) => {
                        let Some(ctx_id) = params["executionContextId"].as_i64() else {
                            return MediaGateOutcome { consumed: true, ..Default::default() };
                        };
                        if id == 0 || (!audio && !video) {
                            return MediaGateOutcome { consumed: true, ..Default::default() };
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
                        let kinds = [(video, PermissionKind::Camera), (audio, PermissionKind::Microphone)]
                            .into_iter()
                            .filter_map(|(on, k)| on.then_some(k))
                            .collect();
                        let session = session_id.filter(|s| !s.is_empty());
                        MediaGateOutcome {
                            consumed: true,
                            ask: Some(MediaAsk {
                                origin,
                                kinds,
                                allow_msg: self.answer_msg(id, true, ctx_id, session),
                                deny_msg: self.answer_msg(id, false, ctx_id, session),
                            }),
                            audible_changed: None,
                        }
                    }
                    Err(_) => MediaGateOutcome { consumed: true, ..Default::default() },
                }
            }
            _ => MediaGateOutcome::default(),
        }
    }
}
