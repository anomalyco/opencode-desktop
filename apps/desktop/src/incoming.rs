//! `poll`'s server messages: the replies to this window's own reads and writes (its inbox), and
//! its server's stream events, which the server's copy took once and each window reacts to
//! (`windows::peer`, `events`).

use opencode_kit::client::{Incoming, Ticket, TicketKind};
use opencode_kit::model::{SessionInfo, Transcript};
use rune::Tx;

use crate::{App, Route, arrivals, attachments, fork, permission, reconnect, status_toast, stop, windows};

/// Takes what came in since the last poll. True when the view changed; `arrivals` is set when
/// prompts may have arrived (its snapshot from before any of it).
pub(crate) fn poll(s: &mut App, tx: &mut Tx, arrivals: &mut Option<arrivals::Arrivals>) -> bool {
    let mut changed = false;
    let replies: Vec<Incoming> = s.inbox.as_ref().map(|inbox| inbox.try_iter().collect()).unwrap_or_default();
    if !replies.is_empty() {
        *arrivals = Some(arrivals::Arrivals::before(s));
        // A reply only this window heard changes the data its server's windows share.
        windows::peer::touch(s);
    }
    for m in replies {
        changed |= reply(s, m, tx, arrivals);
    }
    let heard = windows::peer::heard(s);
    if !heard.is_empty() {
        arrivals.get_or_insert_with(|| arrivals::Arrivals::before(s));
    }
    for h in heard {
        if !matches!(&*h, crate::events::Heard::Message(Incoming::Status(_) | Incoming::Failed(_) | Incoming::StreamDown(_) | Incoming::Unanswered(_))) {
            changed |= reconnect::heard(s);
        }
        changed |= match &*h {
            crate::events::Heard::Event(e) => {
                if crate::motion_log::wanted() { s.motion_events.note(&e.ty); }
                // A session deleted while its tab is open: the tab says it's gone (`session_state`).
                let gone = e.ty == "session.deleted" && e.data["sessionID"].as_str().is_some_and(|id| s.route == Route::here(id.to_owned()));
                if gone && let Some(id) = e.data["sessionID"].as_str() { crate::session_state::missing(s, id, tx); }
                s.react(e) | gone
            }
            crate::events::Heard::Message(m) => message(s, m.clone(), tx),
        };
    }
    changed
}

/// The stream's own news: it came back, or a line about it.
fn message(s: &mut App, m: Incoming, tx: &mut Tx) -> bool {
    match m {
        Incoming::Reconnected => {
            tx.instant();
            s.windows_forgotten();
            crate::session_state::reconnected(s, tx);
            s.sidebar_inbox.branches.reconnected();
            reconnect::back(s);
            // Terminals that failed under the old connection attach again (`server.connected`).
            s.terminal.connections += 1;
            s.pump_outbox();
            s.stream_back();
            s.check_sends();
            s.resync_view();
        }
        // The first open: the link was never down, but the health poll can stand down now.
        Incoming::Opened => reconnect::back(s),
        Incoming::Status(e) => status_toast::from_server(s, e),
        Incoming::Failed(e) => failed(s, e, tx),
        Incoming::StreamDown(e) => reconnect::stream_down(s, e, tx),
        Incoming::Unanswered(e) => reconnect::unanswered(s, e, tx),
        _ => {}
    }
    true
}

/// A read or write that failed: the link hears it, a reply's dock takes its own, and the rest
/// is the toast for a failed write (a read stays in the debug bar: the app retries those).
fn failed(s: &mut App, e: opencode_kit::wire_error::WireError, tx: &mut Tx) {
    reconnect::call_failed(s, &e, tx);
    if permission::failed(s, &e) {
        return;
    }
    status_toast::from_wire(s, e);
}

/// A tracked write's outcome, for the machine that asked under the attempt it asked with.
fn settled(s: &mut App, ticket: Ticket, result: Result<Option<SessionInfo>, opencode_kit::wire_error::WireError>, tx: &mut Tx) {
    let (session, attempt) = (ticket.session.as_str(), ticket.attempt);
    match ticket.kind {
        TicketKind::Stop => {
            if let Err(e) = result {
                stop::failed(s, session, attempt, &e);
            }
        }
        TicketKind::Revert => s.revert_settled(session, attempt, result.map(|_| ())),
        TicketKind::Fork => fork::settled(s, attempt, result, tx),
        TicketKind::Inbox => s.queue_settled(session, attempt, result.map(|_| ()).map_err(Some)),
    }
}

