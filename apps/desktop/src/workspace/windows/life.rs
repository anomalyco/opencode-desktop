//! A window's life as the app's window list sees it: open, a torn-out tab's window still
//! following the pointer, closing, closed. Only open windows are in the list saved for the next
//! launch, so a quit mid-tear doesn't bring the tab back twice.

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
            (Life::Open, LifeEvent::Closing { keep: true }) => (Life::Closing, LifeEffect::Keep),
            (Life::Closing, LifeEvent::Closed) => (Life::Closed, LifeEffect::None),
            (state, _) => (state, LifeEffect::None),
        }
    }

    /// Whether the window is in the list saved for the next launch.
    pub fn saved(self) -> bool {
        self == Life::Open
    }
}
