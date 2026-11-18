//! The palette's remote sources as it runs them: each source's machine ([`crate::search`]) gets the
//! query, its timers and its answers, and its effects go to the server here; then what the list
//! shows meanwhile ([`crate::shown`]) steps.

use std::collections::BTreeSet;

use opencode_kit::client::Server;
use opencode_kit::model::{self, SessionInfo};
use opencode_search::file_rank::{normalize_path, parse_path_and_line, rank_file_query_with};
use opencode_search::query::{HighlightEvent, upsert_latest};
use rune::Tx;

use crate::entries::{MATCHES, active, asks_sessions, files_session, matches_for, searching};
use crate::shown::{Effect, Event, Shown};
use crate::{Reply, Restore, SwitcherHost, SwitcherMode, encode, restore_replied, search, servers};

/// Searches workspace files via `/api/fs/find`, at most `limit` of them.
pub fn fetch_live_find(server: &Server, directory: &str, query: &str, limit: usize) -> Vec<String> {
    let url = format!("/api/fs/find?location%5Bdirectory%5D={}&query={}&type=file&limit={limit}", encode(directory), encode(query));
    let Ok(v) = server.get(&url) else {
        return Vec::new();
    };
    let Some(arr) = v["data"].as_array().or_else(|| v.as_array()) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in arr {
        if item["type"].as_str() == Some("directory") {
            continue;
        }
        let Some(raw) = item["path"].as_str().or_else(|| item.as_str()) else {
            continue;
        };
        let norm = normalize_path(raw);
        if !norm.is_empty() && seen.insert(norm.clone()) {
            out.push(norm);
        }
    }
    out
}

/// Takes in the sources' answers and their timers, and gives each a changed query.
pub fn poll(host: &mut dyn SwitcherHost, tx: &mut Tx, local_answers: bool) -> bool {
    let now = tx.clock();
    // The list scrolled past the rows built: build the ones it shows now.
    let mut changed = local_answers | host.switcher().band.moved();
    if let Some(reply) = host.switcher_mut().fetch.take() {
        let current = host.switcher().sessions.loading_attempt() == Some(reply.attempt);
        let event = match reply.items {
            Ok(sessions) => {
                if current && !sessions.is_empty() {
                    upsert_latest(&mut host.switcher_mut().loaded, sessions, |a, b| a.id == b.id, |s| s.updated);
                }
                search::Event::Answered { attempt: reply.attempt, items: Vec::new() }
            }
            Err(()) => search::Event::Failed { attempt: reply.attempt },
        };
        sessions_step(host, event, tx);
        changed |= current;
    }
    let fixture_due = host.switcher().fixture_find.as_ref().map(|(at, _)| *at);
    let fixture = match fixture_due {
        Some(at) if now + 1e-9 >= at => host.switcher_mut().fixture_find.take().map(|(_, reply)| reply),
        Some(at) => {
            tx.wake_at(at);
            None
        }
        None => None,
    };
    if let Some(reply) = host.switcher_mut().files_fetch.take().or(fixture) {
        let current = host.switcher().files.loading_attempt() == Some(reply.attempt);
        let event = match reply.items {
            Ok(items) => search::Event::Answered { attempt: reply.attempt, items },
            Err(()) => search::Event::Failed { attempt: reply.attempt },
        };
        files_step(host, event, tx);
        changed |= current;
    }
    if !host.switcher().is_open() {
        return changed;
    }
    let restoring = match &host.switcher().restore {
        Restore::Restoring { session, due } => Some((session.clone(), *due)),
        Restore::Idle => None,
    };
    if let Some((session, due)) = restoring {
        if now + 1e-9 >= due {
            restore_replied(host, session, false, "", tx);
            changed = true;
        } else {
            tx.wake_at(due);
        }
    }
    // The other servers' sessions (`servers.rs`), each source on its own.
    changed |= servers::poll_remotes(host, now, tx);
    let (sessions_were, files_were) = {
        let sw = host.switcher();
        (sw.sessions.status(), sw.files.status())
    };
    sessions_step(host, search::Event::Tick { now }, tx);
    files_step(host, search::Event::Tick { now }, tx);
    {
        let sw = host.switcher();
        changed |= sessions_were != sw.sessions.status() || files_were != sw.files.status();
    }
    let landed = changed;
    changed |= query_step(host, now, tx);
    let mut has_rows = None;
    // Results land at once: no row fades in or slides to its place.
    if landed {
        tx.instant();
        let q = host.switcher().query().to_owned();
        let entries = matches_for(host, &q);
        has_rows = Some(!entries.is_empty());
        {
            let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
            host.switcher_mut().active.step(HighlightEvent::Rows(&keys));
        }
        let lit = active(host, &entries);
        // They bring a row the keys chose back into view (`SearchList`), but leave the list
        // where the wheel put it while the first row is the highlight or the pointer took it.
        if let Some(key) = lit.filter(|k| entries.first().is_some_and(|e| &e.key != k) && !host.switcher().active.pointed(k)) {
            tx.scroll_into_view_in("switcher-scroll", format!("sw-{key}"));
        }
    }
    changed | step_shown(host, has_rows, tx)
}

