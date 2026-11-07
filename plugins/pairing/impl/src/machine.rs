//! The pairing page (`machine.ts`): the remote access setting, the dialog that turns it on, and
//! the display sleep setting. One transition owns them, so the dialog follows every change of
//! remote access in the same event. `issued` is the last attempt handed out.

/// How long one read of the remote access setting may take.
pub const REMOTE_READ_TIMEOUT_MS: i64 = 15_000;
/// How many times a failed read of either setting is tried again.
pub const REMOTE_READ_RETRIES: u32 = 3;
/// Turning remote access on the first time creates the tunnel, which can take a minute.
pub const REMOTE_CHANGE_TIMEOUT_MS: i64 = 180_000;
/// The service attaches the tunnel after it starts: asked again this many times, a second apart.
pub const TUNNEL_RETRIES: u32 = 180;
pub const TUNNEL_RETRY_DELAY_MS: i64 = 1_000;
pub const TUNNEL_TIMEOUT_MS: i64 = 10_000;
pub const CODE_TIMEOUT_MS: i64 = 15_000;
pub const SCREEN_TIMEOUT_MS: i64 = 15_000;

/// The wait before read retry `failures`: 1, 2, then 4 seconds.
pub fn read_delay(failures: u32) -> i64 {
    (1_000i64 << failures.min(5)).min(30_000)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Setting {
    Loading { attempt: u64, waiting: bool, failures: u32 },
    Ready(bool),
    Changing { attempt: u64, enabled: bool, restart: bool, previous: Option<bool> },
    Failed { previous: Option<bool> },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dialog {
    Closed,
    /// Neither the local server nor the service can set up pairing.
    Unavailable,
    /// Remote access is being read or turned on.
    TurningOn,
    /// Remote access is on; asking the server for its tunnel address until it advertises one.
    Tunnel { attempt: u64, waiting: bool, retries: u32 },
    Code { attempt: u64, tunnel: String },
    Ready { tunnel: String, code: String, url: String, expires_in: i64, loaded_at: i64, refreshing: Option<u64> },
    CodeError { tunnel: String },
    Failed { tunnel_failed: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct State {
    pub issued: u64,
    /// Whether the local server is there: codes come from it.
    pub server: bool,
    pub remote: Setting,
    pub screen: Setting,
    pub dialog: Dialog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Read,
    Screen,
    Tunnel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    ReadRemote { attempt: u64, timeout: i64 },
    SetRemote { attempt: u64, enabled: bool, restart: bool, timeout: i64 },
    ReadScreen { attempt: u64, timeout: i64 },
    SetScreen { attempt: u64, enabled: bool, timeout: i64 },
    PollTunnel { attempt: u64, timeout: i64 },
    Wait { kind: Wait, attempt: u64, delay: i64 },
    FetchCode { attempt: u64, timeout: i64 },
    /// Ends an attempt's request and timer.
    Abort(u64),
    OpenDialog,
}

pub enum Event {
    ReadDone { attempt: u64, enabled: Option<bool> },
    Toggled(bool),
    ChangeDone { attempt: u64, ok: bool },
    ScreenReadDone { attempt: u64, enabled: Option<bool> },
    ScreenToggled(bool),
    ScreenRetry,
    ScreenChangeDone { attempt: u64, ok: bool },
    DialogOpened,
    DialogClosed,
    TunnelFound { attempt: u64, tunnel: String },
    TunnelMissing { attempt: u64 },
    WaitEnded { kind: Wait, attempt: u64 },
    CodeLoaded { attempt: u64, code: String, expires_in: i64, now: i64 },
    CodeFailed { attempt: u64 },
    CodeRefresh,
    Retry,
    Tick { now: i64 },
}

type Out = Vec<Effect>;

impl State {
    /// The page as it opens: the server as known, and both settings read through the service.
    pub fn new(server: bool) -> (State, Out) {
        let state = State {
            issued: 2,
            server,
            remote: Setting::Loading { attempt: 1, waiting: false, failures: 0 },
            screen: Setting::Loading { attempt: 2, waiting: false, failures: 0 },
            dialog: Dialog::Closed,
        };
        (state, vec![Effect::ReadRemote { attempt: 1, timeout: REMOTE_READ_TIMEOUT_MS }, Effect::ReadScreen { attempt: 2, timeout: SCREEN_TIMEOUT_MS }])
    }

    fn next(&mut self) -> u64 {
        self.issued += 1;
        self.issued
    }

    pub fn apply(&mut self, event: Event) -> Out {
        match event {
            Event::ScreenReadDone { .. } | Event::ScreenToggled(_) | Event::ScreenRetry | Event::ScreenChangeDone { .. } | Event::WaitEnded { kind: Wait::Screen, .. } => self.screen_event(event),
            Event::ReadDone { attempt, enabled } => {
                let Setting::Loading { attempt: a, waiting: false, failures } = self.remote else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let mut out = vec![Effect::Abort(attempt)];
                match enabled {
                    Some(on) => self.remote = Setting::Ready(on),
                    None if failures >= REMOTE_READ_RETRIES => self.remote = Setting::Failed { previous: None },
                    None => {
                        let attempt = self.next();
                        self.remote = Setting::Loading { attempt, waiting: true, failures: failures + 1 };
                        out.push(Effect::Wait { kind: Wait::Read, attempt, delay: read_delay(failures) });
                        return out;
                    }
                }
                self.settle(out)
            }
            Event::WaitEnded { kind: Wait::Read, attempt } => {
                let Setting::Loading { attempt: a, waiting: true, failures } = self.remote else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let next = self.next();
                self.remote = Setting::Loading { attempt: next, waiting: false, failures };
                vec![Effect::Abort(attempt), Effect::ReadRemote { attempt: next, timeout: REMOTE_READ_TIMEOUT_MS }]
            }
            Event::Toggled(enabled) => match self.remote {
                Setting::Ready(on) if on != enabled => {
                    let out = self.change(Vec::new(), enabled, false);
                    self.settle(out)
                }
                // A change reads the setting before it writes, so the switch also works after a
                // read that never answered.
                Setting::Failed { .. } => {
                    let out = self.change(Vec::new(), enabled, false);
                    self.settle(out)
                }
                _ => Vec::new(),
            },
            Event::ChangeDone { attempt, ok } => {
                let Setting::Changing { attempt: a, enabled, previous, .. } = self.remote else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                self.remote = if ok { Setting::Ready(enabled) } else { Setting::Failed { previous } };
                self.settle(vec![Effect::Abort(attempt)])
            }
            Event::DialogOpened => {
                if self.dialog != Dialog::Closed {
                    return Vec::new();
                }
                self.dialog = Dialog::TurningOn;
                let out = vec![Effect::OpenDialog];
                // Every pairing goes through remote access, so opening the dialog tries again after a failure.
                let out = if matches!(self.remote, Setting::Failed { .. }) { self.change(out, true, false) } else { out };
                self.settle(out)
            }
            Event::DialogClosed => self.to_dialog(Vec::new(), Dialog::Closed),
            Event::Retry => {
                let Dialog::Failed { tunnel_failed } = self.dialog else { return Vec::new() };
                self.dialog = Dialog::TurningOn;
                // A tunnel that never connected comes back when the service starts it again.
                let out = if matches!(self.remote, Setting::Ready(_) | Setting::Failed { .. }) { self.change(Vec::new(), true, tunnel_failed) } else { Vec::new() };
                self.settle(out)
            }
            event => self.code_event(event),
        }
    }

    /// The display sleep setting: it reads and retries as remote access does. After a failure,
    /// Try again reads it afresh, and the switch stays enabled, so flipping it sets it outright.
    fn screen_event(&mut self, event: Event) -> Out {
        match event {
            Event::ScreenRetry => {
                if !matches!(self.screen, Setting::Failed { .. }) {
                    return Vec::new();
                }
                let attempt = self.next();
                self.screen = Setting::Loading { attempt, waiting: false, failures: 0 };
                vec![Effect::ReadScreen { attempt, timeout: SCREEN_TIMEOUT_MS }]
            }
            Event::ScreenReadDone { attempt, enabled } => {
                let Setting::Loading { attempt: a, waiting: false, failures } = self.screen else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let mut out = vec![Effect::Abort(attempt)];
                match enabled {
                    Some(on) => self.screen = Setting::Ready(on),
                    None if failures >= REMOTE_READ_RETRIES => self.screen = Setting::Failed { previous: None },
                    None => {
                        let attempt = self.next();
                        self.screen = Setting::Loading { attempt, waiting: true, failures: failures + 1 };
                        out.push(Effect::Wait { kind: Wait::Screen, attempt, delay: read_delay(failures) });
                    }
                }
                out
            }
            Event::WaitEnded { attempt, .. } => {
                let Setting::Loading { attempt: a, waiting: true, failures } = self.screen else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let next = self.next();
                self.screen = Setting::Loading { attempt: next, waiting: false, failures };
                vec![Effect::Abort(attempt), Effect::ReadScreen { attempt: next, timeout: SCREEN_TIMEOUT_MS }]
            }
            Event::ScreenToggled(enabled) => {
                let previous = match self.screen {
                    Setting::Ready(on) if on == enabled => return Vec::new(),
                    Setting::Ready(on) => Some(on),
                    Setting::Failed { previous } => previous,
                    _ => return Vec::new(),
                };
                let attempt = self.next();
                self.screen = Setting::Changing { attempt, enabled, restart: false, previous };
                vec![Effect::SetScreen { attempt, enabled, timeout: SCREEN_TIMEOUT_MS }]
            }
            Event::ScreenChangeDone { attempt, ok } => {
                let Setting::Changing { attempt: a, enabled, previous, .. } = self.screen else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                self.screen = if ok { Setting::Ready(enabled) } else { Setting::Failed { previous } };
                vec![Effect::Abort(attempt)]
            }
            _ => Vec::new(),
        }
    }

    /// The dialog's own requests: the tunnel address, then the code and its replacements.
    fn code_event(&mut self, event: Event) -> Out {
        match event {
            Event::TunnelFound { attempt, tunnel } => {
                let Dialog::Tunnel { attempt: a, waiting: false, .. } = self.dialog else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let next = self.next();
                self.dialog = Dialog::Code { attempt: next, tunnel };
                vec![Effect::Abort(attempt), Effect::FetchCode { attempt: next, timeout: CODE_TIMEOUT_MS }]
            }
            Event::TunnelMissing { attempt } => {
                let Dialog::Tunnel { attempt: a, waiting: false, retries } = self.dialog else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                if retries >= TUNNEL_RETRIES {
                    self.dialog = Dialog::Failed { tunnel_failed: true };
                    return vec![Effect::Abort(attempt)];
                }
                let next = self.next();
                self.dialog = Dialog::Tunnel { attempt: next, waiting: true, retries: retries + 1 };
                vec![Effect::Abort(attempt), Effect::Wait { kind: Wait::Tunnel, attempt: next, delay: TUNNEL_RETRY_DELAY_MS }]
            }
            Event::WaitEnded { kind: Wait::Tunnel, attempt } => {
                let Dialog::Tunnel { attempt: a, waiting: true, retries } = self.dialog else { return Vec::new() };
                if a != attempt {
                    return Vec::new();
                }
                let next = self.next();
                self.dialog = Dialog::Tunnel { attempt: next, waiting: false, retries };
                vec![Effect::Abort(attempt), Effect::PollTunnel { attempt: next, timeout: TUNNEL_TIMEOUT_MS }]
            }
            Event::CodeLoaded { attempt, .. } | Event::CodeFailed { attempt } if !self.current_code(attempt) => Vec::new(),
            Event::CodeLoaded { attempt, code, expires_in, now } => {
                let tunnel = self.tunnel().unwrap_or_default();
                let url = format!("{}/auth/connect/{code}", tunnel.trim_end_matches('/'));
                self.dialog = Dialog::Ready { tunnel, code, url, expires_in, loaded_at: now, refreshing: None };
                vec![Effect::Abort(attempt)]
            }
            Event::CodeFailed { attempt } => {
                self.dialog = Dialog::CodeError { tunnel: self.tunnel().unwrap_or_default() };
                vec![Effect::Abort(attempt)]
            }
            Event::CodeRefresh => match &self.dialog {
                Dialog::CodeError { tunnel } => {
                    let tunnel = tunnel.clone();
                    let attempt = self.next();
                    self.dialog = Dialog::Code { attempt, tunnel };
                    vec![Effect::FetchCode { attempt, timeout: CODE_TIMEOUT_MS }]
                }
                Dialog::Ready { refreshing: None, .. } => self.refresh(),
                _ => Vec::new(),
            },
            // Codes are single-use, so the link is replaced as soon as the server forgets the code.
            Event::Tick { now } => match &self.dialog {
                Dialog::Ready { refreshing: None, .. } if self.remaining(now) == 0 => self.refresh(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn tunnel(&self) -> Option<String> {
        match &self.dialog {
            Dialog::Code { tunnel, .. } | Dialog::Ready { tunnel, .. } | Dialog::CodeError { tunnel } => Some(tunnel.clone()),
            _ => None,
        }
    }

    fn current_code(&self, attempt: u64) -> bool {
        match &self.dialog {
            Dialog::Code { attempt: a, .. } => *a == attempt,
            Dialog::Ready { refreshing, .. } => *refreshing == Some(attempt),
            _ => false,
        }
    }

    fn refresh(&mut self) -> Out {
        let attempt = self.next();
        if let Dialog::Ready { refreshing, .. } = &mut self.dialog {
            *refreshing = Some(attempt);
        }
        vec![Effect::FetchCode { attempt, timeout: CODE_TIMEOUT_MS }]
    }

    /// Turns remote access on or off; the caller checked that no read or change is in flight.
    fn change(&mut self, mut out: Out, enabled: bool, restart: bool) -> Out {
        let previous = match self.remote {
            Setting::Ready(on) => Some(on),
            Setting::Failed { previous } => previous,
            _ => return out,
        };
        let attempt = self.next();
        self.remote = Setting::Changing { attempt, enabled, restart, previous };
        out.push(Effect::SetRemote { attempt, enabled, restart, timeout: REMOTE_CHANGE_TIMEOUT_MS });
        out
    }

    /// Moves an open dialog to the phase that follows from remote access and the server.
    fn settle(&mut self, out: Out) -> Out {
        match &self.dialog {
            Dialog::Closed => out,
            _ if !self.server => self.to_dialog(out, Dialog::Unavailable),
            Dialog::Tunnel { .. } | Dialog::Code { .. } | Dialog::Ready { .. } | Dialog::CodeError { .. } => match self.remote {
                Setting::Failed { .. } => self.to_dialog(out, Dialog::Failed { tunnel_failed: false }),
                Setting::Changing { .. } | Setting::Ready(false) => self.set_up(out),
                // The code needs only the server.
                _ => out,
            },
            Dialog::Failed { tunnel_failed } => {
                let tunnel_failed = *tunnel_failed;
                match self.remote {
                    Setting::Changing { .. } => self.to_dialog(out, Dialog::TurningOn),
                    Setting::Failed { .. } => self.to_dialog(out, Dialog::Failed { tunnel_failed: false }),
                    // A tunnel that never connected waits for Try again, which restarts the service.
                    Setting::Ready(true) if !tunnel_failed => self.start_tunnel(out),
                    _ => out,
                }
            }
            _ => self.set_up(out),
        }
    }

    fn set_up(&mut self, out: Out) -> Out {
        match self.remote {
            Setting::Failed { .. } => self.to_dialog(out, Dialog::Failed { tunnel_failed: false }),
            Setting::Ready(true) => self.start_tunnel(out),
            Setting::Ready(false) => {
                let out = self.change(out, true, false);
                self.to_dialog(out, Dialog::TurningOn)
            }
            _ => self.to_dialog(out, Dialog::TurningOn),
        }
    }

    fn start_tunnel(&mut self, mut out: Out) -> Out {
        out.extend(self.abort_dialog());
        let attempt = self.next();
        self.dialog = Dialog::Tunnel { attempt, waiting: false, retries: 0 };
        out.push(Effect::PollTunnel { attempt, timeout: TUNNEL_TIMEOUT_MS });
        out
    }

    fn to_dialog(&mut self, mut out: Out, next: Dialog) -> Out {
        if self.dialog == next {
            return out;
        }
        out.extend(self.abort_dialog());
        self.dialog = next;
        out
    }

    fn abort_dialog(&self) -> Out {
        match &self.dialog {
            Dialog::Tunnel { attempt, .. } | Dialog::Code { attempt, .. } => vec![Effect::Abort(*attempt)],
            Dialog::Ready { refreshing: Some(a), .. } => vec![Effect::Abort(*a)],
            _ => Vec::new(),
        }
    }

    /// Whole seconds until the shown code expires.
    pub fn remaining(&self, now: i64) -> i64 {
        let Dialog::Ready { expires_in, loaded_at, .. } = &self.dialog else { return 0 };
        (expires_in - (now - loaded_at).max(0) / 1000).max(0)
    }
}

/// What the page shows (`viewPairingPage`).
pub struct PageView {
    pub pair_disabled: bool,
    pub remote_checked: bool,
    pub remote_disabled: bool,
    pub remote_error: bool,
    pub screen_checked: bool,
    pub screen_disabled: bool,
    pub screen_error: bool,
    pub unavailable: bool,
}

pub fn page_view(s: &State) -> PageView {
    let checked = |set: &Setting| matches!(set, Setting::Changing { enabled: true, .. } | Setting::Ready(true) | Setting::Failed { previous: Some(true) });
    PageView {
        pair_disabled: !s.server || matches!(s.remote, Setting::Changing { enabled: false, .. }),
        remote_checked: checked(&s.remote),
        remote_disabled: !matches!(s.remote, Setting::Ready(_) | Setting::Failed { .. }),
        remote_error: matches!(s.remote, Setting::Failed { .. }),
        screen_checked: checked(&s.screen),
        screen_disabled: !matches!(s.screen, Setting::Ready(_) | Setting::Failed { .. }),
        screen_error: matches!(s.screen, Setting::Failed { .. }),
        unavailable: !s.server,
    }
}
