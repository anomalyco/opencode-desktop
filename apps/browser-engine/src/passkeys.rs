//! WebAuthn / passkey gate matching official `packages/gui-plugins/src/browser/webauthn-gate.ts`.
//!
//! CEF OSR draws no WebAuthn request UI:
//! - On macOS and Linux, Chromium's WebAuthn request would hang waiting for a missing UI dialog;
//!   every tab therefore starts with `enforced = true` (`WebAuthn.enable { enableUI: false }` with
//!   no virtual authenticator, plus the page-side fast-path wrapper rejecting immediately with
//!   `NotAllowedError` so sites fall back to password / magic-link / OTP sign-in).
//! - On Windows, Chromium delegates WebAuthn to the system Windows Security dialog on human tabs
//!   (`native = true`, `enforced = false`), and permanently switches to `enforced = true` as soon
//!   as the tab becomes agent-driven (`OpenTab.agent_driven`, `SetAgentDriven(true)`, or agent CDP).
//!
//! All CDP commands produced inside `DevToolsMessageObserver` callbacks are returned as JSON
//! strings for `cdp_ledger.outbox` (`flush_outbox` + `Event::Pump(0)`) so CEF's non-reentrant
//! observer list is never re-entered (`base/observer_list.h:192`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::output::Output;
use crate::wire::{EngineMsg, TabSnapshot};

/// Internal CDP message ID range owned by `TabPasskeyGate` (above app IDs, below
/// `frame_authority::INTERNAL_ID_BASE = 1_600_000_000`).
pub const PASSKEY_ID_BASE: i32 = 1_500_000_000;

/// Exact user- and agent-facing refusal message (`webauthn-gate.ts:17-18`).
pub const PASSKEY_REFUSAL: &str = "Passkeys and security keys are not supported in this browser yet. Use another sign-in method, or sign in with your own browser and import the sign-in.";

static TOKEN_SEQ: AtomicU64 = AtomicU64::new(1);

