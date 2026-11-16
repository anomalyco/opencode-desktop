//! Server events in two steps. `apply_shared` changes the server's copy of the session data
//! (`windows::peer`), once per event whichever window polls first, and says what it did
//! ([`Done`]); `react` then runs in every window of the server, the first included, for what
//! each shows and does itself: its view, plugins, notifications and sounds (each claimed
//! once per event, [`Claim`]), its own sends and stops.

use std::cell::Cell;

use super::*;

/// The latest uninterrupted run of stream writes to one transcript. Bulk loads, clears,
/// other transcript writers and server swaps need no bookkeeping: the cell and both ends of
/// the version interval must match before a reader can narrow its comparison to this id.
#[derive(Default)]
pub(crate) struct TranscriptEdits {
    cell: u32,
    from: u64,
    to: u64,
    session: String,
}

impl TranscriptEdits {
    fn get_mut<'a>(&mut self, map: &'a mut rune::Tracked<HashMap<String, Transcript>>, id: &str) -> Option<&'a mut Transcript> {
        let cell = rune::Tracked::id(map);
        let before = rune::Tracked::version(map);
        let rows = &mut **map;
        if self.cell != cell || self.to != before || self.session != id {
            self.cell = cell;
            self.from = before;
            self.session.clear();
            self.session.push_str(id);
        }
        self.to = before + 1;
        rows.get_mut(id)
    }

    pub(crate) fn only(&self, cell: u32, from: u64, to: u64) -> Option<&str> {
        (cell == self.cell && self.from <= from && from < to && to == self.to).then_some(&self.session)
    }
}

/// A stream message after `apply_shared`, for each window's `react`.
pub enum Heard {
    Event(StreamEvent),
    /// Anything else the stream says: it reconnected, or a line about the stream.
    Message(Incoming),
}

/// Where this poll got the open timeline's geometry. A page wins over stream events in
/// either order: growth opts in a whole stack, not just the row that the stream changed.
#[derive(Default)]
pub(crate) enum TailChange { #[default] None, Live(Route), Page }

pub struct StreamEvent {
    pub ty: String,
    pub data: Value,
    pub done: Done,
    /// The notification and the sound the event calls for, which one window posts or plays.
    pub notice: Cell<Claim>,
    pub sound: Cell<Claim>,
}

/// Whether a window already took what an event calls for once (a notification, a sound). Each
/// window with the session's tab decides; the first to decide yes takes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Claim {
    #[default]
    Open,
    Taken,
}

impl Claim {
    /// `want`: this window would post it. The next state, and whether this window does.
    pub fn step(self, want: bool) -> (Claim, bool) {
        match (self, want) {
            (Claim::Open, true) => (Claim::Taken, true),
            (state, _) => (state, false),
        }
    }
}

impl StreamEvent {
    /// Whether this window takes `claim` for the event: it would, and no window did yet.
    pub fn take(claim: &Cell<Claim>, want: bool) -> bool {
        let (next, take) = claim.get().step(want);
        claim.set(next);
        take
    }
}

/// What `apply_shared` changed.
#[derive(Default)]
pub struct Done {
    /// Running sessions, idle and busy marks, requests (`apply_status`).
    pub status: bool,
    pub inbox: bool,
    /// A revert's mark moved.
    pub revert: bool,
    /// The session's agent or model, archive or metadata.
    pub info: bool,
    /// Selection confirmed by this event, before later events in the batch change it.
    pub selected: Option<Box<pickers::Selection>>,
    /// The session's transcript.
    pub transcript: bool,
    pub renamed: bool,
    pub project: bool,
}

impl App {
    /// A stream message into the server's copy, once (`windows::peer::heard`).
    pub(crate) fn apply_shared(&mut self, m: Incoming) -> Heard {
        match m {
            Incoming::Event { ty, data } => {
                let done = self.apply_event_shared(&ty, &data);
                Heard::Event(StreamEvent { ty, data, done, notice: Cell::default(), sound: Cell::default() })
            }
            Incoming::Reconnected => {
                self.resync_shared();
                Heard::Message(Incoming::Reconnected)
            }
            other => Heard::Message(other),
        }
    }

    /// An event this window alone hears (e.g. fixture playback): both steps here.
    pub(crate) fn apply_event(&mut self, ty: &str, data: &Value) -> bool {
        let Heard::Event(e) = self.apply_shared(Incoming::Event { ty: ty.to_owned(), data: data.clone() }) else { return false };
        self.react(&e)
    }

