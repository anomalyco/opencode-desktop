//! The terminal's server calls that used to hold the window: listing a session's terminals and
//! starting one. Each is a [`TerminalJob`] run off the UI thread; its answer lands as a
//! [`TerminalReply`] on a later frame. A server that takes seconds to answer (a remote one, a
//! slow link) no longer stops the window from answering for that long.
//!
//! As Desktop (`packages/gui-plugins/src/terminal/machine.ts`): no tab shows while a create is
//! out, the tab is added when it succeeds, and a second ask joins the one in flight.

use std::path::{Path, PathBuf};

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::{PersistentPtyInfo, TerminalCreate};
use opencode_kit::pty::PtyStatus;
use rune::Tx;

use crate::{PaneTarget, TerminalJob, TerminalReply, TerminalSurfaceHost, integration, ops, panel_key, persist};

/// The call a session's terminals wait for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Call {
    #[default]
    Idle,
    /// The server is listing them: which there are isn't known yet.
    Listing,
    /// The server is starting one.
    Creating,
}

/// What shows a terminal once the server has started it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum After {
    /// Its tab opens in `pane` of `session`'s side panel and takes the keyboard (New terminal).
    Pane { session: String, pane: Option<PaneTarget> },
    /// Its tab opens in `session`'s side panel, the keyboard staying where it is (a plugin's open).
    Side { session: String },
    Nothing,
}

/// What [`launch`] needs from the host, read on the UI thread.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    /// `WSLENV` for a server on this Windows computer.
    wslenv: Option<String>,
    /// The shells the user turned marks on for.
    marks: Vec<String>,
    /// The server's shell, when it was asked before.
    shell: Option<Option<String>>,
    env_shell: Option<String>,
    integration_dir: Option<PathBuf>,
}

pub fn launch_inputs(host: &dyn TerminalSurfaceHost, srv: &Server) -> Launch {
    Launch {
        wslenv: (srv.is_local() && cfg!(windows)).then(|| integration::wslenv(&host.env_var("WSLENV").unwrap_or_default())),
        marks: host.terminal_marks().to_vec(),
        shell: host.terminal().server_shells.borrow().get(&srv.base).cloned(),
        env_shell: host.env_var("SHELL"),
        integration_dir: host.shell_integration_dir(),
    }
}

/// What a new terminal on `srv` runs: `command` with `args`, or the server's own shell, with
/// that shell's marks when the user turned them on (`integration`; bash's only from a
/// server on this computer, which can read the script's file); and on this Windows computer
/// `WSLENV` carrying `COLORTERM` into WSL. Also the server's shell when this had to ask for it
/// (`GET /api/config/shell`), for the host to keep: so not on the UI thread.
pub fn launch(srv: &Server, l: &Launch, create: TerminalCreate, command: Option<String>, args: Vec<String>) -> (TerminalCreate, Option<Option<String>>) {
    let create = match &l.wslenv {
        Some(v) => create.env("WSLENV", v.clone()),
        None => create,
    };
    if l.marks.is_empty() {
        return (create.command(command, args), None);
    }
    let mut asked = None;
    let program = match &command {
        Some(c) if !c.is_empty() => args.is_empty().then(|| c.clone()),
        _ => match &l.shell {
            Some(known) => known.clone(),
            None => {
                let shell = server_shell(srv, l.env_shell.as_deref());
                asked = Some(shell.clone());
                shell
            }
        },
    };
    let marked = program.as_deref().and_then(|p| {
        let shell = integration::Shell::of(p).filter(|sh| l.marks.iter().any(|m| m == sh.id()))?;
        if shell.reads_a_file() && !srv.is_local() {
            return None;
        }
        integration::launch(shell, p, l.integration_dir.as_deref())
    });
    match marked {
        Some((command, args)) => (create.command(Some(command), args), asked),
        None => (create.command(command, args), asked),
    }
}

/// The shell `srv` starts a terminal in, as its `ShellSelect.environment` picks it on Windows:
/// `SHELL` when it names one (known only for a server on this computer, which shares this
/// process's environment), else the first of its shells (`GET /api/config/shell`: PowerShell 7,
/// Windows PowerShell, Git Bash, cmd).
fn server_shell(srv: &Server, env_shell: Option<&str>) -> Option<String> {
    let listed = srv.get("/api/config/shell").ok().map(|v| {
        let items = v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default();
        items
            .iter()
            .filter_map(|i| Some((i["name"].as_str()?.to_owned(), i["path"].as_str()?.to_owned())))
            .collect::<Vec<_>>()
    });
    let listed: Vec<(String, String)> = listed
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, path)| path.as_bytes().get(1) == Some(&b':'))
        .collect();
    let from_env = env_shell
        .filter(|s| srv.is_local() && cfg!(windows) && !s.is_empty())
        .and_then(|s| {
            let path = Path::new(s);
            if path.is_absolute() {
                return path.is_file().then(|| s.to_owned());
            }
            let name = path.file_stem()?.to_string_lossy().to_ascii_lowercase();
            listed.iter().find(|(n, _)| *n == name).map(|(_, p)| p.clone())
        });
    from_env.or_else(|| listed.first().map(|(_, p)| p.clone()))
}

/// Whether workspace `ws_key`'s terminals are being listed: what it has isn't known yet, so
/// nothing is started or toggled until the answer.
pub fn listing(host: &dyn TerminalSurfaceHost, ws_key: &str) -> bool {
    host.terminal().workspaces.get(ws_key).is_some_and(|ws| ws.call == Call::Listing)
}

