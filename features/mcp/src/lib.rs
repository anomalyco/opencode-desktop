//! A location's MCP servers (Desktop's `providers/connect/mcp.tsx` and `mcp-machine.ts`): their
//! connects, disconnects and sign-ins in flight, one per server at a time, and the MCPs dialog
//! (`/mcp`, mod+;) with its inline error view. Settings › Extensions toggles through the same
//! state, so a server connecting from the dialog stays busy there.

#![forbid(unsafe_code)]

pub mod sign_in;
mod ui;
pub mod view;

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::client::Server;
use opencode_kit::integration::{Authorization, McpServer, McpStatus, MethodKind};
use rune::machine::{Attempt, Seq};
use rune::{Key, KeyEvent, TextEdit, Tx};

pub use ui::{status_dot, status_ink};

/// Overlay stack identifier for the MCPs dialog (`opencode_kernel::OverlayStack`).
pub const OVERLAY_ID: &str = "dialog.mcp";
/// Overlay stack identifier for the MCP browser OAuth sign-in dialog (`opencode_kernel::OverlayStack`).
pub const SIGN_IN_OVERLAY_ID: &str = "dialog.mcp.sign_in";

/// How long one connect, disconnect or sign-in may run before it gives up.
pub const TOGGLE_TIMEOUT_S: f64 = 60.0;

/// The dialog's error view of one failed server (`McpDetailState`).
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Closed,
    /// Opened on a server before the list loaded.
    Opening { attempt: Attempt, name: String },
    /// `error` is the server's latest failure, kept while a retry runs; `copied` is the error
    /// text last copied.
    Open { attempt: Attempt, name: String, error: String, copied: Option<String> },
}

pub struct Dialog {
    /// The folder whose servers it lists.
    pub dir: String,
    pub search: TextEdit,
    pub active: Option<String>,
    pub detail: Detail,
}

/// One server's connect, disconnect or sign-in in flight (`McpToggle`), in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toggle {
    pub dir: String,
    pub name: String,
    pub attempt: Attempt,
}

/// How one toggle's server request came out.
#[derive(Clone, Debug)]
pub enum ToggleOutcome {
    Listed { list: Vec<McpServer>, started: Option<(String, Authorization)> },
    Threw(String),
}

/// A background server request, timer, or next-poll deferred reply scheduled by the host.
#[derive(Clone, Debug)]
pub enum McpJob {
    /// Run `request` against the live [`Server`] after `delay` (`Duration::ZERO` for immediate),
    /// then deliver the resulting [`McpReply`] via [`apply_reply`].
    Server { delay: Duration, request: McpRequest },
    /// Wait `delay` (without hitting the server) and deliver `reply` via [`apply_reply`].
    Timer { delay: Duration, reply: McpReply },
    /// Deliver `reply` on the next UI poll (e.g. a read-only write refusal).
    Defer { reply: McpReply },
}

/// Off-thread server work executed by [`execute_request`].
#[derive(Clone, Debug)]
pub enum McpRequest {
    ListServers { dir: String },
    Toggle { toggle: Toggle, enabled: bool },
    OauthStatus { dir: String, integration: String, attempt: String, poll: Attempt },
    OauthComplete { dir: String, integration: String, attempt: String, submit: Attempt, code: String },
    SyncAfterSignIn { dir: String, attempt: String, sync: Attempt },
    OauthCancel { dir: String, integration: String, attempt: String },
}

/// Result of an [`McpRequest`] or timer, applied on the UI thread by [`apply_reply`].
#[derive(Clone, Debug)]
pub enum McpReply {
    ServersListed { dir: String, result: Result<Vec<McpServer>, String> },
    ToggleSettled { toggle: Toggle, outcome: ToggleOutcome },
    ToggleTimedOut { toggle: Toggle },
    SignInWaitingTimedOut { attempt: String },
    SignInPolled { attempt: String, poll: Attempt, outcome: sign_in::PollOutcome },
    SignInSubmitted { attempt: String, submit: Attempt, result: Result<(), String> },
    SignInSynced { dir: String, attempt: String, sync: Attempt, result: Result<Vec<McpServer>, String> },
    SignInSyncTimedOut { attempt: String, sync: Attempt },
    None,
}

