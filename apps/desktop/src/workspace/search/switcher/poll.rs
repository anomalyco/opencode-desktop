//! The palette's remote sources as it runs them: each source's machine ([`search`]) gets the
//! query, its timers and its answers, and its effects go to the server here; then what the list
//! shows meanwhile ([`super::shown`]) steps.

use opencode_kit::model::{self, SessionInfo};
use rune::Tx;

use super::shown::{Effect, Event, Shown};
use super::{Reply, Restore, SwitcherMode, encode, search};
use crate::files::data::{fetch_live_find, parse_path_and_line};
use crate::mentions::list::upsert_sessions;
use crate::popover::HighlightEvent;
use crate::{App, Source};

impl App {
    /// Takes in the sources' answers and their timers, and gives each a changed query.
    pub fn poll_switcher(&mut self, tx: &mut Tx, local_answers: bool) -> bool {
        let now = tx.clock();
        // The list scrolled past the rows built: build the ones it shows now.
        let mut changed = local_answers | self.switcher.band.moved();
        if let Some(reply) = self.switcher.fetch.take() {
            let current = self.switcher.sessions.loading_attempt() == Some(reply.attempt);
            let event = match reply.items {
                Ok(sessions) => {
                    if current && !sessions.is_empty() {
                        upsert_sessions(&mut self.switcher.loaded, sessions);
                    }
                    search::Event::Answered { attempt: reply.attempt, items: Vec::new() }
                }
                Err(()) => search::Event::Failed { attempt: reply.attempt },
            };
            self.switcher_sessions_step(event, tx);
            changed |= current;
        }
        let fixture_due = self.switcher.fixture_find.as_ref().map(|(at, _)| *at);
        let fixture = match fixture_due {
            Some(at) if now + 1e-9 >= at => self.switcher.fixture_find.take().map(|(_, reply)| reply),
            Some(at) => {
                tx.wake_at(at);
                None
            }
            None => None,
        };
        if let Some(reply) = self.switcher.files_fetch.take().or(fixture) {
            let current = self.switcher.files.loading_attempt() == Some(reply.attempt);
            let event = match reply.items {
                Ok(items) => search::Event::Answered { attempt: reply.attempt, items },
                Err(()) => search::Event::Failed { attempt: reply.attempt },
            };
            self.switcher_files_step(event, tx);
            changed |= current;
        }
        if !self.switcher.open {
            return changed;
        }
        if let Restore::Restoring { session, due } = &self.switcher.restore {
            if now + 1e-9 >= *due {
                let session = session.clone();
                self.switcher_restore_replied(session, false, "", tx);
                changed = true;
            } else {
                tx.wake_at(*due);
            }
        }
        // The other servers' sessions (`servers.rs`), each source on its own.
        changed |= self.poll_remotes(now, tx);
        let (sessions_were, files_were) = (self.switcher.sessions.status(), self.switcher.files.status());
        self.switcher_sessions_step(search::Event::Tick { now }, tx);
        self.switcher_files_step(search::Event::Tick { now }, tx);
        changed |= sessions_were != self.switcher.sessions.status() || files_were != self.switcher.files.status();
        let landed = changed;
        changed |= self.switcher_query(now, tx);
        let mut has_rows = None;
        // Results land at once: no row fades in or slides to its place.
        if landed {
            tx.instant();
            let entries = self.switcher_matches(self.switcher.query());
            has_rows = Some(!entries.is_empty());
            let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
            self.switcher.active.step(HighlightEvent::Rows(&keys));
            let active = self.switcher_active(&entries);
            // They bring a row the keys chose back into view (`SearchList`), but leave the list
            // where the wheel put it while the first row is the highlight or the pointer took it.
            if let Some(key) = active.filter(|k| entries.first().is_some_and(|e| &e.key != k) && !self.switcher.active.pointed(k)) {
                tx.scroll_into_view_in("switcher-scroll", format!("sw-{key}"));
            }
        }
        changed | self.step_switcher_shown(has_rows, tx)
    }

    /// Gives the sources a query they haven't heard. Typed session searches wait for a pause,
    /// and a newer keystroke cancels the last.
    fn switcher_query(&mut self, now: f64, tx: &mut Tx) -> bool {
        let q = self.switcher.query().to_owned();
        if self.switcher.sessions.query() == Some(q.as_str()) {
            return false;
        }
        // A new query starts from the top.
        tx.scroll_to_y("switcher-scroll", 0.0);
        let ask = self.switcher_asks_sessions(&q);
        self.switcher_sessions_step(search::Event::Query { query: q.clone(), ask, wait: !q.is_empty(), now }, tx);
        let files = self.switcher_files_session().is_some().then(|| parse_path_and_line(&q).0).filter(|c| !c.is_empty());
        let file_query = files.clone().unwrap_or_default();
        self.switcher_files_step(search::Event::Query { query: file_query, ask: files.is_some(), wait: false, now }, tx);
        true
    }

