//! Session details' recovery dialog (`WorkspaceMoveDialog`): the new worktree a move made is
//! ready, but the session didn't move into it. Keep it, delete it, or move the session again.
//! What it does follows Desktop's `workspace-move-machine.ts`: every move and delete carries an
//! attempt, so a late answer can't reach a dialog opened since.

use opencode_i18n::t;
use rune::machine::{Attempt, Seq};
use rune::{El, Key, KeyEvent, ScrollSpec, Spring, TextStyle, Transition, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, Loader};

use crate::App;
use crate::jobs::Apply;
use crate::palette::{Pal, kit};
use crate::widgets::{self, BODY, button};

/// `dialog-title`: 15 on 20 at 530.
const TITLE: TextStyle = TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13);
/// `error-details-text`: the code face at 12 on 18.
const ERROR: TextStyle = TextStyle::new(12.0, 400.0).mono().lh(18.0);

/// Why the session didn't move, or why deleting its worktree failed (`WorkspaceMoveFailure`).
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    Failed(String),
    TimedOut,
    /// The session started running, so moving it would pull its checkout away mid-run.
    Running,
}

/// The dialog a retry or delete runs from (`RecoveryDialog`).
#[derive(Clone, Debug, PartialEq)]
enum Dialog {
    /// It keeps showing `reason` until the action settles.
    Open { attempt: Attempt, reason: Reason },
    /// Closed while the action kept running; a late failure toasts instead.
    Dismissed,
}

/// The new worktree's part of Desktop's `WorkspaceMoveState` (`workspace-move-machine.ts`): the
/// session moving into it, the dialog offering to retry or delete it, and the delete. Once
/// nothing is left to do, the app holds none (`idle`).
#[derive(Clone, Debug, PartialEq)]
enum State {
    /// `dialog` is the one a retry runs from; none for the first move.
    Moving { attempt: Attempt, dialog: Option<Dialog> },
    Recovering { dialog: Attempt, reason: Reason },
    Removing { attempt: Attempt, dialog: Dialog },
}

/// The worktree a move made, the session bound for it, and where that stands.
pub struct Recovery {
    session: String,
    project: String,
    directory: String,
    /// The last attempt handed to a move, delete or dialog.
    issued: Seq,
    state: State,
}

#[derive(Clone, Debug, PartialEq)]
enum Event {
    MoveSucceeded { attempt: Attempt },
    /// A failure, a timeout, or `move.blocked` (`Reason::Running`).
    MoveFailed { attempt: Attempt, reason: Reason },
    Retry { attempt: Attempt, running: bool },
    Remove { attempt: Attempt },
    RemoveSucceeded { attempt: Attempt },
    RemoveFailed { attempt: Attempt, reason: Reason },
    /// Keep worktree, Esc, the close cross or a press outside.
    Dismissed { attempt: Attempt },
}

#[derive(Clone, Debug, PartialEq)]
enum Effect {
    /// Moves the session into the worktree, unless it started running (`run-move`).
    Move { attempt: Attempt },
    /// Force-removes the worktree (`run-remove`).
    Remove { attempt: Attempt },
    /// Reports a failure no dialog shows.
    Toast { remove: bool, reason: Reason },
    /// The delete went through: the project's worktrees are listed again.
    Removed,
}