/// Object-safe host boundary for the MCP servers dialog, toggles, and browser OAuth sign-in.
pub trait McpHost: opencode_kernel::AppEnv + opencode_kernel::AppFeedback + opencode_kernel::AppOverlays {
    fn mcp(&self) -> &Mcp;
    fn mcp_mut(&mut self) -> &mut Mcp;
    /// Folder opened when toggling the MCPs dialog (the active session's or draft's directory).
    fn active_directory(&self) -> String;
    /// Whether this window may mutate server state (defaults to [`opencode_kernel::AppEnv::is_live`]).
    fn can_write(&self) -> bool {
        self.is_live()
    }
    /// Whether `ev` matches the host's configured MCP toggle shortcut (default `mod+;`).
    fn is_toggle_bind(&self, ev: &KeyEvent, tx: &Tx) -> bool {
        let _ = (ev, tx);
        false
    }
    /// Whether the MCP toggle shortcut may fire right now (e.g. Connect dialog is closed).
    fn can_toggle_shortcut(&self) -> bool {
        true
    }
    /// Prepares the shell before opening the MCPs dialog (e.g. closes pickers and blurs composer).
    fn before_dialog_open(&mut self) {}
    /// Restores composer focus after closing the MCP or sign-in dialog when appropriate.
    fn auto_focus_composer(&mut self, tx: &mut Tx) {
        let _ = tx;
    }
    /// Opens `url` in the system browser.
    fn open_external(&mut self, url: &str, tx: &mut Tx) {
        drop(tx.open_url(url));
    }
    /// Schedules an off-thread server request, timer, or next-poll deferred reply.
    fn spawn_job(&mut self, job: McpJob);
    /// Notified when an active MCP toggle for `dir` settles or times out (for new-session startup).
    fn on_toggle_settled(&mut self, dir: &str, attempt: Attempt, ok: bool, tx: &mut Tx) {
        let _ = (dir, attempt, ok, tx);
    }
}

/// The MCP servers and their machine (Desktop's `McpState`, `mcp-machine.ts`): the toggles in
/// flight and the dialog's error view. Toggles and error views take attempts from `issued`, so
/// a result whose attempt the state no longer holds is stale.
#[derive(Default)]
pub struct Mcp {
    pub open: Option<Dialog>,
    /// Servers by folder (`""` for the server's default location).
    servers: HashMap<String, Vec<McpServer>>,
    toggles: Vec<Toggle>,
    issued: Seq,
    /// The sign-in that also takes a pasted redirect URL.
    pub sign_in: Option<sign_in::SignIn>,
    /// Fallback editor targets so `on_edit` callbacks never panic if fired after close.
    scratch_search: TextEdit,
    scratch_sign_in: TextEdit,
}

impl Mcp {
    /// `dir`'s servers by name, once read.
    pub fn servers(&self, dir: &str) -> Option<&[McpServer]> {
        self.servers.get(dir).map(Vec::as_slice)
    }

    /// A row's toggle is running: it takes no new toggle.
    pub fn is_busy(&self, dir: &str, name: &str) -> bool {
        self.toggles.iter().any(|t| t.dir == dir && t.name == name)
    }

