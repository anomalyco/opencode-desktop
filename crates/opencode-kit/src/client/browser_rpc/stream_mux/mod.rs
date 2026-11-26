//! Desktop-side multiplexed binary WebSocket stream client with fair round-robin scheduling,
//! bounded per-stream and per-attachment credit flow control, half-close FIN, immediate RESET
//! cancellation, heartbeats, and clean detach teardown
//! (`packages/plugin-browser/src/stream-mux.ts`, opencode#66217).

mod stream;
mod worker;

use std::collections::{BTreeMap, VecDeque};
use std::net::TcpStream;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use tungstenite::WebSocket;
use tungstenite::stream::MaybeTlsStream;

use super::http::AbortHandle;
use super::stream_wire::{
    DEFAULT_CLIENT_STREAM_WINDOW_BYTES, FRAME_DATA_CHUNK_BYTES, MAX_ATTACHMENT_BUFFERED_BYTES,
    MAX_PENDING_CONNECTS_PER_ATTACHMENT, MAX_STREAMS_PER_ATTACHMENT, MAX_WS_BUFFERED_BYTES,
    Frame, ResetCode, encode_frame,
};
use super::types::{CLOSE_ATTACHMENT_REPLACED, MuxCloseInfo, TunnelTarget};
pub use stream::{MuxStream, ReadOutcome};

pub(super) const POLL_SLICE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug)]
pub struct MuxConfig {
    pub stream_window_bytes: u32,
    pub credit_batch_bytes: u32,
    pub frame_chunk_bytes: usize,
    pub max_ws_buffered_bytes: usize,
    pub max_streams: usize,
    pub max_pending_connects: usize,
    pub max_attachment_buffered_bytes: usize,
    pub stream_outbound_high_water_bytes: usize,
    pub stream_outbound_low_water_bytes: usize,
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
}

