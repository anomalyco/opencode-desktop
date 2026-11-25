//! Non-generic terminal workspace operations, PTY lifecycle, and off-thread job execution.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use opencode_kit::client::Server;
use opencode_kit::persistent_pty::{PersistentPtyInfo, TerminalCreate};
use rune::{Tx, Waker};
use terminal::{RestoreState, TerminalConfig, TerminalSession};

use crate::{
    FocusClaimEvent, HoverLink, StoryStageEvent, TabBackend, TerminalJob, TerminalReply, TerminalSurfaceHost,
    TerminalTab, TerminalWorkspace, calls, dir_name, grid, links, mount, panel_key, persist, rename, side_ops,
    workspace_key,
};

pub fn server_id_str(host: &dyn TerminalSurfaceHost) -> &str {
    host.live_server().map_or("fixture", |srv| srv.base.as_str())
}

/// The server whose session terminals this window shows, and whether it may only watch
/// them (`observer == true`): `--read-only` and headless runs never start a shell or type into one.
pub fn live_terminals(host: &dyn TerminalSurfaceHost) -> Option<(Server, bool)> {
    if host.is_story_stage() {
        return None;
    }
    host.live_server().map(|srv| (srv.clone(), !host.allows_terminal_input()))
}

/// The appearance the terminals paint in: the one the window last drew, else the setting.
pub fn terminal_dark(host: &dyn TerminalSurfaceHost) -> bool {
    host.terminal()
        .painted_dark
        .get()
        .unwrap_or_else(|| host.terminal_dark_default())
}

/// The session a terminal workspace belongs to, and the directory its shells start in.
pub(crate) fn terminal_session_dir(host: &dyn TerminalSurfaceHost, ws_key: &str) -> (String, String) {
    let sid = ws_key.split_once('\0').map_or("", |(_, s)| s).to_owned();
    let dir = host
        .session_directory(&sid)
        .unwrap_or_else(|| host.draft_directory())
        .to_owned();
    (sid, dir)
}

/// Whether a terminal of session `sid` has to wait: the server is live and hasn't listed the
/// session yet (a subagent's page at launch), so its folder isn't known. A shell started now
/// would start in another folder (the app's own), which is worse than a late one.
pub fn waits_for_folder(host: &dyn TerminalSurfaceHost, sid: &str) -> bool {
    live_terminals(host).is_some() && !sid.is_empty() && host.session_directory(sid).is_none_or(str::is_empty)
}

pub fn terminal_waker(host: &dyn TerminalSurfaceHost) -> Waker {
    let dirty = host.terminal().dirty_flag.clone();
    let app_waker = host.waker();
    Arc::new(move || {
        dirty.store(true, Ordering::Release);
        if let Some(w) = &app_waker {
            w();
        }
    })
}

/// The workspace session `session_id`'s terminals are in: its side panel's (`AppEnv::panel_root`),
/// so a subagent's page shows its root's terminals, as it shows the rest of its root's strip
/// (opencode#65957). Desktop's terminal model loads the routed session's own terminals
/// (`packages/gui-plugins/src/terminal/model.ts` `load`), which on a subagent's page drops the
/// root's terminal tabs from the shared strip: not copied.
pub fn terminal_workspace_key_for_session(host: &dyn TerminalSurfaceHost, session_id: &str) -> String {
    workspace_key(server_id_str(host), host.panel_root(session_id))
}

pub fn active_terminal_workspace_key(host: &dyn TerminalSurfaceHost) -> Option<String> {
    if host.is_story_stage() {
        return Some(host.story_workspace_key().to_owned());
    }
    let sid = host.active_session_id()?;
    Some(terminal_workspace_key_for_session(host, sid))
}

/// Makes terminal `id` session `sid`'s active one; with `focus` it takes the keyboard (a tab
/// the user selected, not one a restore did).
pub fn select_session_terminal(host: &mut dyn TerminalSurfaceHost, sid: &str, id: &str, focus: bool) {
    side_ops::on_focus(host, sid, id, !focus);
}

/// Loads a session's terminals and, when it has none, starts one, as opening Desktop's
/// terminal panel does. A read-only window only shows the ones the server already has.
pub fn ensure_workspace_ready(host: &mut dyn TerminalSurfaceHost, ws_key: &str, after: calls::After) {
    persist::ensure_terminal_workspace_hydrated(host, ws_key);
    let empty = host
        .terminal()
        .workspaces
        .get(ws_key)
        .is_none_or(|ws| ws.tabs.is_empty() && ws.failure.is_none());
    let watch_only = live_terminals(host).is_some_and(|(_, observer)| observer);
    if empty && !watch_only {
        new_terminal_in(host, ws_key, after);
    }
}

