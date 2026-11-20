//! Recent ranks the shared session records by the last list response. Projects and Customize
//! use the records' live server times (`shell/titlebar/recent-sessions-model.ts:397-417`,
//! `sidebar-organized.tsx:238-251`, `sidebar-customized.tsx:190-275`).

use std::collections::{HashMap, VecDeque};
use std::time::Duration;
use rune::time::Instant;
use opencode_kit::client::Request;
use opencode_kit::model::SessionInfo;
use serde_json::Value;
use crate::{App, Source};

#[derive(Clone, Default)]
pub struct Page {
    times: HashMap<String, i64>,
    idles: HashMap<String, i64>,
    range: Option<(i64, i64)>,
    sends: HashMap<String, SendTime>,
    receipts: VecDeque<(String, String)>,
}

/// `createSessionRecency` (packages/app/src/runtime/server/data.ts:95-98,139-188).
/// After catch-up, keep the server's time as a floor for a stale page.
#[derive(Clone, Copy)]
enum SendTime {
    Accepted { sent_at: i64, server_updated_at: i64 },
    CaughtUp(i64),
}

impl Page {
    fn listed(&mut self, list: &[SessionInfo]) {
        self.times.clear();
        self.idles.clear();
        for session in list {
            self.observe(session);
            self.times.insert(session.id.clone(), session.active_ms());
            self.idles.insert(session.id.clone(), session.idle.unwrap_or(0));
        }
        self.range = self.times.values().min().zip(self.times.values().max()).map(|(&min, &max)| (min, max));
    }

    /// Ports `mergeRootSessionIndex` (`packages/app/src/runtime/server/session-index.ts:4-20`):
    /// passive metadata reads (opening a tab, browser permission grant `writePermissions`, rename,
    /// model/agent switches) keep the indexed `updated` time; only a completed run
    /// (`info.idle > indexed_idle`) advances an already-indexed session's recency.
    pub fn merge_info(&mut self, info: &mut SessionInfo) {
        let Some(&indexed_updated) = self.times.get(&info.id) else { return };
        let indexed_idle = self.idles.get(&info.id).copied().unwrap_or(0);
        let next_idle = info.idle.unwrap_or(0);
        if next_idle > indexed_idle {
            self.idles.insert(info.id.clone(), next_idle);
            self.times.insert(info.id.clone(), info.active_ms());
        } else if info.updated != indexed_updated {
            info.updated = indexed_updated;
        }
    }

    pub fn would_observe(&self, session: &SessionInfo) -> bool {
        matches!(self.sends.get(&session.id), Some(SendTime::Accepted { server_updated_at, .. }) if session.active_ms() > *server_updated_at)
    }

    pub fn observe(&mut self, session: &SessionInfo) {
        if let Some(state) = self.sends.get_mut(&session.id)
            && let SendTime::Accepted { server_updated_at, .. } = *state
            && session.active_ms() > server_updated_at {
            *state = SendTime::CaughtUp(session.active_ms());
        }
    }

    fn accepted(&mut self, session: &str, prompt: &str, sent_at: i64, raw: i64) -> bool {
        // The enqueue and HTTP receipt can land in either order, even across a list reload.
        if self.receipts.iter().any(|(_, id)| id == prompt) { return false; }
        self.receipts.push_back((session.to_owned(), prompt.to_owned()));
        if self.receipts.len() > 512 && let Some((old, _)) = self.receipts.pop_front()
            && !self.receipts.iter().any(|(session, _)| session == &old) {
            self.sends.remove(&old);
        }
        let previous = match self.sends.get(session) {
            Some(SendTime::Accepted { server_updated_at, .. }) => *server_updated_at,
            Some(SendTime::CaughtUp(time)) => *time,
            None => 0,
        };
        let server_updated_at = raw.max(previous).max(self.times.get(session).copied().unwrap_or_default());
        self.sends.insert(session.to_owned(), SendTime::Accepted { sent_at, server_updated_at });
        true
    }

