//! When a tab's load wait is over (`nav::step_wait_load`), as a pure function of what the wait
//! sees. It depends on `std` only, so a table program outside the repo can include it by path.
//!
//! Covers:
//! - Back/forward commit settling (BFCache + CDP main-frame events + same-document).
//! - `navigationProgressDeadline` (`packages/gui-plugins/src/browser/navigation-deadline.ts`,
//!   opencode#66217): distinguishes a committed document still streaming subresources at the 15 s
//!   deadline (`DeadlineOutcome::StillLoading`) from a true no-progress stall (`TimedOut`).
//! - `replacementNavigation` (`packages/gui-plugins/src/browser/navigation-turn.ts`,
//!   opencode#66217): distinguishes a client redirect / superseding navigation (`ERR_ABORTED`)
//!   that commits a replacement document from a genuine no-commit abort (stop/download/204).

use std::time::Duration;

/// Baseline captured when a navigation starts so we can prove whether a new document committed.
#[derive(Clone, Debug)]
pub(crate) struct CommitBaseline {
    pub document: u64,
    pub contexts_cleared: u64,
    pub frame_navigations: u64,
    pub is_new_tab: bool,
}

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
    pub document: u64,
}

/// The frame tracker's main-frame counts now (`TabFrameState`).
#[derive(Clone, Copy)]
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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DeadlineOutcome {
    /// Main document already committed while subresources keep loading (`navigationProgressDeadline`).
    StillLoading,
    /// No document committed before the 15 s navigation deadline (`NavigationDeadlineError`).
    TimedOut,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoadErrorDisposition {
    /// Client redirect / replacement navigation is still loading after an `ERR_ABORTED` hop.
    WaitReplacement,
    /// A replacement document already committed despite an earlier `ERR_ABORTED` hop.
    CommittedOk,
    /// Aborted with no committed document (stop, cancelled, download, or 204).
    AbortedNoCommit,
    /// Concrete network/navigation error.
    Failed,
}

/// The wait: its history start (back/forward only), whether it saw the tab loading, how long it
/// has run and when it gives up.
pub(crate) struct Waiting<'a> {
    pub history: Option<&'a HistoryFrom>,
    pub saw_loading: bool,
    pub elapsed: Duration,
    pub deadline: Duration,
}

pub(crate) fn is_committed(base: &CommitBaseline, tab: TabNow<'_>, frames: Option<FramesNow>) -> bool {
    if tab.document > base.document {
        return true;
    }
    if let Some(f) = frames {
        if f.frame_navigations > base.frame_navigations {
            return true;
        }
        if f.contexts_cleared > base.contexts_cleared && f.main_context {
            return true;
        }
    }
    if base.is_new_tab && !tab.url.is_empty() && tab.url != "about:blank" {
        return tab.document > 0 || frames.is_some_and(|f| f.frame_navigations > 0);
    }
    false
}

pub(crate) fn deadline_outcome(
    base: &CommitBaseline,
    tab: TabNow<'_>,
    frames: Option<FramesNow>,
) -> DeadlineOutcome {
    if is_committed(base, tab, frames) {
        DeadlineOutcome::StillLoading
    } else {
        DeadlineOutcome::TimedOut
    }
}

pub(crate) fn is_aborted_load_error(err: &str) -> bool {
    let trimmed = err.trim();
    trimmed.eq_ignore_ascii_case("ERR_ABORTED")
        || trimmed.contains("ERR_ABORTED")
        || trimmed == "-3"
}

pub(crate) fn classify_load_error(
    err: &str,
    base: &CommitBaseline,
    tab: TabNow<'_>,
    frames: Option<FramesNow>,
) -> LoadErrorDisposition {
    if !is_aborted_load_error(err) {
        return LoadErrorDisposition::Failed;
    }
    if is_committed(base, tab, frames) {
        return LoadErrorDisposition::CommittedOk;
    }
    if tab.loading {
        return LoadErrorDisposition::WaitReplacement;
    }
    LoadErrorDisposition::AbortedNoCommit
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
