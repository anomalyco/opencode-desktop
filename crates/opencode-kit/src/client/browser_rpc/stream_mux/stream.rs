//! Bidirectional multiplexed stream handle ([`MuxStream`]) with credit replenishment, bounded
//! outbound backpressure, half-close FIN, and automatic RESET on drop.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use rune::time::Instant;

use super::super::http::AbortHandle;
use super::super::stream_wire::{FRAME_DATA_CHUNK_BYTES, Frame, ResetCode};
use super::{ClientMux, POLL_SLICE, StreamStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Data(usize),
    Eof,
    TimedOut,
}

struct StreamInner {
    mux: Arc<ClientMux>,
    id: u32,
    finished: AtomicBool,
}

impl StreamInner {
    fn reset(&self, code: ResetCode, reason: &str) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut g = self.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        let should_reset = g.streams.get(&self.id).is_some_and(|s| {
            s.status != StreamStatus::Closed
                && !(s.inbound_fin_received && s.inbound_bytes == 0 && s.outbound_fin_sent)
        });
        g.cleanup_stream(
            self.id,
            None,
            should_reset.then(|| (code, reason.to_owned())),
        );
        g.streams.remove(&self.id);
        drop(g);
        self.mux.cv.notify_all();
        let _ = self.mux.poller.notify();
    }
}

impl Drop for StreamInner {
    fn drop(&mut self) {
        self.reset(ResetCode::Cancelled, "");
    }
}

/// One bidirectional duplex stream multiplexed over [`ClientMux`]. Cheaply cloneable for
/// split reader/writer threads; sends `RESET(CANCELLED)` when the last handle drops unless
/// cleanly half-closed on both ends.
#[derive(Clone)]
pub struct MuxStream {
    inner: Arc<StreamInner>,
}

impl MuxStream {
    pub(super) fn new(mux: Arc<ClientMux>, id: u32) -> Self {
        Self { inner: Arc::new(StreamInner { mux, id, finished: AtomicBool::new(false) }) }
    }

