//! Unified upstream stream abstraction (binary WebSocket [`MuxStream`] or legacy chunked RPC)
//! plus negotiated transport mode and per-target `tunnel.grant` barrier for [`super::proxy::TunnelProxy`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use rune::time::Instant;

use super::super::http::AbortHandle;
use super::super::stream_mux::{ClientMux, MuxConfig, MuxStream, ReadOutcome};
use super::super::stream_wire::ResetCode;
use super::client::{PREWARM_WAIT_TIMEOUT, open_authenticated_websocket};
use super::super::types::{
    LocationRef, MuxCloseInfo, STREAM_CAPABILITY, STREAM_PROTOCOL_VERSION, TUNNEL_CHUNK_BYTES,
    TunnelTarget, TunnelUpgradeError, TunnelUpgradeFailureKind,
};
use crate::client::Server;

const GRANT_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const MODE_WAIT_SLICE: Duration = Duration::from_millis(100);

/// Explicit negotiated transport mode for one remote attachment (`network.ts:47-51`).
pub(super) enum TransportMode {
    /// Waiting for `attached` capabilities or the prewarmed binary WebSocket handshake.
    Pending,
    /// Prewarmed multiplexed binary WebSocket is live.
    Stream(Arc<ClientMux>),
    /// Explicit non-downgrading failure (never falls back to legacy RPC or direct networking).
    Failed(TunnelUpgradeError),
    /// Server did not advertise `tunnel.ws.v1` or `OPENCODE_BROWSER_TUNNEL_TRANSPORT=legacy`.
    Legacy,
}

struct ModeInner {
    mode: TransportMode,
    disposed: bool,
    advertised_stream: bool,
    prewarm_started: bool,
}

/// Thread-safe controller for negotiated transport mode and per-target `tunnel.grant` gates.
pub(super) struct TransportController {
    server: Server,
    location: LocationRef,
    session_id: String,
    connection_id: String,
    inner: Mutex<ModeInner>,
    cv: Condvar,
    grants: Mutex<BTreeMap<(String, u16), Arc<(Mutex<bool>, Condvar)>>>,
}

impl TransportController {
    pub fn new(server: Server, location: LocationRef, session_id: String, connection_id: String) -> Self {
        Self {
            server,
            location,
            session_id,
            connection_id,
            inner: Mutex::new(ModeInner {
                mode: TransportMode::Pending,
                disposed: false,
                advertised_stream: false,
                prewarm_started: false,
            }),
            cv: Condvar::new(),
            grants: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn stream_active(&self) -> bool {
        let g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        !g.disposed && matches!(&g.mode, TransportMode::Stream(m) if !m.is_closed())
    }

    pub fn advertises_stream(&self) -> bool {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner).advertised_stream
    }

    /// Activates the negotiated transport once `attached` arrives (`network.ts:179-194`).
    pub fn activate(
        self: &Arc<Self>,
        capabilities: &[String],
        on_stream_ready: Box<dyn FnOnce() + Send + 'static>,
        on_stream_closed: Box<dyn FnOnce(MuxCloseInfo) + Send + 'static>,
        on_reset_pools: Arc<dyn Fn() + Send + Sync + 'static>,
    ) {
        let server_advertised = capabilities.iter().any(|c| c == STREAM_CAPABILITY);
        #[allow(clippy::disallowed_methods)]
        let forced_legacy = std::env::var("OPENCODE_BROWSER_TUNNEL_TRANSPORT")
            .ok()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("legacy"));
        {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.disposed || !matches!(g.mode, TransportMode::Pending) || g.prewarm_started {
                return;
            }
            g.advertised_stream = server_advertised;
            if forced_legacy || !server_advertised {
                g.mode = TransportMode::Legacy;
                drop(g);
                self.cv.notify_all();
                return;
            }
            g.prewarm_started = true;
        }

        let ctrl = self.clone();
        std::thread::Builder::new()
            .name("browser-tunnel-prewarm".into())
            .spawn(move || {
                ctrl.prewarm_stream(on_stream_ready, on_stream_closed, on_reset_pools);
            })
            .ok();
    }

