//! Shared tab-strip pointer gestures matching official OpenCode Desktop (`tab-nav.tsx:457-465`,
//! `tabs.tsx:107-115`):
//!
//! - **Closable tabs (`closable_tab`):** a middle click closes the tab on release over the tab
//!   or its close button (whose press walks up to the tab), without switching tabs, starting a
//!   drag, changing focus, or starting middle-click autoscroll on button-down.
//! - **Non-closable session tabs (`non_closable_tab`):** consumes middle-button down as a no-op
//!   so middle-clicking a pinned or Recent session row inside a scrollable sidebar never starts
//!   middle-click autoscroll (`tab-nav.tsx:457-461`).

use rune::{El, Tx};

/// Shared tab gesture helpers on [`El`], alongside [`crate::focus::FocusExt`].
pub trait TabExt {
    /// Closes the tab on a middle click (release over the tab or its close button) and suppresses
    /// middle-down autoscroll.
    fn closable_tab<S: 'static>(self, on_close: impl Fn(&mut S, &mut Tx) + 'static) -> Self;

    /// Consumes middle clicks as a no-op so non-closable session tabs inside scrollable sidebars
    /// never start middle-click autoscroll.
    fn non_closable_tab<S: 'static>(self) -> Self;
}

impl TabExt for El {
    fn closable_tab<S: 'static>(self, on_close: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        self.on_middle_press(on_close)
    }

    fn non_closable_tab<S: 'static>(self) -> Self {
        self.on_middle_press(|_: &mut S, tx: &mut Tx| tx.live_only())
    }
}
