//! Off-main-thread terminal session worker (`TerminalSession`) and deterministic [`StreamHandle`].
//! An OpenCode session terminal's worker lives in `persistent_pty.rs`.
//!
//! Every worker sleeps until it has something to do: a stream's worker on its channel, a
//! persistent PTY's on its socket and channel together (`inbox.rs`). Neither wakes on a timer
//! unless a program holds its frame (`?2026`) or a resize waits out its debounce.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;
use rune::time::Instant;

use opencode_kit::client::Server;
use parking_lot::Mutex;
use rune::Waker;

use crate::core::VtCore;
use crate::inbox::{Inbox, SocketWait};
#[cfg(not(target_arch = "wasm32"))]
use crate::persistent_pty::run_persistent_worker;
use crate::publish::{self, Handoff, Publish};
use crate::theme::TerminalTheme;
use crate::types::{
    GridPoint, KeyInput, MouseInput, ScrollOp, SelectionOp, TerminalSnapshot, VtEvent,
};

pub const MAX_PERSIST_SCROLLBACK: usize = 2_000;
/// The most output fed before the worker looks at its commands again.
pub(crate) const COALESCE_BUDGET: Duration = Duration::from_millis(2);
pub(crate) const COALESCE_MAX_BYTES: usize = 256 * 1024;
/// A UI that hasn't read a snapshot for this long is showing the terminal again: it waits for
/// the screen as it is instead of drawing the old one first.
const SHOWN_AGAIN: Duration = Duration::from_millis(50);
/// The longest a returning reader waits for that screen.
const SHOW_WAIT: Duration = Duration::from_millis(16);

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
    /// Ctrl+Break, for a Windows console.
    Break,
    /// Find this in the screen and scrollback (`None`: stop).
    Find(Option<String>),
    /// Select the next match, toward older output when true.
    FindStep(bool),
    /// The OSC 8 link at a viewport cell.
    LinkAt { col: u16, row: u16, reply: Sender<Option<String>> },
    /// The selection as HTML.
    SelectionHtml(Sender<Option<String>>),
    /// Drop the scrollback, keeping the screen.
    ClearScrollback,
    /// A stream's program wrote this.
    Output(Vec<u8>),
    /// A stream's program exited.
    Exit(Option<u32>),
    /// The UI read the snapshot; publish the screen if it changed since.
    Nudge,
    /// The UI is about to draw a terminal it hadn't drawn for a while: publish the screen now
    /// and say when it's there.
    Show(Sender<()>),
    Shutdown,
}

pub(crate) struct SharedState {
    pub(crate) snapshot: Mutex<Arc<TerminalSnapshot>>,
    pub(crate) handoff: Handoff,
    /// Bumped by every publish and status change, so a view can tell it changed without reading.
    pub(crate) generation: AtomicU64,
    /// When the UI last read the snapshot (ns on the monotonic clock).
    pub(crate) read_at: AtomicU64,
    pub(crate) status: Mutex<SessionStatus>,
    pub(crate) selected_text: Mutex<Option<String>>,
    pub(crate) events: Mutex<Vec<VtEvent>>,
    pub(crate) cursor: AtomicI64,
    pub(crate) revealed: AtomicBool,
}

impl SharedState {
    fn new(snapshot: Arc<TerminalSnapshot>, status: SessionStatus, cursor: i64, revealed: bool) -> SharedState {
        SharedState {
            snapshot: Mutex::new(snapshot),
            handoff: Handoff::default(),
            generation: AtomicU64::new(0),
            read_at: AtomicU64::new(0),
            status: Mutex::new(status),
            selected_text: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            cursor: AtomicI64::new(cursor),
            revealed: AtomicBool::new(revealed),
        }
    }

    pub(crate) fn set_status(&self, status: SessionStatus) {
        *self.status.lock() = status;
        self.generation.fetch_add(1, Ordering::Release);
    }
}