    /// The toggles in flight in `dir`, which a new session there waits for (`toggling`).
    pub fn toggling_in(&self, dir: &str) -> Vec<Attempt> {
        self.toggles.iter().filter(|t| t.dir == dir).map(|t| t.attempt).collect()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn is_sign_in_open(&self) -> bool {
        self.sign_in.is_some()
    }

    pub(crate) fn search_mut(&mut self) -> &mut TextEdit {
        match &mut self.open {
            Some(open) => &mut open.search,
            None => &mut self.scratch_search,
        }
    }

    pub(crate) fn sign_in_field_mut(&mut self) -> &mut TextEdit {
        match &mut self.sign_in {
            Some(x) => &mut x.field,
            None => &mut self.scratch_sign_in,
        }
    }
}

/// `McpEvent`, less the mounts, which only tell Desktop when to drop the state.
enum Event<'a> {
    /// A row's switch or sign-in button, or the error view's Disable or Try again.
    ToggleRequested { dir: &'a str, name: &'a str, enabled: bool, status: Option<&'a McpStatus> },
    /// The server started a browser sign-in; the toggle goes on to refresh the list.
    ToggleAuthorizing { toggle: &'a Toggle, integration: String, auth: Authorization },
    ToggleSucceeded { toggle: &'a Toggle },
    /// `server`: the refreshed status failed, else a request threw.
    ToggleFailed { toggle: &'a Toggle, error: String, server: bool },
    ToggleTimedOut { toggle: &'a Toggle },
    /// `focus`: opened from the list, so the view takes the keys from the search.
    DetailOpened { name: &'a str, loaded: bool, status: Option<&'a McpStatus>, focus: bool },
    /// Back or Escape in the error view.
    DetailDismissed,
    DetailCopied { error: String },
    /// The open dialog's server list changed.
    ServerSynced { loaded: bool, status: Option<&'a McpStatus> },
}

enum Effect {
    RunToggle(Toggle, bool),
    OpenExternal(String),
    OpenSignIn { dir: String, name: String, integration: String, auth: Authorization },
    /// The keys move to the error view, or back to the search.
    Focus { search: bool },
    ToastFailed(String),
    ToastTimedOut(String),
}

/// `transitionMcp`.
fn step(mcp: &mut Mcp, event: Event<'_>) -> Vec<Effect> {
    // A result whose toggle the state no longer holds is stale.
    if let Event::ToggleAuthorizing { toggle, .. } | Event::ToggleSucceeded { toggle } | Event::ToggleFailed { toggle, .. } | Event::ToggleTimedOut { toggle } = &event
        && !mcp.toggles.contains(toggle)
    {
        return vec![];
    }
    match event {
        Event::ToggleRequested { dir, name, enabled, status } => {
            // One toggle per server at a time, from its row and its error view alike.
            if mcp.toggles.iter().any(|t| t.dir == dir && t.name == name) || status == Some(&McpStatus::Pending) {
                return vec![];
            }
            let toggle = Toggle { dir: dir.to_owned(), name: name.to_owned(), attempt: mcp.issued.next() };
            mcp.toggles.push(toggle.clone());
            vec![Effect::RunToggle(toggle, enabled)]
        }
        Event::ToggleAuthorizing { toggle, integration, auth } => {
            // The browser takes over from the error view; the toggle still refreshes the list.
            let mut effects = close_shown(mcp, toggle);
            effects.push(Effect::OpenExternal(auth.url.clone()));
            if auth.paste {
                effects.push(Effect::OpenSignIn { dir: toggle.dir.clone(), name: toggle.name.clone(), integration, auth });
            }
            effects
        }
        Event::ToggleSucceeded { toggle } => {
            mcp.toggles.retain(|t| t != toggle);
            close_shown(mcp, toggle)
        }
        Event::ToggleTimedOut { toggle } => {
            mcp.toggles.retain(|t| t != toggle);
            vec![Effect::ToastTimedOut(toggle.name.clone())]
        }
        Event::ToggleFailed { toggle, error, server } => {
            mcp.toggles.retain(|t| t != toggle);
            // The open error view shows the server's new failure itself.
            if server
                && let Some(Detail::Open { name, error: shown, .. }) = detail_mut(mcp, &toggle.dir)
                && *name == toggle.name
            {
                *shown = error;
                return vec![];
            }
            vec![Effect::ToastFailed(if server { format!("{}: {error}", toggle.name) } else { error })]
        }
        Event::DetailOpened { name, loaded, status, focus } => {
            let Some(open) = mcp.open.as_mut() else { return vec![] };
            if !loaded {
                open.detail = Detail::Opening { attempt: mcp.issued.next(), name: name.to_owned() };
                return vec![];
            }
            // Only a failed server has an error to show.
            let Some(McpStatus::Failed(error)) = status else {
                open.detail = Detail::Closed;
                return vec![];
            };
            open.detail = Detail::Open { attempt: mcp.issued.next(), name: name.to_owned(), error: error.clone(), copied: None };
            if focus { vec![Effect::Focus { search: false }] } else { vec![] }
        }
        Event::DetailDismissed => {
            let Some(open) = mcp.open.as_mut().filter(|o| matches!(o.detail, Detail::Open { .. })) else { return vec![] };
            open.detail = Detail::Closed;
            vec![Effect::Focus { search: true }]
        }
        Event::DetailCopied { error } => {
            if let Some(Dialog { detail: Detail::Open { copied, .. }, .. }) = mcp.open.as_mut() {
                *copied = Some(error);
            }
            vec![]
        }
        Event::ServerSynced { loaded, status } => {
            let Some(open) = mcp.open.as_mut() else { return vec![] };
            if !loaded {
                return vec![];
            }
            match (&mut open.detail, status) {
                (Detail::Closed, _) => vec![],
                (Detail::Opening { attempt, name }, Some(McpStatus::Failed(error))) => {
                    open.detail = Detail::Open { attempt: *attempt, name: std::mem::take(name), error: error.clone(), copied: None };
                    vec![]
                }
                (Detail::Opening { .. }, _) => {
                    open.detail = Detail::Closed;
                    vec![]
                }
                // A reconnect, a disable or a removal, here or elsewhere, leaves nothing to show.
                (Detail::Open { .. }, None | Some(McpStatus::Connected | McpStatus::Disabled)) => {
                    open.detail = Detail::Closed;
                    vec![Effect::Focus { search: true }]
                }
                // A retry stays open while it connects or asks to sign in, and a new failure
                // replaces the error in place.
                (Detail::Open { error: shown, .. }, Some(McpStatus::Failed(error))) => {
                    shown.clone_from(error);
                    vec![]
                }
                (Detail::Open { .. }, _) => vec![],
            }
        }
    }
}

/// The error view of the folder `dir`'s dialog.
fn detail_mut<'a>(mcp: &'a mut Mcp, dir: &str) -> Option<&'a mut Detail> {
    mcp.open.as_mut().filter(|o| o.dir == dir).map(|o| &mut o.detail)
}

/// The error view of `toggle`'s server closes, and the keys go back to the search.
fn close_shown(mcp: &mut Mcp, toggle: &Toggle) -> Vec<Effect> {
    let Some(d) = detail_mut(mcp, &toggle.dir) else { return vec![] };
    if !matches!(&*d, Detail::Open { name, .. } if *name == toggle.name) {
        return vec![];
    }
    *d = Detail::Closed;
    vec![Effect::Focus { search: true }]
}

/// `dir`'s servers by name, once read.
pub fn servers<'a>(host: &'a dyn McpHost, dir: &str) -> Option<&'a [McpServer]> {
    host.mcp().servers(dir)
}

/// A row's toggle is running: it takes no new toggle.
pub fn busy(host: &dyn McpHost, dir: &str, name: &str) -> bool {
    host.mcp().is_busy(dir, name)
}

/// The toggles in flight in `dir`, which a new session there waits for (`toggling`).
pub fn toggling_in(host: &dyn McpHost, dir: &str) -> Vec<Attempt> {
    host.mcp().toggling_in(dir)
}

/// Reads `dir`'s servers again.
pub fn sync(host: &mut dyn McpHost, dir: &str) {
    host.spawn_job(McpJob::Server {
        delay: Duration::ZERO,
        request: McpRequest::ListServers { dir: dir.to_owned() },
    });
}

/// Opens the MCPs dialog on the active session's or draft's folder, or closes it.
pub fn toggle_dialog(host: &mut dyn McpHost, tx: &mut Tx) {
    tx.instant();
    if host.mcp().open.is_some() {
        close_dialog(host, tx);
        return;
    }
    let dir = host.active_directory();
    open_dialog(host, dir, None, tx);
}

/// Opens the dialog on `dir`; `server` opens it straight on that server's error.
pub fn open_dialog(host: &mut dyn McpHost, dir: String, server: Option<&str>, tx: &mut Tx) {
    tx.instant();
    host.before_dialog_open();
    let mut search = TextEdit::new("");
    search.focused = server.is_none();
    sync(host, &dir);
    host.mcp_mut().open = Some(Dialog { dir, search, active: None, detail: Detail::Closed });
    let return_focus = tx.focused().map(str::to_owned);
    host.on_overlay_opened(OVERLAY_ID, return_focus);
    if let Some(name) = server {
        open_detail_with(host, name, false);
    }
}

/// Closes the MCPs dialog, notifies `OverlayStack`, and restores composer focus when appropriate.
pub fn close_dialog(host: &mut dyn McpHost, tx: &mut Tx) {
    tx.instant();
    if host.mcp().open.is_none() {
        return;
    }
    // Its error view goes with it (`detail.closed`).
    host.mcp_mut().open = None;
    host.on_overlay_closed(OVERLAY_ID);
    host.auto_focus_composer(tx);
}

/// The error view of `name`, opened from the list, while it has failed.
pub fn open_detail(host: &mut dyn McpHost, name: &str) {
    open_detail_with(host, name, true);
}

fn open_detail_with(host: &mut dyn McpHost, name: &str, focus: bool) {
    let effects = {
        let mcp = host.mcp_mut();
        let Some(dir) = mcp.open.as_ref().map(|o| o.dir.clone()) else { return };
        let list = mcp.servers.get(&dir);
        let status = list.and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        let event = Event::DetailOpened { name, loaded: list.is_some(), status: status.as_ref(), focus };
        step(mcp, event)
    };
    apply_effects(host, effects, &mut Tx::default());
}

/// Back or Escape in the error view.
pub fn close_detail(host: &mut dyn McpHost) {
    let effects = step(host.mcp_mut(), Event::DetailDismissed);
    apply_effects(host, effects, &mut Tx::default());
}

/// The error view's Copy error.
pub fn mark_copied(host: &mut dyn McpHost, error: String) {
    let _ = step(host.mcp_mut(), Event::DetailCopied { error });
}

/// Connects (`enabled`) or disconnects a server, or starts its sign-in when it needs one
/// (`mcp.tsx` `toggle`). One toggle per server at a time; a connecting server takes none.
pub fn toggle_server(host: &mut dyn McpHost, dir: &str, name: &str, enabled: bool) {
    let status = servers(host, dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
    let effects = step(host.mcp_mut(), Event::ToggleRequested { dir, name, enabled, status: status.as_ref() });
    apply_effects(host, effects, &mut Tx::default());
}

/// Executes an [`McpRequest`] off the UI thread against the live [`Server`].
pub fn execute_request(srv: &Server, request: McpRequest) -> McpReply {
    match request {
        McpRequest::ListServers { dir } => {
            let result = srv.mcp_servers((!dir.is_empty()).then_some(dir.as_str()));
            McpReply::ServersListed { dir, result }
        }
        McpRequest::Toggle { toggle, enabled } => {
            let at = (!toggle.dir.is_empty()).then_some(toggle.dir.as_str());
            let outcome = match toggle_on_server(srv, at, &toggle.name, enabled) {
                Ok(outcome) => outcome,
                Err(e) => ToggleOutcome::Threw(e),
            };
            McpReply::ToggleSettled { toggle, outcome }
        }
        McpRequest::OauthStatus { dir, integration, attempt, poll } => {
            let at = (!dir.is_empty()).then_some(dir.as_str());
            let outcome = match srv.oauth_status(&integration, &attempt, at) {
                Ok(opencode_kit::integration::AttemptStatus::Pending(_)) => sign_in::PollOutcome::Pending,
                Ok(opencode_kit::integration::AttemptStatus::Complete) => sign_in::PollOutcome::Complete,
                Ok(opencode_kit::integration::AttemptStatus::Failed(m)) => sign_in::PollOutcome::Failed(m),
                Ok(opencode_kit::integration::AttemptStatus::Expired) => sign_in::PollOutcome::Expired,
                Err(e) => sign_in::PollOutcome::Unreachable(e),
            };
            McpReply::SignInPolled { attempt, poll, outcome }
        }
        McpRequest::OauthComplete { dir, integration, attempt, submit, code } => {
            let at = (!dir.is_empty()).then_some(dir.as_str());
            let result = srv.oauth_complete(&integration, &attempt, at, &code);
            McpReply::SignInSubmitted { attempt, submit, result }
        }
        McpRequest::SyncAfterSignIn { dir, attempt, sync } => {
            let at = (!dir.is_empty()).then_some(dir.as_str());
            let result = srv.mcp_servers(at);
            McpReply::SignInSynced { dir, attempt, sync, result }
        }
        McpRequest::OauthCancel { dir, integration, attempt } => {
            let at = (!dir.is_empty()).then_some(dir.as_str());
            drop(srv.oauth_cancel(&integration, &attempt, at));
            McpReply::None
        }
    }
}

fn toggle_on_server(srv: &Server, at: Option<&str>, name: &str, enabled: bool) -> Result<ToggleOutcome, String> {
    let found = srv.mcp_servers(at)?.into_iter().find(|s| s.name == name);
    let Some(found) = found else {
        return Ok(ToggleOutcome::Listed { list: srv.mcp_servers(at)?, started: None });
    };
    if !enabled || !matches!(found.status, McpStatus::NeedsAuth(_)) {
        srv.mcp_toggle(name, at, enabled)?;
    }
    let list = srv.mcp_servers(at)?;
    let current = list.iter().find(|s| s.name == name);
    // A server that needs to sign in starts its browser sign-in.
    let started = match current {
        Some(McpServer { status: McpStatus::NeedsAuth(_), integration: Some(id), .. }) if enabled => {
            let integration = srv.integration(id, at)?;
            let method = integration
                .methods
                .iter()
                .find(|m| m.kind == MethodKind::Oauth && m.form.is_empty())
                .ok_or_else(|| format!("MCP server {name} requires an interactive authentication form"))?;
            Some((id.clone(), srv.oauth_connect(id, at, method.id.as_deref().unwrap_or_default(), None)?))
        }
        _ => None,
    };
    Ok(ToggleOutcome::Listed { list, started })
}

/// Applies a completed [`McpReply`] back onto `host` on the UI thread.
pub fn apply_reply(host: &mut dyn McpHost, reply: McpReply, tx: &mut Tx) {
    match reply {
        McpReply::ServersListed { dir, result } => {
            if let Ok(list) = result {
                tx.instant();
                apply_listed(host, &dir, list, tx);
            }
        }
        McpReply::ToggleSettled { toggle, outcome } => apply_settled(host, &toggle, outcome, tx),
        McpReply::ToggleTimedOut { toggle } => {
            let live = host.mcp().toggles.contains(&toggle);
            let effects = step(host.mcp_mut(), Event::ToggleTimedOut { toggle: &toggle });
            apply_effects(host, effects, tx);
            if live {
                host.on_toggle_settled(&toggle.dir, toggle.attempt, false, tx);
            }
        }
        McpReply::SignInWaitingTimedOut { attempt } => sign_in::apply_waiting_timed_out(host, &attempt, tx),
        McpReply::SignInPolled { attempt, poll, outcome } => sign_in::apply_polled(host, &attempt, poll, outcome, tx),
        McpReply::SignInSubmitted { attempt, submit, result } => sign_in::apply_submitted(host, &attempt, submit, result, tx),
        McpReply::SignInSynced { dir, attempt, sync, result } => sign_in::apply_synced(host, &dir, &attempt, sync, result, tx),
        McpReply::SignInSyncTimedOut { attempt, sync } => sign_in::apply_sync_timed_out(host, &attempt, sync, tx),
        McpReply::None => {}
    }
}

/// mod+; toggles the dialog; Escape steps back from the error view, then closes.
pub fn key(host: &mut dyn McpHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if sign_in::key(host, ev, tx) {
        return true;
    }
    if host.can_toggle_shortcut() && host.is_toggle_bind(ev, tx) {
        toggle_dialog(host, tx);
        return true;
    }
    let detail_open = match &host.mcp().open {
        Some(open) => matches!(open.detail, Detail::Open { .. }),
        None => return false,
    };
    if ev.key == Key::Escape {
        if detail_open {
            tx.instant();
            close_detail(host);
        } else {
            close_dialog(host, tx);
        }
        return true;
    }
    view::list_key(host, ev, tx)
}

/// `dir`'s servers as the server listed them; the error view follows its server.
pub(crate) fn apply_listed(host: &mut dyn McpHost, dir: &str, list: Vec<McpServer>, tx: &mut Tx) {
    let effects = {
        let mcp = host.mcp_mut();
        mcp.servers.insert(dir.to_owned(), list);
        let Some(open) = mcp.open.as_ref().filter(|o| o.dir == dir) else { return };
        let name = match &open.detail {
            Detail::Closed => return,
            Detail::Opening { name, .. } | Detail::Open { name, .. } => name.clone(),
        };
        let status = mcp.servers.get(dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        step(mcp, Event::ServerSynced { loaded: true, status: status.as_ref() })
    };
    apply_effects(host, effects, tx);
}

fn apply_effects(host: &mut dyn McpHost, effects: Vec<Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            Effect::RunToggle(toggle, enabled) => run_toggle(host, toggle, enabled),
            Effect::OpenExternal(url) => {
                if !host.is_headless() {
                    host.open_external(&url, tx);
                }
            }
            Effect::OpenSignIn { dir, name, integration, auth } => sign_in::open(host, dir, name, integration, auth, tx),
            Effect::Focus { search } => {
                if let Some(open) = host.mcp_mut().open.as_mut() {
                    open.search.focused = search;
                }
            }
            Effect::ToastFailed(message) => host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), message),
            Effect::ToastTimedOut(name) => {
                host.toast_fail(opencode_i18n::t!("common.requestFailed").to_owned(), format!("{name} took too long to respond"));
            }
        }
    }
}

