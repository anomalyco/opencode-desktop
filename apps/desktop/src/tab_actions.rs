use super::*;
use crate::tabs::{ClosedTab, TabKind};

impl App {
    /// A fresh profile's tabs: the most recent root sessions.
    pub(crate) fn recent_tabs(&self) -> Vec<Tab> {
        let mut roots: Vec<&SessionInfo> = self.sessions.iter().filter(|s| s.parent.is_none()).collect();
        roots.sort_by_key(|s| std::cmp::Reverse(s.updated));
        roots.iter().take(8).map(|s| Tab::session(s.id.clone(), s.title.clone(), s.directory.clone())).collect()
    }

    pub(crate) fn save_tabs(&mut self) {
        // Behind the setup card the tabs are put aside, not closed: nothing to save.
        if self.launch.offline {
            return;
        }
        // Keep titles fresh for sessions outside the loaded list next launch.
        for t in self.tabs.iter_mut().filter(|t| t.kind == TabKind::Session) {
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
        self.ancestors(id).last().unwrap_or(id)
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

    /// The open draft's folder, where its session will start; off a draft, the folder last picked.
    pub fn draft_dir(&self) -> &str {
        match &self.route {
            Route::Draft(id) => self.tabs.iter().find(|t| t.id == *id).map_or(self.default_dir.as_str(), |t| t.directory.as_str()),
            _ => &self.default_dir,
        }
    }

    /// Picks the folder for the open draft (Desktop's `updateDraft`), and for drafts to come.
    pub fn set_draft_dir(&mut self, dir: String) {
        let open = match &self.route {
            Route::Draft(id) => self.tabs.iter_mut().find(|t| t.id == *id && t.directory != dir),
            _ => None,
        };
        if let Some(tab) = open {
            tab.directory = dir.clone();
            self.save_tabs();
        }
        self.default_dir = dir;
    }

    /// Adds a "New session" draft tab in the last folder picked and shows it (`newDraft`).
    pub(crate) fn open_draft_tab(&mut self) -> String {
        let id = tabs::draft_id();
        self.tabs.push(Tab::draft(id.clone(), self.default_dir.clone()));
        self.route = Route::Draft(id.clone());
        if self.stage.is_none() {
            self.composer.focused = true;
        }
        self.save_tabs();
        id
    }

    /// Shows tab `id`: a session tab opens its session, a draft tab its new-session page.
    pub fn select_tab(&mut self, id: &str, tx: &mut Tx) {
        if !self.tabs.iter().any(|t| t.id == id && t.is_draft()) {
            return self.open(id, tx);
        }
        self.close_settings();
        self.close_picker(false);
        self.opening = None;
        self.route = Route::Draft(id.to_owned());
        if self.stage.is_none() {
            self.composer.focused = true;
        }
        tx.settle();
    }

    /// The draft the open page shows becomes session `id`'s tab, in its place (Desktop's
    /// `prepareSession`). The draft's id, or None when no draft is open.
    pub(crate) fn promote_draft(&mut self, id: &str, directory: &str) -> Option<String> {
        let Route::Draft(draft) = self.route.clone() else { return None };
        let tab = self.tabs.iter_mut().find(|t| t.id == draft)?;
        *tab = Tab::session(id, "", directory);
        self.forget_draft_history(&draft);
        self.save_tabs();
        Some(draft)
    }

    /// A start failed: session `id`'s tab turns back into draft `draft` in its place (the
    /// promotion's rollback), or comes back at the end while its page is still showing.
    pub(crate) fn demote_session(&mut self, id: &str, draft: &str, directory: &str) {
        let back = Tab::draft(draft, directory);
        match self.tabs.iter_mut().find(|t| t.id == id) {
            Some(tab) => *tab = back,
            None if self.route == Route::Session(id.to_owned()) => self.tabs.push(back),
            None => return,
        }
        self.save_tabs();
    }

    pub fn close_tab(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let shown = self.current_tab().as_deref() == Some(id);
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let tab = self.tabs.remove(i);
        match tab.kind {
            TabKind::Session => {
                self.forget_history(|s, x| s.root_of(x) == id);
                let prompted = self.prompted(id);
                self.closed.push(ClosedTab { tab, index: i, prompted });
            }
            // A closed draft goes with its words; it can't be reopened (Desktop's `disposeTab`).
            TabKind::Draft => {
                self.forget_draft_history(id);
                self.restore.drafts.remove(id);
                if shown {
                    self.restore.shown = None;
                    let focused = self.composer.focused;
                    self.composer = TextEdit::new("");
                    self.composer.focused = focused;
                }
            }
        }
        if shown {
            match self.tabs.get(i).or_else(|| self.tabs.get(i.wrapping_sub(1))).map(|t| t.id.clone()) {
                Some(next) => self.select_tab(&next, tx),
                None => {
                    self.close_picker(false);
                    self.go_home(tx);
                }
            }
        }
        self.save_tabs();
    }

    /// ⇧⌘T: the newest closed tab back in its place.
    pub(crate) fn reopen_tab(&mut self, tx: &mut Tx) {
        self.reopen_closed(None, false, tx);
    }

    /// Reopens closed tab `target` (or the newest), at its old index or, with `append`, at the
    /// end as "Recently closed" does (`reopenClosedTab`).
    pub(crate) fn reopen_closed(&mut self, target: Option<&str>, append: bool, tx: &mut Tx) {
        let Some(entry) = self.closed.take(&self.tabs, target) else { return };
        let at = if append { self.tabs.len() } else { entry.index.min(self.tabs.len()) };
        let id = entry.tab.id.clone();
        self.tabs.insert(at, entry.tab);
        self.open(&id, tx);
        self.save_tabs();
    }

    /// Whether session `id` had a prompt, so "Recently closed" lists it once closed (Desktop's
    /// `TabInfo.prompted`). One whose messages never loaded counts when the server has it.
    fn prompted(&self, id: &str) -> bool {
        match self.transcripts.get(id) {
            Some(t) => t.messages.iter().any(|m| matches!(m, Message::User { .. })),
            None => self.session(id).is_some(),
        }
    }

    fn forget_draft_history(&mut self, draft: &str) {
        self.sync_history();
        self.nav.forget(|p| matches!(&p.route, Route::Draft(id) if id == draft));
    }

    /// The tab the open page belongs to, if any: a draft's, or the root session's.
    pub(crate) fn current_tab(&self) -> Option<String> {
        match &self.route {
            Route::Draft(id) => self.tabs.iter().any(|t| t.id == *id).then(|| id.clone()),
            Route::Session(id) => {
                let root = self.root_of(id);
                self.tabs.iter().any(|t| t.id == root && !t.is_draft()).then(|| root.to_owned())
            }
            Route::Home => None,
        }
    }

    pub(crate) fn select_adjacent(&mut self, offset: i32, unread_only: bool, tx: &mut Tx) {
        let order: Vec<&str> = self.tabs.iter().map(|t| t.id.as_str()).collect();
        let current = self.current_tab();
        let next = shortcuts::adjacent(&order, current.as_deref(), offset, |k| {
            !unread_only || (Some(k) != current.as_deref() && self.tab_status(k) == Some(TabStatus::Unread))
        });
        if let Some(id) = next {
            self.select_tab(&id, tx);
        }
    }
}
