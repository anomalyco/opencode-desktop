//! Desktop host adapter for `opencode-feature-inbox::queue` (`features/inbox`).

pub use opencode_feature_inbox::queue::*;

mod story;

use std::collections::HashMap;

use opencode_feature_inbox::send_failed::SendFailedHost;
use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::client::{PathAttachment, Prompt, Write};
use opencode_kit::model::{Attachment, Delivery, Inbox, InboxItem, PromptComment, Transcript};
use opencode_kit::wire_error::WireError;
use rune::{KeyEvent, TextEdit, Tx};

use crate::{App, Route, Source};

impl QueueHost for App {
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

    fn queue_ops(&self) -> &HashMap<String, machine::Queue> {
        &self.queue_ops
    }

    fn queue_ops_mut(&mut self) -> &mut HashMap<String, machine::Queue> {
        &mut self.queue_ops
    }

    fn queue_drag(&self) -> &drag::Drag {
        &self.queue_drag
    }

    fn queue_drag_mut(&mut self) -> &mut drag::Drag {
        &mut self.queue_drag
    }

    fn queue_runs(&self) -> &run::Runs {
        &self.queue_runs
    }

    fn queue_runs_mut(&mut self) -> &mut run::Runs {
        &mut self.queue_runs
    }

    fn clock(&self) -> f64 {
        *self.clock
    }

    fn active_session_id(&self) -> Option<&str> {
        App::active_session_id(self)
    }

    fn is_fixture(&self) -> bool {
        matches!(*self.source, Source::Fixture)
    }

    fn can_write(&self) -> bool {
        App::can_write(self)
    }

    fn write(&mut self, write: Write) -> bool {
        App::write(self, write)
    }

    fn send_write(&self, session: &str, prompt: Prompt) -> Write {
        App::send_write(self, session, prompt)
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

    fn take_composer_files(&mut self) -> Vec<Attachment> {
        std::mem::take(&mut self.attach.files)
    }

    fn composer_paths(&self) -> &[PathAttachment] {
        &self.attach.paths
    }

    fn extend_composer_paths(&mut self, paths: Vec<PathAttachment>) {
        self.attach.paths.extend(paths);
    }

    fn extend_composer_attachments(&mut self, files: Vec<Attachment>, paths: Vec<PathAttachment>) {
        self.attach.files.extend(files);
        self.attach.paths.extend(paths);
    }

    fn set_composer_attachments(&mut self, files: Vec<Attachment>, paths: Vec<PathAttachment>) {
        self.attach.files = files;
        self.attach.paths = paths;
    }

    fn composer_shell(&self) -> bool {
        self.add.shell()
    }

    fn sync_composer_shell(&mut self, shell: bool) {
        if shell != self.add.shell() {
            use session_view::composer::mode::{Event, Mode};
            self.composer_mode(Event::Synced(if shell { Mode::Shell } else { Mode::Normal }));
        }
    }

    fn take_composer_comments(&mut self) -> Vec<PromptComment> {
        self.quote.take()
    }

    fn clear_line_comments(&mut self, session: &str) {
        App::clear_line_comments(self, session);
    }

    fn merge_parked_draft(&mut self, session: &str, text: String, files: Vec<Attachment>, paths: Vec<PathAttachment>) {
        if let Some(key) = crate::restore::draft_key(&Route::here(session.into())) {
            let draft = self.restore.drafts.entry(key.clone()).or_default();
            *draft = machine::merged_text(draft, &text);
            let parked = self.restore.parked.entry(key).or_default();
            parked.files.extend(files);
            parked.paths.extend(paths);
        }
    }

    fn edit_wire(&self) -> EditWire {
        let wire = self.mention_wire();
        EditWire {
            text: wire.text,
            display: wire.display,
            agents: wire.agents,
            skills: wire.skills,
            files: wire.files,
        }
    }

    fn has_staged_revert(&self, session: &str) -> bool {
        self.session(session).and_then(|i| i.revert.as_ref()).is_some()
    }

    fn dock_shown(&self) -> bool {
        App::dock_shown(self)
    }

    fn on_steered(&mut self, session: &str, id: &str) {
        self.steered(session, id);
    }

    fn on_prompt_accepted(&mut self, session: &str, id: &str) -> bool {
        self.prompt_accepted(session, id)
    }

    fn on_inbox_changed(&mut self, session: &str) {
        crate::rewind::inbox_changed(self, session);
    }

    fn on_peer_touch(&mut self) {
        crate::windows::peer::touch(self);
    }

    fn offer_undo_removed(&mut self, session: &str, text: String, files: Vec<Attachment>) {
        crate::undo::offer(
            self,
            opencode_i18n::t!("rune.undo.queueRemoved"),
            crate::undo::Undoable::Unqueued { session: session.to_owned(), text, files },
        );
    }

    fn toast_failed_wire(&mut self, title: &str, error: &WireError) {
        crate::status_toast::failed_wire(self, title, error);
    }

    fn toast_fail(&mut self, title: &str, description: &str) {
        crate::status_toast::fail(self, title, description);
    }

    fn toast_say(&mut self, title: &str, description: &str) {
        crate::status_toast::say(self, title, description);
    }

    fn spawn_queue_job(&mut self, job: QueueJob) {
        match job {
            QueueJob::DeferFailed { session, attempt, now } => {
                self.later(Box::new(move |app: &mut App, _| {
                    apply_reply(app, QueueReply::DeferredFailed { session, attempt, now });
                }));
            }
            QueueJob::Rewrite(task) => {
                let key = crate::servers::runtime::active(self).clone();
                let deliver = self.deliverer();
                self.job(move |srv| {
                    let progress_key = key.clone();
                    let reply = execute_rewrite(srv, task, move |update| {
                        let k = progress_key.clone();
                        deliver(Box::new(move |app: &mut App, _| {
                            crate::servers::runtime::with_server(app, &k, |app| apply_reply(app, update));
                        }));
                    });
                    Box::new(move |app: &mut App, _| {
                        crate::servers::runtime::with_server(app, &key, |app| apply_reply(app, reply));
                    })
                });
            }
        }
    }

    fn submit_alternate(&mut self, tx: &mut Tx) {
        self.submit(true, tx);
    }

    fn send_failed(&self) -> Option<&dyn SendFailedHost> {
        Some(self)
    }

    fn send_failed_mut(&mut self) -> Option<&mut dyn SendFailedHost> {
        Some(self)
    }
}

impl App {
    pub fn inbox_for(&self, session: &str) -> Option<&Inbox> {
        inbox_for(self, session)
    }

