//! Recent ranks the shared session records by the last list response. Projects and Customize
//! use the records' live server times (`shell/titlebar/recent-sessions-model.ts:397-417`,
//! `sidebar-organized.tsx:238-251`, `sidebar-customized.tsx:190-275`).

use std::collections::HashMap;
use std::time::Duration;
use rune::time::Instant;
use opencode_kit::model::SessionInfo;
use serde_json::Value;
use crate::{App, Request, Source};

#[derive(Clone, Default)]
pub struct Page {
    times: HashMap<String, i64>,
    range: Option<(i64, i64)>,
}

impl Page {
    pub fn from_list(list: &[SessionInfo]) -> Self {
        let times: HashMap<_, _> = list.iter().map(|s| (s.id.clone(), s.active_ms())).collect();
        let range = times.values().min().zip(times.values().max()).map(|(&min, &max)| (min, max));
        Self { times, range }
    }

    pub fn time(&self, session: &SessionInfo) -> i64 {
        if let Some(time) = self.times.get(&session.id) { return *time; }
        // A known session past the page stays behind its oldest row. A new one may join it.
        match self.range {
            Some((oldest, newest)) if session.created <= newest => session.active_ms().min(oldest),
            _ => session.active_ms(),
        }
    }
}

/// `recentReloadsOn`, recent-sessions-model.ts:295-311.
pub fn reloads_on(app: &App, ty: &str, data: &Value, sid: &str) -> bool {
    match ty {
        "session.created" => data["parentID"].is_null(),
        "session.archived" | "session.unarchived" => true,
        "session.execution.interrupted" if data["reason"] == "shutdown" => false,
        "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => app.session(sid).is_none_or(|s| s.parent.is_none()),
        _ => false,
    }
}

/// One trailing-edge reload per server (`recent-sessions.tsx:133-146,319-322,334-344`).
#[derive(Default)]
pub enum Reload { #[default] Idle, Scheduled(Instant) }
enum Event { Invalidate, Poll, Refresh }
enum Effect { None, Wake(Duration), Fetch }

impl Reload {
    fn step(&mut self, event: Event, now: Instant) -> Effect {
        match event {
            Event::Invalidate => *self = Self::Scheduled(now + Duration::from_millis(250)),
            Event::Refresh => *self = Self::Idle,
            Event::Poll => match *self {
                Self::Idle => (),
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
        *self.recent_page = Page::from_list(&list);
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

    pub(crate) fn refresh_recent(&mut self) {
        self.recent_reload.step(Event::Refresh, Instant::now());
    }

    pub(crate) fn poll_recent_reload(&mut self, tx: &mut rune::Tx) {
        match self.recent_reload.step(Event::Poll, Instant::now()) {
            Effect::Wake(delay) => tx.wake_after(delay),
            Effect::Fetch => if let Source::Live { requests: Some(r), .. } = &*self.source { let _ = r.send(Request::Sessions); },
            Effect::None => (),
        }
    }

    pub(crate) fn reload_recent_on_focus(&mut self) {
        for key in self.checkout.keys() {
            crate::servers::runtime::with_server(self, &key, |s| s.schedule_recent_reload());
        }
    }
}
