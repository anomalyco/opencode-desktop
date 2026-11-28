//! How an engine process ended, as `Fault::Stopped` names it.

/// An engine process's exit code, or on Unix the signal that ended it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Code(u32),
    Signal(i32),
}

impl std::fmt::Display for Ended {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Windows' codes are NTSTATUS values, read in hex; Unix's are small numbers.
            Self::Code(code) if cfg!(windows) => write!(f, "exit code {code:#010x}"),
            Self::Code(code) => write!(f, "exit code {code}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
        }
    }
}
