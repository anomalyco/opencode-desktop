use super::*;

impl App {
    /// Opens a session: lands at the bottom on the first frame with nothing animating in.
    pub fn open(&mut self, id: &str, tx: &mut Tx) {
        self.cancel_open(tx);
        if self.pages.has_pool() && self.route != Route::Session(id.to_owned()) {
            self.begin_open(id, tx);
        } else {
            self.open_now(id, tx);
            self.open_page(id, tx);
        }
    }

    /// Commits the route, header and transcript together, once the opening gate is ready.
    pub(crate) fn open_now(&mut self, id: &str, tx: &mut Tx) {
        self.close_picker(false);
        self.activity_open = false;
        if self.route != Route::Session(id.to_owned()) {
            // Its timeline mounts afresh on its end, so "Jump to latest" from the last visit is stale.
            self.ends.forget(id);
        }
        self.route = Route::Session(id.to_owned());
        let root = self.root_of(id).to_owned();
        // As in Desktop, you can type straight away. A subagent's session has no composer, and
        // a story's keys stay with the storybook.
        if root == id && self.stage.is_none() && !self.question_shown() {
            self.composer.focused = true;
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == root) {
            // Viewing it reads it (locally: this build never writes to the server).
            s.viewed = s.idle.max(s.viewed);
        }
        if !self.tabs.iter().any(|t| t.id == root) {
            let (title, directory) = self.session(&root).map(|s| (s.title.clone(), s.directory.clone())).unwrap_or_default();
            self.tabs.push(Tab { id: root.clone(), title, directory });
            self.save_tabs();
        }
        if let Some(pid) = self.session(&root).and_then(|s| self.project_for_session(s)).map(|p| p.id.clone()) {
            self.touch_project(&pid);
        }
        if self.side_open() {
            self.review.dirty = true;
        }
        tx.settle();
        match &self.source {
            Source::Live { requests: Some(r), .. } => {
                if !self.pages.has_pool() { let _ = r.send(Request::Session(id.to_owned())); }
                let _ = r.send(Request::Pending(id.to_owned()));
                let _ = r.send(Request::Inbox(id.to_owned()));
                crate::compaction::ask(self, id);
            }
            Source::Live { .. } => {}
            Source::Fixture => self.start_stream_if_due(id),
        }
    }

    /// The event stream came back: re-reads what a gap may have made stale (what's running, the
    /// session list, pending requests and the open session's latest page), like the first load.
    pub(crate) fn resync(&mut self) {
        let Source::Live { requests: Some(r), .. } = &self.source else { return };
        let _ = r.send(Request::Active);
        let _ = r.send(Request::Sessions);
        let _ = r.send(Request::Projects);
        let open = match &self.route {
            Route::Session(id) => Some(id.clone()),
            _ => None,
        };
        let mut asked: Vec<String> = self.pending.iter().map(|p| p.session().to_owned()).collect();
        asked.extend(open.clone());
        asked.sort();
        asked.dedup();
        for id in asked {
            let _ = r.send(Request::Pending(id));
        }
        if let Some(id) = open {
            let _ = r.send(Request::Inbox(id.clone()));
            self.reload_session(&id);
        }
    }

    pub(crate) fn start_stream_if_due(&mut self, id: &str) {
        let stream = &self.fixture["stream"];
        if stream["session"].as_str() != Some(id) || stream["played"].as_bool() == Some(true) || flag("--no-stream") {
            return;
        }
        let at = self.clock;
        for e in stream["events"].as_array().into_iter().flatten() {
            let t = at + e["t"].as_f64().unwrap_or(0.0);
            self.playback.push_back((t, e["type"].as_str().unwrap_or("").to_owned(), e["data"].clone()));
        }
        self.fixture["stream"]["played"] = true.into();
        self.wake_at_playback();
    }

    /// Applies the fixture's streaming turn at once (the finished session).
    pub(crate) fn finish_stream(&mut self) {
        let events = self.fixture["stream"]["events"].as_array().cloned().unwrap_or_default();
        for e in events {
            self.apply_event(e["type"].as_str().unwrap_or(""), &e["data"]);
        }
        self.fixture["stream"]["played"] = true.into();
    }

    /// Windowed runs sleep between frames; a timer thread wakes the window for each event.
    pub(crate) fn wake_at_playback(&self) {
        let Some(w) = self.waker.clone() else { return };
        let times: Vec<f64> = self.playback.iter().map(|e| e.0 - self.clock).collect();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            for t in times {
                let d = std::time::Duration::from_secs_f64(t.max(0.0));
                if let Some(rest) = d.checked_sub(start.elapsed()) {
                    std::thread::sleep(rest);
                }
                w();
            }
        });
    }
}