    fn prewarm_stream(
        self: &Arc<Self>,
        on_stream_ready: Box<dyn FnOnce() + Send + 'static>,
        on_stream_closed: Box<dyn FnOnce(MuxCloseInfo) + Send + 'static>,
        on_reset_pools: Arc<dyn Fn() + Send + Sync + 'static>,
    ) {
        let abort = AbortHandle::new();
        let issued = match self.server.browser_rpc_tunnel_ticket(
            &self.location,
            &self.session_id,
            &self.connection_id,
            STREAM_PROTOCOL_VERSION,
            &abort,
        ) {
            Ok(t) => t,
            Err(err) => {
                self.fail_upgrade(err);
                return;
            }
        };
        if self.inner.lock().unwrap_or_else(PoisonError::into_inner).disposed {
            return;
        }
        let ws = match open_authenticated_websocket(
            &self.server,
            &self.location,
            &self.session_id,
            &self.connection_id,
            &issued,
        ) {
            Ok(ws) => ws,
            Err(err) => {
                self.fail_upgrade(err);
                return;
            }
        };
        if self.inner.lock().unwrap_or_else(PoisonError::into_inner).disposed {
            return;
        }
        let ctrl_closed = self.clone();
        let reset_pools = on_reset_pools.clone();
        let mux = match ClientMux::spawn(
            ws,
            MuxConfig::default(),
            Box::new(move |info| {
                reset_pools();
                let notify = {
                    let mut g = ctrl_closed.inner.lock().unwrap_or_else(PoisonError::into_inner);
                    if g.disposed {
                        false
                    } else {
                        let msg = if info.replaced {
                            "Browser attachment was replaced.".to_owned()
                        } else if info.reason.is_empty() {
                            "Browser stream tunnel disconnected.".to_owned()
                        } else {
                            info.reason.clone()
                        };
                        g.mode = TransportMode::Failed(TunnelUpgradeError::new(
                            TunnelUpgradeFailureKind::Transport,
                            msg,
                        ));
                        true
                    }
                };
                ctrl_closed.cv.notify_all();
                if notify {
                    on_stream_closed(info);
                }
            }),
        ) {
            Ok(m) => m,
            Err(msg) => {
                self.fail_upgrade(TunnelUpgradeError::new(TunnelUpgradeFailureKind::Transport, msg));
                return;
            }
        };
        {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if g.disposed {
                drop(g);
                mux.close(1000, "desktop detached");
                return;
            }
            g.mode = TransportMode::Stream(mux);
        }
        self.cv.notify_all();
        on_stream_ready();
    }

    fn fail_upgrade(&self, err: TunnelUpgradeError) {
        let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if !g.disposed {
            g.mode = TransportMode::Failed(err);
        }
        drop(g);
        self.cv.notify_all();
    }