fn fill_os_random(buf: &mut [u8; 16]) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Security::Cryptography::{
            BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
        };
        // SAFETY: `buf` is a valid writable 16-byte buffer and `BCRYPT_USE_SYSTEM_PREFERRED_RNG`
        // uses the OS CSPRNG without requiring an algorithm handle.
        unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_ok() }
    }
    #[cfg(unix)]
    {
        use std::io::Read as _;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(buf))
            .is_ok()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = buf;
        false
    }
}

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if !fill_os_random(&mut bytes) {
        let mut h = Sha256::new();
        h.update(TOKEN_SEQ.fetch_add(1, Ordering::Relaxed).to_le_bytes());
        h.update(std::process::id().to_le_bytes());
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.update(d.as_nanos().to_le_bytes());
        }
        bytes.copy_from_slice(&h.finalize()[..16]);
    }
    let mut out = String::with_capacity(33);
    out.push('_');
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// Builds the page-side fast-path wrapper (`webauthn-gate.ts:38-108`).
pub fn webauthn_gate_script(report: &str, answer: &str, native: bool) -> String {
    let report_json = serde_json::to_string(report).unwrap_or_else(|_| "\"_r\"".to_owned());
    let answer_json = serde_json::to_string(answer).unwrap_or_else(|_| "\"_a\"".to_owned());
    let refusal_json = serde_json::to_string(PASSKEY_REFUSAL).unwrap_or_else(|_| "\"\"".to_owned());
    let native_js = if native { "true" } else { "false" };
    format!(
        r#"(() => {{
  const ask = globalThis[{report_json}]
  if (typeof ask !== "function") return
  delete globalThis[{report_json}]
  const proto = globalThis.CredentialsContainer && CredentialsContainer.prototype
  const credential = globalThis.PublicKeyCredential
  if (!proto) return
  const apply = Reflect.apply
  const listen = EventTarget.prototype.addEventListener
  const warn = console.warn
  const waiting = new Map()
  let next = 0
  apply(listen, globalThis, [{answer_json}, (event) => {{
    const reply = event.detail
    const resolve = Array.isArray(reply) && waiting.get(reply[0])
    if (!resolve) return
    waiting.delete(reply[0])
    resolve(reply[1] === true)
  }}])
  const refused = () => new DOMException({refusal_json}, "NotAllowedError")
  const stopped = (signal) => signal.reason ?? new DOMException("The operation was aborted.", "AbortError")
  const request = (kind, target, self, args) => {{
    const options = args[0]
    if (options === null || (typeof options !== "object" && typeof options !== "function")) return apply(target, self, args)
    let publicKey, mediation, signal
    try {{
      publicKey = options.publicKey
      mediation = options.mediation
      signal = options.signal
    }} catch (error) {{
      return Promise.reject(error)
    }}
    const fixed = Object.create(options, {{
      publicKey: {{ value: publicKey, enumerable: true }},
      mediation: {{ value: mediation, enumerable: true }},
      signal: {{ value: signal, enumerable: true }},
    }})
    const call = () => apply(target, self, [fixed, ...args.slice(1)])
    if (!publicKey) return call()
    const abortable = signal instanceof AbortSignal
    if (abortable && signal.aborted) return Promise.reject(stopped(signal))
    if (kind === "get" && mediation === "conditional")
      return new Promise((_, reject) => abortable && signal.addEventListener("abort", () => reject(stopped(signal)), {{ once: true }}))
    if (kind === "get" && mediation === "immediate") return Promise.reject(new DOMException("No passkey is available here.", "NotAllowedError"))
    const id = ++next
    const decided = {native_js} ? new Promise((resolve) => waiting.set(id, resolve)) : Promise.resolve(false)
    ask(JSON.stringify({{ id, kind }}))
    return decided.then((allowed) => {{
      if (allowed) return call()
      apply(warn, console, ["navigator.credentials." + kind + ": " + {refusal_json}])
      return Promise.reject(refused())
    }})
  }}
  const wrap = (kind) => new Proxy(proto[kind], {{ apply: (target, self, args) => request(kind, target, self, args) }})
  for (const kind of ["get", "create"])
    Object.defineProperty(proto, kind, {{ value: wrap(kind), writable: true, configurable: true, enumerable: true }})
  if (!credential) return
  const answerWith = (name, run) => {{
    const original = credential[name]
    if (typeof original === "function")
      Object.defineProperty(credential, name, {{ value: new Proxy(original, {{ apply: (target, self, args) => run(target, self, args) }}), writable: true, configurable: true }})
  }}
  answerWith("isConditionalMediationAvailable", () => Promise.resolve(false))
  if (!{native_js}) answerWith("isUserVerifyingPlatformAuthenticatorAvailable", () => Promise.resolve(false))
  const unavailable = ["conditionalGet", "conditionalCreate", "immediateGet", ...({native_js} ? [] : ["hybridTransport", "passkeyPlatformAuthenticator", "userVerifyingPlatformAuthenticator"])]
  answerWith("getClientCapabilities", (target, self, args) =>
    apply(target, self, args).then((found) => ({{ ...found, ...Object.fromEntries(unavailable.map((name) => [name, false])) }})),
  )
}})()"#
    )
}

#[derive(Deserialize)]
struct PasskeyAsk {
    id: u32,
    kind: String,
}

#[derive(Default)]
struct ContextState {
    top_frame: Option<String>,
    /// Keyed by `(session_id, execution_context_id)` -> `(origin, frame_id)`.
    contexts: HashMap<(String, i64), (String, Option<String>)>,
}

/// Per-tab WebAuthn / passkey gate coordinated over internal CDP.
pub struct TabPasskeyGate {
    tab_id: u64,
    native: bool,
    enforced: AtomicBool,
    webauthn_enabled: AtomicBool,
    enable_msg_id: AtomicI32,
    next_id: AtomicI32,
    report: String,
    answer: String,
    state: Mutex<ContextState>,
}

