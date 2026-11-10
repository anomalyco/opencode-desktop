use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{Incoming, Ticket, TicketKind, Write};
use opencode_kit::model::{Message, SessionInfo};
use rune::{Key, KeyEvent, Mods, Runtime, TextEdit};
use serde_json::json;

use crate::{App, Route};

const DT: f64 = 1.0 / 120.0;

struct Harness {
    rt: Runtime<App>,
    sid: String,
    writes: Receiver<Write>,
    server: Sender<Incoming>,
}

fn harness() -> Harness {
    let mut app = App::fixture_session();
    app.finish_stream();
    let Route::Session { id: sid, .. } = app.route.clone() else { panic!("no open session") };
    let writes = app.fake_writes();
    let (server, inbox) = channel();
    app.inbox = Some(inbox);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    // Bring the latest user prompt above the long final reply into view.
    rt.wheel(640.0, 400.0, 520.0);
    for _ in 0..45 {
        rt.frame(DT);
    }
    Harness { rt, sid, writes, server }
}

/// A write as the server sees it, its ticket set aside.
fn untracked(w: Write) -> Write {
    match w {
        Write::Tracked { write, .. } => *write,
        w => w,
    }
}

/// The ticket of a session's first write of `kind`.
fn ticket(kind: TicketKind, session: &str) -> Ticket {
    Ticket { kind, session: session.into(), attempt: 1 }
}

fn shows_text(rt: &Runtime<App>, needle: &str) -> bool {
    rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref().is_some_and(|t| t.contains(needle)))
}

