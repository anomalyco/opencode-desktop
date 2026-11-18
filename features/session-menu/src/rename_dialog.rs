//! Rename from the inbox's and the project sidebar's session menus: Desktop's `RenameDialog`
//! (`shell/titlebar/project-session-row.tsx:215-316`, opened from `inbox.tsx:185-197` and
//! `project-session-row.tsx:185-197`), run as its machine runs it
//! (`shell/titlebar/session-rename-dialog-machine.ts`). The title opens selected; Save sends a
//! changed one and waits, the field off, until the server answers, at most 15 s. A failure or a
//! timeout gives the field back with the draft and says so; a save that lands after its timeout
//! still closes the dialog if the draft asks for that title.

use opencode_i18n::t;
use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;
use opencode_ui::widgets::{DialogSize, Fit, button, dialog_window, sized_modal};
use rune::machine::{Attempt, Effects, Seq};
use rune::{Cx, El, TextEdit, Tx, col, row, spacer, text};
use rune_kit::{ButtonVariant, TextInput};
use session_view::palette::{Elevate as _, Pal, kit};

use crate::host::{SessionMenuHost, on_server};
use crate::{RENAME_OVERLAY_ID, Target};

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    Closed,
    /// `late`: the title of a save that timed out, which may still land.
    Editing { target: Target, initial: String, late: Option<String> },
    Saving { target: Target, initial: String, attempt: Attempt, title: String, due: f64 },
}

pub enum Event<'a> {
    Open { target: Target, title: String },
    Submitted { draft: &'a str, now: f64 },
    Cancelled,
    /// `server` answered a save of `session` to `title`; `draft` is the field's text now.
    Replied { session: &'a str, server: &'a ServerKey, title: &'a str, ok: bool, draft: &'a str },
    Tick { now: f64 },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// The field opens on `title`, all of it selected (`input.select`), and takes focus.
    Arm(String),
    /// The field takes focus again once it's enabled (`input.focus`).
    Refocus,
    Close,
    Save { target: Target, title: String },
    WakeAt(f64),
    /// `toast.failed`.
    Failed,
}

pub fn step(seq: &mut Seq, state: &mut State, event: Event<'_>) -> Effects<Effect> {
    match event {
        Event::Open { target, title } => {
            if *state != State::Closed {
                return Effects::none();
            }
            *state = State::Editing { target, initial: title.clone(), late: None };
            Effects::one(Effect::Arm(title))
        }
        Event::Cancelled => close(state),
        Event::Submitted { draft, now } => {
            let State::Editing { target, initial, .. } = state else {
                return Effects::none();
            };
            let title = draft.trim();
            if title.is_empty() {
                return Effects::none();
            }
            if title == initial {
                return close(state);
            }
            let (target, initial, title, due) = (target.clone(), initial.clone(), title.to_owned(), now + crate::rename::SAVE_TIMEOUT);
            *state = State::Saving { target: target.clone(), initial, attempt: seq.next(), title: title.clone(), due };
            Effects::two(Effect::Save { target, title }, Effect::WakeAt(due))
        }
        Event::Replied { session, server, title, ok, draft } => match state {
            State::Saving { target, title: sent, .. } if target.id == session && target.server == *server && sent == title => {
                if ok {
                    return close(state);
                }
                back_to_editing(state, None)
            }
            // A save that timed out landed after all (`saveSucceeded`).
            State::Editing { target, initial, late } if ok && target.id == session && target.server == *server && late.as_deref() == Some(title) => {
                if draft.trim() == title {
                    return close(state);
                }
                *initial = title.to_owned();
                *late = None;
                Effects::none()
            }
            _ => Effects::none(),
        },
        Event::Tick { now } => match state {
            State::Saving { due, title, .. } if now + 1e-9 >= *due => {
                let late = Some(title.clone());
                back_to_editing(state, late)
            }
            _ => Effects::none(),
        },
    }
}

fn close(state: &mut State) -> Effects<Effect> {
    if *state == State::Closed {
        return Effects::none();
    }
    *state = State::Closed;
    Effects::one(Effect::Close)
}

fn back_to_editing(state: &mut State, late: Option<String>) -> Effects<Effect> {
    let State::Saving { target, initial, .. } = std::mem::take(state) else {
        return Effects::none();
    };
    *state = State::Editing { target, initial, late };
    Effects::two(Effect::Failed, Effect::Refocus)
}

/// Opens the rename dialog on `t`'s current title.
pub fn open(host: &mut dyn SessionMenuHost, t: &Target) {
    let Some(title) = on_server(host, &t.server, |h| h.session(&t.id).map(|i| i.title.clone())).flatten() else {
        return;
    };
    crate::finish_rename(host, true);
    dispatch(host, Event::Open { target: t.clone(), title }, "");
}

/// Submits the rename dialog's current draft.
pub fn submit(host: &mut dyn SessionMenuHost) {
    let draft = host.menus().dialog_edit.text.clone();
    let now = host.clock();
    dispatch(host, Event::Submitted { draft: &draft, now }, "");
}

