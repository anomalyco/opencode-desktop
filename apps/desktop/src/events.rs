use super::*;

impl App {
    /// Tab marks: running sessions, read state and pending requests.
    pub(crate) fn apply_status(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let id = |v: &Value| v.as_str().unwrap_or("").to_owned();
        match ty {
            "session.interrupt.acknowledged" => {
                // An idle/already-stopping execution has no new end event to send us.
                if data["interrupted"] == false {
                    crate::stop::confirmed(self, sid);
                }
                true
            }
            "session.execution.started" => {
                if let Some(missing) = self.missing_ancestor(sid) {
                    self.fetch_info(&missing);
                }
                self.stops.forget(sid);
                // The run's start, as the server stamps `time.busy`: the inbox's Working age.
                let now = self.now_ms;
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.busy = Some(now);
                }
                self.running.insert(sid.to_owned())
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let was = self.running.remove(sid);
                crate::stop::confirmed(self, sid);
                if ty == "session.execution.interrupted" && data["reason"] == "shutdown" {
                    return was;
                }
                let now = self.now_ms;
                // Desktop reports the open session's idle mark as viewed, so it never lights up.
                let open = matches!(&self.route, Route::Session(r) if self.root_of(r) == self.root_of(sid));
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.idle = Some(now);
                    if open {
                        s.viewed = Some(now);
                    }
                }
                // The server's own idle mark, like Desktop's re-read after a run ends.
                if let Source::Live { requests: Some(r), .. } = &self.source {
                    let _ = r.send(Request::Info(sid.to_owned()));
                }
                true
            }
            "vcs.branch.updated" => {
                let Some(dir) = data["eventLocation"]["directory"].as_str() else { return false };
                self.sidebar_inbox.branches.updated(dir, data["branch"].as_str().map(str::to_owned));
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
                let at = data["eventCreated"].as_i64().unwrap_or(self.now_ms);
                self.sidebar_inbox.asked(p.id(), at);
                self.add_pending(p);
                true
            }
            "permission.replied" | "form.replied" | "form.cancelled" => {
                let rid = if ty == "permission.replied" { id(&data["requestID"]) } else { id(&data["id"]) };
                let had = self.requests.iter().any(|(_, r)| *r == rid) || self.pending.iter().any(|p| p.id() == rid);
                self.forget(&rid);
                had
            }
            _ => false,
        }
    }

    /// `session.revert.staged` hides the transcript from its boundary on, `cleared` shows it again, and
    /// `committed` ends the revert and drops what it hid: the steers waiting behind it and their glows
    /// (the transcript's own rows go in `Transcript::apply`).
    pub(crate) fn apply_revert(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let Some(next) = crate::rewind::resolve_event(self, ty, data, sid) else {
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
            if let Some(t) = self.transcripts.get(sid) {
                for m in &t.messages[t.visible_len(Some(to))..] {
                    changed |= self.glows.remove(m.id()).is_some();
                }
            }
        }
        changed
    }

    /// Whether `sid`'s transcript is on screen: the open session's family (its page, agent
    /// rows and subagent panels) is the only place transcripts draw.
    fn shows_transcript(&self, sid: &str) -> bool {
        matches!(&self.route, Route::Session(open) if self.root_of(open) == self.root_of(sid))
    }

    pub(crate) fn apply_event(&mut self, ty: &str, data: &Value) -> bool {
        let sid = data["sessionID"].as_str().unwrap_or("").to_owned();
        let now = self.now_ms;
        if matches!(self.source, Source::Fixture) { self.served.record(ty, data); }
        crate::plugin_host::server_event(self, ty, data);
        let mut changed = self.note_agent(ty, data, &sid);
        changed |= self.apply_status(ty, data, &sid);
        changed |= self.note_review_event(ty);
        if ty.starts_with("pty.") || ty.starts_with("persistent-pty.") {
            changed |= self.apply_pty_event(ty, data);
        }
        crate::notifications::event(self, ty, data, &sid);
        crate::sounds::event(self, ty, data, &sid);
        if ty.starts_with("session.inbox.") && !sid.is_empty() {
            changed |= self.apply_inbox(ty, data, &sid);
            crate::subagents::reply::event(self, ty, data, &sid);
            crate::background::admitted(self, ty, data, &sid);
        }
        if ty.starts_with("session.revert.") && !sid.is_empty() {
            changed |= self.apply_revert(ty, data, &sid);
        }
        changed |= self.apply_selected(ty, data, &sid);
        changed |= self.apply_archive(ty, data, &sid);
        if matches!(ty, "session.archived" | "session.unarchived") { self.switcher.invalidate_sessions(); }
        if let Some(t) = self.transcripts.get_mut(&sid) {
            // A transcript shows only in its own family's page, so another session's stream
            // updates its state without rebuilding the view.
            changed |= t.apply(ty, data, now) && self.shows_transcript(&sid);
        }
        if ty.starts_with("session.") && !sid.is_empty() {
            // A plain session page with side/top tabs shows neither the age nor the
            // recency order of unrelated sessions. Still keep their metadata current.
            let shows_age = !matches!(self.route, Route::Session(_)) || self.shows_transcript(&sid)
                || self.tab_layout() == crate::layout_state::TabLayout::Inbox || self.switcher.open
                || self.mentions.open || self.side_open() || self.settings.open;
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                if s.updated < now - 1000 || ty == "session.step.ended" {
                    s.updated = now;
                    self.switcher.invalidate_sessions();
                    changed |= shows_age;
                }
                if ty == "session.renamed"
                    && let Some(t) = data["title"].as_str()
                {
                    s.title = t.to_owned();
                    self.switcher.invalidate_sessions();
                    changed = true;
                }
            } else if ty == "session.created" || ty == "session.step.started" {
                // A session we haven't listed: refresh the list (one GET).
                if let Source::Live { requests: Some(r), .. } = &self.source {
                    let _ = r.send(Request::Sessions);
                }
            }
            // A step started in the open session may follow a prompt we haven't seen.
            if ty == "session.step.started" && self.route == Route::Session(sid.clone()) {
                self.reload_session(&sid);
            }
        }
        if ty.starts_with("worktree.") || ty == "vcs.branch.listed" {
            if self.apply_worktree_event(ty, data) {
                self.switcher.invalidate_sessions();
                changed = true;
            }
        }
        if ty == "project.updated"
            && let Some(updated) = opencode_kit::model::parse_project(data)
        {
            match self.projects.iter_mut().find(|p| p.id == updated.id && updated.id != "global") {
                Some(p) => *p = updated,
                None => self.projects.push(updated),
            }
            self.sort_projects();
            self.switcher.invalidate_sessions();
            changed = true;
        }
        changed
    }
}

impl App {
    /// Archives and metadata changed from other clients.
    fn apply_archive(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let now = self.now_ms;
        let Some(i) = self.sessions.iter_mut().find(|i| i.id == sid) else { return false };
        match ty {
            "session.archived" | "session.unarchived" => i.archived = (ty == "session.archived").then_some(now),
            "session.metadata.updated" => i.metadata = data["metadata"].clone(),
            _ => return false,
        }
        true
    }
}