    /// Reads available inbound bytes, replenishing `WINDOW_UPDATE` credits in 128 KiB batches.
    pub fn read_timeout(
        &self,
        buf: &mut [u8],
        timeout: Option<Duration>,
        abort: &AbortHandle,
    ) -> Result<ReadOutcome, String> {
        if buf.is_empty() {
            return Ok(ReadOutcome::Data(0));
        }
        let deadline = timeout.map(|t| Instant::now() + t);
        let id = self.inner.id;
        let mut g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if abort.is_aborted() {
                return Err("Browser tunnel stream read was cancelled.".into());
            }
            let credit_batch = g.cfg.credit_batch_bytes;
            let mut send_window_update = None;
            if let Some(s) = g.streams.get_mut(&id) {
                if s.inbound_bytes > 0 {
                    let mut copied = 0usize;
                    while copied < buf.len() {
                        let Some(front) = s.inbound_chunks.front() else { break };
                        let avail = front.len() - s.inbound_offset;
                        let take = avail.min(buf.len() - copied);
                        buf[copied..copied + take]
                            .copy_from_slice(&front[s.inbound_offset..s.inbound_offset + take]);
                        copied += take;
                        s.inbound_offset += take;
                        if s.inbound_offset >= front.len() {
                            s.inbound_chunks.pop_front();
                            s.inbound_offset = 0;
                        }
                    }
                    s.inbound_bytes = s.inbound_bytes.saturating_sub(copied);
                    s.consumed_since_credit = s.consumed_since_credit.saturating_add(copied as u32);
                    if !s.inbound_fin_received && s.consumed_since_credit >= credit_batch {
                        let delta = s.consumed_since_credit;
                        s.consumed_since_credit = 0;
                        s.recv_window = s.recv_window.saturating_add(delta);
                        send_window_update = Some(delta);
                    }
                    g.release_bytes(copied);
                    if let Some(delta) = send_window_update {
                        g.enqueue_control(&Frame::WindowUpdate { stream_id: id, delta });
                    }
                    g.maybe_finish_stream(id);
                    drop(g);
                    if send_window_update.is_some() {
                        let _ = self.inner.mux.poller.notify();
                    }
                    return Ok(ReadOutcome::Data(copied));
                }
                if s.inbound_fin_received {
                    g.maybe_finish_stream(id);
                    return Ok(ReadOutcome::Eof);
                }
                if s.status == StreamStatus::Closed {
                    let err = s
                        .error
                        .clone()
                        .unwrap_or_else(|| "Browser tunnel stream closed.".into());
                    return Err(err);
                }
            } else {
                return Err(g.close_reason.clone());
            }
            let wait = match deadline {
                Some(dl) => {
                    let now = Instant::now();
                    if now >= dl {
                        return Ok(ReadOutcome::TimedOut);
                    }
                    dl.saturating_duration_since(now).min(POLL_SLICE)
                }
                None => POLL_SLICE,
            };
            let (next_g, _) = self
                .inner
                .mux
                .cv
                .wait_timeout(g, wait)
                .unwrap_or_else(PoisonError::into_inner);
            g = next_g;
        }
    }

    /// Writes all `data` with bounded per-stream high/low water backpressure and attachment buffer
    /// enforcement. If the mux detaches while credit-starved, settles immediately with an error.
    pub fn write_all(&self, data: &[u8], abort: &AbortHandle) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        let id = self.inner.id;
        let mut offset = 0usize;
        while offset < data.len() {
            let chunk_cap = self
                .inner
                .mux
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .cfg
                .stream_outbound_high_water_bytes
                .max(FRAME_DATA_CHUNK_BYTES);
            let end = (offset + chunk_cap).min(data.len());
            let slice = &data[offset..end];
            let mut g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if abort.is_aborted() {
                    return Err("Browser tunnel write was cancelled.".into());
                }
                if g.closed {
                    return Err(g.close_reason.clone());
                }
                let Some(s) = g.streams.get(&id) else {
                    return Err(g.close_reason.clone());
                };
                if s.status != StreamStatus::Open {
                    return Err(s
                        .error
                        .clone()
                        .unwrap_or_else(|| "Browser tunnel stream closed.".into()));
                }
                if s.outbound_bytes < g.cfg.stream_outbound_high_water_bytes {
                    break;
                }
                let (next_g, _) = self
                    .inner
                    .mux
                    .cv
                    .wait_timeout(g, POLL_SLICE)
                    .unwrap_or_else(PoisonError::into_inner);
                g = next_g;
            }
            if !g.reserve_bytes(slice.len()) {
                let err = "Browser tunnel attachment buffer limit exceeded.".to_owned();
                g.cleanup_stream(
                    id,
                    Some(err.clone()),
                    Some((ResetCode::LimitExceeded, err.clone())),
                );
                drop(g);
                let _ = self.inner.mux.poller.notify();
                return Err(err);
            }
            if let Some(s) = g.streams.get_mut(&id) {
                s.outbound_chunks.push_back(slice.to_vec());
                s.outbound_bytes += slice.len();
            }
            g.mark_writable(id);
            let need_wait = g
                .streams
                .get(&id)
                .is_some_and(|s| s.outbound_bytes >= g.cfg.stream_outbound_high_water_bytes);
            drop(g);
            let _ = self.inner.mux.poller.notify();
            if need_wait {
                let mut g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    if abort.is_aborted() {
                        return Err("Browser tunnel write was cancelled.".into());
                    }
                    if g.closed {
                        return Err(g.close_reason.clone());
                    }
                    let Some(s) = g.streams.get(&id) else {
                        return Err(g.close_reason.clone());
                    };
                    if s.status != StreamStatus::Open {
                        return Err(s
                            .error
                            .clone()
                            .unwrap_or_else(|| "Browser tunnel stream closed.".into()));
                    }
                    if s.outbound_bytes <= g.cfg.stream_outbound_low_water_bytes {
                        break;
                    }
                    let (next_g, _) = self
                        .inner
                        .mux
                        .cv
                        .wait_timeout(g, POLL_SLICE)
                        .unwrap_or_else(PoisonError::into_inner);
                    g = next_g;
                }
            }
            offset = end;
        }
        Ok(())
    }

    /// Sends half-close `FIN` after all queued outbound bytes drain. On clean detach before `FIN`
    /// is transmitted, returns a cancellation error instead of reporting delivery.
    pub fn finish_write(&self, abort: &AbortHandle) -> Result<(), String> {
        let id = self.inner.id;
        {
            let mut g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
            if g.closed {
                return Err(g.close_reason.clone());
            }
            let Some(s) = g.streams.get_mut(&id) else {
                return Err(g.close_reason.clone());
            };
            if s.status != StreamStatus::Open {
                if s.outbound_fin_sent {
                    return Ok(());
                }
                return Err(s
                    .error
                    .clone()
                    .unwrap_or_else(|| "Browser tunnel stream closed.".into()));
            }
            s.outbound_fin_pending = true;
            g.mark_writable(id);
        }
        let _ = self.inner.mux.poller.notify();

        let mut g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if abort.is_aborted() {
                return Err("Browser tunnel stream close was cancelled.".into());
            }
            let Some(s) = g.streams.get(&id) else {
                return Err(g.close_reason.clone());
            };
            if s.outbound_fin_sent {
                return Ok(());
            }
            if g.closed || s.status == StreamStatus::Closed {
                return Err(s.error.clone().unwrap_or_else(|| g.close_reason.clone()));
            }
            let (next_g, _) = self
                .inner
                .mux
                .cv
                .wait_timeout(g, POLL_SLICE)
                .unwrap_or_else(PoisonError::into_inner);
            g = next_g;
        }
    }

    /// Whether this stream is open, error-free, and idle for HTTP/1.1 keep-alive pool reuse.
    pub fn is_reusable(&self) -> bool {
        let g = self.inner.mux.state.lock().unwrap_or_else(PoisonError::into_inner);
        if g.closed {
            return false;
        }
        g.streams.get(&self.inner.id).is_some_and(|s| {
            s.status == StreamStatus::Open
                && s.error.is_none()
                && !s.inbound_fin_received
                && !s.outbound_fin_pending
                && !s.outbound_fin_sent
                && s.inbound_bytes == 0
                && s.outbound_bytes == 0
        })
    }

    /// Immediately sends `RESET` and tears down stream resources (e.g. on abort or seek).
    pub fn reset(&self, code: ResetCode, reason: &str) {
        self.inner.reset(code, reason);
    }
}