/// Opens session `sid`'s terminal tab (a new terminal when `new_tab` or it has none), leaving
/// the keyboard where it was.
pub fn open_terminal_for_session(host: &mut dyn TerminalSurfaceHost, sid: &str, new_tab: bool) -> Option<String> {
    if waits_for_folder(host, sid) {
        host.terminal_mut().asked = Some(side_ops::Asked::Open { session: sid.to_owned(), new_tab });
        return None;
    }
    let mut out = None;
    host.keeping_keyboard(&mut |h: &mut dyn TerminalSurfaceHost| {
        let ws_key = terminal_workspace_key_for_session(h, sid);
        persist::ensure_terminal_workspace_hydrated(h, &ws_key);
        // Which terminals it has isn't known yet: asked again when the server has listed them.
        if calls::listing(h, &ws_key) {
            h.terminal_mut().asked = Some(side_ops::Asked::Open { session: sid.to_owned(), new_tab });
            return;
        }
        // On a live server the new one's tab shows when the server has started it (`calls::After`).
        let after = calls::After::Side { session: sid.to_owned() };
        if new_tab {
            new_terminal_in(h, &ws_key, after);
        } else {
            ensure_workspace_ready(h, &ws_key, after);
        }
        let Some(id) = h.terminal().workspaces.get(&ws_key).and_then(|ws| ws.active.clone()) else {
            return;
        };
        let key = panel_key(&id);
        h.show_side_tab(sid, &key, None);
        out = Some(id);
    });
    out
}

/// The keyboard goes to the routed session's active terminal, in the pane showing its tab.
pub fn focus_terminal_surface(host: &mut dyn TerminalSurfaceHost) {
    let active_tab = match host.active_session_id().map(str::to_owned) {
        Some(sid) => {
            let wk = terminal_workspace_key_for_session(host, &sid);
            host.terminal()
                .workspaces
                .get(&wk)
                .and_then(|ws| ws.active.clone())
                .map(|id| (sid, panel_key(&id)))
        }
        None => None,
    };
    match &active_tab {
        Some((sid, key)) => host.focus_side_pane(Some((sid.as_str(), key.as_str()))),
        None => host.focus_side_pane(None),
    }
    let now = host.clock();
    let pane = host.terminal_mut();
    pane.story.step(StoryStageEvent::Focus);
    drop(pane.focus_claim.step(FocusClaimEvent::Request));
    pane.reset_blink(now);
}

pub fn terminal_tab_mut<'a>(host: &'a mut dyn TerminalSurfaceHost, ws_key: &str, tab_id: &str) -> Option<&'a mut TerminalTab> {
    host.terminal_mut()
        .workspaces
        .get_mut(ws_key)?
        .tabs
        .iter_mut()
        .find(|t| t.id == tab_id)
}

pub fn select_terminal_tab(host: &mut dyn TerminalSurfaceHost, ws_key: &str, tab_id: &str) {
    let mut changed = false;
    if let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key)
        && ws.tabs.iter().any(|t| t.id == tab_id)
        && ws.active.as_deref() != Some(tab_id)
    {
        ws.active = Some(tab_id.to_owned());
        changed = true;
    }
    focus_terminal_surface(host);
    if changed {
        persist::queue_terminal_save(host);
    }
}

pub fn select_adjacent_terminal_tab(host: &mut dyn TerminalSurfaceHost, ws_key: &str, offset: i32, tx: &mut Tx) -> bool {
    let next = host.terminal().workspaces.get(ws_key).and_then(|ws| {
        if ws.tabs.len() <= 1 {
            return None;
        }
        let keys: Vec<&str> = ws.tabs.iter().map(|t| t.id.as_str()).collect();
        adjacent_key(&keys, ws.active.as_deref(), offset)
    });
    if let Some(id) = next {
        tx.instant();
        select_terminal_tab(host, ws_key, &id);
        return true;
    }
    false
}