    /// Fires `tunnel.grant` on a worker thread and installs a short per-origin gate so any
    /// immediate proxy connect to `(host, port)` waits for the grant before sending `OPEN`.
    pub fn grant_private_target(self: &Arc<Self>, target: TunnelTarget, extra_ports: Vec<u16>) {
        if !self.advertises_stream() {
            return;
        }
        let key = (target.host.to_ascii_lowercase(), target.port);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        self.grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone(), gate.clone());
        let ctrl = self.clone();
        std::thread::Builder::new()
            .name("browser-tunnel-grant".into())
            .spawn(move || {
                let _ = ctrl.server.browser_rpc_tunnel_grant(
                    &ctrl.location,
                    &ctrl.session_id,
                    &ctrl.connection_id,
                    &target,
                    &extra_ports,
                );
                let (lock, cv) = &*gate;
                *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
                cv.notify_all();
                let mut map = ctrl.grants.lock().unwrap_or_else(PoisonError::into_inner);
                if map.get(&key).is_some_and(|cur| Arc::ptr_eq(cur, &gate)) {
                    map.remove(&key);
                }
            })
            .ok();
    }

    fn wait_pending_grant(&self, target: &TunnelTarget, abort: &AbortHandle) {
        let key = (target.host.to_ascii_lowercase(), target.port);
        let gate = self
            .grants
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let Some(gate) = gate else { return };
        let deadline = Instant::now() + GRANT_WAIT_TIMEOUT;
        let (lock, cv) = &*gate;
        let mut done = lock.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done && !abort.is_aborted() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let wait = deadline.saturating_duration_since(now).min(MODE_WAIT_SLICE);
            let (next, _) = cv.wait_timeout(done, wait).unwrap_or_else(PoisonError::into_inner);
            done = next;
        }
    }

    /// Opens one upstream stream via the negotiated transport (waiting up to 15 s while `Pending`).
    pub fn open_stream(
        &self,
        target: &TunnelTarget,
        abort: &AbortHandle,
    ) -> Result<UpstreamStream, String> {
        self.wait_pending_grant(target, abort);
        let deadline = Instant::now() + PREWARM_WAIT_TIMEOUT;
        let chosen = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if abort.is_aborted() {
                    return Err("Browser tunnel open was cancelled.".into());
                }
                if g.disposed {
                    return Err("Browser attachment is closed.".into());
                }
                match &g.mode {
                    TransportMode::Stream(mux) => break Ok(Some(mux.clone())),
                    TransportMode::Legacy => break Ok(None),
                    TransportMode::Failed(err) => break Err(err.message.clone()),
                    TransportMode::Pending => {
                        let now = Instant::now();
                        if now >= deadline {
                            return Err("Browser stream tunnel timed out while connecting.".into());
                        }
                        let wait = deadline.saturating_duration_since(now).min(MODE_WAIT_SLICE);
                        let (next, _) = self
                            .cv
                            .wait_timeout(g, wait)
                            .unwrap_or_else(PoisonError::into_inner);
                        g = next;
                    }
                }
            }
        }?;

        match chosen {
            Some(mux) => mux.open_stream(target, abort).map(UpstreamStream::Mux),
            None => {
                let tunnel_id = self
                    .server
                    .browser_rpc_tunnel_open(
                        &self.location,
                        &self.session_id,
                        &self.connection_id,
                        target,
                        abort,
                    )
                    .map_err(|e| e.to_string())?;
                Ok(UpstreamStream::Legacy(LegacyStream::new(
                    self.server.clone(),
                    self.location.clone(),
                    self.session_id.clone(),
                    self.connection_id.clone(),
                    tunnel_id,
                )))
            }
        }
    }

    /// Cleanly detaches and closes any live [`ClientMux`] (`1000`, `"desktop detached"`).
    pub fn dispose(&self) {
        let mux = {
            let mut g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
            g.disposed = true;
            match &g.mode {
                TransportMode::Stream(m) => Some(m.clone()),
                _ => None,
            }
        };
        self.cv.notify_all();
        if let Some(m) = mux {
            m.close(1000, "desktop detached");
        }
    }
}

/// Legacy RPC tunnel handle matching the [`UpstreamStream`] interface.
#[derive(Clone)]
pub(super) struct LegacyStream {
    inner: Arc<LegacyInner>,
}

struct LegacyInner {
    server: Server,
    location: LocationRef,
    session_id: String,
    connection_id: String,
    tunnel_id: String,
    closed: AtomicBool,
    eof_seen: AtomicBool,
    read_buf: Mutex<(Vec<u8>, usize)>,
}

impl LegacyStream {
    fn new(
        server: Server,
        location: LocationRef,
        session_id: String,
        connection_id: String,
        tunnel_id: String,
    ) -> Self {
        Self {
            inner: Arc::new(LegacyInner {
                server,
                location,
                session_id,
                connection_id,
                tunnel_id,
                closed: AtomicBool::new(false),
                eof_seen: AtomicBool::new(false),
                read_buf: Mutex::new((Vec::new(), 0)),
            }),
        }
    }

