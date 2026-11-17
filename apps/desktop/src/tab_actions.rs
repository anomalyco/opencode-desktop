use super::*;
use crate::tabs::{ClosedTab, TabKind};

impl App {
    pub(crate) fn save_tabs(&mut self) {
        // Keep titles fresh for sessions outside the loaded list next launch.
        let here = crate::servers::runtime::active(self).clone();
        for t in self.tabs.iter_mut().filter(|t| t.kind == TabKind::Session && t.server == here) {
            if let Some(s) = self.sessions.iter().find(|s| s.id == t.id) {
                t.title = s.title.clone();
                t.directory = s.directory.clone();
            }
        }
        if self.persists() {
            // The mixed list and what `tabs.info` keeps of each, as Desktop's window file has them.
            let (mut list, info): (Vec<_>, serde_json::Map<String, Value>) = self.tabs.iter().map(Tab::to_doc).fold((Vec::new(), serde_json::Map::new()), |(mut list, mut info), (tab, i)| {
                if !i.is_empty() {
                    info.insert(tab.key().encode(), i.to_json());
                }
                list.push(tab.to_json());
                (list, info)
            });
            list.extend(self.restore.unread_tabs.iter().cloned());
            crate::windows::store::save(&self.window, "tabs", &Value::Array(list));
            crate::windows::store::save(&self.window, "tabs.info", &Value::Object(info));
        }
    }

    /// The server listed its sessions or one session's info: a tab whose saved title or folder that
    /// changed is saved again, so a launch that can't reach the server still names its tabs (it was
    /// saved only when the tabs themselves changed).
    pub(crate) fn note_tab_titles(&mut self) {
        let here = crate::servers::runtime::active(self).clone();
        let stale = self.tabs.iter().filter(|t| t.kind == TabKind::Session && t.server == here).any(|t| {
            self.sessions.iter().find(|s| s.id == t.id).is_some_and(|s| s.title != t.title || s.directory != t.directory)
        });
        if stale {
            self.save_tabs();
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
        match self.tab(id) {
            Some(t) if !t.is_draft() => self.tab_status_of(t),
            _ => self.status_here(id),
        }
    }

    /// Tab `t`'s mark: on a server the window isn't working against, as of when it last did.
    pub fn tab_status_of(&self, t: &Tab) -> Option<TabStatus> {
        if t.server != *crate::servers::runtime::active(self) {
            return self.checkout.snap(&t.server, &t.id);
        }
        self.status_here(&t.id)
    }

    /// Session `id`'s mark on the server the window works against.
    fn status_here(&self, id: &str) -> Option<TabStatus> {
        let marks = self.marks(id);
        if marks.busy {
            return Some(TabStatus::Busy);
        }
        if self.route == Route::here(id.to_owned()) && !crate::session_menu::unread::manual(self, crate::servers::runtime::active(self), id) {
            return None;
        }
        if marks.attention {
            return Some(TabStatus::Attention);
        }
        let info = self.session(id).filter(|_| marks.unread)?;
        // As in the inbox, an unread run that failed gets the danger dot.
        Some(if info.outcome.as_deref() == Some("failed") { TabStatus::Failed } else { TabStatus::Unread })
    }

    /// Where the tab `key` names is: its `Tab::uid`, else the draft or session with that id, the
    /// checked-out server's first (a session id alone can name tabs on two keys of one server).
    pub(crate) fn tab_at(&self, key: &str) -> Option<usize> {
        let here = crate::servers::runtime::active(self);
        self.tabs
            .iter()
            .position(|t| t.uid() == key)
            .or_else(|| self.tabs.iter().position(|t| t.id == key && (t.is_draft() || t.server == *here)))
            .or_else(|| self.tabs.iter().position(|t| t.id == key))
    }

    /// The tab `key` names (`tab_at`).
    pub(crate) fn tab(&self, key: &str) -> Option<&Tab> {
        self.tab_at(key).map(|i| &self.tabs[i])
    }

    /// Home's button and ⌘B: Home, or back to the tab that opened Home if it's still here.
    pub(crate) fn toggle_home(&mut self, tx: &mut Tx) -> bool {
        if self.project_navigation() {
            crate::view::home::browse::open(self, tx);
            return true;
        }
        if self.route == Route::Home && !self.settings.open {
            if let Some(uid) = self.home_return_tab.take()
                && self.tabs.iter().any(|tab| tab.uid() == uid)
            {
                self.select_tab(&uid, tx);
            }
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
            self.side.edge_flight = Some(*self.clock + crate::panels::view::SIDE_FLIGHT);
            tx.wake_after(std::time::Duration::from_secs_f64(crate::panels::view::SIDE_FLIGHT + 0.01));
        }
        *self.sidebar_open = !*self.sidebar_open;
        let event = if *self.sidebar_open { crate::layout_state::PeekEvent::DockOpened } else { crate::layout_state::PeekEvent::DockClosed };
        self.layout.peek_event(event, *self.clock);
        tx.wake_after(std::time::Duration::from_secs_f64(crate::layout_state::PEEK_HANDOFF_TIMEOUT));
        true
    }

    /// How wide the docked sidebar is: none for top tabs, or while it's closed.
    pub(crate) fn sidebar_shown_w(&self) -> f32 {
        if *self.sidebar_open && self.shows_sidebar() { *self.sidebar_w } else { 0.0 }
    }

    /// The open draft's folder, where its session will start; off a draft, the folder last picked.
    pub fn draft_dir(&self) -> &str {
        match &*self.route {
            Route::Draft(id) => self.tabs.iter().find(|t| t.id == *id).map_or(self.default_dir.as_str(), |t| t.directory.as_str()),
            _ => &self.default_dir,
        }
    }

    /// Picks the folder for the open draft (Desktop's `updateDraft`), and for drafts to come.
    pub fn set_draft_dir(&mut self, dir: String) {
        crate::view::draft_location::picked(self, &dir);
        self.forget_new_worktree();
        self.note_destination(&dir, "local");
        let open = match &*self.route {
            Route::Draft(id) => self.tabs.iter_mut().find(|t| t.id == *id && t.directory != dir),
            _ => None,
        };
        if let Some(tab) = open {
            tab.directory = dir.clone();
            self.save_tabs();
        }
        *self.default_dir = dir;
    }

    /// Adds a "New session" draft tab in the last folder picked and shows it (`newDraft`).
    pub(crate) fn open_draft_tab(&mut self) -> String {
        let id = tabs::draft_id();
        self.tabs.push(Tab::draft(id.clone(), (*self.default_dir).clone()));
        *self.route = Route::Draft(id.clone());
        let _ = self.show_draft();
        self.apply_default_destination();
        self.composer.focused = self.stage.is_none() && !session_view::style::touch_controls();
        self.save_tabs();
        id
    }

    /// Shows tab `key` (`tab_at`): a session tab opens its session, a draft tab its new-session
    /// page.
    pub fn select_tab(&mut self, key: &str, tx: &mut Tx) {
        let Some(tab) = self.tab(key).cloned() else { return self.open(key, tx) };
        // The window works against the tab's server from here on (`servers::runtime`).
        crate::servers::runtime::switch(self, &tab.server);
        if !tab.is_draft() {
            return self.open(&tab.id, tx);
        }
        self.close_settings();
        self.close_picker(false);
        *self.opening = None;
        *self.route = Route::Draft(tab.id);
        let _ = self.show_draft();
        if !crate::auto_focus_composer(tx) {
            self.composer.focused = false;
        } else if self.stage.is_none() {
            self.composer.focused = true;
        }
        tx.settle();
    }

    /// The draft the open page shows becomes session `id`'s tab, in its place (Desktop's
    /// `prepareSession`). The draft's id, or None when no draft is open.
    pub(crate) fn promote_draft(&mut self, id: &str, directory: &str) -> Option<String> {
        let Route::Draft(draft) = (*self.route).clone() else { return None };
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
            None if self.route == Route::here(id.to_owned()) => self.tabs.push(back),
            None => return,
        }
        self.save_tabs();
    }