    fn project(&self, id: &str, raw: i64) -> i64 {
        match self.sends.get(id) {
            Some(SendTime::Accepted { sent_at, .. }) => raw.max(*sent_at),
            Some(SendTime::CaughtUp(time)) => raw.max(*time),
            None => raw,
        }
    }

    pub fn active_ms(&self, session: &SessionInfo) -> i64 {
        self.project(&session.id, session.active_ms())
    }

    pub fn time(&self, session: &SessionInfo) -> i64 {
        // A known session past the page stays behind its oldest row. A new one may join it.
        let snapshot = match (self.times.get(&session.id), self.range) {
            (Some(time), _) => *time,
            (None, Some((oldest, newest))) if session.created <= newest => session.active_ms().min(oldest),
            _ => session.active_ms(),
        };
        self.project(&session.id, snapshot)
    }
}

/// `recentReloadsOn`, recent-sessions-model.ts (#64503): reload page one only when a new root
/// session is created. Other changes (run ends, renames, archive) reach rows through live events.
pub fn reloads_on(ty: &str, data: &Value) -> bool {
    ty == "session.created" && data["parentID"].is_null()
}

/// One trailing-edge reload per server (`recent-sessions.tsx:133-146,319-322,334-344`).
#[derive(Default)]
pub enum Reload { #[default] BeforeFirstPage, Idle, Scheduled(Instant) }
enum Event { Invalidate, Poll, Refresh, Listed }
enum Effect { None, Wake(Duration), Fetch }

impl Reload {
    fn step(&mut self, event: Event, now: Instant) -> Effect {
        match event {
            Event::Invalidate | Event::Refresh if matches!(self, Self::BeforeFirstPage) => (),
            Event::Listed if matches!(self, Self::BeforeFirstPage) => *self = Self::Idle,
            Event::Listed => (),
            Event::Invalidate => *self = Self::Scheduled(now + Duration::from_millis(250)),
            Event::Refresh => *self = Self::Idle,
            Event::Poll => match *self {
                Self::BeforeFirstPage | Self::Idle => (),
                Self::Scheduled(at) if now < at => return Effect::Wake(at - now),
                Self::Scheduled(_) => { *self = Self::Idle; return Effect::Fetch; }
            },
        }
        Effect::None
    }
}

impl App {
    /// Replace the page but keep sessions known only from tabs or family reads.
    pub(crate) fn sessions_listed(&mut self, mut list: Vec<SessionInfo>) {
        self.recent_page.listed(&list);
        self.recent_reload.step(Event::Listed, Instant::now());
        for info in &mut list {
            if let Some(old) = self.session(&info.id) { info.viewed = info.viewed.max(old.viewed); }
        }
        list.extend(self.sessions.drain(..).filter(|old| !self.recent_page.times.contains_key(&old.id)));
        *self.sessions = list;
        self.sidebar_inbox.heard_sessions();
        self.note_tab_titles();
    }

    pub(crate) fn schedule_recent_reload(&mut self) {
        self.recent_reload.step(Event::Invalidate, Instant::now());
    }

    pub(crate) fn prompt_accepted(&mut self, session: &str, prompt: &str) -> bool {
        let raw = self.session(session).map_or(0, SessionInfo::active_ms);
        self.recent_page.accepted(session, prompt, *self.now_ms, raw)
    }

    pub(crate) fn refresh_recent(&mut self) {
        self.recent_reload.step(Event::Refresh, Instant::now());
    }

    pub(crate) fn poll_recent_reload(&mut self, tx: &mut rune::Tx) {
        match self.recent_reload.step(Event::Poll, Instant::now()) {
            Effect::Wake(delay) => tx.wake_after(delay),
            Effect::Fetch => if let Source::Live { requests: Some(r), .. } = &*self.source { r.send(Request::Sessions).ok(); },
            Effect::None => (),
        }
    }
}
