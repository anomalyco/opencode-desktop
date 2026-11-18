//! Search sessions across servers (`shell/commands/session-search.tsx`): ⇧⌘F and the Inbox's
//! Archived button search every listed server that isn't down, each its own source, so a slow or
//! failing server never holds back or hides another's sessions. With two servers or more, a
//! session on a server other than the app's own reads "<project> · <server>"
//! (`palette.session.location`), and the rows merge newest first.
//!
//! The server the window works against runs through the palette's own source (`poll.rs`); each
//! other one is a [`Remote`]: the root sessions it had loaded when the search opened, and what its
//! server finds for the query.

use opencode_kit::model::{self, Project, SessionInfo};
use opencode_kit::servers::ServerKey;
use rune::{Latest, Tx};

use super::{Action, Entry, MATCHES, RECENT_SESSIONS, Reply, SwitcherMode, encode, search};
use crate::mentions::list::upsert_sessions;
use crate::view::title_of;
use crate::{App, Source};

/// Another server's source.
pub(crate) struct Remote {
    pub server: ServerKey,
    /// What a row calls its server (`servers.tag`): None for the app's own.
    pub tag: Option<String>,
    conn: opencode_kit::client::Server,
    /// Its root sessions loaded when the search opened, and those its server found since.
    known: Vec<SessionInfo>,
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
}

/// The searches take two servers or more to show a server's name.
pub(crate) fn tag(s: &App, key: &ServerKey, servers: usize) -> Option<String> {
    (servers >= 2 && !key.is_sidecar()).then(|| crate::servers::registry::label_of(s, key))
}

/// The servers the search reads besides the window's own: the visible ones that aren't down and
/// that the window has a half of.
pub(crate) fn open(s: &mut App) -> Vec<Remote> {
    if !matches!(s.switcher.mode, SwitcherMode::Sessions | SwitcherMode::Archived) || !s.live() {
        return Vec::new();
    }
    let visible = crate::servers::registry::visible(s);
    let active = crate::servers::runtime::active(s).clone();
    let usable: Vec<ServerKey> = visible.iter().map(|e| e.key()).collect();
    let count = usable.len();
    let waker = (*s.waker).clone();
    let mut out = Vec::new();
    for key in usable.into_iter().filter(|k| *k != active) {
        let tag = tag(s, &key, count);
        let read = |s: &mut App| {
            let conn = match &*s.source {
                Source::Live { server, .. } => Some(server.clone()),
                Source::Fixture => None,
            };
            let known: Vec<SessionInfo> = s.sessions.iter().filter(|i| i.parent.is_none()).cloned().collect();
            (conn, known, (*s.projects).clone())
        };
        let Some((Some(conn), known, projects)) = crate::servers::runtime::with_server(s, &key, read) else { continue };
        let fetch = waker.clone().map_or_else(Latest::default, Latest::new);
        let archived = s.switcher.mode == SwitcherMode::Archived;
        out.push(Remote { server: key, tag, conn, known, projects, archived, source: search::Source::default(), fetch });
    }
    out
}

/// What the window's own server's rows add: its name, once two servers or more are searched.
pub(crate) fn own_tag(s: &App) -> Option<String> {
    let key = crate::servers::runtime::active(s);
    let servers = s.switcher.remotes.len() + 1;
    tag(s, key, servers)
}

/// "<project> · <server>".
pub(crate) fn located(project: &str, tag: Option<&str>) -> String {
    match tag {
        Some(server) => opencode_i18n::t!("palette.session.location", project = project, server = server),
        None => project.to_owned(),
    }
}

impl App {
    /// The other servers' sessions for `q`, as the palette's own rows (`createServerSessionEntries`).
    pub(super) fn remote_matches(&self, q: &str, group: &'static str, archived: bool) -> Vec<Entry> {
        let needle = q.trim().to_lowercase();
        let mut out = Vec::new();
        for r in &self.switcher.remotes {
            let mut rows: Vec<&SessionInfo> = r.known.iter().filter(|i| i.archived.is_some() == archived && !(needle.is_empty() && crate::view::is_automation(&i.title))).collect();
            rows.sort_by(|a, b| b.active_ms().cmp(&a.active_ms()));
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
                    updated: Some(ses.updated),
                    open: self.tabs.iter().any(|t| t.is(&r.server, &ses.id)),
                });
                if out.len() >= if needle.is_empty() { RECENT_SESSIONS } else { MATCHES } * self.switcher.remotes.len() {
                    break;
                }
            }
        }
        out
    }

    /// The other servers' searches are still out for the typed query.
    pub(super) fn remotes_searching(&self, q: &str) -> bool {
        self.switcher_asks_sessions(q) && self.switcher.remotes.iter().any(|r| r.loading(q))
    }

    /// Each poll: their answers land, a due debounce or timeout steps, and a new query goes out.
    pub(super) fn poll_remotes(&mut self, now: f64, tx: &mut Tx) -> bool {
        let mut changed = false;
        let q = self.switcher.query().to_owned();
        let ask = self.switcher_asks_sessions(&q);
        let mut seq = std::mem::take(&mut self.switcher.seq);
        for r in &mut self.switcher.remotes {
            let before = r.source.status();
            if let Some(reply) = r.fetch.take() {
                let event = match reply.items {
                    Ok(sessions) => {
                        upsert_sessions(&mut r.known, sessions.into_iter().filter(|i| i.parent.is_none()).collect());
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
            let ask = ask && !opencode_kit::servers::health::is_failed(crate::servers::health::get(&r.server).as_ref());
            if r.source.query() != Some(q.as_str()) || (!ask && r.source.loading()) {
                let fx = search::step(&mut seq, &mut r.source, search::Event::Query { query: q.clone(), ask, wait: !q.is_empty(), now });
                run(r, fx, tx);
            }
            changed |= before != r.source.status();
        }
        self.switcher.seq = seq;
        changed
    }
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
                    let exact = (q.len() > 20 && q.starts_with("ses_")).then(|| server.get(&format!("/api/session/{q}")).ok()).flatten().map(|v| model::parse_session(v.get("data").unwrap_or(&v)));
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
    let dir = crate::projects::clean_dir(&ses.directory);
    let project = r.projects.iter().find(|p| crate::projects::clean_dir(&p.directory) == dir).or_else(|| r.project(&ses.project).filter(|_| ses.project != "global"));
    (project.map(|p| p.id.clone()).unwrap_or_default(), crate::view::project_or_dir(project, &ses.directory))
}