/// `transitionWorkspaceMove`, for the worktree it made. `None` is `idle`.
fn step(r: &mut Recovery, event: Event) -> (bool, Vec<Effect>) {
    let idle = |effects| (false, effects);
    match (&mut r.state, event) {
        (State::Moving { attempt, .. }, Event::MoveSucceeded { attempt: due }) if *attempt == due => idle(vec![]),
        (State::Moving { attempt, dialog }, Event::MoveFailed { attempt: due, reason }) if *attempt == due => match dialog.take() {
            Some(Dialog::Dismissed) => idle(vec![Effect::Toast { remove: false, reason }]),
            // A retry from the open dialog shows its failure in place.
            Some(Dialog::Open { attempt, .. }) => {
                r.state = State::Recovering { dialog: attempt, reason };
                (true, vec![])
            }
            None => {
                r.state = State::Recovering { dialog: r.issued.next(), reason };
                (true, vec![])
            }
        },
        (State::Moving { dialog: Some(dialog), .. } | State::Removing { dialog, .. }, Event::Dismissed { attempt: due }) if open_attempt(dialog) == Some(due) => {
            // The action keeps running; a late failure toasts instead of opening the dialog again.
            *dialog = Dialog::Dismissed;
            (true, vec![])
        }
        (State::Recovering { dialog, .. }, Event::Dismissed { attempt: due }) if *dialog == due => idle(vec![]),
        (State::Recovering { dialog, reason }, Event::Retry { attempt: due, running: false }) if *dialog == due => {
            let (attempt, dialog) = (r.issued.next(), Dialog::Open { attempt: *dialog, reason: reason.clone() });
            r.state = State::Moving { attempt, dialog: Some(dialog) };
            (true, vec![Effect::Move { attempt }])
        }
        (State::Recovering { dialog, reason }, Event::Remove { attempt: due }) if *dialog == due => {
            let (attempt, dialog) = (r.issued.next(), Dialog::Open { attempt: *dialog, reason: reason.clone() });
            r.state = State::Removing { attempt, dialog };
            (true, vec![Effect::Remove { attempt }])
        }
        (State::Removing { attempt, .. }, Event::RemoveSucceeded { attempt: due }) if *attempt == due => idle(vec![Effect::Removed]),
        (State::Removing { attempt, dialog }, Event::RemoveFailed { attempt: due, reason }) if *attempt == due => match dialog {
            Dialog::Dismissed => idle(vec![Effect::Toast { remove: true, reason }]),
            Dialog::Open { attempt, .. } => {
                r.state = State::Recovering { dialog: *attempt, reason };
                (true, vec![])
            }
        },
        _ => (true, vec![]),
    }
}

fn open_attempt(dialog: &Dialog) -> Option<Attempt> {
    match dialog {
        Dialog::Open { attempt, .. } => Some(*attempt),
        Dialog::Dismissed => None,
    }
}

/// Moves session `sid` into the new worktree `directory` of `project`, unless it started running
/// while the worktree was made (`worktree.created`); either failure opens the dialog.
pub(super) fn move_into(s: &mut App, sid: String, project: String, directory: String) {
    let running = s.busy(&sid);
    let mut issued = Seq::default();
    let attempt = issued.next();
    let state = if running { State::Recovering { dialog: attempt, reason: Reason::Running } } else { State::Moving { attempt, dialog: None } };
    s.location.recovery = Some(Recovery { session: sid, project, directory, issued, state });
    if !running {
        run(s, vec![Effect::Move { attempt }]);
    }
}

/// Feeds `event` to the open recovery, and runs what it asks for.
fn recovery_step(s: &mut App, event: Event) {
    let Some(r) = s.location.recovery.as_mut() else { return };
    let (live, effects) = step(r, event);
    let project = r.project.clone();
    if !live {
        s.location.recovery = None;
    }
    for effect in &effects {
        match effect {
            Effect::Toast { remove, reason } => {
                let title = if *remove { t!("plugin.details.move.removeFailed") } else { t!("plugin.details.move.failed") };
                crate::status_toast::fail(s, title, reason_text(reason));
            }
            Effect::Removed => s.list_worktrees(&project),
            _ => {}
        }
    }
    run(s, effects);
}