/// `run-toggle`: the request, with its 60 s timeout.
fn run_toggle(host: &mut dyn McpHost, toggle: Toggle, enabled: bool) {
    if !host.can_write() {
        let error = opencode_i18n::t!("rune.jobs.readOnly").to_owned();
        host.spawn_job(McpJob::Defer {
            reply: McpReply::ToggleSettled { toggle, outcome: ToggleOutcome::Threw(error) },
        });
        return;
    }
    host.spawn_job(McpJob::Timer {
        delay: Duration::from_secs_f64(TOGGLE_TIMEOUT_S),
        reply: McpReply::ToggleTimedOut { toggle: toggle.clone() },
    });
    host.spawn_job(McpJob::Server {
        delay: Duration::ZERO,
        request: McpRequest::Toggle { toggle, enabled },
    });
}

/// What toggle `toggle` came to: a sign-in it started, then its server's refreshed status.
fn apply_settled(host: &mut dyn McpHost, toggle: &Toggle, outcome: ToggleOutcome, tx: &mut Tx) {
    tx.instant();
    let (event, list) = match outcome {
        ToggleOutcome::Threw(error) => (Event::ToggleFailed { toggle, error, server: false }, None),
        ToggleOutcome::Listed { list, started } => {
            if let Some((integration, auth)) = started {
                let effects = step(host.mcp_mut(), Event::ToggleAuthorizing { toggle, integration, auth });
                apply_effects(host, effects, tx);
            }
            let failed = list.iter().find(|x| x.name == toggle.name).and_then(|x| match &x.status {
                McpStatus::Failed(e) => Some(e.clone()),
                _ => None,
            });
            let event = match failed {
                Some(error) => Event::ToggleFailed { toggle, error, server: true },
                None => Event::ToggleSucceeded { toggle },
            };
            (event, Some(list))
        }
    };
    let live = host.mcp().toggles.contains(toggle);
    let ok = matches!(event, Event::ToggleSucceeded { .. });
    let effects = step(host.mcp_mut(), event);
    apply_effects(host, effects, tx);
    if live {
        host.on_toggle_settled(&toggle.dir, toggle.attempt, ok, tx);
    }
    match list {
        Some(list) => apply_listed(host, &toggle.dir, list, tx),
        None => {
            if let Some(list) = host.mcp().servers.get(&toggle.dir).cloned() {
                apply_listed(host, &toggle.dir, list, tx);
            }
        }
    }
}

