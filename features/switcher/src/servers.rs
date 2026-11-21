//! Search sessions across servers (`shell/commands/session-search.tsx`): ⇧⌘F and the Inbox's
//! Archived button search every listed server that isn't down, each its own source, so a slow or
//! failing server never holds back or hides another's sessions. With two servers or more, a
//! session on a server other than the app's own reads "<project> · <server>"
//! (`palette.session.location`), and the rows merge newest first.
//!
//! The server the window works against runs through the palette's own source (`poll.rs`); each
//! other one is a [`Remote`]: the root sessions it had loaded when the search opened, and what its
//! server finds for the query.

use std::collections::HashMap;

use opencode_kit::client::Server;
use opencode_kit::model::{self, Project, SessionInfo};
use opencode_kit::servers::ServerKey;
use opencode_search::query::upsert_latest;
use rune::{Latest, Tx};

use crate::entries::{Action, Entry, MATCHES, RECENT_SESSIONS, asks_sessions};
use crate::{Reply, SwitcherHost, SwitcherMode, clean_dir, encode, is_automation, project_or_dir, search, title_of};

/// Another server's initial snapshot supplied by the host when opening a cross-server session search.
pub struct RemoteSeed {
    pub server: ServerKey,
    /// What a row calls its server (`servers.tag`): `None` for the app's own.
    pub tag: Option<String>,
    pub conn: Server,
    /// Its root sessions loaded when the search opened.
    pub known: Vec<SessionInfo>,
    /// Clamped recency `(active_ms, idle)` indexed per session id from that server's `recent_page`.
    pub indexed: HashMap<String, (i64, i64)>,
    /// Its projects, for each row's name and avatar.
    pub projects: Vec<Project>,
}

/// Another server's source.
pub(crate) struct Remote {
    pub server: ServerKey,
    /// What a row calls its server (`servers.tag`): None for the app's own.
    pub tag: Option<String>,
    conn: Server,
    /// Its root sessions loaded when the search opened, and those its server found since.
    known: Vec<SessionInfo>,
    /// Clamped recency `(active_ms, idle)` indexed per session id (`mergeRootSessionIndex`).
    indexed: HashMap<String, (i64, i64)>,
    /// Its projects, for each row's name and avatar.
    pub projects: Vec<Project>,
    /// It lists archived sessions (the Inbox's Archived).
    archived: bool,
    source: search::Source<()>,
    fetch: Latest<Reply<SessionInfo>>,
}

impl Remote {
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn session(&self, id: &str) -> Option<&SessionInfo> {
        self.known.iter().find(|s| s.id == id)
    }

    pub fn loading(&self, q: &str) -> bool {
        self.source.query() != Some(q) || self.source.loading()
    }

    fn active_ms(&self, s: &SessionInfo) -> i64 {
        match self.indexed.get(&s.id) {
            Some(&(indexed_updated, indexed_idle)) if s.idle.unwrap_or(0) <= indexed_idle => indexed_updated,
            _ => s.active_ms(),
        }
    }
}

/// The searches take two servers or more to show a server's name.
pub fn tag(host: &dyn SwitcherHost, key: &ServerKey, servers: usize) -> Option<String> {
    (servers >= 2 && !key.is_sidecar()).then(|| host.server_label(key))
}

/// The servers the search reads besides the window's own: the visible ones that aren't down and
/// that the window has a half of.
pub(crate) fn open(host: &mut dyn SwitcherHost) -> Vec<Remote> {
    let mode = host.switcher().mode;
    if !matches!(mode, SwitcherMode::Sessions | SwitcherMode::Archived) || host.live_server().is_none() {
        return Vec::new();
    }
    let archived = mode == SwitcherMode::Archived;
    let waker = host.waker();
    host.remote_server_seeds(archived)
        .into_iter()
        .map(|seed| {
            let fetch = waker.clone().map_or_else(Latest::default, Latest::new);
            Remote {
                server: seed.server,
                tag: seed.tag,
                conn: seed.conn,
                known: seed.known,
                indexed: seed.indexed,
                projects: seed.projects,
                archived,
                source: search::Source::default(),
                fetch,
            }
        })
        .collect()
}

/// What the window's own server's rows add: its name, once two servers or more are searched.
pub(crate) fn own_tag(host: &dyn SwitcherHost) -> Option<String> {
    let key = host.active_server();
    let count = host.switcher().remotes.len() + 1;
    tag(host, &key, count)
}

/// "<project> · <server>".
pub(crate) fn located(project: &str, tag: Option<&str>) -> String {
    match tag {
        Some(server) => opencode_i18n::t!("palette.session.location", project = project, server = server),
        None => project.to_owned(),
    }
}