    /// Closes tab `key` (`tab_at`).
    pub fn close_tab(&mut self, key: &str, tx: &mut Tx) {
        tx.instant();
        let Some(i) = self.tab_at(key) else { return };
        let shown = self.current_tab() == Some(self.tabs[i].uid());
        let tab = self.tabs.remove(i);
        let (id, server) = (tab.id.clone(), tab.server.clone());
        match tab.kind {
            TabKind::Session => {
                // Its history goes where its server's sessions are known: the window's own fields
                // while it works against that server.
                crate::servers::runtime::with_server(self, &server, |s| s.forget_history(|s, x| s.root_of(x) == id));
                let prompted = crate::servers::runtime::with_server(self, &server, |s| s.prompted(&id)).unwrap_or(false);
                self.closed.push(ClosedTab { tab, index: i, prompted });
                *self.tab_closed_ms = Some(*self.now_ms);
            }
            // A draft with something in it is kept, words, files and all, and can be reopened
            // (⇧⌘T, or the toast's Undo); Desktop disposes of it (`disposeTab`).
            TabKind::Draft if self.draft_has_content(&id) => {
                self.forget_draft_history(&id);
                if shown {
                    self.park_shown_draft();
                }
                let now = *self.now_ms;
                self.closed.push_draft(ClosedTab { tab, index: i, prompted: false }, now);
                *self.tab_closed_ms = Some(now);
                crate::undo::offer(self, opencode_i18n::t!("rune.undo.draftClosed"), crate::undo::Undoable::DraftTab { draft: id.clone() });
            }
            // An empty one just goes.
            TabKind::Draft => {
                self.forget_draft_history(&id);
                self.restore.drafts.remove(&id);
                self.restore.parked.remove(&id);
                self.add.park(id.clone(), session_view::composer::mode::Mode::Normal);
                if shown {
                    self.drop_shown_draft();
                }
            }
        }
        if shown {
            match self.tabs.get(i).or_else(|| self.tabs.get(i.wrapping_sub(1))).map(Tab::uid) {
                Some(next) => self.select_tab(&next, tx),
                None => {
                    self.close_picker(false);
                    self.go_home(tx);
                    // Desktop leaves nothing focused on Home after its last tab (`fn-key-tab.close`),
                    // where the closed page's focus would fall to New session.
                    tx.blur();
                }
            }
        }
        self.save_tabs();
    }