/// Handle returned by [`TerminalSession::spawn_stream`] to drive a headless/story terminal.
#[derive(Clone)]
pub struct StreamHandle {
    inbox: Inbox,
    written: Arc<Mutex<Vec<u8>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl StreamHandle {
    pub fn feed(&self, bytes: impl AsRef<[u8]>) {
        self.inbox.send(WorkerCmd::Output(bytes.as_ref().to_vec()));
    }

    pub fn exit(&self, code: Option<u32>) {
        self.inbox.send(WorkerCmd::Exit(code));
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
    inbox: Inbox,
}

impl SessionPersistHandle {
    pub fn capture(&self, timeout: Duration) -> Option<PersistedTerminal> {
        // A page's worker runs on the thread that would wait for it.
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.inbox.send(WorkerCmd::Persist(tx)).then_some(())?;
        rx.recv_timeout(timeout).ok()
    }
}

pub struct TerminalSession {
    shared: Arc<SharedState>,
    inbox: Inbox,
}

impl TerminalSession {
    pub fn spawn_stream(
        config: TerminalConfig,
        restore: Option<RestoreState>,
        waker: Waker,
    ) -> Result<(Self, StreamHandle), String> {
        let (tx, rx) = mpsc::channel();
        let inbox = Inbox::channel(tx);
        let written = Arc::new(Mutex::new(Vec::new()));
        let resizes = Arc::new(Mutex::new(Vec::new()));

        let (init_snap, init_revealed, init_cursor) = bootstrap_initial(&config, restore.as_ref())?;
        let shared = Arc::new(SharedState::new(init_snap, SessionStatus::Connected, init_cursor, init_revealed));

        let worker_shared = shared.clone();
        let worker_written = written.clone();
        let worker_resizes = resizes.clone();
        std::thread::Builder::new()
            .name("terminal-stream".into())
            .spawn(move || {
                run_stream_worker(config, restore, rx, worker_shared, worker_written, worker_resizes, waker);
            })
            .map_err(|e| format!("failed to spawn terminal worker: {e}"))?;

        Ok((
            Self { shared, inbox: inbox.clone() },
            StreamHandle {
                inbox,
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
        let (tx, rx) = mpsc::channel();
        let wait = Arc::new(SocketWait::new().map_err(|e| format!("failed to start terminal PTY worker: {e}"))?);
        let inbox = Inbox::socket(tx, wait.clone());
        // The server's checkpoint paints the screen; until it comes, an empty one at its size.
        let blank = Arc::new(TerminalSnapshot::blank(config.cols.max(1), config.rows.max(1), &config.theme));
        let shared = Arc::new(SharedState::new(blank, SessionStatus::Connecting, 0, false));
        let worker_shared = shared.clone();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new()
            .name("terminal-persistent-pty".into())
            .spawn(move || run_persistent_worker(server, pty_id, config, observer, rx, wait, worker_shared, waker))
            .map_err(|e| format!("failed to spawn terminal PTY worker: {e}"))?;
        // A page has no threads: a task of its event loop (`persistent_pty_web.rs`).
        #[cfg(target_arch = "wasm32")]
        crate::persistent_pty::web::start(server, pty_id, config, observer, rx, wait, worker_shared, waker);
        Ok(Self { shared, inbox })
    }

    /// The screen to draw. Reading it lets the worker publish the next one; a terminal drawn
    /// again after a while gets the screen as it is now.
    pub fn snapshot(&self) -> Arc<TerminalSnapshot> {
        let now = monotonic_ns();
        let last = self.shared.read_at.swap(now, Ordering::AcqRel);
        if self.shared.handoff.read() {
            // A page can't wait for its own task: it takes the next screen on a later read.
            if !cfg!(target_arch = "wasm32") && now.saturating_sub(last) >= SHOWN_AGAIN.as_nanos() as u64 {
                let (tx, rx) = mpsc::channel();
                if self.inbox.send(WorkerCmd::Show(tx)) {
                    let _ = rx.recv_timeout(SHOW_WAIT);
                    // The fresh snapshot is this read's.
                    self.shared.handoff.read();
                }
            } else {
                self.inbox.nudge();
            }
        }
        self.shared.snapshot.lock().clone()
    }

    /// The last screen published, for a view drawn out of sight: unlike [`Self::snapshot`] it
    /// doesn't count as read, so the worker publishes no new one for it.
    pub fn peek(&self) -> Arc<TerminalSnapshot> {
        self.shared.snapshot.lock().clone()
    }

    /// Changes with every new screen and status, without reading either.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::Acquire)
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
        self.inbox.send(WorkerCmd::WriteRaw(bytes.into()));
    }

    pub fn send_key(&self, key: KeyInput) {
        self.inbox.send(WorkerCmd::Key(key));
    }

    pub fn send_mouse(&self, mouse: MouseInput) {
        self.inbox.send(WorkerCmd::Mouse(mouse));
    }

    pub fn paste(&self, text: impl Into<String>) {
        self.inbox.send(WorkerCmd::Paste(text.into()));
    }

    pub fn scroll(&self, op: ScrollOp) {
        self.inbox.send(WorkerCmd::Scroll(op));
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        self.inbox.send(WorkerCmd::Resize {
            cols,
            rows,
            cell_w_px,
            cell_h_px,
        });
    }

    pub fn select(&self, op: SelectionOp) {
        self.inbox.send(WorkerCmd::Select(op));
    }

    /// The user pressed on the pane: a shared session terminal comes under this window's
    /// control at its size (Desktop's `interacted`).
    pub fn interact(&self) {
        self.inbox.send(WorkerCmd::Interact);
    }

    /// Ctrl+Break (`CTRL_BREAK_EVENT`) for the program in a Windows console: unlike Ctrl+C a
    /// program can't ignore it. Nothing for a terminal that isn't a Windows console.
    pub fn send_break(&self) {
        self.inbox.send(WorkerCmd::Break);
    }

    /// Finds `needle` in the screen and scrollback, `None` or "" to stop; the snapshot's `find`
    /// shows the matches.
    pub fn find(&self, needle: Option<String>) {
        self.inbox.send(WorkerCmd::Find(needle));
    }

    /// The next match toward older output (`older`) or newer, scrolled into view.
    pub fn find_step(&self, older: bool) {
        self.inbox.send(WorkerCmd::FindStep(older));
    }

    /// The OSC 8 link of a viewport cell, asked of the worker (a click's wait: microseconds).
    pub fn hyperlink_at(&self, col: u16, row: u16) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        self.inbox.send(WorkerCmd::LinkAt { col, row, reply: tx }).then_some(())?;
        rx.recv_timeout(SHOW_WAIT).ok().flatten()
    }

    /// Drops the scrollback in this window's view, keeping the screen (ED 3).
    pub fn clear_scrollback(&self) {
        self.inbox.send(WorkerCmd::ClearScrollback);
    }

    /// The selection as HTML, its colours inline.
    pub fn selected_html(&self) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        self.inbox.send(WorkerCmd::SelectionHtml(tx)).then_some(())?;
        rx.recv_timeout(SHOW_WAIT).ok().flatten()
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
        self.inbox.send(WorkerCmd::SetTheme(theme));
    }

    pub fn persist_handle(&self) -> SessionPersistHandle {
        SessionPersistHandle {
            inbox: self.inbox.clone(),
        }
    }

    pub fn persist_snapshot(&self, timeout: Duration) -> Option<PersistedTerminal> {
        self.persist_handle().capture(timeout)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.inbox.send(WorkerCmd::Shutdown);
    }
}

/// Nanoseconds since the first call in this process, never 0.
fn monotonic_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    (EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64).max(1)
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

/// A worker's screen and what it owes the UI.
pub(crate) struct Screen {
    pub(crate) core: VtCore,
    pub(crate) publish: Publish,
    /// Whether the emulator's own title changes name the tab; a session terminal's come from
    /// the server (`title_changed`).
    titles: bool,
}

impl Screen {
    pub(crate) fn new(core: VtCore, titles: bool) -> Screen {
        Screen { core, publish: Publish::Current, titles }
    }

    /// The emulator's events the UI hears (`titles` decides the title's).
    fn take_events(&mut self) -> Vec<VtEvent> {
        let mut events = self.core.take_events();
        if !self.titles {
            events.retain(|e| !matches!(e, VtEvent::TitleChanged(_)));
        }
        events
    }

    /// Steps the publish machine and carries out its action. Events the UI needs whatever it
    /// draws (bell, title, folder) reach it even when no snapshot goes.
    pub(crate) fn step(&mut self, event: publish::Event, shared: &SharedState, waker: &Waker) {
        let (next, action) = self.publish.step(event, Instant::now(), &shared.handoff);
        self.publish = next;
        let events = self.take_events();
        let has_events = !events.is_empty();
        if has_events {
            shared.events.lock().extend(events);
        }
        match action {
            publish::Action::Publish => self.extract(shared, waker),
            publish::Action::Nothing if has_events => waker(),
            publish::Action::Nothing => {}
        }
    }

    fn extract(&mut self, shared: &SharedState, waker: &Waker) {
        self.core.clear_render_hold();
        let snap = self.core.snapshot();
        if !shared.revealed.load(Ordering::Acquire) && snap.screen_rows.iter().any(|r| !r.trimmed_text().is_empty()) {
            shared.revealed.store(true, Ordering::Release);
        }
        *shared.snapshot.lock() = snap;
        shared.generation.fetch_add(1, Ordering::Release);
        waker();
    }

    /// Publishes the final screen of a worker that is ending, read or not.
    pub(crate) fn last(&mut self, shared: &SharedState, waker: &Waker) {
        let events = self.take_events();
        if !events.is_empty() {
            shared.events.lock().extend(events);
        }
        shared.handoff.published();
        self.publish = Publish::Current;
        self.extract(shared, waker);
    }

    /// Feeds a program's output, stepping the publish machine at the end of each synchronized
    /// frame (`?2026l`) inside it: a batch that ends mid-way through the next frame would
    /// otherwise leave the screen held, and a program drawing back to back showed nothing until
    /// the hold timed out (1 s stalls redrawing a full screen at 60 Hz).
    pub(crate) fn feed(&mut self, mut bytes: &[u8], shared: &SharedState, waker: &Waker) {
        const END: &[u8] = b"\x1b[?2026l";
        while let Some(at) = find_frame_end(bytes, END) {
            let (frame, rest) = bytes.split_at(at + END.len());
            self.core.feed(frame);
            if !self.core.render_held() {
                self.changed(shared, waker);
            }
            bytes = rest;
        }
        self.core.feed(bytes);
    }

    /// The screen changed.
    pub(crate) fn changed(&mut self, shared: &SharedState, waker: &Waker) {
        let held = self.core.render_held();
        self.step(publish::Event::Changed { held }, shared, waker);
    }

    /// The worker woke without new output.
    pub(crate) fn woke(&mut self, shared: &SharedState, waker: &Waker) {
        let held = self.core.render_held();
        self.step(publish::Event::Woke { held }, shared, waker);
    }

    /// Publishes now for a reader that waits (`WorkerCmd::Show`), unless the program holds its frame.
    pub(crate) fn show(&mut self, shared: &SharedState, waker: &Waker, reply: Sender<()>) {
        if self.publish == Publish::Behind {
            self.woke(shared, waker);
        }
        let _ = reply.send(());
    }
}

fn find_frame_end(bytes: &[u8], end: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = bytes[from..].iter().position(|&b| b == 0x1b) {
        let at = from + i;
        if bytes[at..].starts_with(end) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Applies a command that changes the emulator; `false` ends the worker. Encoded input goes to
/// `out_bytes`, a resize to `resized`; `dirty` says the screen changed.
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
        WorkerCmd::Key(key) => out_bytes.extend_from_slice(&core.encode_key(&key)),
        WorkerCmd::Mouse(mouse) => out_bytes.extend_from_slice(&core.encode_mouse(mouse)),
        WorkerCmd::Paste(text) => out_bytes.extend_from_slice(&core.encode_paste(&text)),
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
        WorkerCmd::Output(chunk) => {
            shared.cursor.fetch_add(chunk.len() as i64, Ordering::AcqRel);
            core.feed(&chunk);
            out_bytes.extend_from_slice(&core.take_pty_writes());
            *dirty = true;
        }
        WorkerCmd::Exit(code) => {
            shared.set_status(SessionStatus::Exited { code });
            *dirty = true;
        }
        WorkerCmd::Find(needle) => {
            core.find(needle.as_deref());
            *dirty = true;
        }
        WorkerCmd::FindStep(older) => {
            core.find_step(older);
            *dirty = true;
        }
        WorkerCmd::LinkAt { col, row, reply } => {
            let _ = reply.send(core.hyperlink_at(col, row));
        }
        WorkerCmd::SelectionHtml(reply) => {
            let _ = reply.send(core.selected_html());
        }
        WorkerCmd::ClearScrollback => {
            core.feed(b"\x1b[3J");
            *dirty = true;
        }
        WorkerCmd::Interact | WorkerCmd::Break | WorkerCmd::Nudge | WorkerCmd::Show(_) => {}
        WorkerCmd::Shutdown => return false,
    }
    true
}

fn run_stream_worker(
    config: TerminalConfig,
    restore: Option<RestoreState>,
    rx: Receiver<WorkerCmd>,
    shared: Arc<SharedState>,
    written: Arc<Mutex<Vec<u8>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    waker: Waker,
) {
    let Ok(core) = build_core(&config, restore.as_ref()) else {
        return;
    };
    let mut screen = Screen::new(core, true);

    loop {
        // Asleep until a command, output, or the end of a program's frame hold.
        let first = match screen.publish.deadline() {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => return,
            },
        };

        let mut dirty = false;
        let mut out_bytes = Vec::new();
        let mut resized = None;
        let mut show = None;
        let burst = Instant::now();
        let mut fed = 0usize;
        let mut next = first;
        while let Some(cmd) = next.take() {
            if let WorkerCmd::Output(chunk) = &cmd {
                fed += chunk.len();
            }
            match cmd {
                WorkerCmd::Show(reply) => show = Some(reply),
                WorkerCmd::Output(chunk) => {
                    shared.cursor.fetch_add(chunk.len() as i64, Ordering::AcqRel);
                    screen.feed(&chunk, &shared, &waker);
                    out_bytes.extend_from_slice(&screen.core.take_pty_writes());
                    dirty = true;
                }
                cmd => {
                    if !apply_worker_cmd(cmd, &mut screen.core, &shared, &mut out_bytes, &mut resized, &mut dirty) {
                        return;
                    }
                }
            }
            if fed >= COALESCE_MAX_BYTES || burst.elapsed() >= COALESCE_BUDGET {
                break;
            }
            next = match rx.try_recv() {
                Ok(cmd) => Some(cmd),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            };
        }

        if let Some(sz) = resized {
            resizes.lock().push(sz);
        }
        if !out_bytes.is_empty() {
            written.lock().extend_from_slice(&out_bytes);
        }
        if dirty {
            screen.changed(&shared, &waker);
        } else {
            screen.woke(&shared, &waker);
        }
        if let Some(reply) = show {
            screen.show(&shared, &waker, reply);
        }
    }
}
