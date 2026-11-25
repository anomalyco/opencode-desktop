//! A window's life as the app's window list sees it: open, a torn-out tab's window still
//! following the pointer, closing, closed, kept. The list saved for the next launch holds the
//! open windows, so a quit mid-tear doesn't bring the tab back twice, and a window kept at its
//! close: the app ends with it, and the quit that follows saves the list again.

/// Where a window is in its life.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Life {
    /// A window of its own, which opens again next launch.
    #[default]
    Open,
    /// A torn-out tab's window following the pointer: it goes if the tab comes back to its
    /// strip or lands on another, and stays if the tab is let go elsewhere.
    Torn,
    /// Its close runs; it leaves the list.
    Closing,
    /// Gone from the list.
    Closed,
    /// Closed as the app ends (its last window, or a quit): off the screen and still in the
    /// list. Rune runs the last window's `App::on_quit` after its `on_close` (rune#1773), and
    /// that quit writes the list: without this state it wrote an empty one, and the next launch
    /// opened a new window with none of the tabs.
    Kept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifeEvent {
    /// The torn tab was let go away from every strip: its window stays.
    Kept,
    /// The window closes (its close button, Close Window, the tab coming back). `keep`: it
    /// opens again next launch where it was, as the app quits or it was the last window.
    Closing { keep: bool },
    /// Its close finished.
    Closed,
}

/// What the window list does for a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifeEffect {
    None,
    /// The list saved for the next launch changes.
    SaveIds,
    /// The list changes, and the window's files go: it won't open again.
    Forget,
    /// Its frame is saved, to open again where it was.
    Keep,
}

impl Life {
    pub fn step(self, event: LifeEvent) -> (Life, LifeEffect) {
        match (self, event) {
            (Life::Torn, LifeEvent::Kept) => (Life::Open, LifeEffect::SaveIds),
            (Life::Torn, LifeEvent::Closing { .. }) | (Life::Open, LifeEvent::Closing { keep: false }) => (Life::Closing, LifeEffect::Forget),
            (Life::Open, LifeEvent::Closing { keep: true }) => (Life::Kept, LifeEffect::Keep),
            (Life::Closing, LifeEvent::Closed) => (Life::Closed, LifeEffect::None),
            (state, _) => (state, LifeEffect::None),
        }
    }

    /// Whether the window is open and a window of its own (in the list saved for the next
    /// launch, and one the other windows count and reach).
    pub fn saved(self) -> bool {
        self == Life::Open
    }

    /// Whether the window is in the list saved for the next launch: the open ones and one kept
    /// at its close.
    pub fn listed(self) -> bool {
        matches!(self, Life::Open | Life::Kept)
    }
}
