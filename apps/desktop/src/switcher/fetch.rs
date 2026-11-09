//! The palette's server searches: sessions by title (`/api/session?search=`) and the folder's
//! files (`file.searchFiles`), a newer query cancelling the last.

use opencode_kit::model::{self, SessionInfo};

use super::{Found, SwitcherMode, encode};
use crate::files::data::{fetch_live_find, parse_path_and_line};
use crate::{App, Source};
use rune::Tx;

impl App {
    /// Sends a changed query to the server's session search.
    pub(super) fn request_switcher_sessions(&mut self, tx: &mut Tx) -> bool {
        let q = self.switcher.query().to_owned();
        let archived = self.switcher.mode == SwitcherMode::Archived;
        if q == self.switcher.searched && !(archived && !self.switcher.listed_archived) {
            return false;
        }
        self.switcher.listed_archived |= archived;
        self.switcher.searched = q.clone();
        self.switcher.failed = None;
        tx.scroll_to_y("switcher-scroll", 0.0);
        self.switcher.fetch.cancel();
        let Source::Live { server, .. } = &self.source else { return true };
        // Archived sessions aren't loaded, so the archived search lists the newest with no query.
        if q.is_empty() && !archived {
            return true;
        }
        // Typed searches wait for a pause, and a newer keystroke cancels this one.
        let server = server.clone();
        self.switcher.fetch.request(q, move |q| {
            let path = format!("/api/session?parentID=null&archived={archived}&limit=50&search={}", encode(&q));
            let listed = server.get(&path);
            // `looksLikeSessionID`: a pasted id finds its session even when no title matches.
            let exact = (q.len() > 20 && q.starts_with("ses_"))
                .then(|| server.get(&format!("/api/session/{q}")).ok())
                .flatten()
                .map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
            let failed = listed.is_err() && exact.is_none();
            let mut sessions: Vec<SessionInfo> = exact.into_iter().collect();
            if let Ok(v) = &listed {
                sessions.extend(model::parse_sessions(v));
            }
            Found { query: q, sessions, failed }
        });
        true
    }

    /// Asks the server for the files matching a new query, in the session's folder (a typed
    /// `:line` left out); a newer query cancels the last.
    pub(super) fn request_switcher_files(&mut self) -> bool {
        let Some(sid) = self.switcher_files_session().map(str::to_owned) else { return false };
        let clean = parse_path_and_line(self.switcher.query()).0;
        if clean.is_empty() || self.switcher.files_asked.as_deref() == Some(clean.as_str()) {
            return false;
        }
        self.switcher.files_asked = Some(clean.clone());
        self.switcher.files_fetch.cancel();
        self.switcher.fixture_find = None;
        let Source::Live { server, .. } = &self.source else {
            // The fixture's folder and `find_files` (files only its server has seen) are the
            // answer, `find_ms` later as a server's would be (at once without it).
            let mut known = self.files.sessions.get(&sid).map(|w| w.known_files.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
            known.extend(self.fixture["find_files"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_owned)));
            let found = crate::files::data::rank_file_query(&known, &clean).into_iter().filter(|p| p.to_lowercase().contains(&clean.to_lowercase())).collect();
            match self.fixture["find_ms"].as_f64() {
                Some(ms) if ms > 0.0 => self.switcher.fixture_find = Some((self.clock + ms / 1000.0, (clean, found))),
                _ => self.switcher.files = (clean, found),
            }
            return true;
        };
        let (server, dir) = (server.clone(), self.session(&sid).map(|s| s.directory.clone()).unwrap_or_default());
        self.switcher.files_fetch.request(clean, move |q| {
            let found = fetch_live_find(&server, &dir, &q);
            (q, found)
        });
        true
    }
}
