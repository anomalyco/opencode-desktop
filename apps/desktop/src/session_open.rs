use super::*;

impl App {
    /// Opens a session: lands at the bottom on the first frame with nothing animating in. It
    /// leaves Settings, as any page the sidebar opens does.
    pub fn open(&mut self, id: &str, tx: &mut Tx) {
        self.close_settings();
        if self.opening.as_ref().is_some_and(|o| o.id == id) { return; }
        let held = self.pages.has_pool() && self.route != Route::Session(id.to_owned());
        if self.route != Route::Session(id.to_owned()) { crate::telemetry::open_begin(id, held); }
        if held {
            self.begin_open(id, tx);
        } else {
            self.opening = None;
            self.open_now(id, tx);
            self.open_page(id, tx);
        }
    }

    /// Commits the input destination immediately; the transcript can still be loading.
    pub(crate) fn open_now(&mut self, id: &str, tx: &mut Tx) {
        crate::telemetry::committed(id, false);
        let prev_route = self.route.clone();
        self.close_picker(false);
        self.close_message_menu();
        self.close_fork_dialog(false);
        crate::subagents::shells::set_open(self, false);
        self.todo_open = false;
        if self.route != Route::Session(id.to_owned()) {
            // Its timeline mounts afresh on its end, so "Jump to latest" from the last visit is stale.
            self.ends.forget(id);
        }
        self.route = Route::Session(id.to_owned());
        self.sync_route_draft(&prev_route, id);
        let root = self.root_of(id).to_owned();
        // As in Desktop, you can type straight away. A subagent's page waits for a click into
        // its composer, so its keys (E in the inbox, Esc) stay with the page, and a story's
        // keys stay with the storybook.
        if root == id && self.stage.is_none() && !self.dock_shown() {
            self.composer.focused = true;
        }
        // Viewing it reads it, and its tab, locally.
        for s in self.sessions.iter_mut().filter(|s| s.id == root || s.id == id) {
            s.viewed = s.idle.max(s.viewed);
        }
        if !self.tabs.iter().any(|t| t.id == root) {
            let (title, directory) = self.session(&root).map(|s| (s.title.clone(), s.directory.clone())).unwrap_or_default();
            self.tabs.push(Tab::session(root.clone(), title, directory));
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
            // A session still starting has nothing to read yet (`new_session.rs`).
            Source::Live { .. } if self.starts.has(id) => {}
            Source::Live { requests: Some(r), .. } => {
                if !self.pages.has_pool() { let _ = r.send(Request::Session(id.to_owned())); }
                let _ = r.send(Request::Inbox(id.to_owned()));
                self.read_open_requests(id);
                if let Some(dir) = self.session(id).map(|i| i.directory.clone()) {
                    crate::subagents::shells::list(self, &dir);
                }
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
        if let Some(id) = &open {
            let _ = r.send(Request::Inbox(id.clone()));
        }
        // Requests asked while the stream was down never arrive as events: read again what shows
        // them, as Desktop's store invalidates its syncs on a disconnect.
        self.forget_request_reads();
        crate::subagents::shells::forget_lists(self);
        if let Some(dir) = open.as_deref().and_then(|id| self.session(id)).map(|i| i.directory.clone()) {
            crate::subagents::shells::list(self, &dir);
        }
        let mut asked: Vec<String> = self.pending.iter().map(|p| p.session().to_owned()).collect();
        asked.sort();
        asked.dedup();
        for id in asked {
            self.read_requests(&id);
        }
        if let Some(id) = open {
            self.read_open_requests(&id);
            self.reload_session(&id);
        }
        crate::reconnect::catch_up(self);
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