    pub fn admit(&mut self, session: &str, prompt: Prompt, delivery: Delivery) {
        admit(self, session, prompt, delivery);
    }

    pub fn steer_queued(&mut self, session: &str, id: &str) {
        steer_queued(self, session, id);
    }

    pub fn queue_pending(&mut self, session: &str, id: &str) {
        queue_pending(self, session, id);
    }

    pub fn withdraw(&mut self, session: &str, id: &str) {
        withdraw(self, session, id);
    }

    pub(crate) fn unwithdraw(&mut self, session: &str, text: String, files: Vec<Attachment>) {
        unwithdraw(self, session, text, files);
    }

    pub fn undo_queued(&mut self, session: &str, id: &str) {
        undo_queued(self, session, id);
    }

    pub fn reorder_queued(&mut self, session: &str, ids: Vec<String>) {
        reorder_queued(self, session, ids);
    }

    pub(crate) fn queue_settled(&mut self, session: &str, attempt: u32, result: Result<(), Option<WireError>>) {
        queue_settled(self, session, attempt, result);
    }

    pub(crate) fn queue_timers(&mut self, tx: &mut Tx) {
        poll_timers(self, tx);
    }

    pub fn apply_inbox(&mut self, ty: &str, data: &serde_json::Value, sid: &str) -> bool {
        apply_inbox(self, ty, data, sid)
    }

    pub fn inbox_landed(&mut self, ty: &str, data: &serde_json::Value, sid: &str) -> bool {
        inbox_landed(self, ty, data, sid)
    }

    pub fn inbox_listed(&mut self, sid: &str, rows: &[serde_json::Value]) {
        inbox_listed(self, sid, rows);
    }

    pub fn settle_inbox(&mut self, session: &str, fetched: &[String]) {
        settle_inbox(self, session, fetched);
    }

    pub fn queue_editing(&self) -> Option<&str> {
        editing(self)
    }

    pub fn edit_queued(&mut self, session: &str, id: &str) -> bool {
        edit_queued(self, session, id)
    }

    pub fn confirm_queue_edit(&mut self, alternate: bool) {
        confirm_edit(self, alternate);
    }

    pub(crate) fn leave_queue_edits(&mut self) {
        leave_edits(self);
    }

    pub(crate) fn queue_edit_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        key(self, ev, tx)
    }