/// Cancels and closes the rename dialog (Cancel button, Escape, outside press, or overlay dismiss).
pub fn cancel(host: &mut dyn SessionMenuHost, tx: &mut Tx) {
    tx.instant();
    dispatch(host, Event::Cancelled, "");
}

/// Runs `event` through the dialog's machine and carries out what it asks. `why` is a
/// failure's own words, already said where it arrived.
pub fn dispatch(host: &mut dyn SessionMenuHost, event: Event<'_>, why: &str) {
    let effects = {
        let m = host.menus_mut();
        step(&mut m.seq, &mut m.dialog, event)
    };
    for effect in effects {
        match effect {
            Effect::Arm(title) => {
                let mut edit = TextEdit::new(title);
                edit.anchor = 0;
                edit.focus = edit.text.len();
                edit.focused = true;
                host.menus_mut().dialog_edit = edit;
                host.on_rename_focus_taken();
                host.on_overlay_opened(RENAME_OVERLAY_ID, None);
            }
            Effect::Refocus => host.menus_mut().dialog_edit.focused = true,
            Effect::Close => {
                host.menus_mut().dialog_edit.focused = false;
                host.on_overlay_closed(RENAME_OVERLAY_ID);
            }
            Effect::Save { target, title } => {
                let sent = on_server(host, &target.server, |h| h.write_server(Write::Rename { session: target.id.clone(), title: title.clone() })).unwrap_or(false);
                // A story's write goes nowhere and a build that can't write sends none: each
                // answers at once.
                if host.stage_active() || !sent {
                    let draft = host.menus().dialog_edit.text.clone();
                    let why = if sent { why } else { t!("rune.send.disconnected") };
                    return dispatch(host, Event::Replied { session: &target.id, server: &target.server, title: &title, ok: sent, draft: &draft }, why);
                }
            }
            Effect::WakeAt(_) => {}
            // A refusal in the server's words has its own toast (`session_write_replied`).
            Effect::Failed if why.is_empty() => host.toast_fail(t!("common.requestFailed"), t!("common.requestFailed")),
            Effect::Failed if why == t!("rune.send.disconnected") => host.toast_fail(t!("common.requestFailed"), why),
            Effect::Failed => {}
        }
    }
}

/// The save's deadline (`save.timed-out`).
pub fn poll(host: &mut dyn SessionMenuHost, now: f64, tx: &mut Tx) -> bool {
    let State::Saving { due, .. } = host.menus().dialog else {
        return false;
    };
    if now + 1e-9 < due {
        tx.wake_at(due);
        return false;
    }
    dispatch(host, Event::Tick { now }, "");
    true
}

fn submit_from_ui(host: &mut dyn SessionMenuHost, tx: &mut Tx) {
    tx.instant();
    submit(host);
}

/// `<Dialog fit>`: the normal size; the title in its header, the large field in a 16 pt body,
/// and a ghost Cancel beside a contrast Save (loading while it saves) in its footer.
pub fn view<H: SessionMenuHost + 'static>(s: &H, p: &Pal, cx: &Cx<H>) -> Vec<El> {
    let saving = match &s.menus().dialog {
        State::Closed => return vec![],
        State::Editing { .. } => false,
        State::Saving { .. } => true,
    };
    let k = kit(p);
    let title = t!("command.session.rename");
    let input = TextInput::new(&s.menus().dialog_edit)
        .large()
        .width(None)
        .label(title.to_owned())
        .disabled(saving)
        .on_edit(|s: &mut H| &mut s.menus_mut().dialog_edit)
        .on_submit(|s: &mut H, tx: &mut Tx| submit_from_ui(s, tx))
        .build(k)
        .key("rename-dialog-input");
    let cancel_btn = button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut H, tx: &mut Tx| cancel(s, tx))
        .build(k)
        .key("rename-dialog-cancel");
    let can_submit = !saving && !s.menus().dialog_edit.text.trim().is_empty();
    let save_btn = button(t!("common.save"), if saving { ButtonVariant::Loading } else { ButtonVariant::Contrast })
        .disabled(!can_submit)
        .on_press_tx(|s: &mut H, tx: &mut Tx| submit_from_ui(s, tx))
        .build(k)
        .key("rename-dialog-save");
    let card = col()
        .key("rename-dialog")
        .role("dialog")
        .label_static(title)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .modal_tx(true, |s: &mut H, tx: &mut Tx| cancel(s, tx))
        .child(row().key("rename-dialog-header").w_full().padding(16.0).child(text(title).font(crate::view::TITLE).color(p.text)))
        .child(col().key("rename-dialog-body").w_full().px(16.0).child(input))
        .child(row().key("rename-dialog-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel_btn).child(save_btn));
    sized_modal("rename-dialog", p, DialogSize::Normal, Fit::Content, dialog_window(cx, s.zoom()), card)
}
