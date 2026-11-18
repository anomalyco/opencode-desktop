//! Desktop host adapter for `opencode-feature-inbox::send_failed` (`features/inbox`).

pub use opencode_feature_inbox::send_failed::*;

pub(crate) mod stories;

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use opencode_kit::client::{PathAttachment, Prompt, Server, Write};
use opencode_kit::model::{Attachment, Inbox, Message, ModelRef, PromptComment, Transcript};
use opencode_kit::wire_error::WireError;
use rune::{TextEdit, Tx, Waker};
use session_view::composer::history::Entry;

use crate::{App, Route, Source};

/// Configures process-wide SQLite/IndexedDB outbox persistence callbacks once at startup.
pub fn init() {
    configure(StorageOps {
        with_storage: |f| crate::storage::with_storage(f),
        hand_off_doc: crate::storage::hand_off_doc,
    });
}

impl SendFailedHost for App {
    fn unsent(&self) -> &Log {
        &self.unsent
    }

    fn unsent_untracked(&self) -> &Log {
        rune::Tracked::untracked(&self.unsent)
    }

    fn unsent_mut(&mut self) -> &mut Log {
        &mut self.unsent
    }

    fn inboxes(&self) -> &HashMap<String, Inbox> {
        &self.inboxes
    }

    fn inboxes_mut(&mut self) -> &mut HashMap<String, Inbox> {
        &mut self.inboxes
    }

    fn transcripts(&self) -> &HashMap<String, Transcript> {
        &self.transcripts
    }

    fn transcripts_mut(&mut self) -> &mut HashMap<String, Transcript> {
        &mut self.transcripts
    }

    fn clock(&self) -> f64 {
        *self.clock
    }

    fn waker(&self) -> Option<Waker> {
        (*self.waker).clone()
    }

    fn window_id(&self) -> &str {
        &self.window.id
    }

    fn peer_token(&self) -> u64 {
        crate::windows::peer::token(self)
    }

    fn persists(&self) -> bool {
        App::persists(self)
    }

    fn with_storage(&self, op: StorageOp) {
        crate::storage::with_storage(op);
    }

    fn active_draft_document(&self) -> Option<String> {
        crate::restore::draft_key(&self.route).and_then(|k| self.draft_document_of(&k))
    }

    fn hand_off_draft_doc(&mut self, key: &str) -> u64 {
        crate::storage::hand_off_doc(key)
    }

    fn is_fixture(&self) -> bool {
        matches!(*self.source, Source::Fixture)
    }

    fn is_reconnect_down(&self) -> bool {
        crate::reconnect::down(self)
    }

    fn live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn live_allow_send(&self) -> bool {
        matches!(*self.source, Source::Live { allow_send: true, .. })
    }

    fn live_writer(&self) -> Option<Sender<Write>> {
        match &*self.source {
            Source::Live { writes: Some(writer), .. } => Some(writer.clone()),
            _ => None,
        }
    }

    fn write(&mut self, write: Write) -> bool {
        App::write(self, write)
    }

    fn send_write(&self, session: &str, prompt: Prompt) -> Write {
        App::send_write(self, session, prompt)
    }

    fn is_session_here(&self, session: &str) -> bool {
        *self.route == Route::here(session.to_owned())
    }

