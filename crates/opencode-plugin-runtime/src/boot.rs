//! Explicit state machine for staged plugin host startup (`Boot`, `Folders`, `Doze`) and
//! manifest platform/channel filtering.

use opencode_plugin_spec::{Channel, Manifest, Platform};

use crate::sources::Found;

/// The plugin folders (`--plugin-dir`, overrides, installed plugins) of a window: not read yet,
/// being read on a worker, or in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Folders {
    #[default]
    Unread,
    Reading,
    Read,
}

/// Whether the built-ins queued in `later` that have a record of this build still owe their
/// after-frame doze (`doze_owed`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Doze {
    #[default]
    Done,
    Owed,
}

/// Where a window's plugin host boot stands: unstarted, or started with its folder scan, deferred
/// built-ins, and one-shot after-frame doze latch.
#[derive(Default)]
pub enum Boot {
    #[default]
    Unstarted,
    Started {
        /// Where reading the plugin folders stands.
        folders: Folders,
        /// Built-ins this launch's first screen doesn't show, started after its first frame.
        later: Vec<Found>,
        /// The boot record knows the built-ins' records, and those of `later` that have one
        /// haven't gone to sleep yet (`doze_owed`).
        doze: Doze,
    },
}

pub enum BootEvent {
    Start { later: Vec<Found>, doze: Doze },
    Doze,
    FirstFrame,
    FoldersLoaded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootEffect {
    Doze,
    StartLater,
    ReadFolders,
}

impl Boot {
    pub fn is_started(&self) -> bool {
        matches!(self, Self::Started { .. })
    }

    pub fn later(&self) -> &[Found] {
        match self {
            Self::Unstarted => &[],
            Self::Started { later, .. } => later,
        }
    }

    pub fn take_later(&mut self) -> Vec<Found> {
        match self {
            Self::Unstarted => Vec::new(),
            Self::Started { later, .. } => std::mem::take(later),
        }
    }

    pub fn push_later(&mut self, found: Found) {
        if let Self::Started { later, .. } = self {
            later.push(found);
        }
    }

    pub fn extend_later(&mut self, found: Vec<Found>) {
        if let Self::Started { later, .. } = self {
            later.extend(found);
        }
    }

    pub fn set_later(&mut self, keep: Vec<Found>) {
        if let Self::Started { later, .. } = self {
            *later = keep;
        }
    }

    pub fn pop_front_later(&mut self) -> Option<Found> {
        match self {
            Self::Started { later, .. } if !later.is_empty() => Some(later.remove(0)),
            _ => None,
        }
    }

    pub fn step(&mut self, event: BootEvent) -> Vec<BootEffect> {
        match (self, event) {
            (boot @ Self::Unstarted, BootEvent::Start { later, doze }) => {
                *boot = Self::Started { folders: Folders::Unread, later, doze };
                Vec::new()
            }
            (Self::Started { doze: doze @ Doze::Owed, .. }, BootEvent::Doze) => {
                *doze = Doze::Done;
                vec![BootEffect::Doze]
            }
            (Self::Started { folders, .. }, BootEvent::FirstFrame) => {
                let mut effects = vec![BootEffect::StartLater];
                if *folders == Folders::Unread {
                    *folders = Folders::Reading;
                    effects.push(BootEffect::ReadFolders);
                }
                effects
            }
            (Self::Started { folders, .. }, BootEvent::FoldersLoaded) => {
                *folders = Folders::Read;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// Whether `m` targets this operating system and release `channel`.
pub fn runs_here(m: &Manifest, channel: Channel) -> bool {
    let here = if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    };
    (m.os.is_empty() || m.os.contains(&here)) && (m.channels.is_empty() || m.channels.contains(&channel))
}