pub(crate) fn clear_active(host: &mut dyn McpHost, tx: &mut Tx) {
    tx.instant();
    if let Some(o) = host.mcp_mut().open.as_mut() {
        o.active = None;
    }
}

pub(crate) fn set_active_on_hover(host: &mut dyn McpHost, name: &str, inside: bool, tx: &mut Tx) {
    if inside && let Some(o) = host.mcp_mut().open.as_mut() {
        tx.instant();
        o.active = Some(name.to_owned());
    }
}

pub(crate) fn open_error_from_row(host: &mut dyn McpHost, dir: &str, name: &str, tx: &mut Tx) {
    tx.instant();
    if host.mcp().open.as_ref().is_some_and(|o| o.dir == dir) {
        open_detail(host, name);
    } else {
        open_dialog(host, dir.to_owned(), Some(name), tx);
    }
}

pub(crate) fn toggle_from_ui(host: &mut dyn McpHost, dir: &str, name: &str, enabled: bool, tx: &mut Tx) {
    tx.instant();
    toggle_server(host, dir, name, enabled);
}

pub(crate) fn close_detail_from_ui(host: &mut dyn McpHost, tx: &mut Tx) {
    tx.instant();
    close_detail(host);
}

pub(crate) fn copy_error(host: &mut dyn McpHost, details: &str, error: String, tx: &mut Tx) {
    tx.copy(details.to_owned());
    mark_copied(host, error);
}

pub(crate) fn detail_busy(host: &dyn McpHost, dir: &str, name: &str) -> bool {
    busy(host, dir, name) || servers(host, dir).and_then(|l| l.iter().find(|m| m.name == name)).is_some_and(|m| m.status == McpStatus::Pending)
}
