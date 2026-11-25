//! Stop before the model has answered takes the prompt back into the composer, as Desktop's
//! does (opencode#65505: `packages/app/src/session/composer/adapter.ts:57-70`,
//! `session/revert.ts:289-309`). Each session's is a [`TakeBack`] machine
//! (`opencode_runtime::machines::take_back`); this carries out what it asks.
//!
//! Desktop waits for the run to settle and then reads the session's messages. Here the run's own
//! end on the event stream is that moment: every event before it has been applied, so what the
//! prompt was answered with is all there. The commit still waits on the server first.

use opencode_kit::client::{Ticket, TicketKind, Write};
use opencode_kit::model::{Message, NoticeKind, Part};
use opencode_runtime::machines::take_back::{Effect, Event, Prompt};
use rune::machine::Attempt;

use super::{RewindHost, SavedDraft, draft_from_message};

pub type TakeBack = opencode_runtime::machines::take_back::TakeBack<SavedDraft>;

/// The latest prompt (its id and text), unless the model has already answered it with text or a
/// tool call, or something else has run since: a shell command, a compaction and a synthetic
/// prompt each start a run of their own (`session-domain.ts:7-27` `selectUnansweredPrompt`).
pub fn unanswered(messages: &[Message]) -> Option<(&str, &str)> {
    let at = messages.iter().rposition(|m| matches!(m, Message::User { .. }))?;
    let Message::User { id, text } = &messages[at] else { return None };
    let answered = messages[at + 1..].iter().any(|m| match m {
        Message::Note { .. } | Message::Compaction(_) => true,
        // The synthetic messages the transcript keeps: a restart, and a background shell's,
        // subagent's or script's report.
        Message::Notice(n) => matches!(n.kind, NoticeKind::Restart | NoticeKind::Finished(_) | NoticeKind::Job { .. }),
        Message::Assistant(a) => a.parts.iter().any(|p| match p {
            Part::Tool { .. } => true,
            Part::Text { text } => !text.trim().is_empty(),
            Part::Reasoning { .. } => false,
        }),
        Message::User { .. } => false,
    });
    (!answered).then_some((id.as_str(), text.as_str()))
}

/// Stop was asked for in `session`: it takes the prompt back only from an empty composer with
/// nothing queued or pending, since a draft or a waiting prompt means the user has moved on
/// (`adapter.ts:61-64`).
pub fn stopped(host: &mut dyn RewindHost, session: &str) {
    let waiting = host.inbox(session).is_some_and(|i| !i.items.is_empty() || i.compaction.is_some());
    let clear = !waiting && host.capture_draft(session).is_empty();
    step(host, session, Event::Stopped { clear });
    // The fixture and the storybook answer as a server whose run the stop interrupted.
    if host.is_simulated() {
        step(host, session, Event::Acknowledged { interrupted: true });
    }
}

/// The server answered `session`'s interrupt.
pub fn acknowledged(host: &mut dyn RewindHost, session: &str, interrupted: bool) {
    if host.rewind().taking.contains_key(session) {
        step(host, session, Event::Acknowledged { interrupted });
    }
}

/// `session`'s run ended.
pub fn run_ended(host: &mut dyn RewindHost, session: &str) {
    if !host.rewind().taking.contains_key(session) {
        return;
    }
    let found = host.transcript(session).and_then(|t| unanswered(&t.messages)).map(|(id, text)| (id.to_owned(), text.to_owned()));
    let unanswered = found.map(|(message, text)| {
        let draft = draft_from_message(host, &message, &text);
        Prompt { message, draft }
    });
    step(host, session, Event::RunEnded { unanswered });
}

/// `Incoming::Settled` for a take-back's ticket.
pub fn settled(host: &mut dyn RewindHost, session: &str, attempt: u32, ok: bool) {
    step(host, session, Event::Settled { attempt: Attempt(attempt), ok });
}

/// A prompt went to `session`, a run started in it, or its stop failed: nothing is taken back.
pub fn forget(host: &mut dyn RewindHost, session: &str) {
    if host.rewind().taking.contains_key(session) {
        step(host, session, Event::Forgotten);
    }
}

fn step(host: &mut dyn RewindHost, session: &str, event: Event<SavedDraft>) {
    let effects = host.rewind_mut().taking.entry(session.to_owned()).or_default().step(event);
    for effect in effects {
        match effect {
            Effect::Commit { attempt, message } => commit(host, session, attempt, message),
            // `revert.ts` `restore`: the prompt replaces the draft.
            Effect::Restore { prompt } => {
                host.exit_shell();
                host.apply_draft(session, prompt.draft);
            }
        }
    }
}

fn commit(host: &mut dyn RewindHost, session: &str, attempt: Attempt, message: String) {
    // The fixture and the storybook act as if the server agreed at once: the prompt and what
    // followed it leave, as a `session.revert.committed` event makes them.
    if host.is_simulated() {
        if let Some(t) = host.transcript_mut(session)
            && let Some(at) = t.messages.iter().position(|m| m.id() == message)
        {
            t.messages.truncate(at);
        }
        return step(host, session, Event::Settled { attempt, ok: true });
    }
    let ticket = Ticket { kind: TicketKind::TakeBack, session: session.to_owned(), attempt: attempt.0 };
    let write = Write::TakeBack { session: session.to_owned(), message };
    if !host.send_write(Write::Tracked { ticket, write: Box::new(write) }) {
        step(host, session, Event::Settled { attempt, ok: false });
    }
}
