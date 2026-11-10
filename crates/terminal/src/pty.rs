//! Off-main-thread terminal session worker (`TerminalSession`) and deterministic [`StreamHandle`].
//! An OpenCode session terminal's worker lives in `persistent_pty.rs`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use opencode_kit::client::Server;
use parking_lot::Mutex;
use rune::Waker;

use crate::core::VtCore;
use crate::persistent_pty::run_persistent_worker;
use crate::theme::TerminalTheme;
use crate::types::{
    GridPoint, KeyInput, MouseInput, ScrollOp, SelectionOp, TerminalSnapshot, VtEvent,
};

pub const MAX_PERSIST_SCROLLBACK: usize = 2_000;
pub(crate) const COALESCE_BUDGET: Duration = Duration::from_millis(2);
pub(crate) const COALESCE_MAX_BYTES: usize = 256 * 1024;
pub(crate) const RENDER_HOLD_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub cols: u16,
    pub rows: u16,
    pub cell_w_px: u32,
    pub cell_h_px: u32,
    pub scrollback_lines: usize,
    pub theme: TerminalTheme,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            cell_w_px: 8,
            cell_h_px: 18,
            scrollback_lines: 10_000,
            theme: TerminalTheme::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RestoreState {
    pub buffer: Vec<u8>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub scroll_y: Option<usize>,
    pub cursor: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedTerminal {
    pub buffer: Vec<u8>,
    pub cols: u16,
    pub rows: u16,
    pub scroll_y: u64,
    pub cursor: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Exited { code: Option<u32> },
    Failed(String),
}

pub(crate) enum WorkerCmd {
    WriteRaw(Vec<u8>),
    Key(KeyInput),
    Mouse(MouseInput),
    Paste(String),
    Scroll(ScrollOp),
    Resize {
        cols: u16,
        rows: u16,
        cell_w_px: u32,
        cell_h_px: u32,
    },
    Select(SelectionOp),
    SetTheme(TerminalTheme),
    Persist(Sender<PersistedTerminal>),
    /// The user pressed on the pane, which takes control of a shared terminal at its size.
    Interact,
    Shutdown,
}

enum StreamInbound {
    Output(Vec<u8>),
    Exit(Option<u32>),
}

pub(crate) struct SharedState {
    pub(crate) snapshot: Mutex<Arc<TerminalSnapshot>>,
    pub(crate) status: Mutex<SessionStatus>,
    pub(crate) selected_text: Mutex<Option<String>>,
    pub(crate) events: Mutex<Vec<VtEvent>>,
    pub(crate) cursor: AtomicI64,
    pub(crate) revealed: AtomicBool,
}

/// Handle returned by [`TerminalSession::spawn_stream`] to drive a headless/story terminal.
#[derive(Clone)]
pub struct StreamHandle {
    in_tx: Sender<StreamInbound>,
    written: Arc<Mutex<Vec<u8>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl StreamHandle {
    pub fn feed(&self, bytes: impl AsRef<[u8]>) {
        let _ = self.in_tx.send(StreamInbound::Output(bytes.as_ref().to_vec()));
    }

    pub fn exit(&self, code: Option<u32>) {
        let _ = self.in_tx.send(StreamInbound::Exit(code));
    }

    pub fn take_written(&self) -> Vec<u8> {
        std::mem::take(&mut *self.written.lock())
    }

    pub fn take_resizes(&self) -> Vec<(u16, u16)> {
        std::mem::take(&mut *self.resizes.lock())
    }
}

/// Lightweight `Send` handle so background save threads (`opencode-saves`) can snapshot a
/// terminal's 2,000-row VT buffer without blocking the UI thread or holding `TerminalSession` alive.
#[derive(Clone)]
pub struct SessionPersistHandle {
    cmd_tx: Sender<WorkerCmd>,
}

impl SessionPersistHandle {
    pub fn capture(&self, timeout: Duration) -> Option<PersistedTerminal> {
        let (tx, rx) = mpsc::channel();
        self.cmd_tx.send(WorkerCmd::Persist(tx)).ok()?;
        rx.recv_timeout(timeout).ok()
    }
}

pub struct TerminalSession {
    shared: Arc<SharedState>,
    cmd_tx: Sender<WorkerCmd>,
}

impl TerminalSession {
    pub fn spawn_stream(
        config: TerminalConfig,
        restore: Option<RestoreState>,
        waker: Waker,
    ) -> Result<(Self, StreamHandle), String> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel();
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));

        let (init_snap, init_revealed, init_cursor) = bootstrap_initial(&config, restore.as_ref())?;
        let shared = Arc::new(SharedState {
            snapshot: Mutex::new(init_snap),
            status: Mutex::new(SessionStatus::Connected),
            selected_text: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            cursor: AtomicI64::new(init_cursor),
            revealed: AtomicBool::new(init_revealed),
        });

        let worker_shared = shared.clone();
        let worker_written = written.clone();
        let worker_resizes = resizes.clone();
        std::thread::Builder::new()
            .name("terminal-stream".into())
            .spawn(move || {
                run_stream_worker(
                    config,
                    restore,
                    cmd_rx,
                    in_rx,
                    worker_shared,
                    worker_written,
                    worker_resizes,
                    waker,
                );
            })
            .map_err(|e| format!("failed to spawn terminal worker: {e}"))?;

        Ok((
            Self { shared, cmd_tx },
            StreamHandle {
                in_tx,
                written,
                resizes,
            },
        ))
    }

    /// Attaches to an OpenCode session terminal (`persistent_pty.rs`): restores its screen from
    /// the server's snapshot, then streams it. An observer watches without taking control.
    pub fn spawn_persistent_pty(
        server: Server,
        pty_id: String,
        config: TerminalConfig,
        observer: bool,
        waker: Waker,
    ) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (init_snap, _, _) = bootstrap_initial(&config, None)?;
        let shared = Arc::new(SharedState {
            snapshot: Mutex::new(init_snap),
            status: Mutex::new(SessionStatus::Connecting),
            selected_text: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            cursor: AtomicI64::new(0),
            revealed: AtomicBool::new(false),
        });
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("terminal-persistent-pty".into())
            .spawn(move || run_persistent_worker(server, pty_id, config, observer, cmd_rx, worker_shared, waker))
            .map_err(|e| format!("failed to spawn terminal PTY worker: {e}"))?;
        Ok(Self { shared, cmd_tx })
    }

    pub fn snapshot(&self) -> Arc<TerminalSnapshot> {
        self.shared.snapshot.lock().clone()
    }

    pub fn status(&self) -> SessionStatus {
        self.shared.status.lock().clone()
    }

    pub fn revealed(&self) -> bool {
        self.shared.revealed.load(Ordering::Acquire)
    }

    pub fn cursor_offset(&self) -> i64 {
        self.shared.cursor.load(Ordering::Acquire)
    }

    pub fn selected_text(&self) -> Option<String> {
        self.shared.selected_text.lock().clone()
    }

    pub fn drain_events(&self) -> Vec<VtEvent> {
        std::mem::take(&mut *self.shared.events.lock())
    }

    pub fn write_bytes(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.cmd_tx.send(WorkerCmd::WriteRaw(bytes.into()));
    }

    pub fn send_key(&self, key: KeyInput) {
        let _ = self.cmd_tx.send(WorkerCmd::Key(key));
    }

    pub fn send_mouse(&self, mouse: MouseInput) {
        let _ = self.cmd_tx.send(WorkerCmd::Mouse(mouse));
    }

    pub fn paste(&self, text: impl Into<String>) {
        let _ = self.cmd_tx.send(WorkerCmd::Paste(text.into()));
    }

    pub fn scroll(&self, op: ScrollOp) {
        let _ = self.cmd_tx.send(WorkerCmd::Scroll(op));
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let _ = self.cmd_tx.send(WorkerCmd::Resize {
            cols,
            rows,
            cell_w_px,
            cell_h_px,
        });
    }

    pub fn select(&self, op: SelectionOp) {
        let _ = self.cmd_tx.send(WorkerCmd::Select(op));
    }

    /// The user pressed on the pane: a shared session terminal comes under this window's
    /// control at its size (Desktop's `interacted`).
    pub fn interact(&self) {
        let _ = self.cmd_tx.send(WorkerCmd::Interact);
    }

    pub fn select_range_viewport(&self, start: (u16, u16), end: (u16, u16), rectangular: bool) {
        self.select(SelectionOp::Range {
            start: GridPoint::Viewport {
                col: start.0,
                row: start.1,
            },
            end: GridPoint::Viewport {
                col: end.0,
                row: end.1,
            },
            rectangular,
        });
    }

    pub fn set_theme(&self, theme: TerminalTheme) {
        let _ = self.cmd_tx.send(WorkerCmd::SetTheme(theme));
    }

    pub fn persist_handle(&self) -> SessionPersistHandle {
        SessionPersistHandle {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    pub fn persist_snapshot(&self, timeout: Duration) -> Option<PersistedTerminal> {
        self.persist_handle().capture(timeout)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(WorkerCmd::Shutdown);
    }
}

pub(crate) fn bootstrap_initial(
    config: &TerminalConfig,
    restore: Option<&RestoreState>,
) -> Result<(Arc<TerminalSnapshot>, bool, i64), String> {
    let mut core = build_core(config, restore)?;
    let snap = core.snapshot();
    let revealed = snap.screen_rows.iter().any(|r| !r.trimmed_text().is_empty());
    let cursor = restore.and_then(|r| r.cursor).unwrap_or(0);
    Ok((snap, revealed, cursor))
}

pub(crate) fn build_core(
    config: &TerminalConfig,
    restore: Option<&RestoreState>,
) -> Result<VtCore, String> {
    let init_cols = restore
        .and_then(|r| (!r.buffer.is_empty()).then_some(r.cols).flatten())
        .unwrap_or(config.cols)
        .max(1);
    let init_rows = restore
        .and_then(|r| (!r.buffer.is_empty()).then_some(r.rows).flatten())
        .unwrap_or(config.rows)
        .max(1);
    let mut core = VtCore::new(
        init_cols,
        init_rows,
        config.scrollback_lines,
        config.theme.clone(),
    )?;
    core.resize(init_cols, init_rows, config.cell_w_px, config.cell_h_px);
    if let Some(res) = restore {
        if !res.buffer.is_empty() {
            core.feed(&res.buffer);
            let _ = core.take_pty_writes();
            let _ = core.take_events();
        }
        if (init_cols, init_rows) != (config.cols.max(1), config.rows.max(1)) {
            core.resize(config.cols, config.rows, config.cell_w_px, config.cell_h_px);
        }
        if let Some(y) = res.scroll_y {
            core.scroll(ScrollOp::Row(y));
        }
    }
    Ok(core)
}

pub(crate) fn publish_if_ready(
    core: &mut VtCore,
    shared: &SharedState,
    waker: &Waker,
    hold_started: &mut Option<Instant>,
) {
    if core.render_held() {
        let started = *hold_started.get_or_insert_with(Instant::now);
        if started.elapsed() < RENDER_HOLD_TIMEOUT {
            return;
        }
        core.clear_render_hold();
    }
    *hold_started = None;

    let evs = core.take_events();
    if !evs.is_empty() {
        shared.events.lock().extend(evs);
    }
    let snap = core.snapshot();
    if !shared.revealed.load(Ordering::Acquire)
        && snap.screen_rows.iter().any(|r| !r.trimmed_text().is_empty())
    {
        shared.revealed.store(true, Ordering::Release);
    }
    *shared.snapshot.lock() = snap;
    waker();
}

pub(crate) fn apply_worker_cmd(
    cmd: WorkerCmd,
    core: &mut VtCore,
    shared: &SharedState,
    out_bytes: &mut Vec<u8>,
    resized: &mut Option<(u16, u16)>,
    dirty: &mut bool,
) -> bool {
    match cmd {
        WorkerCmd::WriteRaw(data) => out_bytes.extend_from_slice(&data),
        WorkerCmd::Key(key) => {
            let encoded = core.encode_key(&key);
            if !encoded.is_empty() {
                out_bytes.extend_from_slice(&encoded);
            }
        }
        WorkerCmd::Mouse(mouse) => {
            let encoded = core.encode_mouse(mouse);
            if !encoded.is_empty() {
                out_bytes.extend_from_slice(&encoded);
            }
        }
        WorkerCmd::Paste(text) => {
            let encoded = core.encode_paste(&text);
            if !encoded.is_empty() {
                out_bytes.extend_from_slice(&encoded);
            }
        }
        WorkerCmd::Scroll(op) => {
            core.scroll(op);
            *dirty = true;
        }
        WorkerCmd::Resize {
            cols,
            rows,
            cell_w_px,
            cell_h_px,
        } => {
            core.resize(cols, rows, cell_w_px, cell_h_px);
            *resized = Some((cols.max(1), rows.max(1)));
            *dirty = true;
        }
        WorkerCmd::Select(op) => {
            core.select(op);
            *shared.selected_text.lock() = core.selected_text();
            *dirty = true;
        }
        WorkerCmd::SetTheme(theme) => {
            core.set_theme(theme);
            out_bytes.extend_from_slice(&core.take_pty_writes());
            *dirty = true;
        }
        WorkerCmd::Persist(reply) => {
            let snap = shared.snapshot.lock().clone();
            let _ = reply.send(PersistedTerminal {
                buffer: core.serialize_vt_capped(MAX_PERSIST_SCROLLBACK),
                cols: snap.cols,
                rows: snap.rows,
                scroll_y: snap.scrollbar.offset,
                cursor: shared.cursor.load(Ordering::Acquire),
            });
        }
        WorkerCmd::Interact => {}
        WorkerCmd::Shutdown => return false,
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn run_stream_worker(
    config: TerminalConfig,
    restore: Option<RestoreState>,
    cmd_rx: Receiver<WorkerCmd>,
    in_rx: Receiver<StreamInbound>,
    shared: Arc<SharedState>,
    written: Arc<Mutex<Vec<u8>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    waker: Waker,
) {
    let Ok(mut core) = build_core(&config, restore.as_ref()) else {
        return;
    };
    let mut hold_started: Option<Instant> = None;

    loop {
        let mut dirty = false;
        let mut out_bytes = Vec::new();
        let mut resized = None;

        loop {
            match cmd_rx.try_recv() {
                Ok(cmd) => {
                    if !apply_worker_cmd(
                        cmd,
                        &mut core,
                        &shared,
                        &mut out_bytes,
                        &mut resized,
                        &mut dirty,
                    ) {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        if let Some(sz) = resized {
            resizes.lock().push(sz);
        }

        let burst_start = Instant::now();
        let mut fed_bytes = 0usize;
        loop {
            match in_rx.try_recv() {
                Ok(StreamInbound::Output(chunk)) => {
                    fed_bytes += chunk.len();
                    shared
                        .cursor
                        .fetch_add(chunk.len() as i64, Ordering::AcqRel);
                    core.feed(&chunk);
                    out_bytes.extend_from_slice(&core.take_pty_writes());
                    dirty = true;
                    if fed_bytes >= COALESCE_MAX_BYTES
                        || burst_start.elapsed() >= COALESCE_BUDGET
                    {
                        break;
                    }
                }
                Ok(StreamInbound::Exit(code)) => {
                    *shared.status.lock() = SessionStatus::Exited { code };
                    dirty = true;
                    break;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }

        if !out_bytes.is_empty() {
            written.lock().extend_from_slice(&out_bytes);
        }

        if dirty || hold_started.is_some_and(|t| t.elapsed() >= RENDER_HOLD_TIMEOUT) {
            publish_if_ready(&mut core, &shared, &waker, &mut hold_started);
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