    /// ⇧⌘T: the newest closed tab back in its place.
    pub(crate) fn reopen_tab(&mut self, tx: &mut Tx) {
        // The side panel's tab when it closed since the last titlebar tab (`reopensSideTab`,
        // `use-session-commands.tsx:129-136`), wherever focus is.
        let side = self.active_session_id().and_then(|sid| self.side.sessions.get(sid)).and_then(|s| s.closed.first()).map(|c| c.closed_ms);
        if side.is_some_and(|side| self.tab_closed_ms.is_none_or(|tab| side >= tab)) && self.reopen_closed_side_tab(None, tx) {
            return;
        }
        self.reopen_closed(None, false, tx);
    }

    /// Reopens closed tab `target` (or the newest), at its old index or, with `append`, at the
    /// end as "Recently closed" does (`reopenClosedTab`).
    pub(crate) fn reopen_closed(&mut self, target: Option<&str>, append: bool, tx: &mut Tx) {
        let Some(entry) = self.closed.take(&self.tabs, target) else { return };
        let at = if append { self.tabs.len() } else { entry.index.min(self.tabs.len()) };
        let (id, uid, draft) = (entry.tab.id.clone(), entry.tab.uid(), entry.tab.is_draft());
        crate::servers::runtime::switch(self, &entry.tab.server);
        self.tabs.insert(at, entry.tab);
        // A draft comes back with what it held (its words and the rest stayed parked).
        if draft { self.select_tab(&uid, tx) } else { self.open(&id, tx) }
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

    /// The tab the open page belongs to, if any (its `Tab::uid`): a draft's, or the root
    /// session's on the page's server.
    pub(crate) fn current_tab(&self) -> Option<String> {
        match &*self.route {
            Route::Draft(id) => self.tabs.iter().find(|t| t.id == *id).map(Tab::uid),
            Route::Session { id, server } => {
                let root = self.root_of(id);
                self.tabs.iter().find(|t| t.id == root && !t.is_draft() && t.server == *server).map(Tab::uid)
            }
            Route::Home => None,
        }
    }

    /// The order the tab keys step through: the tabs, or in the inbox its rows as they show,
    /// top to bottom, the Working shelf's while it's open.
    pub(crate) fn nav_order(&self) -> Vec<String> {
        if self.tab_layout() != crate::layout_state::TabLayout::Inbox {
            return self.tabs.iter().map(Tab::uid).collect();
        }
        let sections = self.inbox_sections();
        let shelf = if self.restore.inbox.working_expanded { sections.working.len() } else { 0 };
        let listed = sections.current.len() + sections.needs.len() + sections.unread.len() + sections.recent.len();
        sections.rows().take(listed + shelf).map(|e| e.session.id.clone()).collect()
    }

    /// The tab, or inbox row, the open page belongs to.
    fn nav_current(&self) -> Option<String> {
        match (&*self.route, self.tab_layout()) {
            (Route::Session { id, server }, crate::layout_state::TabLayout::Inbox) if !self.settings.open => Some(tabs::session_uid(server, self.root_of(id))),
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

    /// ⌥⇧↑ and ⌥⇧↓, Previous and Next unread session: the nearest tab or inbox row that way
    /// whose dot shows, a question or permission waiting on you or something new, wrapping; none
    /// without an open tab (`session.previous.unseen`, tab-strip.tsx:578-591, `adjacentTabKey`).
    pub(crate) fn select_unread(&mut self, offset: i32, tx: &mut Tx) {
        let order = self.nav_order();
        let Some(current) = self.nav_current().filter(|c| order.contains(c)) else { return };
        let order: Vec<&str> = order.iter().map(String::as_str).collect();
        // Another server's inbox row says so as its server last showed it.
        let dot = |k: &str| matches!(self.tab_status(k), Some(TabStatus::Attention | TabStatus::Unread | TabStatus::Failed)) || self.inbox_row_dotted(k);
        if let Some(id) = shortcuts::adjacent(&order, Some(&current), offset, dot) {
            self.nav_select(&id, tx);
        }
    }
}
