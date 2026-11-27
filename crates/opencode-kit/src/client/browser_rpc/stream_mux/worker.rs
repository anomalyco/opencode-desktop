//! Single-writer non-blocking WebSocket I/O worker for [`super::ClientMux`].

use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use rune::time::Instant;
use tungstenite::protocol::CloseFrame;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Error as WsError, Message, WebSocket};

use super::super::stream_wire::{MAX_STREAM_WINDOW_BYTES, Frame, ResetCode, decode_frame, encode_frame};
use super::super::types::{CLOSE_ATTACHMENT_CLOSED, CLOSE_ATTACHMENT_REPLACED, MuxCloseInfo};
use super::{ClientMux, StreamStatus};

const MAX_READS_PER_TURN: usize = 64;
const MAX_WRITES_PER_TURN: usize = 16;

fn tcp_ref(ws: &WebSocket<MaybeTlsStream<TcpStream>>) -> &TcpStream {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        #[allow(unreachable_patterns)]
        _ => unreachable!(),
    }
}

pub(super) fn run_mux_loop(
    mux: Arc<ClientMux>,
    mut ws: WebSocket<MaybeTlsStream<TcpStream>>,
    on_closed: Box<dyn FnOnce(MuxCloseInfo) + Send + 'static>,
) {
    tcp_ref(&ws).set_nodelay(true).ok();
    tcp_ref(&ws).set_nonblocking(true).ok();
    // SAFETY: `poller.delete(tcp_ref(&ws))` runs unconditionally before `ws` is dropped.
    #[allow(unsafe_code)]
    let added = unsafe { mux.poller.add(tcp_ref(&ws), polling::Event::readable(0)) };
    if let Err(e) = added {
        let info = finish_and_notify(&mux, 1006, format!("Poller failed: {e}"));
        on_closed(info);
        return;
    }

    let (hb_interval, hb_timeout, max_ws_buf) = {
        let g = mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        (g.cfg.heartbeat_interval, g.cfg.heartbeat_timeout, g.cfg.max_ws_buffered_bytes)
    };
    let mut last_heard = Instant::now();
    let mut next_ping = Instant::now() + hb_interval;
    let mut pending_ws_bytes: usize = 0;
    let mut events = polling::Events::new();
    let mut exit_reason: Option<(u16, String, bool)> = None;

    while exit_reason.is_none() {
        let now = Instant::now();
        if let Some((code, reason, send_goaway)) = {
            mux.state.lock().unwrap_or_else(PoisonError::into_inner).pending_close.take()
        } {
            if send_goaway {
                if let Ok(bytes) = encode_frame(&Frame::Goaway {
                    code: ResetCode::Cancelled,
                    reason: reason.clone(),
                }) {
                    let _ = ws.write(Message::Binary(bytes.into()));
                }
                let _ = ws.close(Some(CloseFrame {
                    code: CloseCode::from(code),
                    reason: reason.chars().take(120).collect::<String>().into(),
                }));
                let _ = ws.flush();
            }
            exit_reason = Some((code, reason, false));
            break;
        }
        if !hb_interval.is_zero() {
            if now.saturating_duration_since(last_heard) > hb_timeout {
                exit_reason = Some((1001, "Browser tunnel heartbeat timed out.".into(), true));
                break;
            }
            if now >= next_ping {
                next_ping = now + hb_interval;
                let _ = ws.write(Message::Ping(Vec::new().into()));
                let mut token = [0u8; 8];
                let _ = getrandom::fill(&mut token);
                mux.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .enqueue_control(&Frame::Ping { token });
            }
        }

        let mut wrote_any = false;
        let mut writes_done = 0usize;
        while writes_done < MAX_WRITES_PER_TURN {
            let next_frame = {
                let mut g = mux.state.lock().unwrap_or_else(PoisonError::into_inner);
                if !g.control_queue.is_empty() && pending_ws_bytes < max_ws_buf * 2 {
                    g.control_queue.pop_front()
                } else if pending_ws_bytes < max_ws_buf {
                    g.pop_next_data_frame()
                } else {
                    None
                }
            };
            let Some(frame_bytes) = next_frame else { break };
            let len = frame_bytes.len();
            if let Err(e) = ws.write(Message::Binary(frame_bytes.into())) {
                exit_reason = Some((1006, format!("Browser tunnel write failed: {e}"), true));
                break;
            }
            pending_ws_bytes = pending_ws_bytes.saturating_add(len);
            wrote_any = true;
            writes_done += 1;
        }
        if exit_reason.is_some() {
            break;
        }
        if wrote_any {
            mux.cv.notify_all();
        }
        let mut write_blocked = false;
        if pending_ws_bytes > 0 || wrote_any {
            match ws.flush() {
                Ok(()) => pending_ws_bytes = 0,
                Err(WsError::Io(ref e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    write_blocked = true;
                }
                Err(e) => {
                    exit_reason = Some((1006, format!("Browser tunnel flush failed: {e}"), true));
                    break;
                }
            }
        }
        let has_more_writes = !write_blocked && writes_done >= MAX_WRITES_PER_TURN;

        let mut reads_done = 0usize;
        while reads_done < MAX_READS_PER_TURN {
            match ws.read() {
                Ok(Message::Binary(bytes)) => {
                    last_heard = Instant::now();
                    reads_done += 1;
                    if let Some(err) = handle_inbound_binary(&mux, &bytes) {
                        exit_reason = Some(err);
                        break;
                    }
                }
                Ok(Message::Text(_)) => {
                    let reason = "Server sent text frame on binary browser stream tunnel.".to_owned();
                    send_goaway_protocol_error(&mut ws, &reason);
                    exit_reason = Some((1002, reason, true));
                    break;
                }
                Ok(Message::Ping(_)) => {
                    last_heard = Instant::now();
                    reads_done += 1;
                    let _ = ws.flush();
                }
                Ok(Message::Pong(_) | Message::Frame(_)) => {
                    last_heard = Instant::now();
                    reads_done += 1;
                }
                Ok(Message::Close(frame)) => {
                    let (code, reason) = match frame {
                        Some(cf) => (u16::from(cf.code), cf.reason.to_string()),
                        None => (1000, "Browser tunnel closed.".into()),
                    };
                    exit_reason = Some((code, reason, false));
                    break;
                }
                Err(WsError::Io(ref e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    break;
                }
                Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                    exit_reason = Some((1000, "Browser tunnel closed.".into(), false));
                    break;
                }
                Err(e) => {
                    exit_reason = Some((1006, format!("Browser tunnel socket error: {e}"), true));
                    break;
                }
            }
        }
        if exit_reason.is_some() {
            break;
        }
        if reads_done > 0 {
            mux.cv.notify_all();
        }
        if reads_done >= MAX_READS_PER_TURN || has_more_writes {
            continue;
        }
        let queued_now = {
            let g = mux.state.lock().unwrap_or_else(PoisonError::into_inner);
            g.pending_close.is_some()
                || (!g.control_queue.is_empty() && pending_ws_bytes < max_ws_buf * 2)
                || (!g.round_robin.is_empty() && pending_ws_bytes < max_ws_buf)
        };
        if queued_now && !write_blocked {
            continue;
        }

        events.clear();
        let interest = if write_blocked {
            polling::Event::all(0)
        } else {
            polling::Event::readable(0)
        };
        if mux.poller.modify(tcp_ref(&ws), interest).is_err() {
            exit_reason = Some((1006, "Browser tunnel poller failed.".into(), true));
            break;
        }
        let wait_dur = if hb_interval.is_zero() {
            None
        } else {
            let now = Instant::now();
            let until_ping = next_ping.saturating_duration_since(now);
            let until_timeout = (last_heard + hb_timeout).saturating_duration_since(now);
            Some(until_ping.min(until_timeout).max(Duration::from_millis(10)))
        };
        if let Err(e) = mux.poller.wait(&mut events, wait_dur)
            && e.kind() != ErrorKind::Interrupted
        {
            exit_reason = Some((1006, format!("Browser tunnel poller wait failed: {e}"), true));
            break;
        }
    }

    let _ = mux.poller.delete(tcp_ref(&ws));
    let (code, reason, terminate) = exit_reason.unwrap_or((1000, "Browser tunnel closed.".into(), false));
    if !terminate {
        tcp_ref(&ws).set_nonblocking(false).ok();
        tcp_ref(&ws).set_write_timeout(Some(Duration::from_millis(250))).ok();
        let _ = ws.close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.chars().take(120).collect::<String>().into(),
        }));
        let _ = ws.flush();
    }
    let info = finish_and_notify(&mux, code, reason);
    on_closed(info);
}

