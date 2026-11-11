//! Editing a queued prompt in the composer, as Desktop's queue does (`queue.ts:70-121, 430-479`,
//! `composer/model.ts:540-560`, `editor/interaction.ts:535-540`, `editor/editor.tsx:358-362`).
//! A click on a row's words, or Mod+Up for the first row, puts the prompt in the composer and
//! keeps what the composer held; Return puts the edited prompt back in its place in the queue,
//! the other delivery (Mod+Return, the Steer hint) steers it, and Escape puts the composer back
//! as it was. If the server delivers or drops the prompt meanwhile, the composer gets its draft
//! back, with what was typed into the edit after it.

use opencode_kit::client::rewrite::{edited_admission, is_composer_file, queued_text};
use opencode_kit::client::{Prompt, message_id};
use opencode_kit::model::{self, Delivery};
use rune::{Key, KeyEvent, Tx};
use session_view::composer::history::Entry;

use super::machine::{Draft, Effect, Event, Row, merged_text};
use crate::{App, Route};

impl App {
    /// The queued prompt open in the composer of the open session.
    pub fn queue_editing(&self) -> Option<&str> {
        let Route::Session { id, .. } = &self.route else { return None };
        self.queue_ops.get(id).and_then(|q| q.editing())
    }

    /// The open session's queue rows, as the strip shows them.
    pub(crate) fn queue_rows(&self, session: &str) -> Vec<Row> {
        let items: Vec<&model::InboxItem> = self.inboxes.get(session).map(|i| i.queued().collect()).unwrap_or_default();
        let label = |item: &model::InboxItem| (super::row_text(item), super::row_files(item));
        match self.queue_ops.get(session) {
            Some(q) => q.rows(&items, label),
            None => super::machine::Queue::default().rows(&items, label),
        }
    }

    /// A row's words: the queued prompt goes into the composer (`edit.start`).
    pub fn edit_queued(&mut self, session: &str, id: &str) -> bool {
        let Some(item) = self.inboxes.get(session).and_then(|i| i.queued().find(|i| i.id == id)).cloned() else { return false };
        let loaded = match model::inbox_payload(id) {
            Some(payload) => {
                let text = queued_text(&payload);
                let files = payload["files"].as_array().into_iter().flatten().filter(|f| is_composer_file(f)).filter_map(model::parse_attachment).collect();
                let paths = opencode_kit::mention::path_attachments(&payload["metadata"]).unwrap_or_default();
                Draft { caret: text.len(), entry: Entry { text, pills: Vec::new() }, files, paths, shell: false }
            }
            None => {
                let files = model::user_files(id);
                let files = if files.is_empty() { item.files.clone() } else { files.to_vec() };
                let paths = opencode_kit::mention::user_path_attachments(id).to_vec();
                Draft { caret: item.text.len(), entry: Entry { text: item.text.clone(), pills: Vec::new() }, files, paths, shell: false }
            }
        };
        let event = Event::EditStart { id: id.to_owned(), reverted: self.staged_revert(session), draft: self.composer_draft(), loaded };
        self.step_queue(session, event);
        self.queue_ops.get(session).and_then(|q| q.editing()) == Some(id)
    }

    /// Mod+Up: the first queued prompt goes into the composer.
    fn edit_first_queued(&mut self) -> bool {
        let Route::Session { id, .. } = self.route.clone() else { return false };
        let Some(first) = self.queue_rows(&id).into_iter().next() else { return false };
        self.edit_queued(&id, &first.id)
    }

    /// Return (`queue`) or the other delivery (`steer`) while a queued prompt is open: it goes
    /// back as the composer has it (`edit.confirm`).
    pub fn confirm_queue_edit(&mut self, alternate: bool) {
        let Route::Session { id: session, .. } = self.route.clone() else { return };
        let Some(original) = self.queue_ops.get(&session).and_then(|q| q.editing()).map(str::to_owned) else { return };
        let wire = self.mention_wire();
        let refs = self.attach.paths.clone();
        let prompt = Prompt {
            text: session_view::composer::attachments::path_lines(&wire.text, &refs),
            files: wire.files.into_iter().chain(session_view::composer::attachments::inputs(&self.attach.files)).collect(),
            agents: wire.agents,
            skills: wire.skills,
            display_text: Some(wire.display),
            attachments: refs,
            ..Default::default()
        };
        let payload = model::inbox_payload(&original).unwrap_or_else(|| serde_json::json!({ "text": self.inboxes.get(&session).and_then(|i| i.items.iter().find(|i| i.id == original)).map(|i| i.text.clone()).unwrap_or_default() }));
        let ids = (0..self.inboxes.get(&session).map_or(0, |i| i.queued().count())).map(|_| message_id()).collect();
        let event = Event::EditConfirm {
            delivery: if alternate { Delivery::Steer } else { Delivery::Queue },
            reverted: self.staged_revert(&session),
            prompt: self.composer_draft(),
            admission: edited_admission(&session, &payload, &prompt),
            ids,
            now: self.clock,
        };
        self.step_queue(&session, event);
    }

