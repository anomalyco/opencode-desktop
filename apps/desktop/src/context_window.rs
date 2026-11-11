//! Which of a session's messages Desktop holds, for the Context tab's counts and raw list, which
//! Desktop computes over its loaded messages (`gui-plugins/src/context/tab.tsx` `messages()` is
//! `data.session.message.list`). Desktop reads the newest page on opening a session (20, or 40
//! with the Compact timeline: `app/src/runtime/server/runtime.tsx` `initialMessageLimit`), the
//! pages that complete a turn that page cut (`timeline/sync-machine.ts`), one page of 20 each time
//! its timeline comes within 200 px of the top (`timeline/history-machine.ts` `scroll.top`), and
//! every message that arrives after. Rune's timeline pages in its own steps (`session_pages`), so
//! this keeps what Desktop's arithmetic needs: when the session opened, its first page, and how
//! many times the reader has come near the top since. The tab applies it to the server's messages.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::App;

/// Desktop's window of one session, from when this window opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Window {
    /// When it opened (ms since the epoch): messages created since then are all held.
    pub opened: i64,
    /// The newest page read on opening it.
    pub first: u32,
    /// Older pages of 20 read since: one per approach of the timeline's top.
    pub older: u32,
}

/// Each session's window, by (server, session), and where the reader stood at the last poll.
#[derive(Default)]
pub struct Windows {
    map: HashMap<(String, String), Window>,
    reader: Reader,
}

/// Where the open timeline's reader stands, so each approach of its top counts once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Reader {
    /// Away from the top.
    #[default]
    Away,
    /// Within 200 px of session `0`'s top.
    Near(String),
    /// A page of Rune's own landed above the reader (`session_pages` then asks again where the
    /// reader stands): still near the top in the next moment is the same approach, as Desktop's
    /// own page would have moved the reader down.
    Landed { id: String, at: Instant },
}

/// How long after Rune's page lands its report of the reader's place may take.
const SETTLE: Duration = Duration::from_millis(300);

impl Reader {
    /// The reader's place at a poll (`near`: the session whose top it stands near, empty for none):
    /// the next state, and whether it is a new approach.
    fn step(&self, near: &str, now: Instant) -> (Reader, bool) {
        match (self, near.is_empty()) {
            (Reader::Landed { at, .. }, true) if now.duration_since(*at) < SETTLE => (self.clone(), false),
            (_, true) => (Reader::Away, false),
            (Reader::Near(id), false) if id == near => (self.clone(), false),
            (Reader::Landed { id, at }, false) if id == near && now.duration_since(*at) < SETTLE => (Reader::Near(near.to_owned()), false),
            (_, false) => (Reader::Near(near.to_owned()), true),
        }
    }
}

impl App {
    /// Session `id` opened: its window starts now, unless this window already holds it, as
    /// Desktop's store keeps a session's pages while the app runs.
    pub(crate) fn window_opened(&mut self, id: &str) {
        let first = if self.settings.timeline_detail.preset() == Some(COMPACT) { 40 } else { 20 };
        let key = (server(self), id.to_owned());
        let opened = self.now_ms;
        self.context_windows.map.entry(key).or_insert(Window { opened, first, older: 0 });
    }

    /// Each poll: the open session's timeline coming near its top reads Desktop's next page.
    pub(crate) fn poll_context_window(&mut self) -> bool {
        let (reader, approached) = self.context_windows.reader.step(&self.pages.near_top(), Instant::now());
        if reader == self.context_windows.reader {
            return false;
        }
        self.context_windows.reader = reader;
        let Reader::Near(id) = &self.context_windows.reader else { return false };
        // Rune paints a short latest page before the history behind it; standing at the top of
        // that is no reader's approach.
        let read = self.pages.first_read_done(id);
        let key = (server(self), id.clone());
        match self.context_windows.map.get_mut(&key) {
            Some(w) if approached && read => {
                w.older += 1;
                true
            }
            _ => false,
        }
    }

    /// A page of Rune's own landed above session `id`'s reader.
    pub(crate) fn context_window_landed(&mut self, id: &str) {
        self.context_windows.reader = Reader::Landed { id: id.to_owned(), at: Instant::now() };
    }

    /// Desktop's window of session `id`, once this window opened it.
    pub fn context_window(&self, id: &str) -> Option<Window> {
        self.context_windows.map.get(&(server(self), id.to_owned())).copied()
    }
}

fn server(app: &App) -> String {
    crate::servers::runtime::active(app).as_str().to_owned()
}

/// `session_view::detail::PRESETS`' Compact.
const COMPACT: usize = 2;
