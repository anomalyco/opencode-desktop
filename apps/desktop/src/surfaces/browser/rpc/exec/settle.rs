//! When a tab's load wait is over (`nav::step_wait_load`), as a pure function of what the wait
//! sees. It depends on `std` only, so a table program outside the repo can include it by path.
//!
//! Back/forward settle on their commit, not on loading: a page the back/forward cache restores
//! may never report loading, and its commit's DevTools events can come after its address. Each
//! bumps the tab's generation as it is applied, and the result goes out after the state taken
//! when it returns: one applied later lets the agent's next command be authorized against the old
//! generation and fail as `document_changed`. Measured on every new document (back, forward, four
//! configurations, 20 runs each): its address, the main frame's `Runtime.executionContextsCleared`
//! and, unless the engine runs extensions (which keep `Page.*` to themselves), the main frame's
//! `Page.frameNavigated`, and no other bump. The new document's main execution context must exist
//! again too: clearing removes it, and an evaluate before it is created fails as
//! `document_changed`. A same-document entry (pushState, anchor) bumps only its address, and ends
//! once that moved and the main frame's `Page.navigatedWithinDocument` arrived, with no new
//! document begun: official returns it as soon as the tab stops loading. An entry that sends none
//! of them (same address, no entry that way, or a same-document one while extensions keep `Page.*`)
//! settles at the grace, and a load the wait happened to see never ends a back/forward early.

use std::time::Duration;

/// Where a back/forward started: the tab's address and the frame tracker's counts of the main
/// frame's commits then (`TabFrameState`).
pub(crate) struct HistoryFrom {
    pub url: String,
    pub contexts_cleared: u64,
    pub frame_navigations: u64,
    pub same_document: u64,
}

/// The tab now.
pub(crate) struct TabNow<'a> {
    pub url: &'a str,
    pub loading: bool,
}

/// The frame tracker's main-frame counts now (`TabFrameState`).
pub(crate) struct FramesNow {
    pub contexts_cleared: u64,
    pub frame_navigations: u64,
    pub same_document: u64,
    /// The main frame has an execution context.
    pub main_context: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Settle {
    Wait,
    Done,
    /// The tab closed while the wait ran.
    Closed,
}

/// The wait: its history start (back/forward only), whether it saw the tab loading, how long it
/// has run and when it gives up.
pub(crate) struct Waiting<'a> {
    pub history: Option<&'a HistoryFrom>,
    pub saw_loading: bool,
    pub elapsed: Duration,
    pub deadline: Duration,
}

pub(crate) fn settle(w: &Waiting, tab: Option<TabNow>, frames: Option<FramesNow>, extensions_active: bool, grace: Duration) -> Settle {
    let Some(tab) = tab else { return Settle::Closed };
    if w.elapsed >= w.deadline {
        return Settle::Done;
    }
    if tab.loading {
        return Settle::Wait;
    }
    let ended = match w.history {
        Some(from) => tab.url != from.url && frames.is_some_and(|f| committed(from, &f, extensions_active) || same_document(from, &f)),
        None => w.saw_loading,
    };
    if ended || w.elapsed >= grace { Settle::Done } else { Settle::Wait }
}

fn committed(from: &HistoryFrom, f: &FramesNow, extensions_active: bool) -> bool {
    f.contexts_cleared > from.contexts_cleared
        // A tab whose main-frame navigations have reached the app before gets this one too,
        // whatever the app believes about extensions.
        && ((extensions_active && from.frame_navigations == 0) || f.frame_navigations > from.frame_navigations)
        && f.main_context
}

fn same_document(from: &HistoryFrom, f: &FramesNow) -> bool {
    f.same_document > from.same_document
        && f.contexts_cleared == from.contexts_cleared
        && f.frame_navigations == from.frame_navigations
        && f.main_context
}