fn adjacent_key(order: &[&str], current: Option<&str>, offset: i32) -> Option<String> {
    let n = order.len() as i32;
    if n == 0 {
        return None;
    }
    let Some(index) = current.and_then(|c| order.iter().position(|k| *k == c)) else {
        let idx = if offset < 0 { order.len().saturating_sub(1) } else { 0 };
        return order.get(idx).map(|k| (*k).to_owned());
    };
    (1..n)
        .map(|step| order[(index as i32 + offset * step).rem_euclid(n) as usize])
        .next()
        .map(str::to_owned)
}

/// Removes terminal `tab_id` and ends its shell; its tab has left, or leaves, the strip.
pub fn close_terminal_tab(host: &mut dyn TerminalSurfaceHost, ws_key: &str, tab_id: &str, tx: &mut Tx) {
    if let Some(ws) = host.terminal_mut().workspaces.get_mut(ws_key)
        && let Some(idx) = ws.tabs.iter().position(|t| t.id == tab_id)
    {
        ws.tabs.remove(idx);
        if ws.active.as_deref() == Some(tab_id) {
            ws.active = if idx > 0 {
                ws.tabs.get(idx - 1).map(|t| t.id.clone())
            } else {
                ws.tabs.first().map(|t| t.id.clone())
            };
        }
    }

    // Closing a tab ends its shell on the server, as Desktop's close does.
    if let Some((_, false)) = live_terminals(host)
        && tab_id.starts_with("pty_")
    {
        host.spawn_job(TerminalJob::Remove { pty_id: tab_id.to_owned() });
    }

    tx.instant();
    persist::queue_terminal_save(host);
}

/// Starts a terminal: on a live server, a session shell (`persistentPty.create`) named
/// after the lowest free "Terminal N"; in a fixture scene, a recorded prompt.
pub fn new_terminal_in(host: &mut dyn TerminalSurfaceHost, ws_key: &str, after: calls::After) -> Option<String> {
    let theme = host.terminal_theme(terminal_dark(host));
    let (session_id, directory) = terminal_session_dir(host, ws_key);
    // Never in a folder that isn't the session's: the callers ask again once it's listed.
    if waits_for_folder(host, &session_id) {
        return None;
    }
    let num = {
        let pane = host.terminal_mut();
        pane.touch_lru(ws_key);
        pane.workspaces
            .entry(ws_key.to_owned())
            .or_insert_with(|| TerminalWorkspace { directory: directory.clone(), ..TerminalWorkspace::default() })
            .next_title_number()
    };
    let title = format!("Terminal {num}");
    let (init_cols, init_rows) = estimated_terminal_grid(host);

    if let Some((srv, observer)) = live_terminals(host) {
        if observer {
            return None;
        }
        // Reuse the shown terminal's cwd report, or its own server-side launch folder
        // (e.g. WSL reports /home/me). Only the first tab needs the session folder.
        let directory = {
            let shown = host.terminal().workspaces.get(ws_key).and_then(|ws| {
                ws.active.as_deref().and_then(|id| ws.tabs.iter().find(|t| t.id == id))
            });
            shown.map_or_else(
                || directory.clone(),
                |t| links::shell_folder(&t.session.peek(), if t.cwd.is_empty() { &directory } else { &t.cwd }),
            )
        };
        // The server starts it off this thread: its tab shows when it has (`calls::created`).
        let create = TerminalCreate::new(title).cwd(Some(directory)).size(Some((init_cols, init_rows)));
        calls::create(host, &srv, ws_key, session_id, create, after);
        return None;
    }
    let short_dir = dir_name(&directory);
    let prompt = format!(
        "\x1b[1;34m~/{short_dir}\x1b[0m \x1b[38;5;245mgit:(\x1b[1;36mmain\x1b[0;38;5;245m)\x1b[0m \x1b[1;32m\u{276f}\x1b[0m "
    );
    let banner = if num == 1 {
        format!(
            "{prompt}cargo test --quiet\r\n\x1b[1;32m   Finished\x1b[0m `test` profile in 0.42s\r\n\
             \x1b[1;32mtest result: ok.\x1b[0m 14 passed; 0 failed\r\n\r\n{prompt}"
        )
    } else {
        prompt
    };
    let id = insert_stream_with(
        host,
        ws_key,
        &format!("term_{num}"),
        &title,
        num,
        TerminalConfig { cols: init_cols, rows: init_rows, theme, ..TerminalConfig::default() },
        Some(RestoreState {
            buffer: banner.into_bytes(),
            cols: Some(init_cols),
            rows: Some(init_rows),
            scroll_y: None,
            cursor: None,
        }),
        b"",
        TabBackend::FixtureShell,
    )?;
    focus_terminal_surface(host);
    persist::queue_terminal_save(host);
    Some(id)
}

