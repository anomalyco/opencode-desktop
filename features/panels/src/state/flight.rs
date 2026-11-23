//! A pane a poll opens flies on `SIDE_SPRING`, whatever else that poll lands.

use rune::Tx;

use crate::host::PanelsHost;

/// Whether a pane opened since this frame's poll began. The poll's systems share one `Tx`,
/// whose last `instant()` or `animated()` wins, so a system after the one that opened the pane
/// (a reply's link checks landing, the stream) would otherwise land the pane's flight at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneFlight {
    #[default]
    Idle,
    /// [`crate::state::show_pane_tab`] opened a pane since the poll began.
    Owed,
}

/// Events driving [`PaneFlight`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneFlightEvent {
    /// A pane opened.
    Opened,
    /// The frame's poll begins: a pane an input handler opened flew in that handler's commit.
    PollBegan,
    /// The frame's poll ended.
    PollEnded,
}

/// Effects returned by [`PaneFlight::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneFlightEffect {
    /// Animate the poll's commit (`Tx::animated`).
    Animate,
}

impl PaneFlight {
    pub fn step(&mut self, event: PaneFlightEvent) -> Option<PaneFlightEffect> {
        match event {
            PaneFlightEvent::Opened => {
                *self = Self::Owed;
                None
            }
            PaneFlightEvent::PollBegan => {
                *self = Self::Idle;
                None
            }
            PaneFlightEvent::PollEnded => (std::mem::take(self) == Self::Owed).then_some(PaneFlightEffect::Animate),
        }
    }
}

/// Steps the side state's [`PaneFlight`] without dirtying the `Tracked` slot it lives in.
pub(super) fn step(host: &dyn PanelsHost, event: PaneFlightEvent) -> Option<PaneFlightEffect> {
    let cell = &host.side().pane_flight;
    let mut flight = cell.get();
    let effect = flight.step(event);
    cell.set(flight);
    effect
}

/// Call before the frame's poll runs its systems.
pub fn begin_poll(host: &dyn PanelsHost) {
    step(host, PaneFlightEvent::PollBegan);
}

/// Call after the frame's poll ran its systems: a pane one of them opened flies.
pub fn end_poll(host: &dyn PanelsHost, tx: &mut Tx) {
    if step(host, PaneFlightEvent::PollEnded) == Some(PaneFlightEffect::Animate) {
        tx.animated();
    }
}