fn tap_in(rt: &mut Runtime<App>, path_part: &str, key: &str) {
    let el = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.path.contains(path_part) && e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key} under {path_part}"));
    let (x, y) = (el.rect[0] + el.rect[2] / 2.0, el.rect[1] + el.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(DT);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

#[test]
fn rewind_updates_timeline_immediately_restores_prompt_and_offers_toast_undo() {
    let mut h = harness();
    let users = h.rt.state.visible_user_ids(&h.sid);
    let last_id = users.last().unwrap().clone();
    let last_body = h.rt.state.transcripts[&h.sid]
        .messages
        .iter()
        .find_map(|m| match m {
            Message::User { id, text } if id == &last_id => Some(text.clone()),
            _ => None,
        })
        .unwrap();

    h.rt.update(|s, _| {
        s.composer = TextEdit::new("Unsent draft");
        s.composer.focused = true;
    });
    h.rt.frame(DT);

    tap_in(&mut h.rt, &format!("u-{last_id}"), &format!("user-menu-{last_id}"));
    tap_in(&mut h.rt, "popup-menu", "popup-item-revert");

    assert_eq!(h.rt.state.session(&h.sid).unwrap().revert.as_deref(), Some(last_id.as_str()));
    assert_eq!(h.rt.state.composer.text, last_body);
    assert!(!shows_text(&h.rt, &last_body) || h.rt.elements().into_iter().all(|e| e.exiting || !e.path.contains(&format!("u-{last_id}"))));
    assert!(shows_text(&h.rt, "Reverted"));
    assert!(matches!(
        h.writes.try_recv().map(untracked),
        Ok(Write::RevertStage { session, message, settle: true }) if session == h.sid && message == last_id
    ));

    // Clicking Undo on the Rewound toast before session.revert.staged arrives clears the revert
    // and puts the unsent draft back; a late staged echo for the undone rewind does not clobber it.
    tap_in(&mut h.rt, "status-toast", "toast-undo");
    assert!(h.rt.state.session(&h.sid).unwrap().revert.is_none());
    assert_eq!(h.rt.state.composer.text, "Unsent draft");
    assert!(matches!(h.writes.try_recv().map(untracked), Ok(Write::RevertClear(s)) if s == h.sid));

    h.server
        .send(Incoming::Event {
            ty: "session.revert.staged".into(),
            data: json!({ "sessionID": h.sid, "revert": { "messageID": last_id } }),
        })
        .unwrap();
    h.rt.frame(DT);
    assert!(h.rt.state.session(&h.sid).unwrap().revert.is_none(), "stale staged SSE ignored while Undo clear is pending");

    h.server
        .send(Incoming::Event { ty: "session.revert.cleared".into(), data: json!({ "sessionID": h.sid }) })
        .unwrap();
    h.rt.frame(DT);
    assert!(h.rt.state.session(&h.sid).unwrap().revert.is_none());
}

#[test]
fn undo_after_switching_tabs_restages_prior_revert_and_restores_source_draft() {
    let mut h = harness();
    let users = h.rt.state.visible_user_ids(&h.sid);
    assert!(users.len() >= 2);
    let (first, second) = (users[0].clone(), users[1].clone());
    let other = h.rt.state.tabs.iter().map(|t| t.id.clone()).find(|id| *id != h.sid).unwrap();

    // Session starts already reverted at `second`, with an unsent draft in its composer.
    h.rt.update(|s, _| {
        s.set_session_revert(&h.sid, Some(second.clone()));
        s.composer = TextEdit::new("Draft in source");
        s.composer.focused = true;
    });
    h.rt.frame(DT);

    h.rt.update(|s, tx| s.rewind_to(&h.sid, &first, tx));
    h.rt.frame(DT);
    let _ = h.writes.try_recv();
    assert_eq!(h.rt.state.session(&h.sid).unwrap().revert.as_deref(), Some(first.as_str()));

    // Switch to another tab, then click Undo on the Rewound toast.
    h.rt.update(|s, tx| s.open(&other, tx));
    h.rt.frame(DT);
    assert_eq!(h.rt.state.route, Route::here(other.clone()));

    tap_in(&mut h.rt, "status-toast", "toast-undo");
    assert_eq!(h.rt.state.session(&h.sid).unwrap().revert.as_deref(), Some(second.as_str()));
    assert!(matches!(
        h.writes.try_recv().map(untracked),
        Ok(Write::RevertStage { session, message, settle: false }) if session == h.sid && message == second
    ));

    // Switching back to the source session shows the restored unsent draft.
    h.rt.update(|s, tx| s.open(&h.sid, tx));
    h.rt.frame(DT);
    assert_eq!(h.rt.state.composer.text, "Draft in source");
}

#[test]
fn undo_steps_back_and_redo_restores_all_without_touching_composer() {
    let mut h = harness();
    let users = h.rt.state.visible_user_ids(&h.sid);
    let n = users.len();
    assert!(n >= 2);

    h.rt.update(|s, tx| {
        assert!(s.undo_last(tx));
    });
    let _ = h.writes.try_recv();
    assert_eq!(h.rt.state.visible_user_ids(&h.sid).len(), n - 1);

    h.rt.update(|s, tx| {
        assert!(s.undo_last(tx));
        s.composer = TextEdit::new("keep my edited prompt");
    });
    let _ = h.writes.try_recv();
    assert_eq!(h.rt.state.visible_user_ids(&h.sid).len(), n - 2);

    h.rt.update(|s, tx| {
        assert!(s.redo_revert(tx));
    });
    assert!(matches!(h.writes.try_recv().map(untracked), Ok(Write::RevertClear(s)) if s == h.sid));
    assert_eq!(h.rt.state.visible_user_ids(&h.sid).len(), n);
    assert_eq!(h.rt.state.composer.text, "keep my edited prompt");
}

#[test]
fn server_refusal_rolls_back_timeline_running_state_and_draft_with_fail_toast() {
    let mut h = harness();
    let last_id = h.rt.state.visible_user_ids(&h.sid).pop().unwrap();
    h.rt.update(|s, _| {
        s.running.insert(h.sid.clone());
        s.composer = TextEdit::new("Unsent before failure");
    });
    h.rt.frame(DT);

    h.rt.update(|s, tx| s.rewind_to(&h.sid, &last_id, tx));
    h.rt.frame(DT);
    let _ = h.writes.try_recv();
    assert_eq!(h.rt.state.session(&h.sid).unwrap().revert.as_deref(), Some(last_id.as_str()));
    assert!(!h.rt.state.running.contains(&h.sid));

    h.server
        .send(Incoming::Settled { ticket: ticket(TicketKind::Revert, &h.sid), result: Err(format!("POST /api/session/{}/revert/stage: Snapshot failed", h.sid)) })
        .unwrap();
    h.rt.frame(DT);

    assert!(h.rt.state.session(&h.sid).unwrap().revert.is_none(), "revert rolled back");
    assert!(h.rt.state.running.contains(&h.sid), "running state restored");
    assert_eq!(h.rt.state.composer.text, "Unsent before failure", "draft rolled back");
    assert!(shows_text(&h.rt, "Request failed") && shows_text(&h.rt, "Snapshot failed"));
    assert!(h.rt.state.toast.current().unwrap().persistent);
}

#[test]
fn sending_from_staged_revert_commits_and_keeps_new_message_visible() {
    let mut h = harness();
    let last_id = h.rt.state.visible_user_ids(&h.sid).pop().unwrap();
    h.rt.update(|s, tx| s.rewind_to(&h.sid, &last_id, tx));
    let _ = h.writes.try_recv();

    h.rt.update(|s, tx| {
        s.composer = TextEdit::new("Try a cleaner approach");
        s.send(tx);
    });
    h.rt.frame(DT);

    assert!(matches!(h.writes.try_recv().map(untracked), Ok(Write::SendFromRevert { session, prompt }) if session == h.sid && prompt.text == "Try a cleaner approach"));
    assert!(h.rt.state.session(&h.sid).unwrap().revert.is_none());
    assert!(shows_text(&h.rt, "Try a cleaner approach"));
}

#[test]
fn child_sessions_hide_rewind_and_disable_fork() {
    let mut h = harness();
    let child = crate::fixture_ids::CHILD.to_owned();
    h.rt.state.transcripts.insert(
        child.clone(),
        opencode_kit::model::Transcript {
            session: child.clone(),
            messages: vec![Message::User { id: "msg_child_1".into(), text: "inspect stream".into() }],
        },
    );
    h.rt.update(|s, tx| s.open(&child, tx));
    h.rt.frame(DT);

    tap_in(&mut h.rt, "u-msg_child_1", "user-menu-msg_child_1");
    assert!(!h.rt.elements().into_iter().any(|e| !e.exiting && e.key.as_deref() == Some("popup-item-revert")));
    assert!(!h.rt.state.can_undo_session());
    assert!(!h.rt.state.can_fork_session());
}

#[test]
fn fork_from_menu_and_dialog_opens_forked_session_with_prompt_in_composer() {
    let mut h = harness();
    let users = h.rt.state.visible_user_ids(&h.sid);
    let target_id = users.last().unwrap().clone();
    let target_body = h.rt.state.transcripts[&h.sid]
        .messages
        .iter()
        .find_map(|m| match m {
            Message::User { id, text } if id == &target_id => Some(text.clone()),
            _ => None,
        })
        .unwrap();

    // Open the message's menu and verify Fork from here + Copy message are present.
    tap_in(&mut h.rt, &format!("u-{target_id}/"), &format!("user-menu-{target_id}"));
    crate::popover::tests::grows_whole(&mut h.rt, "popup-menu");
    assert!(shows_text(&h.rt, "Fork from here") && shows_text(&h.rt, "Copy message"));
    tap_in(&mut h.rt, "popup-menu", "popup-item-fork");

    assert!(matches!(
        h.writes.try_recv().map(untracked),
        Ok(Write::Fork { session, before: Some(before) }) if session == h.sid && before == target_id
    ));

    let forked = SessionInfo {
        id: "ses_forked_live".into(),
        project: h.rt.state.session(&h.sid).unwrap().project.clone(),
        title: "Fix flaky reconnect (fork #1)".into(),
        directory: "/Users/demo/code/lumen".into(),
        agent: "build".into(),
        created: 100,
        updated: 100,
        ..Default::default()
    };
    h.server
        .send(Incoming::Settled { ticket: ticket(TicketKind::Fork, &h.sid), result: Ok(Some(forked)) })
        .unwrap();
    h.rt.frame(DT);

    assert_eq!(h.rt.state.route, Route::here("ses_forked_live".into()));
    assert_eq!(h.rt.state.composer.text, target_body);

    // Also verify the Fork from message dialog filters and forks on Enter.
    h.rt.update(|s, tx| {
        s.open(&h.sid, tx);
        assert!(s.open_fork_dialog(tx));
    });
    h.rt.frame(DT);
    assert!(shows_text(&h.rt, "Choose a message to fork from"));
    h.rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    h.rt.frame(DT);
    assert!(matches!(h.writes.try_recv().map(untracked), Ok(Write::Fork { .. })));

    // A fork failure shows Request failed via status_toast::fail.
    h.server
        .send(Incoming::Settled { ticket: Ticket { attempt: 2, ..ticket(TicketKind::Fork, &h.sid) }, result: Err(format!("POST /api/session/{}/fork: Cannot fork empty session", h.sid)) })
        .unwrap();
    h.rt.frame(DT);
    assert!(shows_text(&h.rt, "Request failed") && shows_text(&h.rt, "Cannot fork empty session"));
}

#[test]
fn fork_dialog_and_rewound_toast_stay_inside_viewport_at_zoom_1_6() {
    let mut app = App::fixture_session();
    app.finish_stream();
    app.zoom = 1.6;
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..20 {
        rt.frame(DT);
    }

    rt.update(|s, tx| {
        assert!(s.undo_last(tx));
    });
    for _ in 0..30 {
        rt.frame(DT);
    }
    // Desktop's Sonner region sits at the window's bottom-right whatever is under it (toast.tsx).
    let toast = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("status-toast")).unwrap().rect;
    assert!(toast[0] >= 16.0 && toast[1] >= 16.0 && toast[0] + toast[2] <= 1280.0 - 16.0 && toast[1] + toast[3] <= 820.0 - 16.0, "toast inside window at 1.6x: {toast:?}");

    rt.update(|s, tx| {
        assert!(s.open_fork_dialog(tx));
    });
    rt.frame(DT);
    let dlg = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("fork-dialog")).unwrap().rect;
    assert!(dlg[0] >= 16.0 && dlg[1] >= 16.0 && dlg[0] + dlg[2] <= 1280.0 - 16.0 && dlg[1] + dlg[3] <= 820.0 - 16.0, "fork-dialog inside window at 1.6x: {dlg:?}");
}