impl Default for MuxConfig {
    fn default() -> Self {
        let stream_window_bytes = DEFAULT_CLIENT_STREAM_WINDOW_BYTES;
        let credit_batch_bytes = (256 * 1024).min(stream_window_bytes / 4).max(1);
        let stream_outbound_high_water_bytes = (256 * 1024).min(stream_window_bytes as usize);
        Self {
            stream_window_bytes,
            credit_batch_bytes,
            frame_chunk_bytes: FRAME_DATA_CHUNK_BYTES,
            max_ws_buffered_bytes: MAX_WS_BUFFERED_BYTES,
            max_streams: MAX_STREAMS_PER_ATTACHMENT,
            max_pending_connects: MAX_PENDING_CONNECTS_PER_ATTACHMENT,
            max_attachment_buffered_bytes: MAX_ATTACHMENT_BUFFERED_BYTES,
            stream_outbound_high_water_bytes,
            stream_outbound_low_water_bytes: (stream_outbound_high_water_bytes / 2).max(FRAME_DATA_CHUNK_BYTES),
            heartbeat_interval: Duration::from_secs(25),
            heartbeat_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamStatus {
    Opening,
    Open,
    Closed,
}

pub(super) struct ClientStreamState {
    pub status: StreamStatus,
    pub send_window: u32,
    pub recv_window: u32,
    pub consumed_since_credit: u32,
    pub inbound_chunks: VecDeque<Vec<u8>>,
    pub inbound_offset: usize,
    pub inbound_bytes: usize,
    pub inbound_fin_received: bool,
    pub outbound_chunks: VecDeque<Vec<u8>>,
    pub outbound_offset: usize,
    pub outbound_bytes: usize,
    pub outbound_fin_pending: bool,
    pub outbound_fin_sent: bool,
    pub in_round_robin: bool,
    pub error: Option<String>,
}

impl ClientStreamState {
    pub(super) fn has_writable_output(&self) -> bool {
        if self.status != StreamStatus::Open {
            return false;
        }
        if self.outbound_bytes > 0 && self.send_window > 0 {
            return true;
        }
        self.outbound_bytes == 0 && self.outbound_fin_pending && !self.outbound_fin_sent
    }
}

pub(super) struct MuxState {
    pub cfg: MuxConfig,
    pub closed: bool,
    pub close_reason: String,
    pub pending_close: Option<(u16, String, bool)>,
    pub next_stream_id: u32,
    pub attachment_bytes: usize,
    pub pending_connects: usize,
    pub streams: BTreeMap<u32, ClientStreamState>,
    pub control_queue: VecDeque<Vec<u8>>,
    pub round_robin: VecDeque<u32>,
}

impl MuxState {
    pub(super) fn reserve_bytes(&mut self, bytes: usize) -> bool {
        if bytes == 0 {
            return true;
        }
        if self.attachment_bytes.saturating_add(bytes) > self.cfg.max_attachment_buffered_bytes {
            return false;
        }
        self.attachment_bytes += bytes;
        true
    }

    pub(super) fn release_bytes(&mut self, bytes: usize) {
        self.attachment_bytes = self.attachment_bytes.saturating_sub(bytes);
    }

    pub(super) fn enqueue_control(&mut self, frame: &Frame) {
        if self.closed {
            return;
        }
        if let Ok(bytes) = encode_frame(frame) {
            self.control_queue.push_back(bytes);
        }
    }

    pub(super) fn mark_writable(&mut self, id: u32) {
        if self.closed {
            return;
        }
        if let Some(s) = self.streams.get_mut(&id)
            && !s.in_round_robin
            && s.has_writable_output()
        {
            s.in_round_robin = true;
            self.round_robin.push_back(id);
        }
    }

    pub(super) fn cleanup_stream(
        &mut self,
        id: u32,
        error: Option<String>,
        send_reset: Option<(ResetCode, String)>,
    ) {
        let Some(mut s) = self.streams.remove(&id) else {
            return;
        };
        if s.status == StreamStatus::Opening {
            self.pending_connects = self.pending_connects.saturating_sub(1);
        }
        let to_release = s.outbound_bytes.saturating_add(s.inbound_bytes);
        self.release_bytes(to_release);
        s.status = StreamStatus::Closed;
        s.outbound_chunks.clear();
        s.outbound_offset = 0;
        s.outbound_bytes = 0;
        s.inbound_chunks.clear();
        s.inbound_offset = 0;
        s.inbound_bytes = 0;
        s.error = error;
        if let Some((code, reason)) = send_reset
            && !self.closed
        {
            self.enqueue_control(&Frame::Reset { stream_id: id, code, reason });
        }
        if s.error.is_some() {
            self.streams.insert(id, s);
        }
    }

    pub(super) fn maybe_finish_stream(&mut self, id: u32) {
        if let Some(s) = self.streams.get_mut(&id)
            && s.status == StreamStatus::Open
            && s.inbound_fin_received
            && s.inbound_bytes == 0
            && s.outbound_fin_sent
        {
            s.status = StreamStatus::Closed;
        }
    }

    pub(super) fn pop_next_data_frame(&mut self) -> Option<Vec<u8>> {
        while let Some(id) = self.round_robin.pop_front() {
            let Some(s) = self.streams.get_mut(&id) else {
                continue;
            };
            s.in_round_robin = false;
            if s.status != StreamStatus::Open {
                continue;
            }
            if s.outbound_bytes > 0 && s.send_window > 0 {
                let max_take = self.cfg.frame_chunk_bytes.min(s.send_window as usize);
                let chunk = take_chunk(&mut s.outbound_chunks, &mut s.outbound_offset, max_take);
                let len = chunk.len();
                s.outbound_bytes = s.outbound_bytes.saturating_sub(len);
                s.send_window = s.send_window.saturating_sub(len as u32);
                if s.has_writable_output() {
                    s.in_round_robin = true;
                    self.round_robin.push_back(id);
                }
                self.release_bytes(len);
                return encode_frame(&Frame::Data { stream_id: id, data: chunk }).ok();
            }
            if s.outbound_bytes == 0 && s.outbound_fin_pending && !s.outbound_fin_sent {
                s.outbound_fin_sent = true;
                self.maybe_finish_stream(id);
                return encode_frame(&Frame::Fin { stream_id: id }).ok();
            }
        }
        None
    }

    pub(super) fn finish_shutdown(&mut self, code: u16, reason: String) -> Option<MuxCloseInfo> {
        if self.closed {
            return None;
        }
        self.closed = true;
        self.close_reason = if reason.is_empty() {
            "Browser tunnel disconnected.".to_owned()
        } else {
            reason.clone()
        };
        self.control_queue.clear();
        self.round_robin.clear();
        self.pending_connects = 0;
        self.attachment_bytes = 0;
        let err = self.close_reason.clone();
        for s in self.streams.values_mut() {
            s.status = StreamStatus::Closed;
            s.outbound_chunks.clear();
            s.outbound_offset = 0;
            s.outbound_bytes = 0;
            s.inbound_chunks.clear();
            s.inbound_offset = 0;
            s.inbound_bytes = 0;
            s.in_round_robin = false;
            if s.error.is_none() {
                s.error = Some(err.clone());
            }
        }
        Some(MuxCloseInfo {
            code,
            replaced: code == CLOSE_ATTACHMENT_REPLACED || reason == "replaced",
            reason: self.close_reason.clone(),
        })
    }
}

/// Client-side binary WebSocket multiplexer owning one prewarmed connection.
pub struct ClientMux {
    pub(super) state: Mutex<MuxState>,
    pub(super) cv: Condvar,
    pub(super) poller: Arc<polling::Poller>,
}

impl ClientMux {
    /// Spawns the dedicated `browser-stream-mux` I/O worker thread for `ws`.
    pub fn spawn(
        ws: WebSocket<MaybeTlsStream<TcpStream>>,
        cfg: MuxConfig,
        on_closed: Box<dyn FnOnce(MuxCloseInfo) + Send + 'static>,
    ) -> Result<Arc<Self>, String> {
        let poller = Arc::new(
            polling::Poller::new().map_err(|e| format!("Failed to create stream mux poller: {e}"))?,
        );
        let mux = Arc::new(Self {
            state: Mutex::new(MuxState {
                cfg,
                closed: false,
                close_reason: "Browser tunnel disconnected.".to_owned(),
                pending_close: None,
                next_stream_id: 1,
                attachment_bytes: 0,
                pending_connects: 0,
                streams: BTreeMap::new(),
                control_queue: VecDeque::new(),
                round_robin: VecDeque::new(),
            }),
            cv: Condvar::new(),
            poller,
        });
        let mux_thread = mux.clone();
        std::thread::Builder::new()
            .name("browser-stream-mux".into())
            .spawn(move || worker::run_mux_loop(mux_thread, ws, on_closed))
            .map_err(|e| format!("Failed to spawn browser stream mux thread: {e}"))?;
        Ok(mux)
    }

    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).closed
    }

    pub fn active_streams(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .streams
            .values()
            .filter(|s| s.status != StreamStatus::Closed)
            .count()
    }

    pub fn buffered_bytes(&self) -> usize {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).attachment_bytes
    }