/// `Terminal:Create`: starts a terminal of session `sid` on the server the window works
/// against, as [`new_terminal_in`] does (on this thread), running `command` with `args`
/// in `cwd` instead of the user's shell when given; named `title`, else the lowest free
/// "Terminal N". [`adopt_terminal`] then adds it to the session's terminals.
pub fn start_terminal(
    host: &dyn TerminalSurfaceHost,
    sid: &str,
    title: Option<String>,
    cwd: Option<String>,
    command: Option<String>,
    args: Vec<String>,
) -> Result<PersistentPtyInfo, String> {
    let Some((srv, false)) = live_terminals(host) else {
        return Err("This window can't start terminals".into());
    };
    let wk = terminal_workspace_key_for_session(host, sid);
    let title = title.filter(|t| !t.is_empty()).unwrap_or_else(|| {
        format!(
            "Terminal {}",
            host.terminal().workspaces.get(&wk).map_or(1, TerminalWorkspace::next_title_number)
        )
    });
    let cwd = cwd
        .filter(|c| !c.is_empty())
        .or_else(|| host.session_directory(sid).map(str::to_owned));
    let create = terminal_launch(
        host,
        &srv,
        TerminalCreate::new(title).cwd(cwd).size(Some(estimated_terminal_grid(host))),
        command,
        args,
    );
    srv.terminal_create(sid, &create).map_err(|e| e.to_string())
}

/// [`calls::launch`] on this thread, for a plugin's `Terminal:Create`, which answers at once.
fn terminal_launch(
    host: &dyn TerminalSurfaceHost,
    srv: &Server,
    create: TerminalCreate,
    command: Option<String>,
    args: Vec<String>,
) -> TerminalCreate {
    let (create, shell) = calls::launch(srv, &calls::launch_inputs(host, srv), create, command, args);
    if let Some(shell) = shell {
        host.terminal().server_shells.borrow_mut().insert(srv.base.clone(), shell);
    }
    create
}
/// A terminal [`start_terminal`] started joins session `sid`'s terminals and becomes
/// its active one, unless the server's event already brought it.
pub fn adopt_terminal(host: &mut dyn TerminalSurfaceHost, sid: &str, info: PersistentPtyInfo) {
    let wk = terminal_workspace_key_for_session(host, sid);
    persist::ensure_terminal_workspace_hydrated(host, &wk);
    let Some((srv, _)) = live_terminals(host) else { return };
    let directory = host.session_directory(sid).unwrap_or_default().to_owned();
    let id = info.id.clone();
    let exists = {
        let ws = host
            .terminal_mut()
            .workspaces
            .entry(wk.clone())
            .or_insert_with(|| TerminalWorkspace { directory, ..TerminalWorkspace::default() });
        ws.tabs.iter().any(|t| t.id == id)
    };
    if !exists {
        let Ok(tab) = persistent_tab(host, srv, info) else { return };
        let Some(ws) = host.terminal_mut().workspaces.get_mut(&wk) else { return };
        ws.failure = None;
        ws.tabs.push(tab);
    }
    select_terminal_tab(host, &wk, &id);
    persist::queue_terminal_save(host);
}

/// The grid a new terminal starts at, before the pane first measures itself.
fn estimated_terminal_grid(host: &dyn TerminalSurfaceHost) -> (u16, u16) {
    let (font_choice, style) = grid::terminal_font_style(host.knobs());
    let (cell_w, cell_h) = grid::cell_metrics(font_choice, style);
    let (pane_w, pane_h) = host.terminal_pane_estimate();
    let est_w = (pane_w - grid::PAD_X * 2.0 - 8.0).max(cell_w * 10.0);
    let est_h = (pane_h - grid::BAR_H - grid::PAD_Y * 2.0).max(cell_h * 2.0);
    (((est_w / cell_w).floor() as u16).clamp(10, 500), ((est_h / cell_h).floor() as u16).clamp(2, 240))
}