/// A glide that carries the hovered prompt away from a parked pointer lets its ⋯ and Rewind go:
/// the hover drops while it moves and the actions fade, never held lit at the old place.
#[test]
fn a_glide_that_carries_the_hovered_prompt_away_lets_its_actions_go() {
    let mut h = harness();
    let id = h.rt.state.visible_user_ids(&h.sid).pop().expect("a prompt");
    let more = format!("user-menu-{id}");
    let (x, y) = h.rt.find(&more).expect("the chevron on screen");
    h.rt.pointer_move(x, y);
    for _ in 0..10 {
        h.rt.frame(1.0 / 60.0);
    }
    let actions = |rt: &Runtime<App>| {
        rt.elements().into_iter().filter(|e| !e.exiting && (e.key.as_deref() == Some(more.as_str()) || e.key.as_deref() == Some("menu") && e.path.contains(&format!("u-{id}")))).map(|e| (e.hover, e.opacity)).collect::<Vec<_>>()
    };
    assert!(actions(&h.rt).iter().any(|&(hover, _)| hover > 0.5), "the pointer lights ⋯: {:?}", actions(&h.rt));
    let scroll = format!("scroll-{}", h.sid);
    h.rt.update(move |_, tx| tx.scroll_to_bottom(scroll.clone()));
    for _ in 0..40 {
        h.rt.frame(1.0 / 60.0);
    }
    let after = actions(&h.rt);
    assert!(!after.is_empty() && after.iter().all(|&(hover, opacity)| hover < 0.01 && opacity < 0.01), "{after:?}");
}
