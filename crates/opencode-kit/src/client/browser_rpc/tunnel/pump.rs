//! Bidirectional stream & WebSocket upgrade pumping helpers for [`super::proxy::TunnelProxy`].

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use super::super::framing::{is_http_101_response, write_status};
use super::super::http::AbortHandle;
use super::super::proxy_machine::{ReadEvent, ReadState};
use super::proxy::{MAX_WS_RESP_HEAD, POST_EOF_DRAIN_TIMEOUT, ProxyCtx};
use super::stream::UpstreamStream;
use super::super::types::TUNNEL_CHUNK_BYTES;

pub(super) enum WsHandshake {
    Upgraded,
    RejectedNot101,
    Closed,
}

pub(super) fn pump_bidirectional_stream(
    ctx: &ProxyCtx,
    client_id: u64,
    upstream: &UpstreamStream,
    prebuffered: &[u8],
    client_read: &mut TcpStream,
    abort: &AbortHandle,
) -> bool {
    if !prebuffered.is_empty() && upstream.write_all(prebuffered, abort).is_err() {
        return false;
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    loop {
        match client_read.read(&mut buf) {
            Ok(0) => {
                if ctx.is_expired(client_id) {
                    return false;
                }
                let _ = upstream.finish_write(abort);
                return true;
            }
            Ok(n) => {
                if upstream.write_all(&buf[..n], abort).is_err() {
                    return false;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                if ctx.is_expired(client_id) || abort.is_aborted() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

pub(super) fn await_ws_upgrade(
    upstream: &UpstreamStream,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
) -> WsHandshake {
    let mut buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 8192];
    loop {
        let Ok(n) = upstream.read(&mut tmp, abort) else {
            return WsHandshake::Closed;
        };
        if n > 0 {
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = &buf[..pos];
                let ok101 = is_http_101_response(head);
                if client_write.write_all(&buf).is_err() {
                    return WsHandshake::Closed;
                }
                return if ok101 { WsHandshake::Upgraded } else { WsHandshake::RejectedNot101 };
            }
            if buf.len() > MAX_WS_RESP_HEAD {
                let _ = write_status(client_write, "502 Bad Gateway");
                return WsHandshake::Closed;
            }
        } else {
            if !buf.is_empty() {
                let _ = client_write.write_all(&buf);
            }
            client_write.shutdown(Shutdown::Write).ok();
            return WsHandshake::Closed;
        }
    }
}

pub(super) fn drain_remote_to_client(
    ctx: &ProxyCtx,
    client_id: u64,
    upstream: &UpstreamStream,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
) {
    let mut state = ReadState::Idle;
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    while state.step(ReadEvent::Pull) {
        match upstream.read(&mut buf, abort) {
            Ok(0) => {
                state.step(ReadEvent::Eof);
                client_write.shutdown(Shutdown::Write).ok();
                ctx.arm_post_eof(client_id, POST_EOF_DRAIN_TIMEOUT);
                break;
            }
            Ok(n) => {
                if client_write.write_all(&buf[..n]).is_err() {
                    state.step(ReadEvent::Destroyed);
                    client_write.shutdown(Shutdown::Both).ok();
                    ctx.arm_post_eof(client_id, Duration::ZERO);
                    break;
                }
                state.step(ReadEvent::Chunk);
            }
            Err(_) => {
                state.step(ReadEvent::Destroyed);
                client_write.shutdown(Shutdown::Both).ok();
                ctx.arm_post_eof(client_id, Duration::ZERO);
                break;
            }
        }
    }
}