/// Starts the moves and deletes `effects` ask for; each answers with its attempt.
fn run(s: &mut App, effects: Vec<Effect>) {
    let Some(r) = s.location.recovery.as_ref() else { return };
    let (session, project, dir) = (r.session.clone(), r.project.clone(), r.directory.clone());
    for effect in effects {
        match effect {
            Effect::Move { attempt } => {
                // The session started running before the move left (`move.blocked`).
                if s.busy(&session) {
                    recovery_step(s, Event::MoveFailed { attempt, reason: Reason::Running });
                    continue;
                }
                let (session, dir) = (session.clone(), dir.clone());
                s.job(move |srv| {
                    let moved = srv.move_session(&session, &dir);
                    Box::new(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        let event = match moved {
                            Ok(()) => Event::MoveSucceeded { attempt },
                            Err(e) => Event::MoveFailed { attempt, reason: failure(e) },
                        };
                        recovery_step(s, event);
                    }) as Apply
                });
            }
            Effect::Remove { attempt } => {
                let (project, dir) = (project.clone(), dir.clone());
                s.job(move |srv| {
                    let removed = srv.remove_worktree(&project, &dir);
                    Box::new(move |s: &mut App, tx: &mut Tx| {
                        tx.instant();
                        let event = match removed {
                            Ok(()) => Event::RemoveSucceeded { attempt },
                            Err(e) => Event::RemoveFailed { attempt, reason: failure(e) },
                        };
                        recovery_step(s, event);
                    }) as Apply
                });
            }
            Effect::Toast { .. } | Effect::Removed => {}
        }
    }
}

/// The server's error as a failure; the client's own timeout (30 s for a move, two minutes for
/// a delete, Desktop's `op.timed-out`) as one that took too long.
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

/// The open dialog's attempt, while it shows.
fn dialog(r: &Recovery) -> Option<Attempt> {
    match &r.state {
        State::Recovering { dialog, .. } => Some(*dialog),
        State::Moving { dialog: Some(Dialog::Open { attempt, .. }), .. } | State::Removing { dialog: Dialog::Open { attempt, .. }, .. } => Some(*attempt),
        _ => None,
    }
}

/// Keep worktree, Esc, the close cross or a press outside: the worktree stays, and so does the
/// session. A retry or delete still running toasts if it fails.
fn keep(s: &mut App, tx: &mut Tx) {
    let Some(attempt) = s.location.recovery.as_ref().and_then(dialog) else { return };
    tx.instant();
    recovery_step(s, Event::Dismissed { attempt });
}

/// Retry: moves the session into the worktree again, unless it's running or busy.
fn retry(s: &mut App, tx: &mut Tx) {
    let Some(r) = s.location.recovery.as_ref() else { return };
    let State::Recovering { dialog, .. } = r.state else { return };
    let running = s.busy(&r.session);
    tx.instant();
    recovery_step(s, Event::Retry { attempt: dialog, running });
}

/// Delete worktree: removes it on the server, forced, as the project's start command may have
/// left untracked files in the new checkout.
fn remove(s: &mut App, tx: &mut Tx) {
    let Some(State::Recovering { dialog, .. }) = s.location.recovery.as_ref().map(|r| &r.state) else { return };
    let attempt = *dialog;
    tx.instant();
    recovery_step(s, Event::Remove { attempt });
}

/// Esc keeps the worktree; Enter retries.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.location.recovery.as_ref().and_then(dialog).is_none() {
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
    // `workspaceDialogView`: the latest failure, kept while a retry or delete runs.
    let (reason, retrying, removing) = match &r.state {
        State::Recovering { reason, .. } => (reason, false, false),
        State::Moving { dialog: Some(Dialog::Open { reason, .. }), .. } => (reason, true, false),
        State::Removing { dialog: Dialog::Open { reason, .. }, .. } => (reason, false, true),
        _ => return vec![],
    };
    let pending = retrying || removing;
    let running = s.busy(&r.session);
    let name = opencode_kit::path::basename(r.directory.trim_end_matches(['/', '\\'])).to_owned();
    let error = match reason {
        Reason::Failed(e) => Some(e.clone()),
        _ => None,
    };
    let mut description = t!("plugin.details.move.recovery.description", name = name);
    if error.is_none() {
        description = format!("{description} {}", reason_text(reason));
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
    let mut delete = button(remove_label, ButtonVariant::Ghost).disabled(pending).on_press_tx(remove);
    delete = if removing {
        delete.label_with(spinning(remove_label, Some(danger)))
    } else {
        delete.label_with(move |style, _| text(remove_label).font(style).color(danger))
    };
    let mut again = button(t!("plugin.details.retry"), ButtonVariant::Contrast).disabled(pending || running).on_press_tx(retry);
    if retrying {
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
