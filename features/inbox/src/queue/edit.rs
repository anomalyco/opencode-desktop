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
use opencode_ui::prompt_history::Entry;

use super::QueueHost;
use super::machine::{Draft, Effect, Event, Queue, Row, merged_text};
use super::view::{row_files, row_text};

/// The queued prompt open in the composer of the open session.
pub fn editing(host: &dyn QueueHost) -> Option<&str> {
    let id = host.active_session_id()?;
    host.queue_ops().get(id).and_then(|q| q.editing())
}

/// The open session's queue rows, as the strip shows them.
pub fn rows(host: &dyn QueueHost, session: &str) -> Vec<Row> {
    let items: Vec<&model::InboxItem> = host.inboxes().get(session).map(|i| i.queued().collect()).unwrap_or_default();
    let label = |item: &model::InboxItem| (row_text(item), row_files(item));
    match host.queue_ops().get(session) {
        Some(q) => q.rows(&items, label),
        None => Queue::default().rows(&items, label),
    }
}

/// A row's words: the queued prompt goes into the composer (`edit.start`).
pub fn edit_queued(host: &mut dyn QueueHost, session: &str, id: &str) -> bool {
    let Some(item) = host.inboxes().get(session).and_then(|i| i.queued().find(|i| i.id == id)).cloned() else { return false };
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
    let event = Event::EditStart { id: id.to_owned(), reverted: host.has_staged_revert(session), draft: composer_draft(host), loaded };
    super::step_queue(host, session, event);
    host.queue_ops().get(session).and_then(|q| q.editing()) == Some(id)
}

/// Mod+Up: the first queued prompt goes into the composer.
fn edit_first_queued(host: &mut dyn QueueHost) -> bool {
    let Some(id) = host.active_session_id().map(str::to_owned) else { return false };
    let Some(first) = rows(host, &id).into_iter().next() else { return false };
    edit_queued(host, &id, &first.id)
}

/// Return (`queue`) or the other delivery (`steer`) while a queued prompt is open: it goes
/// back as the composer has it (`edit.confirm`).
pub fn confirm_edit(host: &mut dyn QueueHost, alternate: bool) {
    let Some(session) = host.active_session_id().map(str::to_owned) else { return };
    let Some(original) = host.queue_ops().get(&session).and_then(|q| q.editing()).map(str::to_owned) else { return };
    let wire = host.edit_wire();
    let refs = host.composer_paths().to_vec();
    let prompt = Prompt {
        text: opencode_kit::prompt::path_lines(&wire.text, &refs),
        files: wire.files.into_iter().chain(opencode_kit::prompt::attachment_inputs(host.composer_files())).collect(),
        agents: wire.agents,
        skills: wire.skills,
        display_text: Some(wire.display),
        attachments: refs,
        ..Default::default()
    };
    let payload = model::inbox_payload(&original).unwrap_or_else(|| {
        serde_json::json!({
            "text": host.inboxes().get(&session).and_then(|i| i.items.iter().find(|i| i.id == original)).map(|i| i.text.clone()).unwrap_or_default()
        })
    });
    let ids = (0..host.inboxes().get(&session).map_or(0, |i| i.queued().count())).map(|_| message_id()).collect();
    let event = Event::EditConfirm {
        delivery: if alternate { Delivery::Steer } else { Delivery::Queue },
        reverted: host.has_staged_revert(&session),
        prompt: composer_draft(host),
        admission: edited_admission(&session, &payload, &prompt),
        ids,
        now: host.clock(),
    };
    super::step_queue(host, &session, event);
}

/// The composer's draft is about to swap for another page's: an edit open in a session the
/// route left gives the composer its draft back first, so that draft is what's kept.
pub fn leave_edits(host: &mut dyn QueueHost) {
    let active = host.active_session_id().map(str::to_owned);
    let leaving: Vec<String> = host
        .queue_ops()
        .iter()
        .filter(|(s, q)| q.editing().is_some() && active.as_deref() != Some(s.as_str()))
        .map(|(s, _)| s.clone())
        .collect();
    for session in leaving {
        let current = rows(host, &session);
        let effects = match host.queue_ops_mut().get_mut(&session) {
            Some(queue) => queue.step(Event::Leave, &current),
            None => continue,
        };
        for effect in effects {
            if let Effect::Restore { stash, .. } = effect {
                put_draft(host, stash, false);
            }
        }
    }
}

/// Escape cancels an open edit; Mod+Up opens the first queued prompt.
pub fn key(host: &mut dyn QueueHost, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let plain = !ev.mods.shift && !ev.mods.alt;
    if ev.key == Key::Escape && plain && !ev.mods.cmd && !ev.mods.ctrl && editing(host).is_some() {
        let Some(id) = host.active_session_id().map(str::to_owned) else { return false };
        tx.instant();
        super::step_queue(host, &id, Event::EditCancel);
        return true;
    }
    if ev.key == Key::Up && plain && (ev.mods.cmd || ev.mods.ctrl) {
        tx.instant();
        return edit_first_queued(host);
    }
    false
}

/// What the composer holds (`EditStash`).
pub fn composer_draft(host: &dyn QueueHost) -> Draft {
    Draft {
        entry: Entry::of(host.composer()),
        caret: host.composer().range().1,
        files: host.composer_files().to_vec(),
        paths: host.composer_paths().to_vec(),
        shell: host.composer_shell(),
    }
}

/// The composer takes `draft`, focused with the caret at `caret`.
fn put_draft(host: &mut dyn QueueHost, draft: Draft, focus: bool) {
    if draft.shell != host.composer_shell() {
        host.sync_composer_shell(draft.shell);
    }
    let caret = draft.caret.min(draft.entry.text.len());
    draft.entry.put(host.composer_mut(), caret);
    host.set_composer_attachments(draft.files, draft.paths);
    if focus && !opencode_ui::style::touch_controls() {
        host.composer_mut().focused = true;
    }
}

/// Carries out the queue's composer effects for `session`; the others go back to the caller.
pub(super) fn apply_draft_effect(host: &mut dyn QueueHost, session: &str, effect: Effect) -> Option<Effect> {
    let shown = host.active_session_id() == Some(session);
    match effect {
        Effect::Load(draft) if shown => {
            put_draft(host, draft, true);
            host.composer_mut().focused = true;
        }
        Effect::Restore { stash, focus, keep } if shown => {
            let current = composer_draft(host);
            // What was typed into the edit of a prompt the server took meanwhile joins the
            // draft it gave back.
            let changed = keep.is_some_and(|k| current.entry.text.trim() != k.entry.text.trim() || current.files != k.files || current.paths != k.paths);
            let draft = if changed {
                let text = merged_text(&stash.entry.text, &current.entry.text);
                Draft {
                    caret: text.len(),
                    entry: Entry { text, pills: Vec::new() },
                    files: [stash.files, current.files].concat(),
                    paths: [stash.paths, current.paths].concat(),
                    shell: stash.shell,
                }
            } else {
                stash
            };
            put_draft(host, draft, focus);
        }
        Effect::Merge { text, files, paths } if shown => {
            let merged = merged_text(&host.composer().text, &text);
            let caret = merged.len();
            let composer = host.composer_mut();
            composer.replace_text(merged);
            composer.set_caret(caret);
            if !opencode_ui::style::touch_controls() {
                composer.focused = true;
            }
            host.extend_composer_attachments(files, paths);
        }
        // Another page shows: the words land in the session's saved draft.
        Effect::Merge { text, files, paths } => {
            host.merge_parked_draft(session, text, files, paths);
        }
        Effect::Load(_) | Effect::Restore { .. } => {}
        other => return Some(other),
    }
    None
}