    /// Only live deliveries opt in, never a history page, replay on open, or reconnect read.
    pub(crate) fn live_event(&mut self, ty: String, data: Value) -> bool {
        let Heard::Event(e) = self.apply_shared(Incoming::Event { ty, data }) else { return false };
        self.react_live(&e)
    }

    pub(crate) fn react_live(&mut self, e: &StreamEvent) -> bool {
        let ty = e.ty.as_str();
        if matches!(self.tail_change, TailChange::None) && (e.done.transcript || e.done.inbox)
            && let Some(id) = e.data["sessionID"].as_str()
            && self.showing_here(id)
            && (ty.starts_with("session.text.") || ty.starts_with("session.reasoning.") || ty.starts_with("session.tool.")
                || matches!(ty, "session.synthetic" | "session.user.message" | "session.prompted" | "session.inbox.delivered")
                || (ty == "session.inbox.enqueued" && self.waiting_finish(id, &e.data)))
        {
            self.tail_change = TailChange::Live((*self.route).clone());
        }
        self.react(e)
    }

    fn showing_here(&self, sid: &str) -> bool {
        matches!(&*self.route, Route::Session { server, id } if id == sid && *server == servers::runtime::current())
    }

    fn waiting_finish(&self, sid: &str, data: &Value) -> bool {
        use session_view::TimelineHost as _;
        use session_view::detail::{Placement, notice_placement};
        let Some(id) = data["inboxID"].as_str() else { return false };
        self.session_revert(sid).is_none()
            && self.inboxes.get(sid).is_some_and(|i| i.waiting_notices().any(|n| n.id == id
                && matches!(n.kind, opencode_kit::model::NoticeKind::Finished(_)) && notice_placement(n, &self.timeline_detail()) != Placement::Hidden))
    }

    pub(crate) fn tail_page(&mut self, sid: &str) {
        if self.showing_here(sid) { self.tail_change = TailChange::Page; }
    }

    pub(crate) fn finish_tail_change(&mut self, tx: &mut Tx) {
        if let TailChange::Live(route) = std::mem::take(&mut self.tail_change) && *self.route == route
            && let Route::Session { id, .. } = route && self.ends.at_end(&id) && !self.ends.due(&id) && !self.stream_gaps.contains_key(&id)
        { tx.grow_visible_tail(format!("scroll-{id}/timeline")); }
    }

    fn apply_event_shared(&mut self, ty: &str, data: &Value) -> Done {
        let sid = data["sessionID"].as_str().unwrap_or("").to_owned();
        let now = *self.now_ms;
        let mut done = Done { status: self.apply_status(ty, data, &sid), ..Done::default() };
        if ty.starts_with("session.inbox.") && !sid.is_empty() {
            done.inbox = self.apply_inbox(ty, data, &sid);
        }
        if ty.starts_with("session.revert.") && !sid.is_empty() {
            done.revert = self.apply_revert(ty, data, &sid);
        }
        done.selected = self.apply_selected(ty, data, &sid);
        done.info = done.selected.is_some() | self.apply_archive(ty, data, &sid);
        if ty == "session.usage.updated" {
            done.info |= self.usage_updated(&sid, data["cost"].as_f64());
        }
        if let Some(t) = self.transcript_edits.get_mut(&mut self.transcripts, &sid) {
            done.transcript = match self.stream_gaps.get_mut(&sid) {
                Some(gapped) => {
                    let (changed, reread) = t.apply_caught(ty, data, now, gapped);
                    if gapped.is_empty() {
                        self.stream_gaps.remove(&sid);
                    }
                    if reread {
                        self.reload_session(&sid);
                    }
                    changed
                }
                None => t.apply(ty, data, now),
            };
        }
        if ty == "session.renamed" && let Some(title) = data["title"].as_str()
            && let Some(session) = self.sessions.iter_mut().find(|s| s.id == sid) {
            session.title = title.to_owned();
            done.renamed = true;
        }
        if crate::recent_page::reloads_on(self, ty, data, &sid) {
            self.schedule_recent_reload();
        }
        if ty == "project.updated"
            && let Some(updated) = opencode_kit::model::parse_project(data)
        {
            match self.projects.iter_mut().find(|p| p.id == updated.id && updated.id != "global") {
                Some(p) => *p = updated,
                None => self.projects.push(updated),
            }
            self.sort_projects();
            done.project = true;
        }
        done
    }