    fn composer(&self) -> &TextEdit {
        &self.composer
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn composer_files(&self) -> &[Attachment] {
        &self.attach.files
    }

    fn composer_paths(&self) -> &[PathAttachment] {
        &self.attach.paths
    }

    fn composer_has_attachments(&self) -> bool {
        self.attach.any()
    }

    fn composer_has_context_or_uploads(&self) -> bool {
        !self.attach.context.is_empty() || !self.attach.uploads.is_empty()
    }

    fn composer_quotes(&self) -> Vec<PromptComment> {
        self.quote.quotes()
    }

    fn composer_has_quotes(&self) -> bool {
        self.quote.any()
    }

    fn clear_composer_attachments_and_quotes(&mut self) {
        self.attach.files.clear();
        self.attach.paths.clear();
        let _ = self.quote.take();
    }

    fn add_composer_file(&mut self, file: Attachment) {
        self.attach.add(file);
    }

    fn add_composer_path(&mut self, path: PathAttachment) {
        self.attach.add_path(path);
    }

    fn push_composer_quote(&mut self, quote: PromptComment) {
        self.quote.push(quote);
    }

    fn retain_echoes_without(&mut self, id: &str) {
        self.attach.echoes.retain(|e| e.1 != id);
    }

    fn restore_inactive_draft(&mut self, session: &str, entry: Entry, files: Vec<Attachment>, paths: Vec<PathAttachment>, quotes: Vec<PromptComment>) {
        if self.capture_draft(session).is_empty() {
            let caret = entry.text.len();
            self.apply_draft(session, crate::rewind::SavedDraft { entry, caret, files, paths, quotes });
        }
    }

    fn picked_choice(&self) -> (Option<String>, Option<ModelRef>) {
        crate::pickers::PickerHost::picked_choice(self)
    }

    fn on_send_result(&mut self, id: &str, result: SendResult) {
        crate::quote::send_result(self, id, crate::quote::map_send_result(result));
    }

    fn toast_say(&mut self, title: &str, description: &str) {
        crate::status_toast::say(self, title, description);
    }

    fn toast_fail(&mut self, title: &str, description: &str) {
        crate::status_toast::fail(self, title, description);
    }
}

impl App {
    pub(crate) fn step_send(&mut self, id: &str, event: machine::Event) -> bool {
        step_send(self, id, event)
    }

    pub(crate) fn send_result(&self, id: &str) -> Option<SendResult> {
        send_result(self, id)
    }

    pub(crate) fn newest_send(&self, before: Option<&str>) -> Option<String> {
        newest_send(self, before)
    }

    pub fn send_failed(&mut self, id: &str, error: &WireError) -> bool {
        send_failed(self, id, error)
    }

    pub fn check_sends(&mut self) {
        check_sends(self);
    }

    pub(crate) fn poll_sends(&mut self) -> bool {
        poll_sends(self)
    }

    pub fn judge_sends_by_page(&mut self, session: &str, messages: &[Message]) -> bool {
        judge_sends_by_page(self, session, messages)
    }

    pub fn judge_sends_by_inbox(&mut self, session: &str, ids: &[String]) -> bool {
        judge_sends_by_inbox(self, session, ids)
    }

    pub(crate) fn admitted_send(&mut self, id: &str) -> bool {
        admitted_send(self, id)
    }

    pub(crate) fn open_outbox(&mut self) {
        open_outbox(self);
    }

    pub(crate) fn listen_outbox(&self) {
        listen_outbox(self);
    }

    pub(crate) fn show_outbox(&mut self) {
        show_outbox(self);
    }

    pub(crate) fn enqueue_prepared(&mut self, sent: Sent, write: Write) -> bool {
        enqueue_prepared(self, sent, write)
    }

    pub(crate) fn release_prepared(&mut self, id: &str, directory: Option<String>) {
        release_prepared(self, id, directory);
    }

    pub(crate) fn enqueue_prompt(&mut self, sent: Sent, write: Write) -> bool {
        enqueue_prompt(self, sent, write)
    }

    pub(crate) fn tick_outbox(&mut self, tx: &mut Tx) -> bool {
        tick_outbox(self, tx)
    }

    pub(crate) fn pump_outbox(&mut self) -> bool {
        pump_outbox(self)
    }

    pub(crate) fn restore_draft(&mut self, sent: Sent) {
        restore_draft(self, sent);
    }

    pub(crate) fn retry_restored(&mut self) -> bool {
        retry_restored(self)
    }

    pub(crate) fn outbox_action(&mut self, id: &str, action: &str) {
        outbox_action(self, id, action);
    }
}
