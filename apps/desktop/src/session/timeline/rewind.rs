//! Desktop host adapter for `opencode-feature-history::rewind` (`features/history`).

pub use opencode_feature_history::rewind::*;

use opencode_kit::client::Write;
use opencode_kit::model::{Inbox, SessionInfo, Transcript};
use opencode_kit::wire_error::WireError;
use opencode_ui::popup::Menu;
use rune::Tx;

use crate::{App, Source};

pub(crate) static REWIND: opencode_kernel::Slot<RewindState> = opencode_kernel::Slot::new();

impl RewindHost for App {
    fn rewind(&self) -> &RewindState {
        self.kernel.slots.get(&REWIND)
    }

    fn rewind_mut(&mut self) -> &mut RewindState {
        self.kernel.slots.get_mut(&REWIND)
    }

    fn session(&self, session: &str) -> Option<&SessionInfo> {
        App::session(self, session)
    }

    fn set_session_revert(&mut self, session: &str, revert: Option<String>) {
        if let Some(info) = self.sessions.iter_mut().find(|i| i.id == session) {
            info.revert = revert;
        }
    }

    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.transcripts.get(session)
    }

    fn transcript_mut(&mut self, session: &str) -> Option<&mut Transcript> {
        self.transcripts.get_mut(session)
    }

    fn inbox(&self, session: &str) -> Option<&Inbox> {
        self.inboxes.get(session)
    }

    fn inbox_mut(&mut self, session: &str) -> Option<&mut Inbox> {
        self.inboxes.get_mut(session)
    }

    fn capture_draft(&self, session: &str) -> SavedDraft {
        App::capture_draft(self, session)
    }

    fn apply_draft(&mut self, session: &str, draft: SavedDraft) {
        App::apply_draft(self, session, draft);
    }

    fn exit_shell(&mut self) {
        App::exit_shell(self);
    }

    fn mention_title(&self, session_id: &str) -> Option<String> {
        App::mention_title(self, session_id)
    }

    fn resolve_skill_id(&self, name_or_id: &str) -> Option<&str> {
        self.kernel.slots.get(&crate::mentions::MENTIONS).skills.iter().find(|sk| sk.id == name_or_id || sk.name == name_or_id).map(|sk| sk.id.as_str())
    }

    fn stop_running_family(&mut self, session: &str) -> Vec<String> {
        let root = self.root_of(session).to_owned();
        let stopped: Vec<String> = self.running.iter().filter(|s| self.root_of(s) == root).cloned().collect();
        for sid in &stopped {
            self.running.remove(sid);
        }
        stopped
    }

    fn restore_running(&mut self, sessions: Vec<String>) {
        for sid in sessions {
            self.running.insert(sid);
        }
    }

    fn is_simulated(&self) -> bool {
        self.fixture_run.stage.is_some() || matches!(*self.source, Source::Fixture)
    }

    fn clear_playback(&mut self, session: &str) {
        self.fixture_run.playback.retain(|(_, _, d)| d["sessionID"].as_str() != Some(session));
    }

    fn send_write(&mut self, write: Write) -> bool {
        App::write(self, write)
    }

    fn status_text(&self) -> &str {
        &self.diag.status
    }

    fn format_wire_error(&self, err: &WireError) -> String {
        crate::errors::said(self, "", err)
    }

    fn touch_peer(&mut self) {
        crate::windows::peer::touch(self);
    }

    fn toast_rewound(&mut self, undo: UndoRewind) -> u64 {
        crate::status_toast::rewound(self, undo)
    }

    fn toast_dismiss_id(&mut self, id: u64) {
        crate::status_toast::dismiss_id(self, id);
    }

    fn dismiss_rewound_toast(&mut self, session: &str) {
        if let Some(e) = self.toast.current()
            && matches!(&e.action, Some(crate::status_toast::ToastAction::Custom(crate::status_toast::DesktopToastAction::UndoRewind(u))) if u.session == session)
        {
            crate::status_toast::dismiss(self);
        }
    }

    fn popup_open_at(&self, anchor: &str) -> bool {
        self.popup.open_at(anchor)
    }

    fn dismiss_popup(&mut self, tx: &mut Tx) {
        crate::popup::dismiss(self, tx);
    }

    fn open_message_popup(&mut self, menu: Menu, anchor: String, session: String, message_id: String, tx: &mut Tx) {
        crate::popup::open(self, menu, anchor, tx, move |s: &mut App, picked, tx| {
            on_message_menu_picked(s, &session, &message_id, picked, tx);
        });
    }

    fn queue_pending(&mut self, session: &str, message_id: &str) {
        App::queue_pending(self, session, message_id);
    }

    fn withdraw_pending(&mut self, session: &str, message_id: &str) {
        App::withdraw(self, session, message_id);
    }

    fn fork_from_message(&mut self, session: &str, message_id: &str, tx: &mut Tx) {
        let _ = App::fork_from(self, session, message_id, tx);
    }
}