impl TabPasskeyGate {
    pub fn new(tab_id: u64, initially_enforced: bool) -> Self {
        let native = cfg!(windows);
        Self {
            tab_id,
            native,
            enforced: AtomicBool::new(!native || initially_enforced),
            webauthn_enabled: AtomicBool::new(false),
            enable_msg_id: AtomicI32::new(0),
            next_id: AtomicI32::new(0),
            report: random_token(),
            answer: random_token(),
            state: Mutex::new(ContextState::default()),
        }
    }

    pub fn native(&self) -> bool {
        self.native
    }

    fn alloc_id(&self) -> i32 {
        let n = self.next_id.fetch_add(1, Ordering::Relaxed).rem_euclid(90_000_000);
        PASSKEY_ID_BASE + n
    }

    fn build_cdp(&self, method: &str, params: Value, session_id: Option<&str>) -> (i32, String) {
        let id = self.alloc_id();
        let mut payload = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            payload["sessionId"] = Value::String(sid.to_owned());
        }
        (id, payload.to_string())
    }

    /// CDP messages installing the hidden binding and page-side fast-path wrapper on the root
    /// session (`None`) or a paused web OOPIF session (`Some(sid)`).
    pub fn attach_messages(&self, session_id: Option<&str>) -> Vec<String> {
        let add_binding = self.binding_msg(session_id);
        let (_, add_script) = self.build_cdp(
            "Page.addScriptToEvaluateOnNewDocument",
            json!({
                "source": webauthn_gate_script(&self.report, &self.answer, self.native),
                "runImmediately": true,
            }),
            session_id,
        );
        vec![add_binding, add_script]
    }

    /// `Runtime.addBinding` for the wrapper's report channel on `session_id`. A `Runtime.disable`
    /// on that session drops it (V8 `V8RuntimeAgentImpl::disable` removes `globalBindings`), so
    /// the engine sends it again after each `Runtime.disable` it sends.
    pub fn binding_msg(&self, session_id: Option<&str>) -> String {
        self.build_cdp("Runtime.addBinding", json!({ "name": self.report }), session_id).1
    }

    /// Builds `Page.setInterceptFileChooserDialog` (`file-choosers.ts:19`) for root (`None`) or
    /// an OOPIF session (`Some(sid)`), suppressing HTML file inputs & File System Access pickers
    /// in Blink while the tab is agent-driven or a preview.
    pub fn file_chooser_intercept_msg(&self, enabled: bool, session_id: Option<&str>) -> String {
        self.build_cdp(
            "Page.setInterceptFileChooserDialog",
            json!({ "enabled": enabled, "cancel": enabled }),
            session_id,
        )
        .1
    }

    /// Permanently enforces passkey refusal for this tab (idempotent, never undone): returns
    /// `WebAuthn.enable { enableUI: false }` the first time (or after a failed attempt) so every
    /// frame in the tab's tree (including OOPIFs) fails inside Chromium without touching OS UI.
    pub fn enforce_messages(&self) -> Vec<String> {
        self.enforced.store(true, Ordering::Release);
        if self
            .webauthn_enabled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Vec::new();
        }
        let (id, raw) = self.build_cdp("WebAuthn.enable", json!({ "enableUI": false }), None);
        self.enable_msg_id.store(id, Ordering::Release);
        vec![raw]
    }

    /// Handles a reply in `PASSKEY_ID_BASE..INTERNAL_ID_BASE`. Returns `true` when
    /// `WebAuthn.enable` failed on a native-WebAuthn platform (Windows), where failing open would
    /// leave the system dialog reachable on an agent tab (`chromium.ts:839`).
    pub fn on_reply(&self, msg_id: i32, ok: bool) -> bool {
        if !ok && self.enable_msg_id.load(Ordering::Acquire) == msg_id {
            self.webauthn_enabled.store(false, Ordering::Release);
            return self.native;
        }
        false
    }

    /// Observes a CDP event. Returns `(queued_cdp_messages, consumed)` where `consumed == true`
    /// indicates our private `Runtime.bindingCalled` token and MUST NOT be forwarded to the host.
    pub fn observe_event(
        &self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
        snap: &Mutex<TabSnapshot>,
        out_tx: &Output,
    ) -> (Vec<String>, bool) {
        let sid = session_id.unwrap_or("").to_owned();
        match method {
            "Page.frameNavigated" => {
                if sid.is_empty()
                    && params["frame"]["parentId"].is_null()
                    && let Some(id) = params["frame"]["id"].as_str()
                {
                    self.state.lock().unwrap().top_frame = Some(id.to_owned());
                }
                (Vec::new(), false)
            }
            "Target.detachedFromTarget" => {
                if let Some(detached) = params.get("sessionId").and_then(Value::as_str) {
                    self.state
                        .lock()
                        .unwrap()
                        .contexts
                        .retain(|(k_sid, _), _| k_sid != detached);
                }
                (Vec::new(), false)
            }
            "Runtime.executionContextCreated" => {
                let ctx = &params["context"];
                if ctx["auxData"]["isDefault"].as_bool() == Some(true)
                    && let Some(id) = ctx["id"].as_i64()
                {
                    let origin = ctx["origin"].as_str().unwrap_or("").to_owned();
                    let frame_id = ctx["auxData"]["frameId"].as_str().map(str::to_owned);
                    self.state
                        .lock()
                        .unwrap()
                        .contexts
                        .insert((sid, id), (origin, frame_id));
                }
                (Vec::new(), false)
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = params["executionContextId"].as_i64() {
                    self.state.lock().unwrap().contexts.remove(&(sid, id));
                }
                (Vec::new(), false)
            }
            "Runtime.executionContextsCleared" => {
                self.state
                    .lock()
                    .unwrap()
                    .contexts
                    .retain(|(k_sid, _), _| k_sid != &sid);
                (Vec::new(), false)
            }
            "Runtime.bindingCalled" => {
                if params["name"].as_str() != Some(self.report.as_str()) {
                    return (Vec::new(), false);
                }
                let payload = params["payload"].as_str().unwrap_or("");
                if payload.len() > 256 {
                    return (Vec::new(), true);
                }
                let Ok(ask) = serde_json::from_str::<PasskeyAsk>(payload) else {
                    return (Vec::new(), true);
                };
                if ask.id == 0 || (ask.kind != "get" && ask.kind != "create") {
                    return (Vec::new(), true);
                }
                let ctx_id = params["executionContextId"].as_i64();
                let (origin, main_frame) = {
                    let st = self.state.lock().unwrap();
                    let entry = ctx_id.and_then(|id| st.contexts.get(&(sid.clone(), id)));
                    let origin = entry
                        .map(|(o, _)| o.clone())
                        .filter(|o| !o.is_empty() && o != "null");
                    let main_frame = sid.is_empty()
                        && entry
                            .and_then(|(_, fid)| fid.as_deref())
                            .is_some_and(|fid| st.top_frame.as_deref() == Some(fid));
                    (origin, main_frame)
                };
                let allowed = !self.enforced.load(Ordering::Acquire);
                if !allowed {
                    // Cross-origin subframes are refused silently (`chromium.ts:813`).
                    let main_origin = browser_policy::destination_origin(&snap.lock().unwrap().url);
                    let speaks_for_page = main_frame
                        || (origin.is_some() && origin == main_origin);
                    if speaks_for_page {
                        let _ = out_tx.send(EngineMsg::PasskeyRefused {
                            tab_id: self.tab_id,
                            origin,
                            main_frame,
                            kind: ask.kind,
                        });
                    }
                }
                let mut send = Vec::new();
                if self.native
                    && let Some(exec_id) = ctx_id
                {
                    let answer_json =
                        serde_json::to_string(&self.answer).unwrap_or_else(|_| "\"\"".to_owned());
                    let expr = format!(
                        "EventTarget.prototype.dispatchEvent.call(globalThis, new CustomEvent({answer_json}, {{ detail: [{}, {}] }}))",
                        ask.id,
                        if allowed { "true" } else { "false" }
                    );
                    let (_, raw) = self.build_cdp(
                        "Runtime.evaluate",
                        json!({ "expression": expr, "contextId": exec_id }),
                        session_id,
                    );
                    send.push(raw);
                }
                (send, true)
            }
            _ => (Vec::new(), false),
        }
    }
}
