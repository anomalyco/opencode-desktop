//! Session details' recovery dialog (`WorkspaceMoveDialog`): the new worktree a move made is
//! ready, but the session didn't move into it. Keep it, delete it, or move the session again.

use opencode_i18n::t;
use rune::{El, Key, KeyEvent, ScrollSpec, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, Loader};
use serde_json::json;

use crate::App;
use crate::jobs::Apply;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY, button};

/// `dialog-title`: 15 on 20 at 530.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// `error-details-text`: the code face at 12 on 18.
const ERROR: TextStyle = TextStyle::new(12.0, 400.0).mono().lh(18.0);

/// Why the session didn't move (`WorkspaceMoveFailure`).
#[derive(Clone, Debug)]
pub enum Reason {
    Failed(String),
    TimedOut,
    Running,
}

/// What the dialog is doing for its worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    Idle,
    Retrying,
    Removing,
}

/// The worktree a move made, and the latest failure, kept while a retry or delete runs.
pub struct Recovery {
    session: String,
    project: String,
    directory: String,
    reason: Reason,
    busy: Busy,
}

/// Moves session `sid` into the new worktree `directory` of `project`, unless it started running
/// while the worktree was made; either failure opens the dialog.
pub(super) fn move_into(s: &mut App, sid: String, project: String, directory: String) {
    if s.busy(&sid) {
        s.location.recovery = Some(Recovery { session: sid, project, directory, reason: Reason::Running, busy: Busy::Idle });
        return;
    }
    let (session, dir) = (sid.clone(), directory.clone());
    s.job(move |srv| {
        let moved = srv.move_session(&session, &dir);
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            if let Err(e) = moved {
                let reason = failure(e);
                s.location.recovery = Some(Recovery { session: sid, project, directory, reason, busy: Busy::Idle });
            }
        }) as Apply
    });
}

/// `move.running` or the server's error, as a toast or the dialog's description says it.
fn failure(e: String) -> Reason {
    if e.to_lowercase().contains("timeout") || e.to_lowercase().contains("timed out") {
        return Reason::TimedOut;
    }
    Reason::Failed(e)
}

fn reason_text(reason: &Reason) -> String {
    match reason {
        Reason::Failed(e) => e.clone(),
        Reason::TimedOut => t!("plugin.details.timedOut").to_owned(),
        Reason::Running => t!("plugin.details.move.running").to_owned(),
    }
}

/// Keep worktree, Esc, the close cross or a press outside: the worktree stays, and so does the
/// session. A retry or delete still running toasts if it fails.
fn keep(s: &mut App, tx: &mut Tx) {
    tx.instant();
    s.location.recovery = None;
}

/// Retry: moves the session into the worktree again, unless it's running or busy.
fn retry(s: &mut App, tx: &mut Tx) {
    let Some(r) = s.location.recovery.as_ref().filter(|r| r.busy == Busy::Idle) else { return };
    if s.busy(&r.session) {
        return;
    }
    tx.instant();
    let (session, dir) = (r.session.clone(), r.directory.clone());
    s.location.recovery.as_mut().expect("open").busy = Busy::Retrying;
    s.job(move |srv| {
        let moved = srv.move_session(&session, &dir);
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            settle(s, &dir, moved, t!("plugin.details.move.failed"));
        }) as Apply
    });
}

/// Delete worktree: removes it on the server, forced, as the project's start command may have
/// left untracked files in the new checkout.
fn remove(s: &mut App, tx: &mut Tx) {
    let Some(r) = s.location.recovery.as_mut().filter(|r| r.busy == Busy::Idle) else { return };
    tx.instant();
    r.busy = Busy::Removing;
    let (project, dir) = (r.project.clone(), r.directory.clone());
    s.job(move |srv| {
        let removed = srv.write("DELETE", "/api/worktree", Some(json!({ "projectID": project, "directory": dir, "force": true }))).map(|_| ());
        Box::new(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            let done = removed.is_ok();
            settle(s, &dir, removed, t!("plugin.details.move.removeFailed"));
            if done {
                s.list_worktrees(&project);
            }
        }) as Apply
    });
}

/// A retry or delete for worktree `dir` ended: success closes the dialog, a failure shows in it,
/// or in a toast titled `title` once it's closed.
fn settle(s: &mut App, dir: &str, result: Result<(), String>, title: &str) {
    let open = s.location.recovery.as_mut().filter(|r| r.directory == dir && r.busy != Busy::Idle);
    match (result, open) {
        (Ok(()), Some(_)) => s.location.recovery = None,
        (Ok(()), None) => {}
        (Err(e), Some(r)) => {
            r.reason = failure(e);
            r.busy = Busy::Idle;
        }
        (Err(e), None) => crate::status_toast::fail(s, title, reason_text(&failure(e))),
    }
}