impl App {
    /// Captures `session`'s current composer draft, attachments and quotes.
    pub fn capture_draft(&self, session: &str) -> SavedDraft {
        if self.composer_holds(session) {
            return SavedDraft {
                entry: Entry::of(&self.composer),
                caret: self.composer.range().1.min(self.composer.text.len()),
                files: self.composer_ext.attach.files.clone(),
                paths: self.composer_ext.attach.paths.clone(),
                quotes: self.kernel.slots.get(&crate::quote::QUOTE).quotes(),
            };
        }
        let r = self.kernel.slots.get(&crate::restore::RESTORE);
        let text = r.drafts.get(session).cloned().unwrap_or_default();
        let parked = r.parked.get(session).cloned().unwrap_or_default();
        let caret = parked.caret.filter(|&c| text.is_char_boundary(c)).unwrap_or(text.len());
        SavedDraft { entry: Entry { text, pills: parked.pills }, caret, files: parked.files, paths: parked.paths, quotes: parked.quotes }
    }

    /// Replaces `session`'s composer draft, attachments and quotes (now if open, or for when it opens).
    pub fn apply_draft(&mut self, session: &str, draft: SavedDraft) {
        if self.composer_holds(session) {
            return self.write_composer_draft(draft);
        }
        let parked = crate::restore::Parked::from_parts(
            draft.entry.pills,
            Some(draft.caret),
            draft.files,
            draft.paths,
            Vec::new(),
            draft.quotes,
        );
        self.park_draft(session.to_owned(), draft.entry.text, parked);
    }

    fn write_composer_draft(&mut self, draft: SavedDraft) {
        let focused = self.composer.focused || (!self.fixture_run.stage.is_some_and(|st| st.watch()) && self.has_composer());
        draft.entry.put(&mut self.composer, draft.caret.min(draft.entry.text.len()));
        self.composer.focused = focused;
        self.composer_ext.attach.files = draft.files;
        self.composer_ext.attach.paths = draft.paths;
        let _ = self.kernel.slots.get_mut(&crate::quote::QUOTE).take();
        for q in draft.quotes {
            self.kernel.slots.get_mut(&crate::quote::QUOTE).push(q);
        }
    }

    pub fn visible_user_ids(&self, session: &str) -> Vec<String> {
        visible_user_ids(self, session)
    }

    pub fn can_undo_session(&self) -> bool {
        can_undo_session(self)
    }

    pub fn can_redo_session(&self) -> bool {
        can_redo_session(self)
    }

    pub fn undo_last(&mut self, tx: &mut Tx) -> bool {
        undo_last(self, tx)
    }

    pub fn redo_revert(&mut self, tx: &mut Tx) -> bool {
        redo_revert(self, tx)
    }

    pub fn undo_rewind(&mut self, undo: UndoRewind, tx: &mut Tx) {
        undo_rewind(self, undo, tx);
    }

    pub(crate) fn revert_settled(&mut self, session: &str, attempt: u32, result: Result<(), WireError>) {
        revert_settled(self, session, attempt, result);
    }

    #[cfg(feature = "stories")]
    pub(crate) fn set_session_revert(&mut self, session: &str, revert: Option<String>) {
        set_session_revert(self, session, revert);
    }

    pub(crate) fn commit_staged_revert(&mut self, session: &str) -> bool {
        commit_staged_revert(self, session)
    }

    pub fn toggle_message_menu(&mut self, session: &str, message_id: &str, tx: &mut Tx) {
        toggle_message_menu(self, session, message_id, tx);
    }

    pub fn close_message_menu(&mut self) {
        close_message_menu(self);
    }
}
