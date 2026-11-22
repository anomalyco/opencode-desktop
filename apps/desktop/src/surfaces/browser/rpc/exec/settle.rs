//! When a tab's load wait is over (`nav::step_wait_load`), as a pure function of what the wait
//! sees, so its cases are a table.
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
//! `document_changed`. An entry that sends none of them (same document, same address, no entry
//! that way) settles at the grace, and a load the wait happened to see never ends a back/forward
//! early.

use std::time::Duration;

use rune::time::Instant;

use super::nav::HistoryFrom;

/// The tab now.
pub(super) struct TabNow<'a> {
    pub url: &'a str,
    pub loading: bool,
}

/// The frame tracker's main-frame counts now (`TabFrameState`).
pub(super) struct FramesNow {
    pub contexts_cleared: u64,
    pub frame_navigations: u64,
    /// The main frame has an execution context.
    pub main_context: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Settle {
    Wait,
    Done,
    /// The tab closed while the wait ran.
    Closed,
}

/// The wait's state, from when it started.
pub(super) struct Waiting<'a> {
    pub history: Option<&'a HistoryFrom>,
    pub saw_loading: bool,
    pub started_at: Instant,
    pub deadline: Instant,
}

pub(super) fn settle(w: &Waiting, tab: Option<TabNow>, frames: Option<FramesNow>, extensions_active: bool, now: Instant, grace: Duration) -> Settle {
    let Some(tab) = tab else { return Settle::Closed };
    if now >= w.deadline {
        return Settle::Done;
    }
    if tab.loading {
        return Settle::Wait;
    }
    let ended = match w.history {
        Some(from) => tab.url != from.url && frames.is_some_and(|f| committed(from, &f, extensions_active)),
        None => w.saw_loading,
    };
    if ended || now >= w.started_at + grace { Settle::Done } else { Settle::Wait }
}

fn committed(from: &HistoryFrom, f: &FramesNow, extensions_active: bool) -> bool {
    f.contexts_cleared > from.contexts_cleared
        // A tab whose main-frame navigations have reached the app before gets this one too,
        // whatever the app believes about extensions.
        && ((extensions_active && from.frame_navigations == 0) || f.frame_navigations > from.frame_navigations)
        && f.main_context
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRACE: Duration = Duration::from_millis(1_200);

    struct Row {
        name: &'static str,
        history: bool,
        saw_loading: bool,
        url: &'static str,
        loading: bool,
        frames: Option<(u64, u64, bool)>,
        extensions: bool,
        closed: bool,
        /// Expected 100 ms in, and at the grace (1.2 s).
        early: Settle,
        at_grace: Settle,
    }

    #[test]
    fn when_a_load_wait_is_over() {
        // Every history wait starts on /app with 5 contexts cleared and 4 main-frame navigations.
        let rows = [
            Row { name: "cross-document commit + load end", history: true, saw_loading: true, url: "/page2", loading: false, frames: Some((6, 5, true)), extensions: false, closed: false, early: Settle::Done, at_grace: Settle::Done },
            Row { name: "same-document (pushState / hash) ends at the grace", history: true, saw_loading: false, url: "/app#s", loading: false, frames: Some((5, 4, true)), extensions: false, closed: false, early: Settle::Wait, at_grace: Settle::Done },
            Row { name: "no entry ends at the grace", history: true, saw_loading: false, url: "/app", loading: false, frames: Some((5, 4, true)), extensions: false, closed: false, early: Settle::Wait, at_grace: Settle::Done },
            Row { name: "iframe navigation does not end it", history: true, saw_loading: true, url: "/app", loading: false, frames: Some((5, 4, true)), extensions: false, closed: false, early: Settle::Wait, at_grace: Settle::Done },
            Row { name: "a load seen that is not the history move does not end it", history: true, saw_loading: true, url: "/page2", loading: false, frames: Some((5, 4, true)), extensions: false, closed: false, early: Settle::Wait, at_grace: Settle::Done },
            Row { name: "tab closed resolves", history: true, saw_loading: false, url: "/app", loading: false, frames: Some((5, 4, true)), extensions: false, closed: true, early: Settle::Closed, at_grace: Settle::Closed },
        ];
        for r in rows {
            let start = Instant::now();
            let from = HistoryFrom { url: "/app".into(), contexts_cleared: 5, frame_navigations: 4 };
            let w = Waiting { history: r.history.then_some(&from), saw_loading: r.saw_loading, started_at: start, deadline: start + Duration::from_secs(15) };
            let at = |ms: u64| {
                let tab = (!r.closed).then_some(TabNow { url: r.url, loading: r.loading });
                let frames = r.frames.map(|(c, n, m)| FramesNow { contexts_cleared: c, frame_navigations: n, main_context: m });
                settle(&w, tab, frames, r.extensions, start + Duration::from_millis(ms), GRACE)
            };
            assert_eq!(at(100), r.early, "{}: 100 ms in", r.name);
            assert_eq!(at(1_200), r.at_grace, "{}: at the grace", r.name);
        }
    }

    #[test]
    fn the_deadline_resolves_a_wait_still_loading_at_15_s() {
        let start = Instant::now();
        let from = HistoryFrom { url: "/app".into(), contexts_cleared: 5, frame_navigations: 4 };
        let w = Waiting { history: Some(&from), saw_loading: true, started_at: start, deadline: start + Duration::from_secs(15) };
        let at = |ms: u64| settle(&w, Some(TabNow { url: "/app", loading: true }), None, false, start + Duration::from_millis(ms), GRACE);
        assert_eq!(at(14_999), Settle::Wait);
        assert_eq!(at(15_000), Settle::Done);
    }
}
