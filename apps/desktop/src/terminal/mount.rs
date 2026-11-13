//! Which terminals keep an attachment, as Desktop's `gui-plugins/src/terminal/surface-machine.ts`
//! (`Surface`): a terminal mounts once it shows and stays mounted, so switching tabs keeps its
//! screen. One whose socket ended stays to say why while it shows and leaves once it doesn't, so
//! showing it again mounts a new attachment; one that ended under an older server connection
//! (before a restart) mounts again once the newer connection lists it. Like the TUI, a terminal
//! never reconnects by itself.
//!
//! The grid the pane last asked for, and a press selecting text, are explicit too.

use rune::machine::Effects;

/// A terminal's attachment. `connection` counts the server's connections when it mounted
/// (`sessionConnection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mount {
    Live { connection: u32 },
    /// The socket ended while the terminal showed; its last screen stays under the reason.
    Failed { connection: u32 },
    /// Not attached: showing it mounts a new attachment.
    Unmounted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MountEvent {
    /// Where the terminal is now: whether its pane shows it, and the server's connection.
    Sync { shown: bool, connection: u32 },
    /// The attachment's socket ended (`surface.failed`).
    Failed { shown: bool, connection: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MountEffect {
    /// Attach again: a new snapshot and socket.
    Mount,
}

impl Mount {
    pub fn step(&mut self, event: MountEvent) -> Effects<MountEffect> {
        let (next, effects) = match (*self, event) {
            (Mount::Live { connection: was }, MountEvent::Failed { shown: true, connection }) if connection > was => {
                (Mount::Live { connection }, Effects::one(MountEffect::Mount))
            }
            (Mount::Live { connection }, MountEvent::Failed { shown: true, .. }) => (Mount::Failed { connection }, Effects::none()),
            (Mount::Live { .. }, MountEvent::Failed { shown: false, .. }) => (Mount::Unmounted, Effects::none()),
            (Mount::Failed { .. }, MountEvent::Sync { shown: false, .. }) => (Mount::Unmounted, Effects::none()),
            (Mount::Failed { connection: was }, MountEvent::Sync { shown: true, connection }) if connection > was => {
                (Mount::Live { connection }, Effects::one(MountEffect::Mount))
            }
            (Mount::Unmounted, MountEvent::Sync { shown: true, connection }) => (Mount::Live { connection }, Effects::one(MountEffect::Mount)),
            (same, _) => (same, Effects::none()),
        };
        *self = next;
        effects
    }
}

/// The grid the pane last asked its session for: none until the pane first measures
/// (`pane-machine.ts` `PaneSizing.size`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fit {
    #[default]
    Unmeasured,
    Fitted { cols: u16, rows: u16, cell_px: (u32, u32) },
}

/// A press on the screen selecting text: idle, or held from the cell it started on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextDrag {
    #[default]
    Idle,
    Selecting { anchor: (u16, u16) },
    /// A Shift+press moving the selection's end.
    Extending,
}