/// The other servers' sessions for `q`, as the palette's own rows (`createServerSessionEntries`).
pub(crate) fn remote_matches(host: &dyn SwitcherHost, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
    let needle = q.trim().to_lowercase();
    let remotes = &host.switcher().remotes;
    let mut out = Vec::new();
    for r in remotes {
        let mut rows: Vec<&SessionInfo> = r.known.iter().filter(|i| i.archived.is_some() == archived && !(needle.is_empty() && is_automation(&i.title))).collect();
        rows.sort_by_key(|a| std::cmp::Reverse(r.active_ms(a)));
        for ses in rows {
            let (project_id, project) = project_of(r, ses);
            let title = title_of(ses);
            // Titles only, as official's `createServerSessionEntries` narrows (`shell/commands/palette.ts:439-440`).
            if !needle.is_empty() && !title.to_lowercase().contains(&needle) {
                continue;
            }
            out.push(Entry {
                key: format!("session:{}:{}", r.server, ses.id),
                action: Action::RemoteSession { server: r.server.clone(), id: ses.id.clone() },
                title,
                project: project_id,
                description: located(&project, r.tag.as_deref()),
                hint: None,
                chord: None,
                group,
                updated: Some(r.active_ms(ses)),
                open: host.is_tab_open(Some(&r.server), &ses.id),
            });
            if out.len() >= if needle.is_empty() { RECENT_SESSIONS } else { MATCHES } * remotes.len() {
                break;
            }
        }
    }
    out
}

/// The other servers' searches are still out for the typed query.
pub(crate) fn remotes_searching(host: &dyn SwitcherHost, q: &str) -> bool {
    asks_sessions(host, q) && host.switcher().remotes.iter().any(|r| r.loading(q))
}

/// Each poll: their answers land, a due debounce or timeout steps, and a new query goes out.
pub(crate) fn poll_remotes(host: &mut dyn SwitcherHost, now: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    let q = host.switcher().query().to_owned();
    let ask_base = asks_sessions(host, &q);
    let healthy: Vec<bool> = host.switcher().remotes.iter().map(|r| !host.server_health_failed(&r.server)).collect();
    let sw = host.switcher_mut();
    let mut seq = std::mem::take(&mut sw.seq);
    for (r, ok) in sw.remotes.iter_mut().zip(healthy) {
        let before = r.source.status();
        if let Some(reply) = r.fetch.take() {
            let event = match reply.items {
                Ok(sessions) => {
                    let roots: Vec<SessionInfo> = sessions.into_iter().filter(|i| i.parent.is_none()).collect();
                    for s in &roots {
                        let next_idle = s.idle.unwrap_or(0);
                        r.indexed
                            .entry(s.id.clone())
                            .and_modify(|(t, idle)| {
                                if next_idle > *idle {
                                    *t = s.active_ms();
                                    *idle = next_idle;
                                }
                            })
                            .or_insert_with(|| (s.active_ms(), next_idle));
                    }
                    upsert_latest(&mut r.known, roots, |a, b| a.id == b.id, |s| s.updated);
                    search::Event::Answered { attempt: reply.attempt, items: Vec::new() }
                }
                Err(()) => search::Event::Failed { attempt: reply.attempt },
            };
            let fx = search::step(&mut seq, &mut r.source, event);
            run(r, fx, tx);
            changed = true;
        }
        let fx = search::step(&mut seq, &mut r.source, search::Event::Tick { now });
        run(r, fx, tx);
        let ask = ask_base && ok;
        if r.source.query() != Some(q.as_str()) || (!ask && r.source.loading()) {
            let fx = search::step(&mut seq, &mut r.source, search::Event::Query { query: q.clone(), ask, wait: !q.is_empty(), now });
            run(r, fx, tx);
        }
        changed |= before != r.source.status();
    }
    sw.seq = seq;
    changed
}

fn run(r: &mut Remote, fx: rune::machine::Effects<search::Effect>, tx: &mut Tx) {
    for effect in fx {
        match effect {
            search::Effect::WakeAt(due) => tx.wake_at(due),
            search::Effect::Cancel => r.fetch.cancel(),
            search::Effect::Fetch { attempt, query } => {
                let server = r.conn.clone();
                let archived = r.archived;
                r.fetch.request(query, move |q| {
                    let path = format!("/api/session?parentID=null&archived={archived}&limit=50&search={}", encode(&q));
                    let listed = server.get(&path);
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

/// The project a row names: one rooted at the session's folder, else the session's own project,
/// else its folder's name (`projectForSession`).
fn project_of(r: &Remote, ses: &SessionInfo) -> (String, String) {
    let dir = clean_dir(&ses.directory);
    let project = r.projects.iter().find(|p| clean_dir(&p.directory) == dir).or_else(|| r.project(&ses.project).filter(|_| ses.project != "global"));
    (project.map(|p| p.id.clone()).unwrap_or_default(), project_or_dir(project, &ses.directory))
}