/// Asks the server for session `session`'s terminals; `order` and `active` are the saved ones.
pub fn list(host: &mut dyn TerminalSurfaceHost, ws_key: &str, session: String, stored: persist::StoredWorkspace) {
    let order = stored.all.iter().map(|p| p.id.clone()).collect();
    host.spawn_job(TerminalJob::List { ws: ws_key.to_owned(), session, order, active: stored.active });
}

/// Asks the server to start a terminal for `session`; a second ask while one is out joins it.
pub fn create(host: &mut dyn TerminalSurfaceHost, srv: &Server, ws_key: &str, session: String, create: TerminalCreate, after: After) {
    let launch = launch_inputs(host, srv);
    let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key) else { return };
    if ws.call != Call::Idle {
        return;
    }
    ws.call = Call::Creating;
    host.spawn_job(TerminalJob::Create { ws: ws_key.to_owned(), session, create, launch, after });
}

/// The server's part of [`list`], off the UI thread.
pub fn list_now(srv: &Server, ws: String, session: &str, order: Vec<String>, active: Option<String>) -> TerminalReply {
    let result = srv.terminal_list(session).map_err(|e| e.to_string());
    TerminalReply::Listed { ws, order, active, result }
}

/// The server's part of [`create`], off the UI thread.
pub fn create_now(srv: &Server, ws: String, session: &str, create: TerminalCreate, l: &Launch, after: After) -> TerminalReply {
    let (create, shell) = launch(srv, l, create, None, Vec::new());
    let plain = create.program().is_none();
    let result = srv.terminal_create(session, &create).map_err(|e| e.to_string());
    TerminalReply::Created { ws, plain, shell, after, result }
}

/// The session's terminals as the server listed them, in the saved order; one that arrived
/// meanwhile (another client's, a plugin's) stays.
pub fn listed(host: &mut dyn TerminalSurfaceHost, ws_key: &str, order: &[String], active: Option<String>, result: Result<Vec<PersistentPtyInfo>, String>, tx: &mut Tx) {
    let srv = ops::live_terminals(host).map(|(srv, _)| srv);
    let have: Vec<String> = host.terminal().workspaces.get(ws_key).map(|ws| ws.tabs.iter().map(|t| t.id.clone()).collect()).unwrap_or_default();
    let (tabs, failure) = match (result, srv) {
        // The window left the server meanwhile: nothing of it is attached.
        (_, None) => (Vec::new(), None),
        (Err(e), _) => (Vec::new(), Some(e)),
        (Ok(mut list), Some(srv)) => {
            list.retain(|info| info.status == PtyStatus::Running && !have.contains(&info.id));
            let rank = |id: &str| order.iter().position(|p| p == id).unwrap_or(usize::MAX);
            list.sort_by_key(|info| rank(&info.id));
            let h: &dyn TerminalSurfaceHost = &*host;
            (list.into_iter().filter_map(|info| ops::persistent_tab(h, srv.clone(), info).ok()).collect(), None)
        }
    };
    let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key) else { return };
    ws.call = Call::Idle;
    drop(ws.tabs.splice(0..0, tabs));
    ws.failure = failure.filter(|_| ws.tabs.is_empty());
    ws.active = active.filter(|id| ws.tabs.iter().any(|t| &t.id == id)).or_else(|| ws.active.take()).or_else(|| ws.tabs.first().map(|t| t.id.clone()));
    tx.instant();
}

/// The terminal the server started joins its session's terminals, active, and shows as `after`
/// says; a failed start shows in the panel when the session has no other terminal.
pub fn created(host: &mut dyn TerminalSurfaceHost, ws_key: &str, plain: bool, shell: Option<Option<String>>, after: After, result: Result<PersistentPtyInfo, String>, tx: &mut Tx) {
    if let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key) {
        ws.call = Call::Idle;
    }
    let Some((srv, _)) = ops::live_terminals(host) else { return };
    if let Some(shell) = shell {
        host.terminal().server_shells.borrow_mut().insert(srv.base.clone(), shell);
    }
    // A terminal started as the server's own: its program is that server's shell (which the
    // marks then start), whatever was guessed before.
    if plain
        && let Ok(info) = &result
        && !info.command.is_empty()
    {
        host.terminal().server_shells.borrow_mut().insert(srv.base.clone(), Some(info.command.clone()));
    }
    let id = match result {
        Ok(info) => {
            let id = info.id.clone();
            // The server's event may have brought it already.
            let here = host.terminal().workspaces.get(ws_key).is_some_and(|ws| ws.tabs.iter().any(|t| t.id == id));
            let tab = if here { Ok(None) } else { ops::persistent_tab(host, srv, info).map(Some) };
            tab.map(|tab| (id, tab))
        }
        Err(e) => Err(e),
    };
    let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key) else { return };
    let id = match id {
        Ok((id, tab)) => {
            ws.failure = None;
            ws.tabs.extend(tab);
            ws.active = Some(id.clone());
            id
        }
        Err(_) if !ws.tabs.is_empty() => return,
        Err(e) => {
            ws.failure = Some(e);
            tx.instant();
            return;
        }
    };
    let key = panel_key(&id);
    let routed = |host: &dyn TerminalSurfaceHost, session: &str| host.active_session_id() == Some(session);
    match after {
        After::Pane { session, pane } => {
            host.show_pane_tab(&session, &key, pane, tx);
            if routed(host, &session) {
                ops::focus_terminal_surface(host);
            }
        }
        After::Side { session } => host.keeping_keyboard(&mut |h: &mut dyn TerminalSurfaceHost| h.show_side_tab(&session, &key, None)),
        After::Nothing => {}
    }
    persist::queue_terminal_save(host);
    tx.instant();
}