    /// The composer's draft is about to swap for another page's: an edit open in a session the
    /// route left gives the composer its draft back first, so that draft is what's kept.
    pub(crate) fn leave_queue_edits(&mut self) {
        let leaving: Vec<String> = self.queue_ops.iter().filter(|(s, q)| q.editing().is_some() && !self.route.is_session(s)).map(|(s, _)| s.clone()).collect();
        for session in leaving {
            let rows = self.queue_rows(&session);
            let Some(queue) = self.queue_ops.get_mut(&session) else { continue };
            for effect in queue.step(Event::Leave, &rows) {
                if let Effect::Restore { stash, .. } = effect {
                    self.put_draft(stash, false);
                }
            }
        }
    }

    /// Escape cancels an open edit; Mod+Up opens the first queued prompt.
    pub(crate) fn queue_edit_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        let plain = !ev.mods.shift && !ev.mods.alt;
        if ev.key == Key::Escape && plain && !ev.mods.cmd && !ev.mods.ctrl && self.queue_editing().is_some() {
            let Route::Session { id, .. } = self.route.clone() else { return false };
            tx.instant();
            self.step_queue(&id, Event::EditCancel);
            return true;
        }
        if ev.key == Key::Up && plain && (ev.mods.cmd || ev.mods.ctrl) {
            tx.instant();
            return self.edit_first_queued();
        }
        false
    }

    fn staged_revert(&self, session: &str) -> bool {
        self.session(session).and_then(|i| i.revert.as_ref()).is_some()
    }

    /// What the composer holds (`EditStash`).
    pub(crate) fn composer_draft(&self) -> Draft {
        Draft {
            entry: Entry::of(&self.composer),
            caret: self.composer.range().1,
            files: self.attach.files.clone(),
            paths: self.attach.paths.clone(),
            shell: self.add.shell(),
        }
    }

    /// The composer takes `draft`, focused with the caret at `caret`.
    fn put_draft(&mut self, draft: Draft, focus: bool) {
        if draft.shell != self.add.shell() {
            use session_view::composer::mode::{Event, Mode};
            self.composer_mode(Event::Synced(if draft.shell { Mode::Shell } else { Mode::Normal }));
        }
        draft.entry.put(&mut self.composer, draft.caret.min(draft.entry.text.len()));
        self.attach.files = draft.files;
        self.attach.paths = draft.paths;
        if focus {
            self.composer.focused = true;
        }
    }

    /// Carries out the queue's composer effects for `session`; the others go back to the caller.
    pub(super) fn queue_draft_effect(&mut self, session: &str, effect: Effect) -> Option<Effect> {
        let shown = self.route.is_session(session);
        match effect {
            Effect::Load(draft) if shown => self.put_draft(draft, true),
            Effect::Restore { stash, focus, keep } if shown => {
                let current = self.composer_draft();
                // What was typed into the edit of a prompt the server took meanwhile joins the
                // draft it gave back.
                let changed = keep.is_some_and(|k| current.entry.text.trim() != k.entry.text.trim() || current.files != k.files || current.paths != k.paths);
                let draft = if changed {
                    let text = merged_text(&stash.entry.text, &current.entry.text);
                    Draft { caret: text.len(), entry: Entry { text, pills: Vec::new() }, files: [stash.files, current.files].concat(), paths: [stash.paths, current.paths].concat(), shell: stash.shell }
                } else {
                    stash
                };
                self.put_draft(draft, focus);
            }
            Effect::Merge { text, files, paths } if shown => {
                let merged = merged_text(&self.composer.text, &text);
                let caret = merged.len();
                self.composer.replace_text(merged);
                self.composer.set_caret(caret);
                self.composer.focused = true;
                self.attach.files.extend(files);
                self.attach.paths.extend(paths);
            }
            // Another page shows: the words land in the session's saved draft.
            Effect::Merge { text, files, paths } => {
                let key = crate::restore::draft_key(&Route::here(session.into()));
                if let Some(key) = key {
                    let draft = self.restore.drafts.entry(key.clone()).or_default();
                    *draft = merged_text(draft, &text);
                    let parked = self.restore.parked.entry(key).or_default();
                    parked.files.extend(files);
                    parked.paths.extend(paths);
                }
            }
            Effect::Load(_) | Effect::Restore { .. } => {}
            other => return Some(other),
        }
        None
    }
}