    fn switcher_sessions_step(&mut self, event: search::Event<()>, tx: &mut Tx) {
        let effects = { let sw = &mut *self.switcher; search::step(&mut sw.seq, &mut sw.sessions, event) };
        for effect in effects {
            match effect {
                search::Effect::WakeAt(due) => tx.wake_at(due),
                search::Effect::Cancel => self.switcher.fetch.cancel(),
                search::Effect::Fetch { attempt, query } => {
                    let Source::Live { server, .. } = &*self.source else { continue };
                    let (server, archived) = (server.clone(), self.switcher.mode == SwitcherMode::Archived);
                    self.switcher.fetch.request(query, move |q| {
                        let path = format!("/api/session?parentID=null&archived={archived}&limit=50&search={}", encode(&q));
                        let listed = server.get(&path);
                        // `looksLikeSessionID`: a pasted id finds its session even when no title matches.
                        let exact = (q.len() > 20 && q.starts_with("ses_"))
                            .then(|| server.get(&format!("/api/session/{q}")).ok())
                            .flatten()
                            .map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
                        if listed.is_err() && exact.is_none() {
                            return Reply { attempt, items: Err(()) };
                        }
                        let mut sessions: Vec<SessionInfo> = exact.into_iter().collect();
                        if let Ok(v) = &listed {
                            sessions.extend(model::parse_sessions(v));
                        }
                        Reply { attempt, items: Ok(sessions) }
                    });
                }
            }
        }
    }

    /// The files matching the query, in the session's folder (a typed `:line` left out).
    fn switcher_files_step(&mut self, event: search::Event<String>, tx: &mut Tx) {
        let effects = { let sw = &mut *self.switcher; search::step(&mut sw.seq, &mut sw.files, event) };
        for effect in effects {
            match effect {
                search::Effect::WakeAt(due) => tx.wake_at(due),
                search::Effect::Cancel => {
                    self.switcher.files_fetch.cancel();
                    self.switcher.fixture_find = None;
                }
                search::Effect::Fetch { attempt, query } => {
                    let Some(sid) = self.switcher_files_session().map(str::to_owned) else { continue };
                    let Source::Live { server, .. } = &*self.source else {
                        // The fixture's folder and `find_files` (files only its server has seen)
                        // are the answer, `find_ms` later as a server's would be (at once without it).
                        let mut items = self.files.sessions.get(&sid).map(|w| w.local_files(&query, super::MATCHES)).unwrap_or_default();
                        let extra: Vec<_> = self.fixture["find_files"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_owned)).collect();
                        for path in crate::files::data::rank_file_query(&extra, &query) {
                            if !items.contains(&path) { items.push(path); }
                        }
                        match self.fixture["find_ms"].as_f64() {
                            Some(ms) if ms > 0.0 => {
                                let at = tx.clock() + ms / 1000.0;
                                self.switcher.fixture_find = Some((at, Reply { attempt, items: Ok(items) }));
                                tx.wake_at(at);
                            }
                            _ => {
                                let sw = &mut *self.switcher;
                                search::step(&mut sw.seq, &mut sw.files, search::Event::Answered { attempt, items });
                            }
                        }
                        continue;
                    };
                    let (server, dir) = (server.clone(), self.session(&sid).map(|s| s.directory.clone()).unwrap_or_default());
                    self.switcher.files_fetch.request(query, move |q| Reply { attempt, items: Ok(fetch_live_find(&server, &dir, &q, 100)) });
                }
            }
        }
    }

    /// Steps what the list shows (`shown.rs`) when the query changed, answers landed, or it waits.
    fn step_switcher_shown(&mut self, rows: Option<bool>, tx: &mut Tx) -> bool {
        let before = self.switcher.shown.clone();
        let now = tx.clock();
        let q = self.switcher.query().to_owned();
        let mut effects = vec![self.switcher.shown.step(Event::Tick { now })];
        if rows.is_some() || self.switcher.shown.query() != q || matches!(self.switcher.shown, Shown::Waiting { .. }) {
            let rows = rows.unwrap_or_else(|| !self.switcher_matches(&q).is_empty());
            let (searching, failed) = (self.switcher_searching(), self.switcher.failed_now());
            effects.push(self.switcher.shown.step(Event::Seen { query: &q, rows, searching, failed, now }));
        }
        for effect in effects {
            if let Effect::WakeAt(at) = effect {
                tx.wake_at(at);
            }
        }
        // "Loading" and "No results" appear in place, as rows do.
        let changed = self.switcher.shown != before;
        if changed {
            tx.instant();
        }
        changed
    }
}
