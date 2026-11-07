//! What the host knows and does for a plugin beyond drawing: the app's sessions and the
//! server's model catalog (read in a render), server reads it caches and server requests it
//! sends, saved files, theme tokens, panels and popovers.

use crate::abi::{
    AbiStr, Callback, ModelAbi, REQUEST_CLOSE_POPOVER, REQUEST_INVALIDATE, REQUEST_TOAST_ERROR, REQUEST_TOGGLE_PANEL, REQUEST_TOGGLE_POPOVER,
    RESOURCE_FAILED, RESOURCE_READY, ResourceAbi, Rgba, SessionAbi, STATE_CAN_WRITE, STATE_NOW_MS, STATE_PANEL, STATE_POPOVER, STATE_SERVER_LOCAL,
    STATE_SERVERS, TokensAbi,
};
use crate::boundary::{ctx, drop_packed, host};
use crate::cx::{ActionCx, ViewCx, action_callback};

pub use crate::abi::{MODEL_KNOWN, MODEL_PAID, MODEL_PROVIDER_KNOWN, SESSION_BUSY, SESSION_HAS_USAGE, SESSION_LOADED};

/// A session as the app holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub project_id: String,
    pub parent_id: String,
    pub provider_id: String,
    pub model_id: String,
    /// The model of the step `usage` came from.
    pub usage_provider_id: String,
    pub usage_model_id: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// What it cost, its subagents' sessions included.
    pub cost: f64,
    /// The last step with tokens, after the last finished compaction and before a staged revert.
    pub usage: Option<TokensAbi>,
    /// Changes whenever anything here does.
    pub version: u64,
    pub messages: u32,
    pub busy: bool,
    pub loaded: bool,
}

/// A model as the server's catalog names it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    /// The provider's name; `None` when the catalog doesn't list the provider.
    pub provider_name: Option<String>,
    /// The model's name; `None` when the catalog doesn't list it.
    pub name: Option<String>,
    /// Its context window; `None` when the catalog doesn't say.
    pub context_limit: Option<u64>,
    pub paid: bool,
}

/// A server read the host caches for this plugin.
#[derive(Clone, Debug, PartialEq)]
pub enum Resource {
    /// Not answered yet; holds the previous answer while a newer read loads.
    Loading(Option<String>),
    Ready(String),
    Failed(String),
}

/// Where this plugin's panel is, in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelState {
    Closed,
    /// In the session's strip, not selected.
    Open,
    /// Selected in its pane.
    Active,
    /// Selected, with the side region on screen.
    Visible,
}

fn s(v: AbiStr) -> String {
    // SAFETY: the host's views are valid until its next call, which we copy before making.
    unsafe { v.as_str() }.to_owned()
}

fn state(what: u32, arg: &str, arg2: &str) -> u64 {
    // SAFETY: render, handler and poll contexts accept it; the strings are alive for the call.
    unsafe { (host().host_state)(ctx(), what, AbiStr::new(arg), AbiStr::new(arg2)) }
}

fn panel_state(panel: &str, session: &str) -> PanelState {
    match state(STATE_PANEL, panel, session) {
        3 => PanelState::Visible,
        2 => PanelState::Active,
        1 => PanelState::Open,
        _ => PanelState::Closed,
    }
}

impl<P: 'static> ViewCx<P> {
    /// Session `id` as the app holds it.
    pub fn session(&self, id: &str) -> Option<Session> {
        let mut out = SessionAbi::default();
        // SAFETY: a valid out pointer; the views it gets are copied before the next host call.
        if unsafe { (host().session)(ctx(), AbiStr::new(id), &mut out) } == 0 {
            return None;
        }
        Some(Session {
            id: s(out.id),
            title: s(out.title),
            directory: s(out.directory),
            project_id: s(out.project_id),
            parent_id: s(out.parent_id),
            provider_id: s(out.provider_id),
            model_id: s(out.model_id),
            usage_provider_id: s(out.usage_provider_id),
            usage_model_id: s(out.usage_model_id),
            created_ms: out.created_ms,
            updated_ms: out.updated_ms,
            cost: out.cost,
            usage: (out.flags & SESSION_HAS_USAGE != 0).then_some(out.usage),
            version: out.version,
            messages: out.messages,
            busy: out.flags & SESSION_BUSY != 0,
            loaded: out.flags & SESSION_LOADED != 0,
        })
    }

    /// How the server's catalog names model `model` of provider `provider`.
    pub fn model(&self, provider: &str, model: &str) -> ModelInfo {
        let mut out = ModelAbi::default();
        // SAFETY: as `session`.
        unsafe { (host().model)(ctx(), AbiStr::new(provider), AbiStr::new(model), &mut out) };
        ModelInfo {
            provider_name: (out.flags & MODEL_PROVIDER_KNOWN != 0).then(|| s(out.provider_name)),
            name: (out.flags & MODEL_KNOWN != 0).then(|| s(out.name)),
            context_limit: (out.context_limit > 0).then_some(out.context_limit),
            paid: out.flags & MODEL_PAID != 0,
        }
    }

    /// `GET path` from the server, cached under `path` until `version` changes; the frame
    /// redraws when it lands. Also returns its generation, for a memo's version.
    pub fn resource(&self, path: &str, version: u64) -> (Resource, u32) {
        let mut out = ResourceAbi::default();
        // SAFETY: as `session`.
        unsafe { (host().resource)(ctx(), AbiStr::new(path), version, &mut out) };
        let body = s(out.body);
        let r = match out.state {
            RESOURCE_READY => Resource::Ready(body),
            RESOURCE_FAILED => Resource::Failed(body),
            _ => Resource::Loading((!body.is_empty()).then_some(body)),
        };
        (r, out.generation)
    }

    /// Theme token `name` (Desktop's CSS variable without `--`, like `text-strong`).
    pub fn token(&self, name: &str) -> Option<Rgba> {
        let mut out = [0.0; 4];
        // SAFETY: a valid out pointer.
        (unsafe { (host().token)(ctx(), AbiStr::new(name), &mut out) } != 0).then_some(out)
    }

    /// Where this plugin's panel `panel` is in session `session`.
    pub fn panel_state(&self, panel: &str, session: &str) -> PanelState {
        panel_state(panel, session)
    }

    /// Whether this plugin's popover `id` is open.
    pub fn popover_open(&self, id: &str) -> bool {
        state(STATE_POPOVER, id, "") != 0
    }

    /// How many servers the app knows.
    pub fn servers(&self) -> u64 {
        state(STATE_SERVERS, "", "")
    }

    /// Whether the server runs on this machine.
    pub fn server_local(&self) -> bool {
        state(STATE_SERVER_LOCAL, "", "") != 0
    }

    /// The wall clock, in ms since the Unix epoch.
    pub fn now_ms(&self) -> i64 {
        state(STATE_NOW_MS, "", "") as i64
    }

    /// The local time zone's offset from UTC, in seconds, at `ms` since the Unix epoch.
    pub fn utc_offset(&self, ms: i64) -> i64 {
        state(crate::abi::STATE_UTC_OFFSET, &ms.to_string(), "") as i64
    }
}