    fn read(&self, buf: &mut [u8], abort: &AbortHandle) -> Result<usize, String> {
        {
            let mut rb = self.inner.read_buf.lock().unwrap_or_else(PoisonError::into_inner);
            if rb.1 < rb.0.len() {
                let avail = rb.0.len() - rb.1;
                let take = avail.min(buf.len());
                buf[..take].copy_from_slice(&rb.0[rb.1..rb.1 + take]);
                rb.1 += take;
                if rb.1 >= rb.0.len() {
                    rb.0.clear();
                    rb.1 = 0;
                }
                return Ok(take);
            }
        }
        if self.inner.eof_seen.load(Ordering::Acquire) {
            return Ok(0);
        }
        let chunk = self
            .inner
            .server
            .browser_rpc_tunnel_read(
                &self.inner.location,
                &self.inner.session_id,
                &self.inner.connection_id,
                &self.inner.tunnel_id,
                abort,
            )
            .map_err(|e| e.to_string())?;
        if chunk.eof && chunk.data.is_empty() {
            self.inner.eof_seen.store(true, Ordering::Release);
            return Ok(0);
        }
        if chunk.eof {
            self.inner.eof_seen.store(true, Ordering::Release);
        }
        let take = chunk.data.len().min(buf.len());
        buf[..take].copy_from_slice(&chunk.data[..take]);
        if take < chunk.data.len() {
            let mut rb = self.inner.read_buf.lock().unwrap_or_else(PoisonError::into_inner);
            rb.0 = chunk.data;
            rb.1 = take;
        }
        Ok(take)
    }

    fn write_all(&self, data: &[u8], end: bool, abort: &AbortHandle) -> Result<(), String> {
        if data.is_empty() {
            return self
                .inner
                .server
                .browser_rpc_tunnel_write_abortable(
                    &self.inner.location,
                    &self.inner.session_id,
                    &self.inner.connection_id,
                    &self.inner.tunnel_id,
                    &[],
                    end,
                    abort,
                )
                .map_err(|e| e.to_string());
        }
        let total = data.chunks(TUNNEL_CHUNK_BYTES).len();
        for (idx, chunk) in data.chunks(TUNNEL_CHUNK_BYTES).enumerate() {
            let is_last = end && idx + 1 == total;
            self.inner
                .server
                .browser_rpc_tunnel_write_abortable(
                    &self.inner.location,
                    &self.inner.session_id,
                    &self.inner.connection_id,
                    &self.inner.tunnel_id,
                    chunk,
                    is_last,
                    abort,
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn close(&self) {
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.inner.server.browser_rpc_tunnel_close(
            &self.inner.location,
            &self.inner.session_id,
            &self.inner.connection_id,
            &self.inner.tunnel_id,
        );
    }

    fn is_reusable(&self) -> bool {
        !self.inner.closed.load(Ordering::Acquire) && !self.inner.eof_seen.load(Ordering::Acquire)
    }
}

impl Drop for LegacyInner {
    fn drop(&mut self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.server.browser_rpc_tunnel_close(
                &self.location,
                &self.session_id,
                &self.connection_id,
                &self.tunnel_id,
            );
        }
    }
}

/// Unified remote stream over either binary WebSocket [`MuxStream`] or legacy chunked RPC.
#[derive(Clone)]
pub(super) enum UpstreamStream {
    Mux(MuxStream),
    Legacy(LegacyStream),
}

impl UpstreamStream {
    pub fn read(&self, buf: &mut [u8], abort: &AbortHandle) -> Result<usize, String> {
        match self {
            Self::Mux(m) => match m.read_timeout(buf, None, abort)? {
                ReadOutcome::Data(n) => Ok(n),
                ReadOutcome::Eof | ReadOutcome::TimedOut => Ok(0),
            },
            Self::Legacy(l) => l.read(buf, abort),
        }
    }

    pub fn write_all(&self, data: &[u8], abort: &AbortHandle) -> Result<(), String> {
        match self {
            Self::Mux(m) => m.write_all(data, abort),
            Self::Legacy(l) => l.write_all(data, false, abort),
        }
    }

    pub fn finish_write(&self, abort: &AbortHandle) -> Result<(), String> {
        match self {
            Self::Mux(m) => m.finish_write(abort),
            Self::Legacy(l) => l.write_all(&[], true, abort),
        }
    }

    pub fn is_reusable(&self) -> bool {
        match self {
            Self::Mux(m) => m.is_reusable(),
            Self::Legacy(l) => l.is_reusable(),
        }
    }

    pub fn close_or_reset(&self) {
        match self {
            Self::Mux(m) => m.reset(ResetCode::Cancelled, ""),
            Self::Legacy(l) => l.close(),
        }
    }
}
