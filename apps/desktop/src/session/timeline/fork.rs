//! Desktop host adapter for `opencode-feature-history::fork` (`features/history`).

pub use opencode_feature_history::fork::*;

use opencode_feature_history::rewind::SavedDraft;
use opencode_kit::client::Write;
use opencode_kit::model::{SessionInfo, Transcript};
use opencode_kit::wire_error::WireError;
use rune::{KeyEvent, Tx};

use crate::pickers::PickerHost as _;
use crate::{App, Route, Source};

impl ForkHost for App {
    fn fork(&self) -> &ForkState {
        &self.dialogs.fork
    }

    fn fork_mut(&mut self) -> &mut ForkState {
        &mut self.dialogs.fork
    }

    fn is_session_open(&self, session: &str) -> bool {
        *self.route == Route::here(session.to_owned())
    }

    fn session(&self, session: &str) -> Option<&SessionInfo> {
        App::session(self, session)
    }

    fn upsert_session_front(&mut self, session: SessionInfo) {
        match self.sessions.iter_mut().find(|x| x.id == session.id) {
            Some(slot) => *slot = session,
            None => self.sessions.insert(0, session),
        }
    }

    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.transcripts.get(session)
    }

    fn insert_transcript(&mut self, session: String, transcript: Transcript) {
        self.transcripts.insert(session, transcript);
    }

    fn open_session(&mut self, session: &str, tx: &mut Tx) {
        self.open(session, tx);
    }

    fn apply_draft(&mut self, session: &str, draft: SavedDraft) {
        App::apply_draft(self, session, draft);
    }

    fn mention_title(&self, session_id: &str) -> Option<String> {
        App::mention_title(self, session_id)
    }

    fn resolve_skill_id(&self, name_or_id: &str) -> Option<&str> {
        self.composer_ext.mentions.skills.iter().find(|sk| sk.id == name_or_id || sk.name == name_or_id).map(|sk| sk.id.as_str())
    }

    fn is_simulated(&self) -> bool {
        self.fixture_run.stage.is_some() || matches!(*self.source, Source::Fixture)
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

    fn before_fork_open(&mut self) -> bool {
        let restore = self.composer.focused || self.dialogs.switcher.restore_composer();
        self.close_jump_dialog(false);
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.close_message_menu();
        self.composer.focused = false;
        restore
    }

    fn restore_composer_focus(&mut self, focused: bool) {
        self.composer.focused = focused;
    }

    fn handle_shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }
}

impl App {
    pub fn can_fork_session(&self) -> bool {
        can_fork_session(self)
    }

    pub fn fork_from(&mut self, session: &str, message_id: &str, tx: &mut Tx) -> bool {
        fork_from(self, session, message_id, tx)
    }

    pub fn open_fork_dialog(&mut self, tx: &mut Tx) -> bool {
        open(self, tx)
    }

    pub fn close_fork_dialog(&mut self, restore_focus: bool) {
        close(self, restore_focus);
    }

    #[allow(dead_code)]
    pub fn fork_messages(&self) -> Vec<ForkableMessage> {
        messages(self)
    }

    #[allow(dead_code)]
    pub fn fork_filtered(&self) -> Vec<ForkableMessage> {
        filtered(self)
    }

    pub fn fork_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        key(self, ev, tx)
    }
}