/// One reply to this window's own reads and writes.
fn reply(s: &mut App, m: Incoming, tx: &mut Tx, arrivals: &mut Option<arrivals::Arrivals>) -> bool {
    if !matches!(m, Incoming::Status(_) | Incoming::Failed(_) | Incoming::StreamDown(_) | Incoming::Unanswered(_) | Incoming::SendFailed { .. }) {
        reconnect::heard(s);
    }
    match m {
        Incoming::Sessions(mut list) => {
            s.switcher.invalidate_sessions();
            for n in &mut list {
                if let Some(old) = s.session(&n.id) {
                    n.viewed = n.viewed.max(old.viewed);
                }
            }
            // Sessions past the page (a tab's, a subagent's parent) stay known.
            let rest: Vec<SessionInfo> = s.sessions.drain(..).filter(|o| !list.iter().any(|n| n.id == o.id)).collect();
            list.extend(rest);
            *s.sessions = list;
            s.sidebar_inbox.heard_sessions();
            s.sort_projects();
            crate::rewind::read_sessions(s);
            s.note_tab_titles();
        }
        Incoming::Info(mut info) => {
            s.switcher.invalidate_sessions();
            let parent = info.parent.clone();
            match s.sessions.iter_mut().find(|x| x.id == info.id) {
                Some(old) => {
                    info.viewed = info.viewed.max(old.viewed);
                    *old = info;
                }
                None => s.sessions.push(info),
            }
            if let Some(missing) = parent.and_then(|p| s.missing_ancestor(&p)) {
                s.fetch_info(&missing);
            }
            crate::rewind::read_sessions(s);
            s.note_tab_titles();
        }
        Incoming::Children { sessions, .. } => s.children_listed(sessions),
        Incoming::Messages { session, messages } => {
            s.judge_sends_by_page(&session, &messages);
            s.check_sends();
            let cold = s.launch.cold_transcript.as_deref() == Some(session.as_str());
            if cold {
                s.launch.cold_transcript = None;
            }
            let first = !s.transcripts.contains_key(&session);
            let t = s.transcripts.entry(session.clone()).or_insert_with(|| Transcript { session: session.clone(), messages: vec![] });
            attachments::reconcile(&mut s.attach.echoes, t, &messages);
            let delivered: Vec<String> = messages.iter().map(|m| m.id().to_owned()).collect();
            match s.stream_gaps.get_mut(&session) {
                Some(gapped) => t.merge_caught(messages, gapped),
                None => t.merge(messages),
            }
            reconnect::page_in(s, &session);
            s.settle_inbox(&session, &delivered);
            if (first || cold) && s.route == Route::here(session.clone()) {
                // Guarantee 8: the first load of the open session lands, nothing grows in. One
                // read up from its end (a place kept from the last run) stays where it is: landing
                // the end's follow would carry it there.
                session_view::jump::settle(s, tx);
            }
            if cold {
                *arrivals = None;
            }
        }
        // A write's answer that comes as an event (an interrupt's acknowledgement).
        Incoming::Event { ty, data } => {
            if ty == "client.prompt.accepted" {
                let admitted = s.admitted_send(data["id"].as_str().unwrap_or(""));
                s.pump_outbox();
                return admitted;
            }
            if crate::motion_log::wanted() { s.motion_events.note(&ty); }
            return s.apply_event(&ty, &data);
        }
        Incoming::Status(e) => status_toast::from_server(s, e),
        Incoming::Failed(e) => failed(s, e, tx),
        Incoming::StreamDown(e) => reconnect::stream_down(s, e, tx),
        Incoming::Unanswered(e) => reconnect::unanswered(s, e, tx),
        Incoming::SessionWrite { data, error } => {
            s.session_write_replied(&data, error, tx);
            return true;
        }
        Incoming::SendFailed { session, id, error } => {
            if s.unsent.journaled(&id) {
                s.send_failed(&id, &error);
            } else if !s.start_failed(&session, &id, &error, tx) && !s.send_failed(&id, &error) {
                status_toast::from_wire(s, error);
            }
        }
        Incoming::Pending { session, items } => {
            s.sidebar_inbox.heard_pending(&session);
            s.pending.retain(|p| p.session() != session || items.iter().any(|i| i.id() == p.id()));
            for p in items {
                s.add_pending(p);
            }
        }
        // Only the stream sends this in the app (`windows::peer::heard`); a test's or the fuzz
        // link's inbox gets the same resync.
        Incoming::Reconnected => {
            s.resync_shared();
            return message(s, Incoming::Reconnected, tx);
        }
        Incoming::Opened => return message(s, Incoming::Opened, tx),
        Incoming::Active(ids) => {
            tx.instant();
            s.stops.reconcile(&ids);
            reconnect::active(s, &ids);
            // A stop the server hasn't confirmed yet stays stopped, whichever window pressed it.
            *s.running = ids.into_iter().filter(|id| !s.stops.pending(id)).collect();
        }
        Incoming::Projects(projects) => {
            s.switcher.invalidate_sessions();
            tx.instant();
            *s.projects = projects;
            s.sort_projects();
            s.list_unlisted_worktrees();
        }
        Incoming::ProjectOpened { location, projects, session } => {
            s.apply_project_opened(location, projects, session, tx);
        }
        Incoming::Settled { ticket, result } => settled(s, ticket, result, tx),
        Incoming::Created(info) if s.starts.has(&info.id) => {
            s.started(info, tx);
        }
        Incoming::Created(info) => {
            let id = info.id.clone();
            // A command sent from a draft started it: the draft's tab becomes its tab.
            s.promote_draft(&id, &info.directory);
            if s.session(&id).is_none() {
                s.sessions.insert(0, info);
            }
            *s.status = s.live_status();
            s.open(&id, tx);
        }
    }
    true
}
