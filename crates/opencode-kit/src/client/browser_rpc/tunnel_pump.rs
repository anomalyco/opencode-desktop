//! Request/response & bidirectional stream pumping helpers for [`super::tunnel_proxy::TunnelProxy`].

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use super::framing::{is_http_101_response, write_status};
use super::http::AbortHandle;
use super::proxy_machine::{ReadEvent, ReadState};
use super::tunnel_proxy::{
    BODY_IDLE_TIMEOUT, MAX_WS_RESP_HEAD, POST_EOF_DRAIN_TIMEOUT, ProxyCtx, STREAM_IDLE_TIMEOUT,
};
use super::types::TUNNEL_CHUNK_BYTES;

pub(super) enum WsHandshake {
    Upgraded,
    RejectedNot101,
    Closed,
}

pub(super) fn write_all_chunks(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    data: &[u8],
    end: bool,
    abort: &AbortHandle,
) -> Result<(), ()> {
    if data.is_empty() {
        return ctx
            .server
            .browser_rpc_tunnel_write_abortable(
                &ctx.location,
                &ctx.session_id,
                &ctx.connection_id,
                tunnel_id,
                &[],
                end,
                abort,
            )
            .map_err(|_| ());
    }
    let total = data.chunks(TUNNEL_CHUNK_BYTES).len();
    for (idx, chunk) in data.chunks(TUNNEL_CHUNK_BYTES).enumerate() {
        let is_last = end && idx + 1 == total;
        ctx.server
            .browser_rpc_tunnel_write_abortable(
                &ctx.location,
                &ctx.session_id,
                &ctx.connection_id,
                tunnel_id,
                chunk,
                is_last,
                abort,
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

pub(super) fn pump_exact_http_body(
    ctx: &ProxyCtx,
    client_id: u64,
    tunnel_id: &str,
    prebuffered: &[u8],
    client_read: &mut TcpStream,
    content_length: u64,
    abort: &AbortHandle,
) -> bool {
    let mut remaining = content_length;
    if remaining > 0 && !prebuffered.is_empty() {
        let take = (prebuffered.len() as u64).min(remaining) as usize;
        remaining -= take as u64;
        ctx.bump_idle(client_id, BODY_IDLE_TIMEOUT);
        if write_all_chunks(ctx, tunnel_id, &prebuffered[..take], remaining == 0, abort).is_err() {
            return false;
        }
        if remaining == 0 {
            return true;
        }
    }
    if remaining == 0 {
        return write_all_chunks(ctx, tunnel_id, &[], true, abort).is_ok();
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    while remaining > 0 {
        let want = (buf.len() as u64).min(remaining) as usize;
        match client_read.read(&mut buf[..want]) {
            Ok(0) => return false,
            Ok(n) => {
                ctx.bump_idle(client_id, BODY_IDLE_TIMEOUT);
                remaining -= n as u64;
                if write_all_chunks(ctx, tunnel_id, &buf[..n], remaining == 0, abort).is_err() {
                    return false;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                if ctx.is_expired(client_id) {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

pub(super) fn pump_bidirectional_stream(
    ctx: &ProxyCtx,
    client_id: u64,
    tunnel_id: &str,
    prebuffered: &[u8],
    client_read: &mut TcpStream,
    abort: &AbortHandle,
) -> bool {
    if !prebuffered.is_empty() && write_all_chunks(ctx, tunnel_id, prebuffered, false, abort).is_err() {
        return false;
    }
    let mut buf = vec![0u8; TUNNEL_CHUNK_BYTES];
    loop {
        match client_read.read(&mut buf) {
            Ok(0) => {
                if ctx.is_expired(client_id) {
                    return false;
                }
                let _ = write_all_chunks(ctx, tunnel_id, &[], true, abort);
                return true;
            }
            Ok(n) => {
                ctx.bump_idle(client_id, STREAM_IDLE_TIMEOUT);
                if write_all_chunks(ctx, tunnel_id, &buf[..n], false, abort).is_err() {
                    return false;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                if ctx.is_expired(client_id) {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

pub(super) fn await_ws_upgrade(
    ctx: &ProxyCtx,
    tunnel_id: &str,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
) -> WsHandshake {
    let mut buf = Vec::with_capacity(512);
    loop {
        let Ok(chunk) = ctx.server.browser_rpc_tunnel_read(
            &ctx.location,
            &ctx.session_id,
            &ctx.connection_id,
            tunnel_id,
            abort,
        ) else {
            return WsHandshake::Closed;
        };
        if !chunk.data.is_empty() {
            buf.extend_from_slice(&chunk.data);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = &buf[..pos];
                let ok101 = is_http_101_response(head);
                if client_write.write_all(&buf).is_err() {
                    return WsHandshake::Closed;
                }
                if chunk.eof {
                    client_write.shutdown(Shutdown::Write).ok();
                    return WsHandshake::Closed;
                }
                return if ok101 { WsHandshake::Upgraded } else { WsHandshake::RejectedNot101 };
            }
            if buf.len() > MAX_WS_RESP_HEAD {
                let _ = write_status(client_write, "502 Bad Gateway");
                return WsHandshake::Closed;
            }
        }
        if chunk.eof {
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
    tunnel_id: &str,
    abort: &AbortHandle,
    client_write: &mut TcpStream,
    idle_timeout: Duration,
) {
    let mut state = ReadState::Idle;
    while state.step(ReadEvent::Pull) {
        match ctx.server.browser_rpc_tunnel_read(
            &ctx.location,
            &ctx.session_id,
            &ctx.connection_id,
            tunnel_id,
            abort,
        ) {
            Ok(chunk) => {
                if chunk.eof {
                    state.step(ReadEvent::Eof);
                    client_write.shutdown(Shutdown::Write).ok();
                    // Arm active watchdog kill (`Shutdown::Both` + abort) so stalled POST bodies
                    // and half-open CONNECT/WS clients release their slot in `<= 2 s`.
                    ctx.arm_post_eof(client_id, POST_EOF_DRAIN_TIMEOUT);
                    break;
                }
                if !chunk.data.is_empty() {
                    ctx.bump_idle(client_id, idle_timeout);
                    if client_write.write_all(&chunk.data).is_err() {
                        state.step(ReadEvent::Destroyed);
                        client_write.shutdown(Shutdown::Both).ok();
                        ctx.arm_post_eof(client_id, Duration::ZERO);
                        break;
                    }
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
