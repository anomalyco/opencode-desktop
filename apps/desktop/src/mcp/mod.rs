//! A location's MCP servers (Desktop's `providers/connect/mcp.tsx` and `mcp-machine.ts`): their
//! connects, disconnects and sign-ins in flight, one per server at a time, and the MCPs dialog
//! (`/mcp`, mod+;) with its inline error view. Settings › Extensions toggles through the same
//! state, so a server connecting from the dialog stays busy there.

pub mod sign_in;
pub mod view;

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::integration::{Authorization, McpServer, McpStatus, MethodKind};
use rune::machine::{Attempt, Seq};
use rune::{KeyEvent, TextEdit, Tx};

use crate::App;
use crate::jobs::{Apply, read_only};
use crate::keymap::Bind;

/// How long one connect, disconnect or sign-in may run before it gives up.
const TOGGLE_TIMEOUT_S: f64 = 60.0;

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
#[derive(Clone, Debug, PartialEq)]
struct Toggle {
    dir: String,
    name: String,
    attempt: Attempt,
}

/// The MCP servers and their machine (Desktop's `McpState`, `mcp-machine.ts`): the toggles in
/// flight and the dialog's error view. Toggles and error views take attempts from `issued`, so
/// a result whose attempt the state no longer holds is stale.
#[derive(Default)]
pub struct Mcp {
    pub open: Option<Dialog>,
    /// Servers by folder ("" for the server's default location).
    servers: HashMap<String, Vec<McpServer>>,
    toggles: Vec<Toggle>,
    issued: Seq,
    /// The sign-in that also takes a pasted redirect URL.
    pub sign_in: Option<sign_in::SignIn>,
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
            if server && let Some(Detail::Open { name, error: shown, .. }) = detail_mut(mcp, &toggle.dir)
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

/// How one toggle's request came out.
enum Outcome {
    Listed { list: Vec<McpServer>, started: Option<(String, Authorization)> },
    Threw(String),
}

impl App {
    /// `dir`'s servers by name, once read.
    pub(crate) fn mcp_servers(&self, dir: &str) -> Option<&[McpServer]> {
        self.mcp.servers.get(dir).map(Vec::as_slice)
    }

    /// Reads `dir`'s servers again.
    pub(crate) fn sync_mcp(&self, dir: &str) {
        let dir = dir.to_owned();
        self.job(move |srv| {
            let list = srv.mcp_servers(Some(&dir).filter(|d| !d.is_empty()).map(String::as_str));
            Box::new(move |s: &mut App, tx: &mut Tx| {
                if let Ok(list) = list {
                    tx.instant();
                    s.mcp_listed(&dir, list, tx);
                }
            }) as Apply
        });
    }

    /// A row's toggle is running, or the server is connecting: it takes no new toggle.
    pub(crate) fn mcp_busy(&self, dir: &str, name: &str) -> bool {
        self.mcp.toggles.iter().any(|t| t.dir == dir && t.name == name)
    }

    /// The toggles in flight in `dir`, which a new session there waits for (`toggling`).
    pub(crate) fn mcp_toggling_in(&self, dir: &str) -> Vec<Attempt> {
        self.mcp.toggles.iter().filter(|t| t.dir == dir).map(|t| t.attempt).collect()
    }

    /// Opens the MCPs dialog on the open session's or draft's folder, or closes it.
    pub fn toggle_mcp_dialog(&mut self, tx: &mut Tx) {
        tx.instant();
        if self.mcp.open.is_some() {
            return self.close_mcp_dialog(tx);
        }
        let dir = match &*self.route {
            crate::Route::Session { id, .. } => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
            _ => self.draft_dir().to_owned(),
        };
        self.open_mcp_dialog(dir, None, tx);
    }

    /// Opens the dialog on `dir`; `server` opens it straight on that server's error.
    pub fn open_mcp_dialog(&mut self, dir: String, server: Option<&str>, tx: &mut Tx) {
        use crate::pickers::PickerHost as _;
        tx.instant();
        self.close_picker(false);
        self.composer.focused = false;
        let mut search = TextEdit::new("");
        search.focused = server.is_none();
        self.sync_mcp(&dir);
        self.mcp.open = Some(Dialog { dir, search, active: None, detail: Detail::Closed });
        if let Some(name) = server {
            self.mcp_detail(name, false);
        }
    }