    pub fn ping(&self) {
        let mut token = [0u8; 8];
        let _ = getrandom::fill(&mut token);
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if g.closed {
            return;
        }
        g.enqueue_control(&Frame::Ping { token });
        drop(g);
        let _ = self.poller.notify();
    }

    /// Requests clean detach closure: sends best-effort `GOAWAY` + WS close, settles every
    /// pending opener / reader / credit-starved writer with a cancellation error.
    pub fn close(&self, code: u16, reason: &str) {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.closed && g.pending_close.is_none() {
            g.pending_close = Some((code, reason.to_owned(), true));
        }
        drop(g);
        let _ = self.poller.notify();
    }

    /// Opens one multiplexed TCP stream to `target` on the remote server.
    pub fn open_stream(
        self: &Arc<Self>,
        target: &TunnelTarget,
        abort: &AbortHandle,
    ) -> Result<MuxStream, String> {
        if abort.is_aborted() {
            return Err("Browser tunnel open was cancelled.".into());
        }
        let id = {
            let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                return Err(g.close_reason.clone());
            }
            let live_count = g.streams.values().filter(|s| s.status != StreamStatus::Closed).count();
            if live_count >= g.cfg.max_streams || g.pending_connects >= g.cfg.max_pending_connects {
                return Err(format!(
                    "Browser attachment has reached its {}-stream limit.",
                    g.cfg.max_streams
                ));
            }
            let mut candidate = g.next_stream_id;
            g.next_stream_id = if candidate >= 0xffff_fffe { 1 } else { candidate + 1 };
            if g.streams.contains_key(&candidate) {
                if let Some(id_free) = (1..=0xffff_fffe).find(|k| !g.streams.contains_key(k)) {
                    candidate = id_free;
                } else {
                    return Err("Browser stream ID space exhausted.".into());
                }
            }
            let initial_window = g.cfg.stream_window_bytes;
            g.pending_connects += 1;
            g.streams.insert(
                candidate,
                ClientStreamState {
                    status: StreamStatus::Opening,
                    send_window: 0,
                    recv_window: initial_window,
                    consumed_since_credit: 0,
                    inbound_chunks: VecDeque::new(),
                    inbound_offset: 0,
                    inbound_bytes: 0,
                    inbound_fin_received: false,
                    outbound_chunks: VecDeque::new(),
                    outbound_offset: 0,
                    outbound_bytes: 0,
                    outbound_fin_pending: false,
                    outbound_fin_sent: false,
                    in_round_robin: false,
                    error: None,
                },
            );
            g.enqueue_control(&Frame::Open {
                stream_id: candidate,
                host: target.host.clone(),
                port: target.port,
                initial_window,
            });
            candidate
        };
        let _ = self.poller.notify();

        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if abort.is_aborted() {
                if g.streams.get(&id).is_some_and(|s| s.status == StreamStatus::Opening) {
                    g.cleanup_stream(
                        id,
                        Some("Browser tunnel open was cancelled.".into()),
                        Some((ResetCode::Cancelled, String::new())),
                    );
                    g.streams.remove(&id);
                    drop(g);
                    let _ = self.poller.notify();
                }
                return Err("Browser tunnel open was cancelled.".into());
            }
            match g.streams.get(&id) {
                Some(s) if s.status == StreamStatus::Open => {
                    return Ok(MuxStream::new(self.clone(), id));
                }
                Some(s) if s.status == StreamStatus::Closed => {
                    let err = s
                        .error
                        .clone()
                        .unwrap_or_else(|| "Browser tunnel stream closed while opening.".into());
                    g.streams.remove(&id);
                    return Err(err);
                }
                None => {
                    return Err(if g.closed {
                        g.close_reason.clone()
                    } else {
                        "Browser tunnel stream closed while opening.".into()
                    });
                }
                Some(_) => {
                    let (next_g, _) = self
                        .cv
                        .wait_timeout(g, POLL_SLICE)
                        .unwrap_or_else(PoisonError::into_inner);
                    g = next_g;
                }
            }
        }
    }
}

fn take_chunk(chunks: &mut VecDeque<Vec<u8>>, offset: &mut usize, max_bytes: usize) -> Vec<u8> {
    let Some(head) = chunks.front() else {
        return Vec::new();
    };
    let available = head.len().saturating_sub(*offset);
    if available <= max_bytes {
        let front = chunks.pop_front().unwrap_or_default();
        let start = *offset;
        *offset = 0;
        if start == 0 { front } else { front[start..].to_vec() }
    } else {
        let start = *offset;
        let slice = head[start..start + max_bytes].to_vec();
        *offset = start + max_bytes;
        slice
    }
}
