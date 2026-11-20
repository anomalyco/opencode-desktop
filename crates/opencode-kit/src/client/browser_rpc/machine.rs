//! Explicit per-session `experimental.browser` v4 attachment state machine
//! (`packages/gui-plugins/src/browser/{machine,pane-machine,model}.ts`, `#63500` `PRE-08`).
//!
//! All attachment phases, backoff timers, in-flight command tracking, and `PRE-08` retries on
//! `plugin.updated` / `server.connected` flow through [`AttachMachine::step`], which returns
//! [`AttachEffect`]s for the caller to execute.

use std::collections::BTreeSet;
use std::time::Duration;

use rune::time::Instant;

use super::types::{
    AttachOutcome, ControlFrame, REGISTER_TIMEOUT_MS, RETRY_BASE_MS, RETRY_MAX_MS, new_connection_id,
};

/// Why a session is blocked from automatic re-attach until an explicit wake event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockReason {
    /// Another desktop window replaced this attachment (`output: "replaced"`).
    Replaced,
    /// Server reported `SessionNotFoundError`; wait for `session.created` or session refresh.
    Missing,
    /// Remote server without engine-enforced proxy isolation (`Ready.remote_proxy == false`).
    RemoteUnproxied,
}

/// Explicit lifecycle state of one session's `experimental.browser` v4 attachment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AttachState {
    #[default]
    Idle,
    /// Held `POST /api/rpc/experimental.browser/attach` is in flight waiting for SSE `attached` v4.
    Connecting {
        attempt: u64,
        connection_id: String,
        failures: u32,
        deadline: Instant,
    },
    /// Server completed permission grant and emitted `attached` v4; commands and state sync run.
    Attached {
        attempt: u64,
        connection_id: String,
        in_flight: BTreeSet<String>,
        published_hash: Option<u64>,
    },
    /// Transient disconnect or timeout; waiting until `next_at` to retry.
    Backoff {
        attempt: u64,
        failures: u32,
        next_at: Instant,
    },
    /// Server returned `rpc.unavailable` (browser plugin not loaded/enabled yet; `PRE-08` `#63500`).
    /// Retries immediately on `PluginUpdated` or `ServerConnected`.
    Unavailable,
    /// Server lacks `experimental.browser` v4 (`404`/`405`/`501`/`rpc.not_found`/incompatible v).
    /// Keeps `rune_mcp` fallback active; re-probed on `ServerConnected` or `PluginUpdated`.
    Unsupported,
    /// Blocked until an explicit wake (`SessionCreated`, `UserWake`, or engine capability update).
    Blocked { reason: BlockReason },
    /// Final disposed state for a closed/deleted session.
    Closed,
}

/// Events fed into [`AttachMachine::step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachEvent {
    /// Session is active/open and eligible to attach (local or proxied remote).
    EnsureAttached,
    /// Remote server cannot safely attach because the running engine lacks `remote_proxy`.
    RequireRemoteProxy,
    /// SSE stream opened or reconnected (`server.connected`, `#63500`).
    ServerConnected,
    /// Server emitted `plugin.updated` (`#63500` `PRE-08`): re-probe `Unavailable`/`Unsupported`.
    PluginUpdated,
    /// Server emitted an SSE `rpc.experimental.browser.control` frame.
    Control { frame: ControlFrame },
    /// Held `attach` HTTP request ended with `outcome`.
    AttachEnded { attempt: u64, outcome: AttachOutcome },
    /// A timer tick at `now` (`Connecting` deadline or `Backoff` expiry).
    Tick,
    /// One command completed or was cancelled locally.
    CommandFinished { request_id: String },
    /// Local tab inventory hash changed; publish if attached and hash differs.
    StateChanged { hash: u64 },
    /// Server finished creating a previously missing session (`session.created`).
    SessionCreated,
    /// Session moved directory (`session.moved`): tear down and re-attach under the new location.
    SessionMoved,
    /// User interacted with the browser pane in this window (wakes `Replaced` / `Backoff`).
    UserWake,
    /// Session deleted or window disposed.
    Dispose,
}

