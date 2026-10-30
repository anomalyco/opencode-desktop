//! OpenCode server `/api/pty` WebSocket worker loop (`packages/server/src/handlers/pty.ts`,
//! `packages/gui-extensions/src/terminal/terminal.tsx`).

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use opencode_kit::client::Server;
use opencode_kit::pty::PtyStatus;
use rune::Waker;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::core::VtCore;
use crate::ffi::GHOSTTY_MODE_COLOR_SCHEME_REPORT;
use crate::pty::{
    COALESCE_BUDGET, COALESCE_MAX_BYTES, PersistedTerminal, RENDER_HOLD_TIMEOUT, RestoreState,
    SessionStatus, SharedState, TerminalConfig, WorkerCmd, apply_worker_cmd, build_core,
    publish_if_ready,
};

const RESIZE_DEBOUNCE: Duration = Duration::from_millis(100);

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_server_worker(
    server: Server,
    directory: String,
    pty_id: String,
    config: TerminalConfig,
    restore: Option<RestoreState>,
    cmd_rx: Receiver<WorkerCmd>,
    shared: Arc<SharedState>,
    waker: Waker,
) {
    let Ok(mut core) = build_core(&config, restore.as_ref()) else {
        return;
    };

    let mut seek: i64 = match restore.as_ref() {
        Some(r) if r.cursor.is_some() => r.cursor.unwrap_or(0),
        Some(r) if !r.buffer.is_empty() => -1,
        _ => 0,
    };

    let mut current_size = (config.cols.max(1), config.rows.max(1));
    let mut last_pushed_size: Option<(u16, u16)> = None;
    let mut pending_resize_at: Option<Instant> = None;
    let mut hold_started: Option<Instant> = None;
    let mut tries: u32 = 0;

    loop {
        let url = match server.pty_connect_url(&directory, &pty_id, Some(seek)) {
            Ok(u) => u,
            Err(err) => {
                if !handle_reconnect_or_exit(
                    &server,
                    &directory,
                    &pty_id,
                    &cmd_rx,
                    &mut core,
                    &shared,
                    &waker,
                    &mut tries,
                    err,
                ) {
                    return;
                }
                continue;
            }
        };

        let mut ws = match tungstenite::connect(&url) {
            Ok((socket, _)) => socket,
            Err(err) => {
                if !handle_reconnect_or_exit(
                    &server,
                    &directory,
                    &pty_id,
                    &cmd_rx,
                    &mut core,
                    &shared,
                    &waker,
                    &mut tries,
                    err.to_string(),
                ) {
                    return;
                }
                continue;
            }
        };

        set_socket_read_timeout(&ws, Some(Duration::from_millis(3)));
        tries = 0;
        *shared.status.lock() = SessionStatus::Connected;
        waker();

        if last_pushed_size != Some(current_size) {
            let _ = server.pty_update(&directory, &pty_id, None, Some(current_size));
            last_pushed_size = Some(current_size);
        }

        if core.mode_enabled(GHOSTTY_MODE_COLOR_SCHEME_REPORT) {
            core.feed(b"\x1b[?996n");
            flush_pty_replies(&mut core, &mut ws);
        }

        let mut abnormal_close: Option<String> = None;
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
                            let _ = ws.close(Some(tungstenite::protocol::CloseFrame {
                                code: CloseCode::Normal,
                                reason: "".into(),
                            }));
                            let _ = ws.flush();
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = ws.close(None);
                        return;
                    }
                }
            }

            if let Some(sz) = resized {
                current_size = sz;
                if last_pushed_size.is_none() {
                    let _ = server.pty_update(&directory, &pty_id, None, Some(sz));
                    last_pushed_size = Some(sz);
                } else if last_pushed_size != Some(sz) {
                    pending_resize_at = Some(Instant::now());
                }
            }

            if let Some(t0) = pending_resize_at
                && t0.elapsed() >= RESIZE_DEBOUNCE
            {
                pending_resize_at = None;
                if last_pushed_size != Some(current_size) {
                    let _ = server.pty_update(&directory, &pty_id, None, Some(current_size));
                    last_pushed_size = Some(current_size);
                }
            }

            if !out_bytes.is_empty() {
                let msg = match String::from_utf8(out_bytes) {
                    Ok(text) => Message::Text(text.into()),
                    Err(e) => Message::Binary(e.into_bytes().into()),
                };
                if let Err(e) = ws.send(msg) {
                    abnormal_close = Some(e.to_string());
                    break;
                }
            }

            let burst_start = Instant::now();
            let mut fed_bytes = 0usize;
            loop {
                match ws.read() {
                    Ok(Message::Text(text)) => {
                        let bytes = text.as_bytes();
                        if !bytes.is_empty() {
                            fed_bytes += bytes.len();
                            let next = shared
                                .cursor
                                .fetch_add(bytes.len() as i64, Ordering::AcqRel)
                                + bytes.len() as i64;
                            seek = next;
                            core.feed(bytes);
                            flush_pty_replies(&mut core, &mut ws);
                            dirty = true;
                        }
                        if fed_bytes >= COALESCE_MAX_BYTES
                            || burst_start.elapsed() >= COALESCE_BUDGET
                        {
                            break;
                        }
                    }
                    Ok(Message::Binary(bin)) => {
                        if bin.first() == Some(&0) {
                            if let Some(next_cursor) = parse_control_cursor(&bin[1..]) {
                                shared.cursor.store(next_cursor, Ordering::Release);
                                seek = next_cursor;
                            }
                        } else if !bin.is_empty() {
                            fed_bytes += bin.len();
                            let next = shared
                                .cursor
                                .fetch_add(bin.len() as i64, Ordering::AcqRel)
                                + bin.len() as i64;
                            seek = next;
                            core.feed(&bin);
                            flush_pty_replies(&mut core, &mut ws);
                            dirty = true;
                        }
                        if fed_bytes >= COALESCE_MAX_BYTES
                            || burst_start.elapsed() >= COALESCE_BUDGET
                        {
                            break;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        let code = frame.as_ref().map_or(1000u16, |f| u16::from(f.code));
                        if code == 1000 || code == 4404 {
                            *shared.status.lock() = SessionStatus::Exited { code: None };
                            publish_if_ready(&mut core, &shared, &waker, &mut hold_started);
                            return;
                        }
                        abnormal_close = Some(format!("websocket closed ({code})"));
                        break;
                    }
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                        ) =>
                    {
                        break;
                    }
                    Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                        abnormal_close = Some("websocket connection closed".into());
                        break;
                    }
                    Err(e) => {
                        abnormal_close = Some(e.to_string());
                        break;
                    }
                }
            }

            if dirty || hold_started.is_some_and(|t| t.elapsed() >= RENDER_HOLD_TIMEOUT) {
                publish_if_ready(&mut core, &shared, &waker, &mut hold_started);
            }

            if abnormal_close.is_some() {
                break;
            }
        }

        if let Some(err) = abnormal_close
            && !handle_reconnect_or_exit(
                &server,
                &directory,
                &pty_id,
                &cmd_rx,
                &mut core,
                &shared,
                &waker,
                &mut tries,
                err,
            )
        {
            return;
        }
    }
}

