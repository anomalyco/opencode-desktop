use super::*;
use crate::tabs::{ClosedTab, TabKind};

impl App {
    pub(crate) fn save_tabs(&mut self) {
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
            tabs::save(&self.window, &server.base, &self.tabs);
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

    /// A tab's mark, from its family's marks (`session_status`): busy while anything in the
    /// family works (unless it's waiting on you), then nothing for the open tab, then attention
    /// before unread.
    pub fn tab_status(&self, id: &str) -> Option<TabStatus> {
        let marks = self.marks(id);
        if marks.busy {
            return Some(TabStatus::Busy);
        }
        if self.route == Route::Session(id.to_owned()) {
            return None;
        }
        if marks.attention {
            return Some(TabStatus::Attention);
        }
        let info = self.session(id).filter(|_| marks.unread)?;
        // As in the inbox, an unread run that failed gets the danger dot.
        Some(if info.outcome.as_deref() == Some("failed") { TabStatus::Failed } else { TabStatus::Unread })
    }

    /// Whether tab `id` shows a dot for something new, failed or not.
    pub(crate) fn tab_unread(&self, id: &str) -> bool {
        matches!(self.tab_status(id), Some(TabStatus::Unread | TabStatus::Failed))
    }

    /// Home's button and ⌘B: Home, or back from it to the last tab.
    pub(crate) fn toggle_home(&mut self, tx: &mut Tx) -> bool {
        if self.route == Route::Home && !self.settings.open {
            self.opening = None;
            self.route = self.tabs.last().map_or(Route::Home, crate::tabs::Tab::route);
            tx.settle();
            return false;
        }
        self.go_home(tx);
        true
    }

    /// ⇧⌘B and the sidebar toggle. Top tabs have no sidebar, so it does nothing there.
    pub(crate) fn toggle_sidebar(&mut self, tx: &mut Tx) -> bool {
        if !self.shows_sidebar() {
            return false;
        }
        // The side region's contents glide with its edge while the sidebar's flight moves it
        // (`panels::view::region_card`), and land at once again after.
        if self.side_open() {
            self.side.edge_flight = Some(self.clock + crate::panels::view::SIDE_FLIGHT);
            tx.wake_after(std::time::Duration::from_secs_f64(crate::panels::view::SIDE_FLIGHT + 0.01));
        }
        self.sidebar_open = !self.sidebar_open;
        let event = if self.sidebar_open { crate::layout_state::PeekEvent::DockOpened } else { crate::layout_state::PeekEvent::DockClosed };
        self.layout.peek_event(event, self.clock);
        tx.wake_after(std::time::Duration::from_secs_f64(crate::layout_state::PEEK_HANDOFF_TIMEOUT));
        true
    }

    /// How wide the docked sidebar is: none for top tabs, or while it's closed.
    pub(crate) fn sidebar_shown_w(&self) -> f32 {
        if self.sidebar_open && self.shows_sidebar() { self.sidebar_w } else { 0.0 }
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
        crate::view::draft_location::picked(self, &dir);
        self.forget_new_worktree();
        self.note_destination(&dir, "local");
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
        self.apply_default_destination();
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
                self.add.park(id.to_owned(), session_view::composer::mode::Mode::Normal);
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

    /// The order the tab keys step through: the tabs, or in the inbox its rows as they show,
    /// top to bottom, the Working shelf's while it's open.
    pub(crate) fn nav_order(&self) -> Vec<String> {
        if self.tab_layout() != crate::layout_state::TabLayout::Inbox {
            return self.tabs.iter().map(|t| t.id.clone()).collect();
        }
        let sections = self.inbox_sections();
        let shelf = if self.restore.inbox.working_expanded { sections.working.len() } else { 0 };
        let listed = sections.current.len() + sections.needs.len() + sections.unread.len() + sections.recent.len();
        sections.rows().take(listed + shelf).map(|e| e.session.id.clone()).collect()
    }

    /// The tab, or inbox row, the open page belongs to.
    fn nav_current(&self) -> Option<String> {
        match (&self.route, self.tab_layout()) {
            (Route::Session(id), crate::layout_state::TabLayout::Inbox) if !self.settings.open => Some(self.root_of(id).to_owned()),
            _ => self.current_tab(),
        }
    }

    /// Shows `id` from the tab keys: its tab, or in the inbox its row's session.
    fn nav_select(&mut self, id: &str, tx: &mut Tx) {
        match self.tab_layout() {
            crate::layout_state::TabLayout::Inbox => self.open_inbox_session(id, tx),
            _ => self.select_tab(id, tx),
        }
    }

    /// ⌘1–⌘9: the `n`th tab, or inbox row, from the top.
    pub(crate) fn select_nth(&mut self, n: usize, tx: &mut Tx) {
        if let Some(id) = self.nav_order().get(n).cloned() {
            self.nav_select(&id, tx);
        }
    }

    /// ⌃Tab, ⌥↑ and ⌥↓: the next tab or inbox row that way, wrapping (`adjacentTabKey`).
    pub(crate) fn select_adjacent(&mut self, offset: i32, tx: &mut Tx) {
        let order = self.nav_order();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        let current = self.nav_current();
        if let Some(id) = shortcuts::adjacent(&order, current.as_deref(), offset, |_| true) {
            self.nav_select(&id, tx);
        }
    }

    /// ⌥⇧↓ and ⌥⇧↑: the next tab or inbox row that needs you, oldest question or permission
    /// first, then the unread ones (`shortcuts::needs_you`), wrapping.
    pub(crate) fn select_needs_you(&mut self, offset: i32, tx: &mut Tx) {
        let order = self.nav_order();
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        let stops = shortcuts::needs_you(&order, |k| self.waiting_since(k), |k| self.tab_unread(k));
        if let Some(id) = shortcuts::next_stop(&stops, self.nav_current().as_deref(), offset) {
            self.nav_select(&id, tx);
        }
    }

    /// When the oldest question or permission in `id`'s family was asked, if one waits on you.
    fn waiting_since(&self, id: &str) -> Option<i64> {
        let (permission, form) = self.family_requests(id);
        permission.into_iter().chain(form).map(|r| self.sidebar_inbox.asked.get(r).copied().unwrap_or(i64::MAX)).min()
    }
}
