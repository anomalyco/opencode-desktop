use super::*;

impl App {
    /// Tab marks: running sessions, read state and pending requests.
    pub(crate) fn apply_status(&mut self, ty: &str, data: &Value, sid: &str) -> bool {
        let id = |v: &Value| v.as_str().unwrap_or("").to_owned();
        match ty {
            "session.execution.started" => {
                if let Some(missing) = self.missing_ancestor(sid) {
                    self.fetch_info(&missing);
                }
                self.running.insert(sid.to_owned())
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                let was = self.running.remove(sid);
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
        let mut changed = self.note_agent(ty, data, &sid);
        if ty.starts_with("config.") {
            changed |= crate::compaction::apply_config(self, ty, data);
        }
        changed |= self.apply_status(ty, data, &sid);
        changed |= self.note_review_event(ty);
        if ty.starts_with("pty.") {
            changed |= self.apply_pty_event(ty, data);
        }
        crate::notifications::event(self, ty, data, &sid);
        if ty.starts_with("session.inbox.") && !sid.is_empty() {
            changed |= self.apply_inbox(ty, data, &sid);
            crate::background::admitted(self, ty, data, &sid);
        }
        if ty.starts_with("session.revert.") && !sid.is_empty() {
            changed |= self.apply_revert(ty, data, &sid);
        }
        changed |= self.apply_selected(ty, data, &sid);
        if let Some(t) = self.transcripts.get_mut(&sid) {
            // A transcript shows only in its own family's page, so another session's stream
            // updates its state without rebuilding the view.
            changed |= t.apply(ty, data, now) && self.shows_transcript(&sid);
        }
        if ty.starts_with("session.") && !sid.is_empty() {
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                if s.updated < now - 1000 || ty == "session.step.ended" {
                    s.updated = now;
                    changed = true;
                }
                if ty == "session.renamed"
                    && let Some(t) = data["title"].as_str()
                {
                    s.title = t.to_owned();
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
        if ty == "project.updated"
            && let Some(updated) = opencode_kit::model::parse_project(data)
        {
            match self.projects.iter_mut().find(|p| p.id == updated.id && updated.id != "global") {
                Some(p) => *p = updated,
                None => self.projects.push(updated),
            }
            self.sort_projects();
            changed = true;
        }
        changed
    }
}