    pub fn close_mcp_dialog(&mut self, tx: &mut Tx) {
        tx.instant();
        // Its error view goes with it (`detail.closed`).
        self.mcp.open = None;
        self.composer.focused = self.has_composer() && !self.settings.open;
    }

    /// The error view of `name`, opened from the list, while it has failed.
    pub(crate) fn open_mcp_detail(&mut self, name: &str) {
        self.mcp_detail(name, true);
    }

    fn mcp_detail(&mut self, name: &str, focus: bool) {
        let Some(open) = &self.mcp.open else { return };
        let list = self.mcp.servers.get(&open.dir);
        let status = list.and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        let event = Event::DetailOpened { name, loaded: list.is_some(), status: status.as_ref(), focus };
        let effects = step(&mut self.mcp, event);
        self.mcp_effects(effects, &mut Tx::default());
    }

    /// Back or Escape in the error view.
    pub(crate) fn close_mcp_detail(&mut self) {
        let effects = step(&mut self.mcp, Event::DetailDismissed);
        self.mcp_effects(effects, &mut Tx::default());
    }

    /// The error view's Copy error.
    pub(crate) fn mcp_copied(&mut self, error: String) {
        step(&mut self.mcp, Event::DetailCopied { error });
    }

    /// `dir`'s servers as the server listed them; the error view follows its server.
    fn mcp_listed(&mut self, dir: &str, list: Vec<McpServer>, tx: &mut Tx) {
        self.mcp.servers.insert(dir.to_owned(), list);
        let Some(open) = self.mcp.open.as_ref().filter(|o| o.dir == dir) else { return };
        let name = match &open.detail {
            Detail::Closed => return,
            Detail::Opening { name, .. } | Detail::Open { name, .. } => name.clone(),
        };
        let status = self.mcp.servers.get(dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        let effects = step(&mut self.mcp, Event::ServerSynced { loaded: true, status: status.as_ref() });
        self.mcp_effects(effects, tx);
    }

    /// Connects (`enabled`) or disconnects a server, or starts its sign-in when it needs one
    /// (`mcp.tsx` `toggle`). One toggle per server at a time; a connecting server takes none.
    pub(crate) fn toggle_mcp(&mut self, dir: &str, name: &str, enabled: bool) {
        let status = self.mcp_servers(dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        let effects = step(&mut self.mcp, Event::ToggleRequested { dir, name, enabled, status: status.as_ref() });
        self.mcp_effects(effects, &mut Tx::default());
    }

    fn mcp_effects(&mut self, effects: Vec<Effect>, tx: &mut Tx) {
        for effect in effects {
            match effect {
                Effect::RunToggle(toggle, enabled) => self.run_mcp_toggle(toggle, enabled),
                Effect::OpenExternal(url) => {
                    if !crate::headless() {
                        let _ = tx.open_url(&url);
                    }
                }
                Effect::OpenSignIn { dir, name, integration, auth } => self.open_mcp_sign_in(dir, name, integration, auth),
                Effect::Focus { search } => {
                    if let Some(open) = self.mcp.open.as_mut() {
                        open.search.focused = search;
                    }
                }
                Effect::ToastFailed(message) => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), message),
                Effect::ToastTimedOut(name) => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), format!("{name} took too long to respond")),
            }
        }
    }

    /// `run-toggle`: the request, with its 60 s timeout.
    fn run_mcp_toggle(&mut self, toggle: Toggle, enabled: bool) {
        if !self.can_write() {
            let error = read_only().to_owned();
            return self.later(Box::new(move |s: &mut App, tx: &mut Tx| s.mcp_settled(&toggle, Outcome::Threw(error), tx)));
        }
        let late = toggle.clone();
        self.job_after(Duration::from_secs_f64(TOGGLE_TIMEOUT_S), move |_| {
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let live = s.mcp.toggles.contains(&late);
                let effects = step(&mut s.mcp, Event::ToggleTimedOut { toggle: &late });
                s.mcp_effects(effects, tx);
                if live {
                    s.start_mcp_settled(&late.dir, late.attempt, false, tx);
                }
            }) as Apply
        });
        self.job(move |srv| {
            let (dir, name) = (toggle.dir.as_str(), toggle.name.as_str());
            let at = Some(dir).filter(|d| !d.is_empty());
            let result = (|| -> Result<(Vec<McpServer>, Option<(String, Authorization)>), String> {
                let found = srv.mcp_servers(at)?.into_iter().find(|s| s.name == name);
                let Some(found) = found else { return Ok((srv.mcp_servers(at)?, None)) };
                if !enabled || !matches!(found.status, McpStatus::NeedsAuth(_)) {
                    srv.mcp_toggle(name, at, enabled)?;
                }
                let list = srv.mcp_servers(at)?;
                let current = list.iter().find(|s| s.name == name);
                // A server that needs to sign in starts its browser sign-in.
                let started = match current {
                    Some(McpServer { status: McpStatus::NeedsAuth(_), integration: Some(id), .. }) if enabled => {
                        let integration = srv.integration(id, at)?;
                        let method = integration.methods.iter().find(|m| m.kind == MethodKind::Oauth && m.form.is_empty())
                            .ok_or_else(|| format!("MCP server {name} requires an interactive authentication form"))?;
                        Some((id.clone(), srv.oauth_connect(id, at, method.id.as_deref().unwrap_or_default(), None)?))
                    }
                    _ => None,
                };
                Ok((list, started))
            })();
            let outcome = match result {
                Ok((list, started)) => Outcome::Listed { list, started },
                Err(e) => Outcome::Threw(e),
            };
            Box::new(move |s: &mut App, tx: &mut Tx| s.mcp_settled(&toggle, outcome, tx)) as Apply
        });
    }

    /// What toggle `toggle` came to: a sign-in it started, then its server's refreshed status.
    fn mcp_settled(&mut self, toggle: &Toggle, outcome: Outcome, tx: &mut Tx) {
        tx.instant();
        let (event, list) = match outcome {
            Outcome::Threw(error) => (Event::ToggleFailed { toggle, error, server: false }, None),
            Outcome::Listed { list, started } => {
                if let Some((integration, auth)) = started {
                    let effects = step(&mut self.mcp, Event::ToggleAuthorizing { toggle, integration, auth });
                    self.mcp_effects(effects, tx);
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
        let live = self.mcp.toggles.contains(toggle);
        let ok = matches!(event, Event::ToggleSucceeded { .. });
        let effects = step(&mut self.mcp, event);
        self.mcp_effects(effects, tx);
        if live {
            self.start_mcp_settled(&toggle.dir, toggle.attempt, ok, tx);
        }
        match list {
            Some(list) => self.mcp_listed(&toggle.dir, list, tx),
            None => {
                if let Some(list) = self.mcp.servers.get(&toggle.dir).cloned() {
                    self.mcp_listed(&toggle.dir, list, tx);
                }
            }
        }
    }
}
/// mod+; toggles the dialog; Escape steps back from the error view, then closes.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if sign_in::key(s, ev, tx) {
        return true;
    }
    if Bind::McpToggle.matches_tx(ev, tx) && s.connect.open.is_none() {
        s.toggle_mcp_dialog(tx);
        return true;
    }
    let Some(open) = &s.mcp.open else { return false };
    if ev.key == rune::Key::Escape {
        if matches!(open.detail, Detail::Open { .. }) {
            tx.instant();
            s.close_mcp_detail();
        } else {
            s.close_mcp_dialog(tx);
        }
        return true;
    }
    view::list_key(s, ev, tx)
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("mcp.toggle", opencode_i18n::key!("command.mcp.toggle"), |s, tx| s.toggle_mcp_dialog(tx))
        .describe(opencode_i18n::key!("command.mcp.toggle.description"))
        .slash("mcp")
        .keys(Bind::McpToggle),
]);
