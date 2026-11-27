//! Binary multiplexed stream frame codec for remote browser traffic
//! (`packages/plugin-browser/src/stream-wire.ts`, opencode#66217).
//!
//! Versioned 10-byte big-endian frame envelope:
//! `[version: u8][type: u8][stream_id: u32 BE][length: u32 BE][payload: length bytes]`.
//! Zero base64 encoding; all frame lengths, stream IDs, ports, hostnames, window deltas, and UTF-8
//! reasons are strictly validated on encode and decode.

use super::types::STREAM_PROTOCOL_VERSION;

pub const FRAME_HEADER_BYTES: usize = 10;
/// Target payload size for outbound DATA frames: small enough for fair interleaving with HMR/control.
pub const FRAME_DATA_CHUNK_BYTES: usize = 16 * 1024;
/// Maximum payload accepted in any single frame.
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 64 * 1024;
/// Hard WebSocket maxPayload cap including header overhead.
pub const MAX_WS_FRAME_BYTES: usize = MAX_FRAME_PAYLOAD_BYTES + FRAME_HEADER_BYTES + 1024;
/// Default server-side per-stream receive window for uploads (1.5 MiB).
pub const DEFAULT_STREAM_WINDOW_BYTES: u32 = 1536 * 1024;
/// Default desktop-side per-stream receive window for downloads (512 KiB; derives a 128 KiB credit batch).
pub const DEFAULT_CLIENT_STREAM_WINDOW_BYTES: u32 = 512 * 1024;
/// Hard upper bound on any stream window to prevent credit overflow attacks (4 MiB).
pub const MAX_STREAM_WINDOW_BYTES: u32 = 4 * 1024 * 1024;
/// Bound WebSocket write buffer so control/HMR frames are never buried behind a bulk download (256 KiB).
pub const MAX_WS_BUFFERED_BYTES: usize = 256 * 1024;
/// Hard cap on total buffered bytes across all streams of one attachment (8 MiB).
pub const MAX_ATTACHMENT_BUFFERED_BYTES: usize = 8 * 1024 * 1024;
/// Hard cap on concurrent streams per attachment.
pub const MAX_STREAMS_PER_ATTACHMENT: usize = 128;
/// Hard cap on concurrent in-flight TCP connects per attachment.
pub const MAX_PENDING_CONNECTS_PER_ATTACHMENT: usize = 32;

pub const MAX_HOST_BYTES: usize = 253;
pub const MAX_REASON_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Open = 0x01,
    Opened = 0x02,
    Data = 0x03,
    WindowUpdate = 0x04,
    Fin = 0x05,
    Reset = 0x06,
    Ping = 0x07,
    Pong = 0x08,
    Goaway = 0x09,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum ResetCode {
    Cancelled = 0x0000,
    ConnectFailed = 0x0001,
    PolicyDenied = 0x0002,
    LimitExceeded = 0x0003,
    ProtocolError = 0x0004,
    AttachmentClosed = 0x0005,
    InternalError = 0x0006,
}

impl ResetCode {
    pub fn from_u16(raw: u16) -> Self {
        match raw {
            0x0000 => Self::Cancelled,
            0x0001 => Self::ConnectFailed,
            0x0002 => Self::PolicyDenied,
            0x0003 => Self::LimitExceeded,
            0x0004 => Self::ProtocolError,
            0x0005 => Self::AttachmentClosed,
            0x0006 => Self::InternalError,
            _ => Self::InternalError,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Cancelled => "Browser tunnel stream was cancelled.",
            Self::ConnectFailed => "Browser tunnel could not connect to the target.",
            Self::PolicyDenied => "Target address is blocked by browser egress policy.",
            Self::LimitExceeded => "Browser tunnel resource limit exceeded.",
            Self::ProtocolError => "Browser tunnel protocol error.",
            Self::AttachmentClosed => "Browser attachment was closed.",
            Self::InternalError => "Browser tunnel target socket error.",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Open { stream_id: u32, host: String, port: u16, initial_window: u32 },
    Opened { stream_id: u32, initial_window: u32 },
    Data { stream_id: u32, data: Vec<u8> },
    WindowUpdate { stream_id: u32, delta: u32 },
    Fin { stream_id: u32 },
    Reset { stream_id: u32, code: ResetCode, reason: String },
    Ping { token: [u8; 8] },
    Pong { token: [u8; 8] },
    Goaway { code: ResetCode, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireProtocolError(pub String);

impl std::fmt::Display for WireProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WireProtocolError {}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST_BYTES
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'%' | b'-'))
}

fn assert_stream_id(id: u32) -> Result<(), WireProtocolError> {
    if id == 0 {
        return Err(WireProtocolError(format!("Invalid streamID: {id}.")));
    }
    Ok(())
}

fn assert_port(port: u16) -> Result<(), WireProtocolError> {
    if port == 0 {
        return Err(WireProtocolError(format!("Invalid port: {port}.")));
    }
    Ok(())
}

fn assert_window(bytes: u32) -> Result<(), WireProtocolError> {
    if bytes == 0 || bytes > MAX_STREAM_WINDOW_BYTES {
        return Err(WireProtocolError(format!("Invalid window/credit size: {bytes}.")));
    }
    Ok(())
}

fn clip_reason(reason: &str) -> &[u8] {
    let bytes = reason.as_bytes();
    if bytes.len() <= MAX_REASON_BYTES {
        return bytes;
    }
    let mut cut = MAX_REASON_BYTES;
    while cut > 0 && !reason.is_char_boundary(cut) {
        cut -= 1;
    }
    &bytes[..cut]
}

fn alloc_header(frame_type: FrameType, stream_id: u32, payload_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_BYTES + payload_len);
    out.push(STREAM_PROTOCOL_VERSION);
    out.push(frame_type as u8);
    out.extend_from_slice(&stream_id.to_be_bytes());
    out.extend_from_slice(&(payload_len as u32).to_be_bytes());
    out
}