    #[allow(dead_code)]
    pub(crate) fn composer_draft(&self) -> machine::Draft {
        composer_draft(self)
    }
}

/// Kernel feature registration for the session prompt queue and durable prompt outbox.
pub struct InboxFeature;

impl Feature<App> for InboxFeature {
    fn id(&self) -> &'static str {
        "feature.inbox"
    }

    fn register(&self, _builder: &mut KernelBuilder<App>) {
        // `poll_sends`, `tick_outbox`, and `queue_timers` stay ordered around `incoming::poll` in
        // `tick.rs`, and `listen_outbox` runs immediately after `storage::on_ready` in `tick::setup`.
    }
}

/// The `queue*` scenes: the open session busy with `n` prompts queued, over the question dock
/// when `ask`.
pub fn scene(s: &mut App, n: usize, ask: bool) {
    crate::scenes::busy_scene(s);
    let Route::Session { id, .. } = (*s.route).clone() else { return };
    let words = ["Then write up what changed in the PR description", "And keep the retry budget as it is", "Bump the version and tag it"];
    for k in 0..n {
        let item = InboxItem { id: format!("msg_zzqueued{k}"), text: words[k % words.len()].into(), delivery: Delivery::Queue, files: Vec::new() };
        s.inboxes.entry(id.clone()).or_default().upsert(item);
    }
    if ask {
        let q = s.fixture["question"].clone();
        s.apply_event("form.created", &q);
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // Busy with prompts queued above the composer or above the question dock;
    // `queue-question-morph` answers the question at 1.5 s and asks again at 2.5 s.
    ("queue", |s| scene(s, 1, false)),
    ("queue-3", |s| scene(s, 3, false)),
    // SCALE's queue sizes (harness/scale `queued_prompts`): a long queue over the composer.
    ("queue-5", |s| scene(s, 5, false)),
    ("queue-25", |s| scene(s, 25, false)),
    ("queue-50", |s| scene(s, 50, false)),
    ("queue-question", |s| scene(s, 1, true)),
    ("queue-question-3", |s| scene(s, 3, true)),
    ("queue-question-morph", |s| {
        scene(s, 1, true);
        let q = s.fixture["question"].clone();
        let replied = serde_json::json!({ "id": q["id"], "sessionID": q["sessionID"] });
        *s.playback = [(1.5, "form.replied".into(), replied), (2.5, "form.created".into(), q)].into();
    }),
]);

/// Timeline › Sends behind waiting steers: two steers wait, the run stops, and two more sends
/// join them under Working, in the order the server delivers them all.
mod behind_story {
    use rune::Play;
    use serde_json::json;

    use crate::App;
    use crate::stories::{Trigger, emit, replay, sid};

    const WORDS: [&str; 4] = ["or did he give me stuff?", "can you check", "this is really weird", "what"];

    const KEYS: &[Trigger] = &[
        replay(open),
        Trigger { key: 's', label: "Send", run: send },
        Trigger { key: 'x', label: "Stop the run", run: stop },
        Trigger { key: 'd', label: "Deliver the oldest", run: deliver },
    ];

    fn open(s: &mut App, tx: &mut Play<App>) {
        s.finish_stream();
        let id = sid(s);
        s.running.insert(id);
        tx.after(0.4, send);
        tx.after(1.0, send);
        tx.after(1.8, stop);
        tx.after(2.4, send);
        tx.after(3.0, send);
        for k in 0..4 {
            tx.after(4.2 + 0.1 * k as f64, deliver);
        }
    }

    fn send(s: &mut App, tx: &mut Play<App>) {
        let sent = s.inbox_for(&sid(s)).map_or(0, |i| i.items.len());
        *s.composer = rune::TextEdit::new(WORDS[sent % WORDS.len()]);
        s.send(tx);
    }

    fn stop(s: &mut App, tx: &mut Play<App>) {
        emit(s, "session.execution.interrupted", json!({ "reason": "user" }), tx);
    }

    fn deliver(s: &mut App, tx: &mut Play<App>) {
        let Some(id) = s.inbox_for(&sid(s)).and_then(|i| i.steering().next()).map(|i| i.id.clone()) else { return };
        emit(s, "session.inbox.delivered", json!({ "inboxID": id }), tx);
    }

    rune_register::register!(crate::stories::STORY_LIST: crate::stories::Story = [
        crate::stories::Story::session("Timeline/Sends behind waiting steers", KEYS),
    ]);
}