/// A tab attached to one of the server's session terminals.
pub fn persistent_tab(
    host: &dyn TerminalSurfaceHost,
    srv: Server,
    info: PersistentPtyInfo,
) -> Result<TerminalTab, String> {
    let observer = live_terminals(host).is_none_or(|(_, observer)| observer);
    let config = TerminalConfig {
        cols: info.size.0,
        rows: info.size.1,
        theme: host.terminal_theme(terminal_dark(host)),
        ..TerminalConfig::default()
    };
    host.fetch_terminal_fonts_if_wasm();
    let session = TerminalSession::spawn_persistent_pty(srv, info.id.clone(), config, observer, terminal_waker(host))?;
    Ok(TerminalTab {
        title_number: persist::number_from_title(&info.title).unwrap_or(0),
        custom_title: info.user_title.is_some(),
        id: info.id,
        title: info.title,
        session,
        backend: TabBackend::PersistentPty,
        scroll_px_accum: 0.0,
        drag: mount::TextDrag::Idle,
        edge_scroll: None,
        mount: mount::Mount::Live { connection: host.terminal().connections },
        fit: Cell::new(mount::Fit::Unmeasured),
        preedit: None,
        cwd: info.cwd.clone(),
        find: None,
        focused: Cell::new(false),
        hover: HoverLink::None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn insert_stream_terminal(
    host: &mut dyn TerminalSurfaceHost,
    ws_key: &str,
    id: &str,
    title: &str,
    title_number: usize,
    config: TerminalConfig,
    restore: Option<RestoreState>,
    initial_bytes: &[u8],
) -> Option<String> {
    insert_stream_with(host, ws_key, id, title, title_number, config, restore, initial_bytes, TabBackend::Stream)
}

#[allow(clippy::too_many_arguments)]
fn insert_stream_with(
    host: &mut dyn TerminalSurfaceHost,
    ws_key: &str,
    id: &str,
    title: &str,
    title_number: usize,
    config: TerminalConfig,
    restore: Option<RestoreState>,
    initial_bytes: &[u8],
    wrap: fn(terminal::StreamHandle) -> TabBackend,
) -> Option<String> {
    let (_, directory) = terminal_session_dir(host, ws_key);
    let connection = host.terminal().connections;
    let waker = terminal_waker(host);
    host.terminal_mut().touch_lru(ws_key);
    let (session, handle) = TerminalSession::spawn_stream(config, restore, waker).ok()?;
    if !initial_bytes.is_empty() {
        handle.feed(initial_bytes);
    }
    let ws = host
        .terminal_mut()
        .workspaces
        .entry(ws_key.to_owned())
        .or_insert_with(|| TerminalWorkspace { directory: directory.clone(), ..TerminalWorkspace::default() });
    ws.tabs.push(TerminalTab {
        id: id.to_owned(),
        title: title.to_owned(),
        title_number,
        custom_title: false,
        session,
        backend: wrap(handle),
        scroll_px_accum: 0.0,
        drag: mount::TextDrag::Idle,
        edge_scroll: None,
        mount: mount::Mount::Live { connection },
        fit: Cell::new(mount::Fit::Unmeasured),
        preedit: None,
        cwd: directory,
        find: None,
        focused: Cell::new(false),
        hover: HoverLink::None,
    });
    ws.active = Some(id.to_owned());
    Some(id.to_owned())
}

/// Runs a [`TerminalJob`] off-thread against the live OpenCode [`Server`].
pub fn execute_job(srv: &Server, job: TerminalJob) -> TerminalReply {
    match job {
        TerminalJob::Rename { ws, pty_id, title, attempt } => {
            let result = srv.terminal_rename(&pty_id, title.as_deref()).map_err(|e| e.to_string());
            TerminalReply::Renamed { ws, pty_id, attempt, result }
        }
        TerminalJob::Remove { pty_id } => {
            drop(srv.terminal_remove(&pty_id));
            TerminalReply::Removed { pty_id }
        }
        TerminalJob::List { ws, session, order, active } => calls::list_now(srv, ws, &session, order, active),
        TerminalJob::Create { ws, session, create, launch, after } => calls::create_now(srv, ws, &session, create, &launch, after),
    }
}

/// Delivers a finished [`TerminalReply`] back on the UI thread.
pub fn apply_reply(host: &mut dyn TerminalSurfaceHost, reply: TerminalReply, tx: &mut Tx) {
    match reply {
        TerminalReply::Renamed { ws, pty_id, attempt, result } => {
            rename::apply_rename_reply(host, &ws, &pty_id, attempt, result, tx);
        }
        TerminalReply::Removed { .. } => {}
        TerminalReply::Listed { ws, order, active, result } => calls::listed(host, &ws, &order, active, result, tx),
        TerminalReply::Created { ws, plain, shell, after, result } => calls::created(host, &ws, plain, shell, after, result, tx),
    }
}
