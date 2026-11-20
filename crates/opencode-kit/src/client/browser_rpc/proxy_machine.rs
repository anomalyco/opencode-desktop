//! Explicit state machines for the desktop's loopback tunnel proxy and each tunnel's read pump
//! (`packages/plugin-browser/src/proxy-machine.ts`).

/// Lifecycle of the local loopback HTTP proxy listener.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyState {
    #[default]
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyEvent {
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyEffect {
    Shutdown,
}

impl ProxyState {
    pub fn admits(self) -> bool {
        self == Self::Open
    }

    pub fn step(&mut self, event: ProxyEvent) -> Option<ProxyEffect> {
        if *self == Self::Closed {
            return None;
        }
        match event {
            ProxyEvent::Close => {
                *self = Self::Closed;
                Some(ProxyEffect::Shutdown)
            }
        }
    }
}

/// Read-side pump state of one tunnel connection: at most one `tunnel.read` in flight, and no reads
/// after EOF or destruction (`BrowserProxyMachine.ReadState`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReadState {
    #[default]
    Idle,
    Reading,
    Ended,
    Destroyed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadEvent {
    Pull,
    Chunk,
    Eof,
    Destroyed,
}

impl ReadState {
    /// Steps the read pump; returns `true` iff the state changed.
    pub fn step(&mut self, event: ReadEvent) -> bool {
        let next = match (*self, event) {
            (Self::Destroyed, _) => Self::Destroyed,
            (_, ReadEvent::Destroyed) => Self::Destroyed,
            (Self::Idle, ReadEvent::Pull) => Self::Reading,
            (Self::Reading, ReadEvent::Eof) => Self::Ended,
            (Self::Reading, ReadEvent::Chunk) => Self::Idle,
            (same, _) => same,
        };
        let changed = next != *self;
        *self = next;
        changed
    }
}