pub fn encode_frame(frame: &Frame) -> Result<Vec<u8>, WireProtocolError> {
    match frame {
        Frame::Open { stream_id, host, port, initial_window } => {
            assert_stream_id(*stream_id)?;
            assert_port(*port)?;
            assert_window(*initial_window)?;
            if !valid_host(host) {
                return Err(WireProtocolError(format!("Invalid OPEN host: {host}")));
            }
            let host_bytes = host.as_bytes();
            let mut out = alloc_header(FrameType::Open, *stream_id, 6 + host_bytes.len());
            out.extend_from_slice(&port.to_be_bytes());
            out.extend_from_slice(&initial_window.to_be_bytes());
            out.extend_from_slice(host_bytes);
            Ok(out)
        }
        Frame::Opened { stream_id, initial_window } => {
            assert_stream_id(*stream_id)?;
            assert_window(*initial_window)?;
            let mut out = alloc_header(FrameType::Opened, *stream_id, 4);
            out.extend_from_slice(&initial_window.to_be_bytes());
            Ok(out)
        }
        Frame::Data { stream_id, data } => {
            assert_stream_id(*stream_id)?;
            if data.is_empty() || data.len() > MAX_FRAME_PAYLOAD_BYTES {
                return Err(WireProtocolError(format!("Invalid DATA length: {}", data.len())));
            }
            let mut out = alloc_header(FrameType::Data, *stream_id, data.len());
            out.extend_from_slice(data);
            Ok(out)
        }
        Frame::WindowUpdate { stream_id, delta } => {
            assert_stream_id(*stream_id)?;
            assert_window(*delta)?;
            let mut out = alloc_header(FrameType::WindowUpdate, *stream_id, 4);
            out.extend_from_slice(&delta.to_be_bytes());
            Ok(out)
        }
        Frame::Fin { stream_id } => {
            assert_stream_id(*stream_id)?;
            Ok(alloc_header(FrameType::Fin, *stream_id, 0))
        }
        Frame::Reset { stream_id, code, reason } => {
            assert_stream_id(*stream_id)?;
            let reason_bytes = clip_reason(reason);
            let mut out = alloc_header(FrameType::Reset, *stream_id, 2 + reason_bytes.len());
            out.extend_from_slice(&(*code as u16).to_be_bytes());
            out.extend_from_slice(reason_bytes);
            Ok(out)
        }
        Frame::Ping { token } => {
            let mut out = alloc_header(FrameType::Ping, 0, 8);
            out.extend_from_slice(token);
            Ok(out)
        }
        Frame::Pong { token } => {
            let mut out = alloc_header(FrameType::Pong, 0, 8);
            out.extend_from_slice(token);
            Ok(out)
        }
        Frame::Goaway { code, reason } => {
            let reason_bytes = clip_reason(reason);
            let mut out = alloc_header(FrameType::Goaway, 0, 2 + reason_bytes.len());
            out.extend_from_slice(&(*code as u16).to_be_bytes());
            out.extend_from_slice(reason_bytes);
            Ok(out)
        }
    }
}