/// Side effects returned by [`AttachMachine::step`] for the driver to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachEffect {
    StartAttach { attempt: u64, connection_id: String },
    AbortAttach { attempt: u64 },
    SendState { connection_id: String },
    FetchCommand { connection_id: String, request_id: String },
    CancelCommands { request_ids: Vec<String> },
    ScheduleTimer { at: Instant },
    SettlePromptGates,
    SetMcpSuppressed { suppressed: bool },
}

/// Explicit per-session `experimental.browser` v4 attachment state machine.
#[derive(Clone, Debug, Default)]
pub struct AttachMachine {
    pub state: AttachState,
    next_attempt: u64,
}

impl AttachMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_attached(&self) -> bool {
        matches!(self.state, AttachState::Attached { .. })
    }

    pub fn is_connecting(&self) -> bool {
        matches!(self.state, AttachState::Connecting { .. })
    }

    /// True once `experimental.browser` v4 is live and `rune_mcp` should suppress `browser_*`.
    pub fn should_suppress_mcp(&self) -> bool {
        self.is_attached()
    }

    /// True when the prompt gate has nothing left to wait for (already attached or will not attach).
    pub fn prompt_gate_settled(&self) -> bool {
        matches!(
            self.state,
            AttachState::Attached { .. }
                | AttachState::Unavailable
                | AttachState::Unsupported
                | AttachState::Blocked { .. }
                | AttachState::Closed
        )
    }

    pub fn connection_id(&self) -> Option<&str> {
        match &self.state {
            AttachState::Connecting { connection_id, .. }
            | AttachState::Attached { connection_id, .. } => Some(connection_id.as_str()),
            _ => None,
        }
    }

    pub fn is_request_in_flight(&self, connection_id: &str, request_id: &str) -> bool {
        matches!(
            &self.state,
            AttachState::Attached { connection_id: cur, in_flight, .. }
                if cur == connection_id && in_flight.contains(request_id)
        )
    }

    pub fn mark_state_published(&mut self, hash: u64) {
        if let AttachState::Attached { published_hash, .. } = &mut self.state {
            *published_hash = Some(hash);
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match &self.state {
            AttachState::Connecting { deadline, .. } => Some(*deadline),
            AttachState::Backoff { next_at, .. } => Some(*next_at),
            _ => None,
        }
    }

    pub fn step(&mut self, event: AttachEvent, now: Instant) -> Vec<AttachEffect> {
        if matches!(self.state, AttachState::Closed) {
            return Vec::new();
        }
        match event {
            AttachEvent::Dispose => self.dispose(),
            AttachEvent::RequireRemoteProxy => self.block(BlockReason::RemoteUnproxied),
            AttachEvent::EnsureAttached => match &self.state {
                AttachState::Idle | AttachState::Blocked { reason: BlockReason::RemoteUnproxied } => {
                    self.start_attach(now)
                }
                _ => Vec::new(),
            },
            AttachEvent::ServerConnected => match &self.state {
                // Re-attach if SSE just (re)connected while still awaiting the `attached` frame
                // or while parked in Idle/Unavailable/Unsupported/Backoff (`#63500` PRE-08).
                AttachState::Idle
                | AttachState::Connecting { .. }
                | AttachState::Unavailable
                | AttachState::Unsupported
                | AttachState::Backoff { .. } => self.start_attach(now),
                _ => Vec::new(),
            },
            AttachEvent::PluginUpdated => match &self.state {
                // PRE-08 (#63500): retry immediately when server plugins finish loading.
                AttachState::Unavailable
                | AttachState::Unsupported
                | AttachState::Backoff { .. } => self.start_attach(now),
                _ => Vec::new(),
            },
            AttachEvent::SessionCreated => match &self.state {
                AttachState::Blocked { reason: BlockReason::Missing } | AttachState::Idle => {
                    self.start_attach(now)
                }
                _ => Vec::new(),
            },
            AttachEvent::UserWake => match &self.state {
                AttachState::Idle
                | AttachState::Backoff { .. }
                | AttachState::Blocked { reason: BlockReason::Replaced } => self.start_attach(now),
                _ => Vec::new(),
            },
            AttachEvent::SessionMoved => {
                let mut effects = self.abort_current();
                effects.extend(self.start_attach(now));
                effects
            }
            AttachEvent::Control { frame } => self.handle_control(frame),
            AttachEvent::AttachEnded { attempt, outcome } => self.handle_ended(attempt, outcome, now),
            AttachEvent::Tick => self.handle_tick(now),
            AttachEvent::CommandFinished { request_id } => {
                if let AttachState::Attached { in_flight, .. } = &mut self.state {
                    in_flight.remove(&request_id);
                }
                Vec::new()
            }
            AttachEvent::StateChanged { hash } => {
                if let AttachState::Attached { connection_id, published_hash, .. } = &mut self.state {
                    if *published_hash == Some(hash) {
                        return Vec::new();
                    }
                    *published_hash = Some(hash);
                    return vec![AttachEffect::SendState { connection_id: connection_id.clone() }];
                }
                Vec::new()
            }
        }
    }

    fn start_attach(&mut self, now: Instant) -> Vec<AttachEffect> {
        self.start_attach_with_failures(now, 0)
    }

    fn start_attach_with_failures(&mut self, now: Instant, failures: u32) -> Vec<AttachEffect> {
        let mut effects = self.abort_current();
        self.next_attempt = self.next_attempt.saturating_add(1);
        let attempt = self.next_attempt;
        let connection_id = new_connection_id();
        let deadline = now + Duration::from_millis(REGISTER_TIMEOUT_MS);
        self.state = AttachState::Connecting {
            attempt,
            connection_id: connection_id.clone(),
            failures,
            deadline,
        };
        effects.push(AttachEffect::StartAttach { attempt, connection_id });
        effects.push(AttachEffect::ScheduleTimer { at: deadline });
        effects
    }

    fn handle_control(&mut self, frame: ControlFrame) -> Vec<AttachEffect> {
        match frame {
            ControlFrame::Foreign | ControlFrame::Unrecognized { .. } => Vec::new(),
            ControlFrame::Incompatible => {
                let mut effects = self.abort_current();
                self.state = AttachState::Unsupported;
                effects.push(AttachEffect::SetMcpSuppressed { suppressed: false });
                effects.push(AttachEffect::SettlePromptGates);
                effects
            }
            ControlFrame::Attached => {
                let AttachState::Connecting { attempt, connection_id, .. } = &self.state else {
                    return Vec::new();
                };
                let (attempt, connection_id) = (*attempt, connection_id.clone());
                self.state = AttachState::Attached {
                    attempt,
                    connection_id: connection_id.clone(),
                    in_flight: BTreeSet::new(),
                    published_hash: None,
                };
                vec![
                    AttachEffect::SetMcpSuppressed { suppressed: true },
                    AttachEffect::SendState { connection_id },
                    AttachEffect::SettlePromptGates,
                ]
            }
            ControlFrame::Command { request_id } => {
                let AttachState::Attached { connection_id, in_flight, .. } = &mut self.state else {
                    return Vec::new();
                };
                if !in_flight.insert(request_id.clone()) {
                    return Vec::new();
                }
                vec![AttachEffect::FetchCommand {
                    connection_id: connection_id.clone(),
                    request_id,
                }]
            }
            ControlFrame::Cancel { request_id } => {
                let AttachState::Attached { in_flight, .. } = &mut self.state else {
                    return Vec::new();
                };
                if in_flight.remove(&request_id) {
                    vec![AttachEffect::CancelCommands { request_ids: vec![request_id] }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    fn handle_ended(&mut self, attempt: u64, outcome: AttachOutcome, now: Instant) -> Vec<AttachEffect> {
        let (matches_attempt, was_attached, prev_failures, in_flight) = match &mut self.state {
            AttachState::Connecting { attempt: cur, failures, .. } => (*cur == attempt, false, *failures, Vec::new()),
            AttachState::Attached { attempt: cur, in_flight, .. } => {
                (*cur == attempt, true, 0, std::mem::take(in_flight).into_iter().collect())
            }
            _ => (false, false, 0, Vec::new()),
        };
        if !matches_attempt {
            return Vec::new();
        }
        let mut effects = Vec::new();
        if !in_flight.is_empty() {
            effects.push(AttachEffect::CancelCommands { request_ids: in_flight });
        }
        if was_attached {
            effects.push(AttachEffect::SetMcpSuppressed { suppressed: false });
        }
        match outcome {
            AttachOutcome::Unsupported => {
                self.state = AttachState::Unsupported;
            }
            AttachOutcome::Unavailable => {
                self.state = AttachState::Unavailable;
            }
            AttachOutcome::Replaced => {
                self.state = AttachState::Blocked { reason: BlockReason::Replaced };
            }
            AttachOutcome::Missing => {
                self.state = AttachState::Blocked { reason: BlockReason::Missing };
            }
            AttachOutcome::Closed | AttachOutcome::Transport(_) => {
                let failures = prev_failures.saturating_add(1);
                let delay = backoff_delay(failures);
                let next_at = now + delay;
                self.state = AttachState::Backoff { attempt, failures, next_at };
                effects.push(AttachEffect::ScheduleTimer { at: next_at });
            }
        }
        effects.push(AttachEffect::SettlePromptGates);
        effects
    }

    fn handle_tick(&mut self, now: Instant) -> Vec<AttachEffect> {
        match &self.state {
            AttachState::Connecting { attempt, failures, deadline, .. } if now >= *deadline => {
                let attempt = *attempt;
                let next_failures = failures.saturating_add(1);
                let next_at = now + backoff_delay(next_failures);
                self.state = AttachState::Backoff { attempt, failures: next_failures, next_at };
                vec![
                    AttachEffect::AbortAttach { attempt },
                    AttachEffect::ScheduleTimer { at: next_at },
                    AttachEffect::SettlePromptGates,
                ]
            }
            AttachState::Backoff { failures, next_at, .. } if now >= *next_at => {
                let prev_failures = *failures;
                self.start_attach_with_failures(now, prev_failures)
            }
            _ => Vec::new(),
        }
    }

    fn block(&mut self, reason: BlockReason) -> Vec<AttachEffect> {
        if matches!(self.state, AttachState::Blocked { reason: cur } if cur == reason) {
            return Vec::new();
        }
        let mut effects = self.abort_current();
        self.state = AttachState::Blocked { reason };
        effects.push(AttachEffect::SettlePromptGates);
        effects
    }

    fn dispose(&mut self) -> Vec<AttachEffect> {
        let mut effects = self.abort_current();
        self.state = AttachState::Closed;
        effects.push(AttachEffect::SettlePromptGates);
        effects
    }

    fn abort_current(&mut self) -> Vec<AttachEffect> {
        let mut effects = Vec::new();
        match &mut self.state {
            AttachState::Connecting { attempt, .. } => {
                effects.push(AttachEffect::AbortAttach { attempt: *attempt });
            }
            AttachState::Attached { attempt, in_flight, .. } => {
                let ids: Vec<String> = std::mem::take(in_flight).into_iter().collect();
                if !ids.is_empty() {
                    effects.push(AttachEffect::CancelCommands { request_ids: ids });
                }
                effects.push(AttachEffect::AbortAttach { attempt: *attempt });
                effects.push(AttachEffect::SetMcpSuppressed { suppressed: false });
            }
            _ => {}
        }
        effects
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let shift = failures.saturating_sub(1).min(5);
    let ms = RETRY_BASE_MS.saturating_mul(1u64 << shift).min(RETRY_MAX_MS);
    Duration::from_millis(ms)
}
