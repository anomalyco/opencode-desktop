//! Explicit sidecar connection lifecycle state machine for [`crate::client::BrowserHost`].

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use crate::installer::EngineStatus;
use crate::protocol::HostMsg;

/// Tabs a connect sent itself, by id: the address and size (width, height in points, scale)
/// each opened at.
#[cfg(feature = "engine")]
pub(crate) type EarlyOpensMap = HashMap<u64, (String, (f32, f32, f32))>;

/// Stage a connected engine was in before it stopped answering (`LiveStage::Hung`).
#[cfg(feature = "engine")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HungBefore {
    Connecting,
    Ready,
}

/// Lifecycle of a live socket connection to the sidecar engine.
#[cfg(feature = "engine")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveStage {
    /// Socket attached (`Hello` queued), waiting for `EngineMsg::Ready`.
    Connecting,
    /// `EngineMsg::Ready` received.
    Ready,
    /// No answer within `ANSWER_DEADLINE`; a late message restores `before`.
    Hung { before: HungBefore },
}

/// Phase of the engine sidecar connection protected by [`crate::client::BrowserHost::shared`].
///
/// Replaces separate `status`, `outbox`, `engine` (PID), `early`, and `conn_gen` primitives so
/// a disconnected host cannot carry a live outbox or unconsumed early opens, can retain a child
/// PID only in `HungDetached` (until the process exits or Retry kills it), and a stale connection
/// generation cannot overwrite a newer one.
pub(crate) enum EnginePhase {
    /// No live outbox: absent, downloading, spawning before attach, lost/failed, shut down, or a
    /// mock preview tab (`EngineStatus::Ready` without a sidecar).
    Disconnected(EngineStatus),
    /// Disconnected via `Shutdown` or `Unprepare` while hung, with our spawned child process still
    /// potentially alive until its watcher reports exit (`ProcessExited`) or Retry kills it (`EndHung`).
    #[cfg(feature = "engine")]
    HungDetached {
        /// Connection generation that spawned `pid`, so a stale watcher from an earlier engine
        /// cannot clear a newer engine's PID even if the OS recycles it.
        spawn_gen: u64,
        pid: u32,
    },
    /// Connected to a sidecar over its Unix socket.
    #[cfg(feature = "engine")]
    Connected {
        stage: LiveStage,
        outbox: Sender<HostMsg>,
        /// PID of the child this app spawned (`None` when attached to a pre-existing socket).
        pid: Option<u32>,
        /// Tabs `wire_stream` sent behind `Hello`, drained once by `flush_pending_opens`.
        early: EarlyOpensMap,
    },
    /// Synthetic outbox installed by `BrowserHost::capture_sent`, keeping arbitrary status.
    Captured {
        status: EngineStatus,
        outbox: Sender<HostMsg>,
    },
}

/// Shared connection generation and phase guarded by one mutex across UI and sidecar threads.
pub(crate) struct SharedEngine {
    pub(crate) generation: u64,
    pub(crate) phase: EnginePhase,
}

pub(crate) enum EngineEvent {
    MockStatus(EngineStatus),
    Capture(Sender<HostMsg>),
    Ready,
    Shutdown,
    #[cfg(feature = "engine")]
    BeginConnect,
    #[cfg(feature = "engine")]
    EndHung,
    #[cfg(feature = "engine")]
    Unprepare,
    #[cfg(feature = "engine")]
    Attach { generation: u64, outbox: Sender<HostMsg>, early: EarlyOpensMap },
    #[cfg(feature = "engine")]
    ProcessSpawned { generation: u64, pid: u32 },
    #[cfg(feature = "engine")]
    ProcessExited { generation: u64, pid: u32 },
    #[cfg(feature = "engine")]
    LinkStatus { generation: u64, status: EngineStatus },
    #[cfg(feature = "engine")]
    InstallStatus(EngineStatus),
    #[cfg(feature = "engine")]
    Hung { generation: u64 },
    #[cfg(feature = "engine")]
    Recovered { generation: u64 },
    #[cfg(feature = "engine")]
    Lost { generation: u64, fault: crate::installer::Fault },
}

pub(crate) enum EngineEffect {
    None,
    ReadyApplied,
    Shutdown { outbox: Option<Sender<HostMsg>> },
    #[cfg(feature = "engine")]
    Generation(u64),
    #[cfg(feature = "engine")]
    EndHung { kill_pid: Option<u32> },
    #[cfg(feature = "engine")]
    Rejected,
    #[cfg(feature = "engine")]
    Wake,
    #[cfg(feature = "engine")]
    LostFirst,
}