pub fn decode_frame(raw: &[u8]) -> Result<Frame, WireProtocolError> {
    if raw.len() < FRAME_HEADER_BYTES {
        return Err(WireProtocolError(format!("Frame too short: {} bytes.", raw.len())));
    }
    let version = raw[0];
    if version != STREAM_PROTOCOL_VERSION {
        return Err(WireProtocolError(format!("Unsupported stream protocol version: {version}.")));
    }
    let frame_type = raw[1];
    let stream_id = u32::from_be_bytes([raw[2], raw[3], raw[4], raw[5]]);
    let length = u32::from_be_bytes([raw[6], raw[7], raw[8], raw[9]]) as usize;
    if length > MAX_FRAME_PAYLOAD_BYTES {
        return Err(WireProtocolError(format!("Frame payload exceeds limit: {length} bytes.")));
    }
    if raw.len() != FRAME_HEADER_BYTES + length {
        return Err(WireProtocolError(format!(
            "Frame length mismatch: header declares {length}, actual {}.",
            raw.len() - FRAME_HEADER_BYTES
        )));
    }
    let payload = &raw[FRAME_HEADER_BYTES..];

    match frame_type {
        0x01 => {
            assert_stream_id(stream_id)?;
            if !(7..=6 + MAX_HOST_BYTES).contains(&length) {
                return Err(WireProtocolError(format!("Invalid OPEN payload length: {length}.")));
            }
            let port = u16::from_be_bytes([payload[0], payload[1]]);
            assert_port(port)?;
            let initial_window = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
            assert_window(initial_window)?;
            let host = decode_utf8(&payload[6..])?;
            if !valid_host(&host) {
                return Err(WireProtocolError("Invalid OPEN target host.".into()));
            }
            Ok(Frame::Open { stream_id, host, port, initial_window })
        }
        0x02 => {
            assert_stream_id(stream_id)?;
            if length != 4 {
                return Err(WireProtocolError(format!("Invalid OPENED payload length: {length}.")));
            }
            let initial_window = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            assert_window(initial_window)?;
            Ok(Frame::Opened { stream_id, initial_window })
        }
        0x03 => {
            assert_stream_id(stream_id)?;
            if length == 0 {
                return Err(WireProtocolError("DATA frame must carry at least 1 byte.".into()));
            }
            Ok(Frame::Data { stream_id, data: payload.to_vec() })
        }
        0x04 => {
            assert_stream_id(stream_id)?;
            if length != 4 {
                return Err(WireProtocolError(format!(
                    "Invalid WINDOW_UPDATE payload length: {length}."
                )));
            }
            let delta = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            assert_window(delta)?;
            Ok(Frame::WindowUpdate { stream_id, delta })
        }
        0x05 => {
            assert_stream_id(stream_id)?;
            if length != 0 {
                return Err(WireProtocolError(format!("Invalid FIN payload length: {length}.")));
            }
            Ok(Frame::Fin { stream_id })
        }
        0x06 => {
            assert_stream_id(stream_id)?;
            if !(2..=2 + MAX_REASON_BYTES).contains(&length) {
                return Err(WireProtocolError(format!("Invalid RESET payload length: {length}.")));
            }
            let code = ResetCode::from_u16(u16::from_be_bytes([payload[0], payload[1]]));
            let reason = if length > 2 { decode_utf8(&payload[2..])? } else { String::new() };
            Ok(Frame::Reset { stream_id, code, reason })
        }
        0x07 | 0x08 => {
            if stream_id != 0 {
                return Err(WireProtocolError("PING/PONG must use streamID 0.".into()));
            }
            if length != 8 {
                return Err(WireProtocolError(format!("Invalid PING/PONG payload length: {length}.")));
            }
            let mut token = [0u8; 8];
            token.copy_from_slice(payload);
            if frame_type == 0x07 {
                Ok(Frame::Ping { token })
            } else {
                Ok(Frame::Pong { token })
            }
        }
        0x09 => {
            if stream_id != 0 {
                return Err(WireProtocolError("GOAWAY must use streamID 0.".into()));
            }
            if !(2..=2 + MAX_REASON_BYTES).contains(&length) {
                return Err(WireProtocolError(format!("Invalid GOAWAY payload length: {length}.")));
            }
            let code = ResetCode::from_u16(u16::from_be_bytes([payload[0], payload[1]]));
            let reason = if length > 2 { decode_utf8(&payload[2..])? } else { String::new() };
            Ok(Frame::Goaway { code, reason })
        }
        other => Err(WireProtocolError(format!("Unknown stream frame type: {other}."))),
    }
}

fn decode_utf8(bytes: &[u8]) -> Result<String, WireProtocolError> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| WireProtocolError("Frame contains invalid UTF-8.".into()))
}