/// Esc keeps the worktree; Enter retries.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.location.recovery.is_none() {
        return false;
    }
    match ev.key {
        Key::Escape => keep(s, tx),
        Key::Enter => retry(s, tx),
        _ => {}
    }
    true
}

/// The dialog over the scrim (`Dialog fit`): its title and what happened, then Keep worktree,
/// Delete worktree and Retry, below the server's error when it gave one.
pub fn view(s: &App, p: &Pal) -> Vec<El> {
    let Some(r) = &s.location.recovery else { return vec![] };
    let running = s.busy(&r.session);
    let name = opencode_kit::path::basename(r.directory.trim_end_matches(['/', '\\'])).to_owned();
    let error = match &r.reason {
        Reason::Failed(e) => Some(e.clone()),
        _ => None,
    };
    let mut description = t!("plugin.details.move.recovery.description", name = name);
    if error.is_none() {
        description = format!("{description} {}", reason_text(&r.reason));
    }
    let close = el().key("move-recovery-close").role("button").label(t!("common.close")).size(20.0, 20.0).mt(-2.0).mr(-2.0).radius(4.0).center()
        .hover(p.hover).press(p.pressed)
        .on_press_tx(keep)
        .child(icon(&widgets::DIALOG_CLOSE, 16.0).color(p.icon_muted));
    let header = row().key("move-recovery-header").w_full().padding(16.0)
        .child(col().grow(1.0).shrink().min_w(0.0).gap(4.0)
            .child(text(t!("plugin.details.move.recovery.title")).font(TITLE).color(p.text))
            .child(text(description).font(BODY).color(p.muted).wrap_anywhere()))
        .child(close);
    let spinning = |label: &str, ink_of: Option<rune::Color>| {
        let label = label.to_owned();
        let loader = Loader::new().size(16.0).build(kit(p));
        move |style: TextStyle, ink: rune::Color| row().gap(4.0).center_y().child(loader).child(text(label).font(style).color(ink_of.unwrap_or(ink)))
    };
    let danger = p.danger;
    let remove_label = t!("plugin.details.move.recovery.remove");
    let mut delete = button(remove_label, ButtonVariant::Ghost).disabled(r.busy != Busy::Idle).on_press_tx(remove);
    delete = match r.busy {
        Busy::Removing => delete.label_with(spinning(remove_label, Some(danger))),
        _ => delete.label_with(move |style, _| text(remove_label).font(style).color(danger)),
    };
    let mut again = button(t!("plugin.details.retry"), ButtonVariant::Contrast).disabled(r.busy != Busy::Idle || running).on_press_tx(retry);
    if r.busy == Busy::Retrying {
        again = again.label_with(spinning(t!("plugin.details.retry"), None));
    }
    let actions = row().key("move-recovery-actions").w_full().gap(8.0).justify_end()
        .child(button(t!("plugin.details.move.recovery.keep"), ButtonVariant::Ghost).on_press_tx(keep).build(kit(p)).key("move-recovery-keep"))
        .child(delete.build(kit(p)).key("move-recovery-delete"))
        .child(again.build(kit(p)).key("move-recovery-retry"));
    let body = match error {
        // `ErrorDetails`: the error in a selectable block above the actions.
        Some(e) => col().key("move-recovery-error").w_full().gap(12.0).px(16.0).pb(16.0)
            .child(col().key("move-recovery-error-text").w_full().max_h(240.0).scroll(ScrollSpec { stick_bottom: false }).py(8.0).px(12.0).radius(6.0).fill(p.layer3)
                .child(text(e).font(ERROR).color(p.text).wrap_anywhere().selectable(true)))
            .child(actions),
        None => actions.padding(16.0),
    };
    let dialog = widgets::overlay(p, col())
        .key("move-recovery")
        .role("dialog")
        .label(t!("plugin.details.move.recovery.title"))
        .w(480.0)
        .radius(6.0)
        .fill(p.layer1)
        .clip_soft(0.0)
        .transition(Transition::none())
        .motion(Spring::NONE)
        .modal(true, |s: &mut App| keep(s, &mut Tx::default()))
        .child(header)
        .child(body);
    widgets::modal("move-recovery", p, dialog, None)
}
