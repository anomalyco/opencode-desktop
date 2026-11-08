//! A location's MCP servers (Desktop's `providers/connect/mcp.tsx` and `mcp-machine.ts`): their
//! connects, disconnects and sign-ins in flight, one per server at a time, and the MCPs dialog
//! (`/mcp`, mod+;) with its inline error view. Settings › Extensions toggles through the same
//! state, so a server connecting from the dialog stays busy there.

pub mod view;

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::integration::{McpServer, McpStatus, MethodKind};
use rune::{KeyEvent, TextEdit, Tx};

use crate::App;
use crate::jobs::{Apply, read_only};
use crate::keymap::Bind;

/// How long one connect, disconnect or sign-in may run before it gives up.
const TOGGLE_TIMEOUT_S: f64 = 60.0;

/// The dialog's error view of one failed server.
#[derive(Clone, Debug, PartialEq)]
pub enum Detail {
    Closed,
    /// Opened on a server before the list loaded.
    Opening { attempt: u32, name: String },
    /// `copied` is the error text last copied.
    Open { attempt: u32, name: String, error: String, copied: Option<String> },
}

pub struct Dialog {
    /// The folder whose servers it lists.
    pub dir: String,
    pub search: TextEdit,
    pub active: Option<String>,
    pub detail: Detail,
}

#[derive(Default)]
pub struct Mcp {
    pub open: Option<Dialog>,
    /// Servers by folder ("" for the server's default location).
    servers: HashMap<String, Vec<McpServer>>,
    /// Toggles in flight: (folder, server, attempt).
    toggles: Vec<(String, String, u32)>,
    issued: u32,
}