    /// Running sessions, read state and pending requests, in the server's copy.
    fn apply_status(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let id = |v: &Value| v.as_str().unwrap_or("").to_owned();
        match ty {
            "session.execution.started" => {
                if let Some(missing) = self.missing_ancestor(sid) {
                    self.fetch_info(&missing);
                }
                self.stops.forget(sid);
                // The run's start, as the server stamps `time.busy`: the inbox's Working age.
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.busy = Some(data["eventCreated"].as_i64().unwrap_or(*self.now_ms));
                }
                self.running.insert(sid.to_owned())
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let was = self.running.remove(sid);
                crate::stop::confirmed(self, sid);
                if ty == "session.execution.interrupted" && data["reason"] == "shutdown" {
                    return was;
                }
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.idle = Some(data["eventCreated"].as_i64().unwrap_or(*self.now_ms));
                }
                // The server's own idle mark, like Desktop's re-read after a run ends.
                if let Source::Live { requests: Some(r), .. } = &*self.source {
                    let _ = r.send(Request::Info(sid.to_owned()));
                }
                true
            }
            "session.viewed" => {
                let idle = data["idle"].as_i64();
                self.sessions.iter_mut().find(|s| s.id == sid).is_some_and(|s| {
                    s.viewed = s.viewed.max(idle);
                    true
                })
            }
            "permission.asked" | "form.created" => {
                let parsed = if ty == "form.created" { opencode_kit::model::parse_form(data) } else { opencode_kit::model::parse_permission(data) };
                let Some(p) = parsed else { return false };
                self.add_pending(p);
                true
            }
            "permission.replied" | "form.replied" | "form.cancelled" => {
                let rid = if ty == "permission.replied" { id(&data["requestID"]) } else { id(&data["id"]) };
                let had = self.requests.iter().any(|(_, r)| *r == rid) || self.pending.iter().any(|p| p.id() == rid);
                self.pending.retain(|p| p.id() != rid);
                self.requests.retain(|(_, r)| *r != rid);
                crate::notifications::answered(self, &rid);
                had
            }
            _ => false,
        }
    }

    /// `session.revert.staged` hides the transcript from its boundary on, `cleared` shows it again,
    /// and `committed` ends the revert and drops the steers waiting behind it. The transcript's
    /// own rows go in `Transcript::apply`.
    fn apply_revert(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        // The stream's truth: the window that asked keeps its own mark over a stale echo
        // (`revert_landed`).
        let Some(next) = crate::rewind::reported(ty, data) else {
            return false;
        };
        let mut changed = false;
        if let Some(info) = self.sessions.iter_mut().find(|i| i.id == sid) {
            changed |= std::mem::replace(&mut info.revert, next) != info.revert;
        }
        if ty == "session.revert.committed"
            && let Some(to) = data["to"].as_str()
        {
            if let Some(inbox) = self.inboxes.get_mut(sid) {
                let before = inbox.items.len();
                inbox.items.retain(|i| i.id.as_str() < to);
                changed |= inbox.items.len() != before;
            }
        }
        changed
    }

    /// Whether `sid`'s transcript is on screen: the open session's family (its page, agent
    /// rows and subagent panels) is the only place transcripts draw.
    fn shows_transcript(&self, sid: &str) -> bool {
        matches!(&*self.route, Route::Session { id: open, .. } if self.root_of(open) == self.root_of(sid))
    }

    /// An event the server's copy took, in this window: what it shows and does. True when its
    /// view changed.
    pub(crate) fn react(&mut self, e: &StreamEvent) -> bool {
        let (ty, data, done) = (e.ty.as_str(), &e.data, &e.done);
        let sid = data["sessionID"].as_str().unwrap_or("").to_owned();
        if matches!(*self.source, Source::Fixture) { self.served.record(ty, data); }
        crate::plugin_host::server_event(self, ty, data);
        let mut changed = self.note_agent(ty, data, &sid);
        changed |= done.status | self.status_landed(ty, data, &sid);
        changed |= self.note_review_event(ty);
        if ty.starts_with("pty.") || ty.starts_with("persistent-pty.") {
            changed |= self.apply_pty_event(ty, data);
        }
        crate::notifications::event(self, e, &sid);
        crate::sounds::event(self, e, &sid);
        crate::speech::event(self, ty, data, &sid);
        if ty.starts_with("session.inbox.") && !sid.is_empty() {
            changed |= done.inbox | self.inbox_landed(ty, data, &sid);
            crate::background::admitted(self, ty, data, &sid);
        }
        if ty.starts_with("shell.") {
            changed |= crate::subagents::shells::event(self, ty, data);
        }
        if ty.starts_with("session.revert.") && !sid.is_empty() {
            changed |= done.revert | self.revert_landed(ty, data, &sid);
        }
        if let Some(selected) = &done.selected {
            self.selected_landed(&sid, selected);
        }
        changed |= done.info;
        if matches!(ty, "session.archived" | "session.unarchived") { self.session_search.invalidate(); }
        if ty == "session.text.delta" && self.shows_transcript(&sid) {
            crate::telemetry::streamed(&self.window.id, &sid, data);
        }
        // A transcript shows only in its own family's page, so another session's stream
        // updates its state without rebuilding the view.
        changed |= done.transcript && self.shows_transcript(&sid);
        if ty.starts_with("session.") && !sid.is_empty() {
            // A step started in the open session may follow a prompt we haven't seen.
            if ty == "session.step.started" && self.route == Route::here(sid.clone()) {
                self.reload_session(&sid);
            }
        }
        if ty == "worktree.updated" && let Some(project) = data["projectID"].as_str() {
            self.worktree_updated(project);
        }
        if done.project || done.renamed {
            self.session_search.invalidate();
            changed = true;
        }
        changed
    }

    /// After `apply_status`, in this window: its stops, its sidebar's request times, its
    /// question dock and replies on their way, and the open session's read mark.
    fn status_landed(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        match ty {
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                // Desktop reports the open session's idle mark as viewed, so it never lights
                // up: every window that shows it marks it, whichever window took the event.
                let open = matches!(&*self.route, Route::Session { id: r, .. } if self.root_of(r) == self.root_of(sid));
                let marked = open && self.sessions.iter_mut().find(|s| s.id == sid).is_some_and(|s| {
                    let was = s.viewed;
                    s.viewed = s.viewed.max(s.idle);
                    s.viewed != was
                });
                if marked { crate::windows::peer::touch(self); }
                marked
            }
            "vcs.branch.updated" => {
                let Some(dir) = data["eventLocation"]["directory"].as_str() else { return false };
                self.sidebar_inbox.branches.updated(dir, data["branch"].as_str().map(str::to_owned));
                true
            }
            "permission.asked" | "form.created" => {
                let id = if data["form"].is_object() { &data["form"]["id"] } else { &data["id"] };
                let at = data["eventCreated"].as_i64().unwrap_or(*self.now_ms);
                if let Some(id) = id.as_str() { self.sidebar_inbox.asked(id, at); }
                false
            }
            "permission.replied" | "form.replied" | "form.cancelled" => {
                let rid = if ty == "permission.replied" { &data["requestID"] } else { &data["id"] };
                let rid = rid.as_str().unwrap_or("");
                self.question.forget(rid);
                self.permission_sent.0.remove(rid);
                false
            }
            _ => false,
        }
    }

    /// After `apply_revert`, in this window: a revert it asked for keeps its own mark over a
    /// stale event's.
    fn revert_landed(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let mut changed = false;
        if let Some(next) = crate::rewind::landed(self, ty, data, sid)
            && let Some(info) = self.sessions.iter_mut().find(|i| i.id == sid)
            && info.revert != next
        {
            info.revert = next;
            crate::windows::peer::touch(self);
            changed = true;
        }
        changed
    }
}

impl App {
    /// Archives and metadata changed from other clients.
    fn apply_archive(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        if !matches!(ty, "session.archived" | "session.unarchived" | "session.metadata.updated") {
            return false;
        }
        let Some(i) = self.sessions.iter_mut().find(|i| i.id == sid) else { return false };
        match ty {
            "session.archived" => i.archived = Some(data["eventCreated"].as_i64().unwrap_or(*self.now_ms)),
            "session.unarchived" => i.archived = None,
            "session.metadata.updated" => i.metadata = data["metadata"].clone(),
            _ => return false,
        }
        true
    }
}