impl SharedEngine {
    pub(crate) fn new(initial: EngineStatus) -> Self {
        Self { generation: 0, phase: EnginePhase::Disconnected(initial) }
    }

    pub(crate) fn status(&self) -> EngineStatus {
        match &self.phase {
            EnginePhase::Disconnected(s) => s.clone(),
            #[cfg(feature = "engine")]
            EnginePhase::HungDetached { .. } => EngineStatus::Failed(crate::installer::Fault::Hung.into()),
            #[cfg(feature = "engine")]
            EnginePhase::Connected { stage, .. } => match stage {
                LiveStage::Connecting => EngineStatus::Connecting,
                LiveStage::Ready => EngineStatus::Ready,
                LiveStage::Hung { .. } => EngineStatus::Failed(crate::installer::Fault::Hung.into()),
            },
            EnginePhase::Captured { status, .. } => status.clone(),
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        match &self.phase {
            EnginePhase::Disconnected(_) => false,
            #[cfg(feature = "engine")]
            EnginePhase::HungDetached { .. } => false,
            #[cfg(feature = "engine")]
            EnginePhase::Connected { .. } => true,
            EnginePhase::Captured { .. } => true,
        }
    }

    pub(crate) fn outbox(&self) -> Option<&Sender<HostMsg>> {
        match &self.phase {
            EnginePhase::Disconnected(_) => None,
            #[cfg(feature = "engine")]
            EnginePhase::HungDetached { .. } => None,
            #[cfg(feature = "engine")]
            EnginePhase::Connected { outbox, .. } => Some(outbox),
            EnginePhase::Captured { outbox, .. } => Some(outbox),
        }
    }

    #[cfg(feature = "engine")]
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub(crate) fn take_early(&mut self) -> HashMap<u64, (String, (f32, f32, f32))> {
        match &mut self.phase {
            #[cfg(feature = "engine")]
            EnginePhase::Connected { early, .. } => std::mem::take(early),
            #[cfg(feature = "engine")]
            EnginePhase::HungDetached { .. } => HashMap::new(),
            EnginePhase::Disconnected(_) | EnginePhase::Captured { .. } => HashMap::new(),
        }
    }

    pub(crate) fn step(&mut self, event: EngineEvent) -> EngineEffect {
        match event {
            EngineEvent::MockStatus(next) => {
                // Accepted difference from the old separate `status`/`outbox` locks, which only
                // overwrote `status` and left `outbox` intact: turning `Connected` into
                // `Disconnected` is safe because `MockStatus` is only called by `set_mock_status`,
                // `insert_mock_tab`, and `with_socket` (stories, fixtures, and fresh hosts), never
                // on a live sidecar connection.
                self.phase = match std::mem::replace(&mut self.phase, EnginePhase::Disconnected(EngineStatus::Unsupported)) {
                    EnginePhase::Captured { outbox, .. } => EnginePhase::Captured { status: next, outbox },
                    _ => EnginePhase::Disconnected(next),
                };
                EngineEffect::None
            }
            EngineEvent::Capture(outbox) => {
                let status = self.status();
                self.phase = EnginePhase::Captured { status, outbox };
                EngineEffect::None
            }
            EngineEvent::Ready => match &mut self.phase {
                #[cfg(feature = "engine")]
                EnginePhase::Connected { stage, .. } => {
                    *stage = LiveStage::Ready;
                    EngineEffect::ReadyApplied
                }
                EnginePhase::Captured { status, .. } => {
                    *status = EngineStatus::Ready;
                    EngineEffect::ReadyApplied
                }
                #[cfg(feature = "engine")]
                EnginePhase::HungDetached { .. } => EngineEffect::None,
                EnginePhase::Disconnected(_) => EngineEffect::None,
            },
            EngineEvent::Shutdown => {
                #[cfg(feature = "engine")]
                let prev_gen = self.generation;
                self.generation = self.generation.wrapping_add(1);
                let prev = std::mem::replace(&mut self.phase, EnginePhase::Disconnected(EngineStatus::Unsupported));
                let (next_phase, outbox) = match prev {
                    #[cfg(feature = "engine")]
                    EnginePhase::Connected { stage: LiveStage::Hung { .. }, outbox, pid: Some(pid), .. } => {
                        (EnginePhase::HungDetached { spawn_gen: prev_gen, pid }, Some(outbox))
                    }
                    #[cfg(feature = "engine")]
                    EnginePhase::Connected { stage: LiveStage::Hung { .. }, outbox, pid: None, .. } => {
                        (EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into())), Some(outbox))
                    }
                    #[cfg(feature = "engine")]
                    EnginePhase::Connected { outbox, .. } => (EnginePhase::Disconnected(EngineStatus::Installed), Some(outbox)),
                    #[cfg(feature = "engine")]
                    detached @ EnginePhase::HungDetached { .. } => (detached, None),
                    EnginePhase::Captured { status, outbox } => {
                        let next = match status {
                            EngineStatus::Ready | EngineStatus::Connecting => EngineStatus::Installed,
                            other => other,
                        };
                        (EnginePhase::Disconnected(next), Some(outbox))
                    }
                    EnginePhase::Disconnected(status) => {
                        let next = match status {
                            EngineStatus::Ready | EngineStatus::Connecting => EngineStatus::Installed,
                            other => other,
                        };
                        (EnginePhase::Disconnected(next), None)
                    }
                };
                self.phase = next_phase;
                EngineEffect::Shutdown { outbox }
            }
            #[cfg(feature = "engine")]
            EngineEvent::BeginConnect => {
                self.generation = self.generation.wrapping_add(1);
                self.phase = EnginePhase::Disconnected(EngineStatus::Connecting);
                EngineEffect::Generation(self.generation)
            }
            #[cfg(feature = "engine")]
            EngineEvent::EndHung => match &self.phase {
                EnginePhase::Connected { stage: LiveStage::Hung { .. }, pid, .. } => {
                    let kill_pid = *pid;
                    self.generation = self.generation.wrapping_add(1);
                    self.phase = EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into()));
                    EngineEffect::EndHung { kill_pid }
                }
                EnginePhase::HungDetached { pid, .. } => {
                    let kill_pid = Some(*pid);
                    self.phase = EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into()));
                    EngineEffect::EndHung { kill_pid }
                }
                EnginePhase::Disconnected(s) if s.is_hung() => EngineEffect::EndHung { kill_pid: None },
                EnginePhase::Captured { status, .. } if status.is_hung() => {
                    self.phase = EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into()));
                    EngineEffect::EndHung { kill_pid: None }
                }
                _ => EngineEffect::None,
            },
            #[cfg(feature = "engine")]
            EngineEvent::Unprepare => {
                if !self.is_connected() {
                    return EngineEffect::None;
                }
                let prev_gen = self.generation;
                self.generation = self.generation.wrapping_add(1);
                let prev = std::mem::replace(&mut self.phase, EnginePhase::Disconnected(EngineStatus::Unsupported));
                let (next_phase, outbox) = match prev {
                    EnginePhase::Connected { stage: LiveStage::Hung { .. }, outbox, pid: Some(pid), .. } => {
                        (EnginePhase::HungDetached { spawn_gen: prev_gen, pid }, Some(outbox))
                    }
                    EnginePhase::Connected { stage: LiveStage::Hung { .. }, outbox, pid: None, .. } => {
                        (EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into())), Some(outbox))
                    }
                    EnginePhase::Connected { outbox, .. } => (EnginePhase::Disconnected(EngineStatus::Installed), Some(outbox)),
                    EnginePhase::Captured { status, outbox } => {
                        let next = match status {
                            EngineStatus::Ready | EngineStatus::Connecting => EngineStatus::Installed,
                            other => other,
                        };
                        (EnginePhase::Disconnected(next), Some(outbox))
                    }
                    detached @ (EnginePhase::Disconnected(_) | EnginePhase::HungDetached { .. }) => (detached, None),
                };
                self.phase = next_phase;
                EngineEffect::Shutdown { outbox }
            }
            #[cfg(feature = "engine")]
            EngineEvent::Attach { generation, outbox, early } => {
                if self.generation != generation {
                    return EngineEffect::Rejected;
                }
                self.phase = EnginePhase::Connected { stage: LiveStage::Connecting, outbox, pid: None, early };
                EngineEffect::Wake
            }
            #[cfg(feature = "engine")]
            EngineEvent::ProcessSpawned { generation, pid } => {
                if self.generation == generation
                    && let EnginePhase::Connected { pid: slot, .. } = &mut self.phase
                {
                    *slot = Some(pid);
                }
                EngineEffect::None
            }
            #[cfg(feature = "engine")]
            EngineEvent::ProcessExited { generation, pid } => {
                match &mut self.phase {
                    EnginePhase::Connected { pid: slot, .. } if self.generation == generation && *slot == Some(pid) => {
                        *slot = None;
                    }
                    EnginePhase::HungDetached { spawn_gen, pid: hung_pid } if *spawn_gen == generation && *hung_pid == pid => {
                        self.phase = EnginePhase::Disconnected(EngineStatus::Failed(crate::installer::Fault::Hung.into()));
                    }
                    _ => {}
                }
                EngineEffect::None
            }
            #[cfg(feature = "engine")]
            EngineEvent::LinkStatus { generation, status } => {
                if self.generation != generation || !matches!(self.phase, EnginePhase::Disconnected(_)) {
                    return EngineEffect::None;
                }
                self.phase = EnginePhase::Disconnected(status);
                EngineEffect::Wake
            }
            #[cfg(feature = "engine")]
            EngineEvent::InstallStatus(status) => {
                if !matches!(self.phase, EnginePhase::Disconnected(_)) {
                    return EngineEffect::None;
                }
                self.phase = EnginePhase::Disconnected(status);
                EngineEffect::Wake
            }
            #[cfg(feature = "engine")]
            EngineEvent::Hung { generation } => {
                if self.generation != generation {
                    return EngineEffect::None;
                }
                if let EnginePhase::Connected { stage, .. } = &mut self.phase {
                    let before = match *stage {
                        LiveStage::Connecting => HungBefore::Connecting,
                        LiveStage::Ready => HungBefore::Ready,
                        LiveStage::Hung { .. } => return EngineEffect::None,
                    };
                    *stage = LiveStage::Hung { before };
                    return EngineEffect::Wake;
                }
                EngineEffect::None
            }
            #[cfg(feature = "engine")]
            EngineEvent::Recovered { generation } => {
                if self.generation != generation {
                    return EngineEffect::None;
                }
                if let EnginePhase::Connected { stage, .. } = &mut self.phase
                    && let LiveStage::Hung { before } = *stage
                {
                    *stage = match before {
                        HungBefore::Connecting => LiveStage::Connecting,
                        HungBefore::Ready => LiveStage::Ready,
                    };
                    return EngineEffect::Wake;
                }
                EngineEffect::None
            }
            #[cfg(feature = "engine")]
            EngineEvent::Lost { generation, fault } => {
                if self.generation != generation {
                    return EngineEffect::None;
                }
                match &self.phase {
                    EnginePhase::Connected { .. } | EnginePhase::Disconnected(EngineStatus::Connecting) => {
                        self.phase = EnginePhase::Disconnected(EngineStatus::Failed(fault.into()));
                        EngineEffect::LostFirst
                    }
                    EnginePhase::Disconnected(EngineStatus::Failed(f))
                        if f.fault == crate::installer::Fault::Stopped && fault == crate::installer::Fault::OutOfMemory =>
                    {
                        self.phase = EnginePhase::Disconnected(EngineStatus::Failed(fault.into()));
                        EngineEffect::Wake
                    }
                    _ => EngineEffect::None,
                }
            }
        }
    }
}

/// Capabilities advertised by the running (or most recently run) engine's `Ready` message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EngineCaps {
    #[default]
    Unknown,
    Known { downloads: bool, policy: bool, dialogs: bool },
}

impl EngineCaps {
    pub(crate) fn downloads(self) -> bool {
        matches!(self, Self::Known { downloads: true, .. })
    }

    pub(crate) fn policy(self) -> bool {
        matches!(self, Self::Known { policy: true, .. })
    }

    pub(crate) fn dialogs(self) -> bool {
        matches!(self, Self::Known { dialogs: true, .. })
    }
}

/// One-shot background update check per `BrowserHost`, started once the first page reaches `Ready`.
#[cfg(feature = "engine")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UpdateCheck {
    #[default]
    Pending,
    Started,
}

#[cfg(feature = "engine")]
impl UpdateCheck {
    pub(crate) fn is_pending(self) -> bool {
        self == Self::Pending
    }

    pub(crate) fn step(&mut self, has_tabs: bool, ready: bool) -> bool {
        if *self == Self::Pending && has_tabs && ready {
            *self = Self::Started;
            true
        } else {
            false
        }
    }
}