fn send_goaway_protocol_error(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, reason: &str) {
    if let Ok(bytes) = encode_frame(&Frame::Goaway {
        code: ResetCode::ProtocolError,
        reason: reason.to_owned(),
    }) {
        let _ = ws.write(Message::Binary(bytes.into()));
        let _ = ws.flush();
    }
}

fn finish_and_notify(mux: &ClientMux, code: u16, reason: String) -> MuxCloseInfo {
    let info = {
        let mut g = mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        g.finish_shutdown(code, reason.clone()).unwrap_or(MuxCloseInfo {
            code,
            replaced: code == CLOSE_ATTACHMENT_REPLACED || reason == "replaced",
            reason,
        })
    };
    mux.cv.notify_all();
    info
}

fn handle_inbound_binary(mux: &ClientMux, raw: &[u8]) -> Option<(u16, String, bool)> {
    let frame = match decode_frame(raw) {
        Ok(f) => f,
        Err(e) => {
            let msg = e.to_string();
            mux.state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .enqueue_control(&Frame::Goaway { code: ResetCode::ProtocolError, reason: msg.clone() });
            return Some((1002, msg, true));
        }
    };
    let mut g = mux.state.lock().unwrap_or_else(PoisonError::into_inner);
    match frame {
        Frame::Opened { stream_id, initial_window } => {
            if let Some(s) = g.streams.get_mut(&stream_id)
                && s.status == StreamStatus::Opening
            {
                s.status = StreamStatus::Open;
                s.send_window = initial_window;
                g.pending_connects = g.pending_connects.saturating_sub(1);
            }
        }
        Frame::Data { stream_id, data } => {
            let Some((status, fin_rcvd, recv_win)) = g
                .streams
                .get(&stream_id)
                .map(|s| (s.status, s.inbound_fin_received, s.recv_window))
            else {
                return None;
            };
            if status != StreamStatus::Open {
                return None;
            }
            if fin_rcvd {
                g.cleanup_stream(
                    stream_id,
                    Some("Server sent DATA after FIN.".into()),
                    Some((ResetCode::ProtocolError, "Server sent DATA after FIN.".into())),
                );
                return None;
            }
            let len = data.len();
            if len as u32 > recv_win {
                g.cleanup_stream(
                    stream_id,
                    Some("Server exceeded stream receive window.".into()),
                    Some((ResetCode::ProtocolError, "Server exceeded stream receive window.".into())),
                );
                return None;
            }
            if !g.reserve_bytes(len) {
                g.cleanup_stream(
                    stream_id,
                    Some("Attachment receive buffer exceeded.".into()),
                    Some((ResetCode::LimitExceeded, "Attachment receive buffer exceeded.".into())),
                );
                return None;
            }
            if let Some(s) = g.streams.get_mut(&stream_id) {
                s.recv_window -= len as u32;
                s.inbound_chunks.push_back(data);
                s.inbound_bytes += len;
            }
        }
        Frame::WindowUpdate { stream_id, delta } => {
            let Some((status, send_win)) = g.streams.get(&stream_id).map(|s| (s.status, s.send_window)) else {
                return None;
            };
            if status != StreamStatus::Open {
                return None;
            }
            if u64::from(send_win) + u64::from(delta) > u64::from(MAX_STREAM_WINDOW_BYTES) {
                g.cleanup_stream(
                    stream_id,
                    Some("Server sent overflowing WINDOW_UPDATE.".into()),
                    Some((ResetCode::ProtocolError, "Server sent overflowing WINDOW_UPDATE.".into())),
                );
                return None;
            }
            if let Some(s) = g.streams.get_mut(&stream_id) {
                s.send_window += delta;
            }
            g.mark_writable(stream_id);
        }
        Frame::Fin { stream_id } => {
            if let Some(s) = g.streams.get_mut(&stream_id)
                && s.status == StreamStatus::Open
            {
                s.inbound_fin_received = true;
                g.maybe_finish_stream(stream_id);
            }
        }
        Frame::Reset { stream_id, code, reason } => {
            let msg = if reason.is_empty() { code.message().to_owned() } else { reason };
            g.cleanup_stream(stream_id, Some(msg), None);
        }
        Frame::Ping { token } => {
            g.enqueue_control(&Frame::Pong { token });
        }
        Frame::Pong { .. } => {}
        Frame::Goaway { reason, .. } => {
            let code = if reason == "replaced" {
                CLOSE_ATTACHMENT_REPLACED
            } else {
                CLOSE_ATTACHMENT_CLOSED
            };
            let msg = if reason.is_empty() {
                "Browser tunnel closed by server.".to_owned()
            } else {
                reason
            };
            return Some((code, msg, false));
        }
        Frame::Open { .. } => {
            let msg = "Server may not send OPEN frames.".to_owned();
            g.enqueue_control(&Frame::Goaway { code: ResetCode::ProtocolError, reason: msg.clone() });
            return Some((1002, msg, true));
        }
    }
    None
}