fn flush_pty_replies(core: &mut VtCore, ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) {
    let replies = core.take_pty_writes();
    if replies.is_empty() {
        return;
    }
    let msg = match String::from_utf8(replies) {
        Ok(s) => Message::Text(s.into()),
        Err(e) => Message::Binary(e.into_bytes().into()),
    };
    let _ = ws.send(msg);
}

pub fn parse_control_cursor(json_bytes: &[u8]) -> Option<i64> {
    let v: serde_json::Value = serde_json::from_slice(json_bytes).ok()?;
    v["cursor"].as_i64().filter(|&c| c >= 0)
}

#[allow(clippy::too_many_arguments)]
fn handle_reconnect_or_exit(
    server: &Server,
    directory: &str,
    pty_id: &str,
    cmd_rx: &Receiver<WorkerCmd>,
    core: &mut VtCore,
    shared: &SharedState,
    waker: &Waker,
    tries: &mut u32,
    err: String,
) -> bool {
    match server.pty_get(directory, pty_id) {
        Ok(info) if info.status == PtyStatus::Exited => {
            *shared.status.lock() = SessionStatus::Exited {
                code: info.exit_code,
            };
            waker();
            return false;
        }
        Err(e) if e.contains("404") || e.contains("NotFound") => {
            *shared.status.lock() = SessionStatus::Exited { code: None };
            waker();
            return false;
        }
        _ => {}
    }

    if *tries >= 6 {
        *shared.status.lock() = SessionStatus::Failed(err);
        waker();
        return false;
    }

    *tries += 1;
    *shared.status.lock() = SessionStatus::Reconnecting { attempt: *tries };
    waker();

    let backoff_ms = (250u64 << (*tries - 1).min(4)).min(4_000);
    let deadline = Instant::now() + Duration::from_millis(backoff_ms);
    while Instant::now() < deadline {
        match cmd_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(WorkerCmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Ok(WorkerCmd::Persist(reply)) => {
                let snap = shared.snapshot.lock().clone();
                let _ = reply.send(PersistedTerminal {
                    buffer: core.serialize_vt(),
                    cols: snap.cols,
                    rows: snap.rows,
                    scroll_y: snap.scrollbar.offset,
                    cursor: shared.cursor.load(Ordering::Acquire),
                });
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    true
}

fn set_socket_read_timeout(ws: &WebSocket<MaybeTlsStream<TcpStream>>, dur: Option<Duration>) {
    match ws.get_ref() {
        MaybeTlsStream::Plain(tcp) => {
            let _ = tcp.set_read_timeout(dur);
        }
        MaybeTlsStream::Rustls(tls) => {
            let _ = tls.get_ref().set_read_timeout(dur);
        }
        _ => {}
    }
}