impl ActionCx {
    /// Opens this plugin's panel `panel`, or closes it when it shows (`layout.toggle`).
    pub fn toggle_panel(&mut self, panel: &str) {
        self.request(REQUEST_TOGGLE_PANEL, panel);
    }

    /// Where this plugin's panel `panel` is in session `session`.
    pub fn panel_state(&self, panel: &str, session: &str) -> PanelState {
        panel_state(panel, session)
    }

    /// Opens this plugin's popover `id` beside the element keyed `anchor`, or closes it.
    pub fn toggle_popover(&mut self, id: &str, anchor: &str) {
        self.request(REQUEST_TOGGLE_POPOVER, &format!("{id}\n{anchor}"));
    }

    pub fn close_popover(&mut self) {
        self.request(REQUEST_CLOSE_POPOVER, "");
    }

    pub fn popover_open(&self, id: &str) -> bool {
        state(STATE_POPOVER, id, "") != 0
    }

    /// A toast with a title and a line under it.
    pub fn toast_with(&mut self, title: &str, description: &str) {
        self.toast(&format!("{title}\n{description}"));
    }

    /// A failure's toast, which stays until dismissed.
    pub fn toast_error(&mut self, title: &str, description: &str) {
        self.request(REQUEST_TOAST_ERROR, &format!("{title}\n{description}"));
    }

    /// Drops this plugin's cached reads whose path starts with `prefix`.
    pub fn invalidate(&mut self, prefix: &str) {
        self.request(REQUEST_INVALIDATE, prefix);
    }

    /// Opens a new session's draft in `directory` with `prompt` typed into it (`layout.draft`).
    pub fn new_draft(&mut self, directory: &str, prompt: &str) {
        self.request(crate::abi::REQUEST_NEW_DRAFT, &format!("{directory}\n{prompt}"));
    }

    /// The wall clock, in ms since the Unix epoch.
    pub fn now_ms(&self) -> i64 {
        state(STATE_NOW_MS, "", "") as i64
    }

    /// Whether the app may write to its server.
    pub fn can_write(&self) -> bool {
        state(STATE_CAN_WRITE, "", "") != 0
    }

    /// Sends `method path` (with JSON `body`, or none when empty) to the server off the app's
    /// thread; `done` runs once it answers, with the body as `cx.input()` and the error (empty
    /// on success) as `cx.input_extra()`.
    pub fn server_request<P: 'static>(&mut self, method: &str, path: &str, body: &str, done: impl Fn(&mut P, &mut ActionCx) + 'static) -> bool {
        let cb = action_callback::<P, _>(done);
        // SAFETY: handler contexts accept it; the strings are alive for the call; the host takes `cb`.
        let ok = unsafe { (host().server_request)(ctx(), AbiStr::new(method), AbiStr::new(path), AbiStr::new(body), cb) } == crate::abi::AbiStatus::Ok;
        refuse(ok, cb)
    }

    /// Saves `content` as `name` where the app saves downloads; `done` runs once it's written,
    /// with the path as `cx.input()` and the error (empty on success) as `cx.input_extra()`.
    pub fn save_file<P: 'static>(&mut self, name: &str, content: &str, done: impl Fn(&mut P, &mut ActionCx) + 'static) -> bool {
        let cb = action_callback::<P, _>(done);
        // SAFETY: as `server_request`.
        let ok = unsafe { (host().save_file)(ctx(), AbiStr::new(name), AbiStr::new(content), cb) } == crate::abi::AbiStatus::Ok;
        refuse(ok, cb)
    }

    fn request(&mut self, kind: u32, arg: &str) {
        // SAFETY: handler and poll contexts accept it.
        unsafe { (host().request)(ctx(), kind, AbiStr::new(arg)) };
    }
}

/// Frees a callback the host refused.
fn refuse(ok: bool, cb: Callback) -> bool {
    if !ok {
        drop_packed(cb.data, cb.drop_data);
    }
    ok
}