/// Gives the sources a query they haven't heard. Typed session searches wait for a pause,
/// and a newer keystroke cancels the last.
fn query_step(host: &mut dyn SwitcherHost, now: f64, tx: &mut Tx) -> bool {
    let q = host.switcher().query().to_owned();
    if host.switcher().sessions.query() == Some(q.as_str()) {
        return false;
    }
    // A new query starts from the top.
    tx.scroll_to_y("switcher-scroll", 0.0);
    let ask = asks_sessions(host, &q);
    sessions_step(host, search::Event::Query { query: q.clone(), ask, wait: !q.is_empty(), now }, tx);
    let files = files_session(host).is_some().then(|| parse_path_and_line(&q).0).filter(|c| !c.is_empty());
    let file_query = files.clone().unwrap_or_default();
    files_step(host, search::Event::Query { query: file_query, ask: files.is_some(), wait: false, now }, tx);
    true
}

fn sessions_step(host: &mut dyn SwitcherHost, event: search::Event<()>, tx: &mut Tx) {
    let effects = {
        let sw = host.switcher_mut();
        search::step(&mut sw.seq, &mut sw.sessions, event)
    };
    for effect in effects {
        match effect {
            search::Effect::WakeAt(due) => tx.wake_at(due),
            search::Effect::Cancel => host.switcher_mut().fetch.cancel(),
            search::Effect::Fetch { attempt, query } => {
                let Some(server) = host.live_server() else { continue };
                let archived = host.switcher().mode == SwitcherMode::Archived;
                host.switcher_mut().fetch.request(query, move |q| {
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
fn files_step(host: &mut dyn SwitcherHost, event: search::Event<String>, tx: &mut Tx) {
    let effects = {
        let sw = host.switcher_mut();
        search::step(&mut sw.seq, &mut sw.files, event)
    };
    for effect in effects {
        match effect {
            search::Effect::WakeAt(due) => tx.wake_at(due),
            search::Effect::Cancel => {
                let sw = host.switcher_mut();
                sw.files_fetch.cancel();
                sw.fixture_find = None;
            }
            search::Effect::Fetch { attempt, query } => {
                let Some(sid) = files_session(host).map(str::to_owned) else { continue };
                let Some(server) = host.live_server() else {
                    // The fixture's folder and `find_files` (files only its server has seen)
                    // are the answer, `find_ms` later as a server's would be (at once without it).
                    let mut items = host.local_workspace_files(&sid, &query, MATCHES);
                    let (extra, find_ms) = host.fixture_find().unwrap_or_default();
                    for path in rank_file_query_with(&extra, &query, rune::text::compare_names) {
                        if !items.contains(&path) {
                            items.push(path);
                        }
                    }
                    match find_ms {
                        Some(ms) if ms > 0.0 => {
                            let at = tx.clock() + ms / 1000.0;
                            host.switcher_mut().fixture_find = Some((at, Reply { attempt, items: Ok(items) }));
                            tx.wake_at(at);
                        }
                        _ => {
                            let sw = host.switcher_mut();
                            search::step(&mut sw.seq, &mut sw.files, search::Event::Answered { attempt, items });
                        }
                    }
                    continue;
                };
                let dir = host.session(&sid).map(|s| s.directory.clone()).unwrap_or_default();
                host.switcher_mut().files_fetch.request(query, move |q| Reply { attempt, items: Ok(fetch_live_find(&server, &dir, &q, 100)) });
            }
        }
    }
}

/// Steps what the list shows (`shown.rs`) when the query changed, answers landed, or it waits.
fn step_shown(host: &mut dyn SwitcherHost, rows: Option<bool>, tx: &mut Tx) -> bool {
    let before = host.switcher().shown.clone();
    let now = tx.clock();
    let q = host.switcher().query().to_owned();
    let tick_effect = host.switcher_mut().shown.step(Event::Tick { now });
    let mut effects = vec![tick_effect];
    let needs_seen = {
        let shown = &host.switcher().shown;
        rows.is_some() || shown.query() != q || matches!(shown, Shown::Waiting { .. })
    };
    if needs_seen {
        let has_rows = rows.unwrap_or_else(|| !matches_for(host, &q).is_empty());
        let is_searching = searching(host);
        let failed = host.switcher().failed_now();
        effects.push(host.switcher_mut().shown.step(Event::Seen {
            query: &q,
            rows: has_rows,
            searching: is_searching,
            failed,
            now,
        }));
    }
    for effect in effects {
        if let Effect::WakeAt(at) = effect {
            tx.wake_at(at);
        }
    }
    // "Loading" and "No results" appear in place, as rows do.
    let changed = host.switcher().shown != before;
    if changed {
        tx.instant();
    }
    changed
}