/// What a toggle came to.
enum Settled {
    Succeeded,
    /// `server` when the refreshed status failed, else a request threw.
    Failed { error: String, server: bool },
    TimedOut,
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
                    s.mcp.servers.insert(dir.clone(), list);
                    s.mcp_synced(&dir, tx);
                }
            }) as Apply
        });
    }

    /// A row's toggle is running, or the server is connecting: it takes no new toggle.
    pub(crate) fn mcp_busy(&self, dir: &str, name: &str) -> bool {
        self.mcp.toggles.iter().any(|(d, n, _)| d == dir && n == name)
    }

    /// Opens the MCPs dialog on the open session's or draft's folder, or closes it.
    pub fn toggle_mcp_dialog(&mut self, tx: &mut Tx) {
        tx.instant();
        if self.mcp.open.is_some() {
            return self.close_mcp_dialog(tx);
        }
        let dir = match &self.route {
            crate::Route::Session(id) => self.session(id).map(|s| s.directory.clone()).unwrap_or_default(),
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
            self.open_mcp_detail(name);
        }
    }

    pub fn close_mcp_dialog(&mut self, tx: &mut Tx) {
        tx.instant();
        self.mcp.open = None;
        self.composer.focused = self.has_composer() && !self.settings.open;
    }

    /// The error view of `name`, while it has failed.
    pub(crate) fn open_mcp_detail(&mut self, name: &str) {
        let Some(open) = &self.mcp.open else { return };
        let status = self.mcp_servers(&open.dir).map(|l| l.iter().find(|s| s.name == name).map(|s| s.status.clone()));
        self.mcp.issued += 1;
        let attempt = self.mcp.issued;
        let detail = match status {
            None => Detail::Opening { attempt, name: name.to_owned() },
            Some(Some(McpStatus::Failed(error))) => Detail::Open { attempt, name: name.to_owned(), error, copied: None },
            Some(_) => Detail::Closed,
        };
        let open = self.mcp.open.as_mut().expect("open");
        open.detail = detail;
        open.search.focused = matches!(open.detail, Detail::Closed);
    }

    pub(crate) fn close_mcp_detail(&mut self) {
        if let Some(open) = self.mcp.open.as_mut() {
            open.detail = Detail::Closed;
            open.search.focused = true;
        }
    }

    /// The error view follows its server: a reconnect, a disable or a removal returns to the
    /// list, and a new failure replaces the error in place.
    fn mcp_synced(&mut self, dir: &str, _tx: &mut Tx) {
        let Some(open) = self.mcp.open.as_ref().filter(|o| o.dir == dir) else { return };
        let (attempt, name) = match &open.detail {
            Detail::Closed => return,
            Detail::Opening { attempt, name } | Detail::Open { attempt, name, .. } => (*attempt, name.clone()),
        };
        let status = self.mcp_servers(dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        let busy = self.mcp_busy(dir, &name);
        let open = self.mcp.open.as_mut().expect("open");
        match (&open.detail, status) {
            (Detail::Opening { .. }, Some(McpStatus::Failed(error))) => open.detail = Detail::Open { attempt, name, error, copied: None },
            (Detail::Opening { .. }, _) => open.detail = Detail::Closed,
            (_, None | Some(McpStatus::Connected | McpStatus::Disabled)) if !busy => {
                open.detail = Detail::Closed;
                open.search.focused = true;
            }
            (Detail::Open { error: old, copied, .. }, Some(McpStatus::Failed(error))) if *old != error => {
                open.detail = Detail::Open { attempt, name, error, copied: copied.clone() };
            }
            _ => {}
        }
    }

    /// Connects (`enabled`) or disconnects a server, or starts its sign-in when it needs one
    /// (`mcp.tsx` `toggle`). One toggle per server at a time; a connecting server takes none.
    pub(crate) fn toggle_mcp(&mut self, dir: &str, name: &str, enabled: bool) {
        let status = self.mcp_servers(dir).and_then(|l| l.iter().find(|s| s.name == name)).map(|s| s.status.clone());
        if self.mcp_busy(dir, name) || status == Some(McpStatus::Pending) {
            return;
        }
        self.mcp.issued += 1;
        let attempt = self.mcp.issued;
        self.mcp.toggles.push((dir.to_owned(), name.to_owned(), attempt));
        let (dir, name) = (dir.to_owned(), name.to_owned());
        if !self.can_write() {
            let error = read_only().to_owned();
            return self.later(Box::new(move |s: &mut App, tx: &mut Tx| s.mcp_settled(&dir, &name, attempt, Settled::Failed { error, server: false }, tx)));
        }
        let (d, n) = (dir.clone(), name.clone());
        self.job_after(Duration::from_secs_f64(TOGGLE_TIMEOUT_S), move |_| {
            Box::new(move |s: &mut App, tx: &mut Tx| s.mcp_settled(&d, &n, attempt, Settled::TimedOut, tx)) as Apply
        });
        self.job(move |srv| {
            let at = Some(dir.as_str()).filter(|d| !d.is_empty());
            let result = (|| -> Result<(Vec<McpServer>, Option<String>), String> {
                let found = srv.mcp_servers(at)?.into_iter().find(|s| s.name == name);
                let Some(found) = found else { return Ok((srv.mcp_servers(at)?, None)) };
                if !enabled || !matches!(found.status, McpStatus::NeedsAuth(_)) {
                    srv.mcp_toggle(&name, at, enabled)?;
                }
                let list = srv.mcp_servers(at)?;
                let current = list.iter().find(|s| s.name == name);
                // A server that needs to sign in starts its browser sign-in.
                let url = match current {
                    Some(McpServer { status: McpStatus::NeedsAuth(_), integration: Some(id), .. }) if enabled => {
                        let integration = srv.integration(id, at)?;
                        let method = integration.methods.iter().find(|m| m.kind == MethodKind::Oauth && m.form.is_empty())
                            .ok_or_else(|| format!("MCP server {name} requires an interactive authentication form"))?;
                        Some(srv.oauth_connect(id, at, method.id.as_deref().unwrap_or_default(), None)?.url)
                    }
                    _ => None,
                };
                Ok((list, url))
            })();
            Box::new(move |s: &mut App, tx: &mut Tx| {
                let settled = match result {
                    Ok((list, url)) => {
                        let failed = list.iter().find(|x| x.name == name).and_then(|x| match &x.status {
                            McpStatus::Failed(e) => Some(e.clone()),
                            _ => None,
                        });
                        s.mcp.servers.insert(dir.clone(), list);
                        if let Some(url) = url && !crate::headless() {
                            let _ = tx.open_url(&url);
                        }
                        match failed {
                            Some(error) => Settled::Failed { error, server: true },
                            None => Settled::Succeeded,
                        }
                    }
                    Err(error) => Settled::Failed { error, server: false },
                };
                s.mcp_settled(&dir, &name, attempt, settled, tx);
            }) as Apply
        });
    }

    fn mcp_settled(&mut self, dir: &str, name: &str, attempt: u32, settled: Settled, tx: &mut Tx) {
        let before = self.mcp.toggles.len();
        self.mcp.toggles.retain(|(d, n, a)| !(d == dir && n == name && *a == attempt));
        if self.mcp.toggles.len() == before {
            return;
        }
        tx.instant();
        let shown = self.mcp.open.as_ref().is_some_and(|o| o.dir == dir && matches!(&o.detail, Detail::Open { name: n, .. } if n == name));
        match settled {
            Settled::Succeeded => {
                if shown {
                    self.close_mcp_detail();
                }
            }
            Settled::TimedOut => crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), format!("{name} took too long to respond")),
            // The open error view shows the server's new failure itself.
            Settled::Failed { error, server: true } if shown => {
                if let Some(Detail::Open { error: e, .. }) = self.mcp.open.as_mut().map(|o| &mut o.detail) {
                    *e = error;
                }
            }
            Settled::Failed { error, server } => {
                let message = if server { format!("{name}: {error}") } else { error };
                crate::status_toast::fail(self, opencode_i18n::t!("common.requestFailed"), message);
            }
        }
        self.mcp_synced(dir, tx);
    }
}

/// mod+; toggles the dialog; Escape steps back from the error view, then closes.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
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
