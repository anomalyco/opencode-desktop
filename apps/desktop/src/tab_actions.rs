use super::*;

impl App {
    /// A fresh profile's tabs: the most recent root sessions.
    pub(crate) fn recent_tabs(&self) -> Vec<Tab> {
        let mut roots: Vec<&SessionInfo> = self.sessions.iter().filter(|s| s.parent.is_none()).collect();
        roots.sort_by_key(|s| std::cmp::Reverse(s.updated));
        roots.iter().take(8).map(|s| Tab { id: s.id.clone(), title: s.title.clone(), directory: s.directory.clone() }).collect()
    }

    pub(crate) fn save_tabs(&mut self) {
        // Keep titles fresh for sessions outside the loaded list next launch.
        for t in &mut self.tabs {
            if let Some(s) = self.sessions.iter().find(|s| s.id == t.id) {
                t.title = s.title.clone();
                t.directory = s.directory.clone();
            }
        }
        #[cfg(test)]
        tabs::SAVED.with(|saved| *saved.borrow_mut() = Some(self.tabs.iter().map(|t| t.id.clone()).collect()));
        if let Source::Live { server, .. } = &self.source
            && self.persists()
        {
            tabs::save(&server.base, &self.tabs);
        }
    }

    /// A dropped tab: it takes its new place, and ⌘1–9 follow the new order.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.save_tabs();
    }

    /// The root of a session's family.
    pub fn root_of<'a>(&'a self, id: &'a str) -> &'a str {
        let mut cur = id;
        for _ in 0..32 {
            match self.session(cur).and_then(|s| s.parent.as_deref()) {
                Some(p) => cur = p,
                None => break,
            }
        }
        cur
    }

    /// A tab's mark, as Desktop decides it: busy while anything in the family runs (unless it's
    /// waiting on you), then nothing for the open tab, then attention before unread.
    pub fn tab_status(&self, id: &str) -> Option<TabStatus> {
        let attention = self.requests.iter().any(|(s, _)| self.root_of(s) == id);
        if !attention && self.running.iter().any(|s| self.root_of(s) == id) {
            return Some(TabStatus::Busy);
        }
        if self.route == Route::Session(id.to_owned()) {
            return None;
        }
        if attention {
            return Some(TabStatus::Attention);
        }
        self.session(id).is_some_and(SessionInfo::unread).then_some(TabStatus::Unread)
    }

    pub fn close_tab(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.tabs.remove(i);
        self.closed.push((i, tab));
        if self.route == Route::Session(id.to_owned()) {
            match self.tabs.get(i).or_else(|| self.tabs.get(i.wrapping_sub(1))).map(|t| t.id.clone()) {
                Some(next) => self.open(&next, tx),
                None => {
                    self.close_picker(false);
                    self.route = Route::Home;
                    tx.settle();
                }
            }
        }
        self.save_tabs();
    }

    /// The tab the open page belongs to, if any.
    pub(crate) fn current_tab(&self) -> Option<String> {
        let Route::Session(id) = &self.route else { return None };
        let root = self.root_of(id);
        self.tabs.iter().any(|t| t.id == root).then(|| root.to_owned())
    }

    pub(crate) fn select_adjacent(&mut self, offset: i32, unread_only: bool, tx: &mut Tx) {
        let order: Vec<&str> = self.tabs.iter().map(|t| t.id.as_str()).collect();
        let current = self.current_tab();
        let next = shortcuts::adjacent(&order, current.as_deref(), offset, |k| {
            !unread_only || (Some(k) != current.as_deref() && self.tab_status(k) == Some(TabStatus::Unread))
        });
        if let Some(id) = next {
            self.open(&id, tx);
        }
    }
}
