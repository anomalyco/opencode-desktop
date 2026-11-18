//! Rename from the inbox's and the project sidebar's session menus: Desktop's `RenameDialog`
//! (`shell/titlebar/project-session-row.tsx:215-316`, opened from `inbox.tsx:185-197` and
//! `project-session-row.tsx:185-197`), run as its machine runs it
//! (`shell/titlebar/session-rename-dialog-machine.ts`). The title opens selected; Save sends a
//! changed one and waits, the field off, until the server answers, at most 15 s. A failure or a
//! timeout gives the field back with the draft and says so; a save that lands after its timeout
//! still closes the dialog if the draft asks for that title.

use rune::machine::{Attempt, Effects, Seq};
use rune::{El, TextEdit, Tx, col, row, spacer, text};
use rune_kit::{ButtonVariant, TextInput};

use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;

use super::Target;
use crate::App;
use crate::palette::{Elevate as _, Pal};
use opencode_i18n::t;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum State {
    #[default]
    Closed,
    /// `late`: the title of a save that timed out, which may still land.
    Editing { target: Target, initial: String, late: Option<String> },
    Saving { target: Target, initial: String, attempt: Attempt, title: String, due: f64 },
}

pub(crate) enum Event<'a> {
    Open { target: Target, title: String },
    Submitted { draft: &'a str, now: f64 },
    Cancelled,
    /// `server` answered a save of `session` to `title`; `draft` is the field's text now.
    Replied { session: &'a str, server: &'a ServerKey, title: &'a str, ok: bool, draft: &'a str },
    Tick { now: f64 },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
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

pub(crate) fn step(seq: &mut Seq, state: &mut State, event: Event<'_>) -> Effects<Effect> {
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
            let State::Editing { target, initial, .. } = state else { return Effects::none() };
            let title = draft.trim();
            if title.is_empty() {
                return Effects::none();
            }
            if title == initial {
                return close(state);
            }
            let (target, initial, title, due) = (target.clone(), initial.clone(), title.to_owned(), now + super::rename::SAVE_TIMEOUT);
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
    let State::Saving { target, initial, .. } = std::mem::take(state) else { return Effects::none() };
    *state = State::Editing { target, initial, late };
    Effects::two(Effect::Failed, Effect::Refocus)
}

impl App {
    /// Opens the dialog on `t`'s title.
    pub fn open_rename_dialog(&mut self, t: &Target) {
        let Some(title) = crate::servers::runtime::with_server(self, &t.server, |s| s.session(&t.id).map(|i| i.title.clone())).flatten() else { return };
        self.finish_rename(true);
        self.rename_dialog_event(Event::Open { target: t.clone(), title }, "");
    }

    fn submit_rename_dialog(&mut self) {
        let draft = self.menus.dialog_edit.text.clone();
        self.rename_dialog_event(Event::Submitted { draft: &draft, now: *self.clock }, "");
    }

    /// Runs `event` through the dialog's machine and carries out what it asks. `why` is a
    /// failure's own words, already said where it arrived.
    pub(crate) fn rename_dialog_event(&mut self, event: Event<'_>, why: &str) {
        let m = &mut *self.menus;
        for effect in step(&mut m.seq, &mut m.dialog, event) {
            match effect {
                Effect::Arm(title) => {
                    let mut edit = TextEdit::new(title);
                    edit.anchor = 0;
                    edit.focus = edit.text.len();
                    edit.focused = true;
                    self.menus.dialog_edit = edit;
                    self.composer.focused = false;
                    self.search.focused = false;
                }
                Effect::Refocus => self.menus.dialog_edit.focused = true,
                Effect::Close => self.menus.dialog_edit.focused = false,
                Effect::Save { target, title } => {
                    let sent = crate::servers::runtime::with_server(self, &target.server, |s| s.write(Write::Rename { session: target.id.clone(), title: title.clone() })).unwrap_or(false);
                    // A story's write goes nowhere and a build that can't write sends none: each
                    // answers at once.
                    if self.stage.is_some() || !sent {
                        let draft = self.menus.dialog_edit.text.clone();
                        let why = if sent { why } else { opencode_i18n::t!("rune.send.disconnected") };
                        return self.rename_dialog_event(Event::Replied { session: &target.id, server: &target.server, title: &title, ok: sent, draft: &draft }, why);
                    }
                }
                Effect::WakeAt(_) => {}
                // A refusal in the server's words has its own toast (`session_write_replied`).
                Effect::Failed if why.is_empty() => crate::status_toast::fail(self, t!("common.requestFailed"), t!("common.requestFailed")),
                Effect::Failed if why == opencode_i18n::t!("rune.send.disconnected") => crate::status_toast::fail(self, t!("common.requestFailed"), why),
                Effect::Failed => {}
            }
        }
    }

    /// The save's deadline (`save.timed-out`).
    pub(crate) fn poll_rename_dialog(&mut self, now: f64, tx: &mut Tx) -> bool {
        let State::Saving { due, .. } = self.menus.dialog else { return false };
        if now + 1e-9 < due {
            tx.wake_at(due);
            return false;
        }
        self.rename_dialog_event(Event::Tick { now }, "");
        true
    }
}

/// `<Dialog fit>`: the normal size; the title in its header, the large field in a 16 pt body,
/// and a ghost Cancel beside a contrast Save (loading while it saves) in its footer.
pub fn view(s: &App, p: &Pal, cx: &rune::Cx<App>) -> Vec<El> {
    let saving = match &s.menus.dialog {
        State::Closed => return vec![],
        State::Editing { .. } => false,
        State::Saving { .. } => true,
    };
    let kit = crate::palette::kit(p);
    let title = t!("command.session.rename");
    let input = TextInput::new(&s.menus.dialog_edit)
        .large()
        .width(None)
        .label(title.to_owned())
        .disabled(saving)
        .on_edit(|s: &mut App| &mut s.menus.dialog_edit)
        .on_submit(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.submit_rename_dialog();
        })
        .build(kit)
        .key("rename-dialog-input");
    let cancel = crate::widgets::button(t!("common.cancel"), ButtonVariant::Ghost)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.rename_dialog_event(Event::Cancelled, "");
        })
        .build(kit)
        .key("rename-dialog-cancel");
    let can_submit = !saving && !s.menus.dialog_edit.text.trim().is_empty();
    let save = crate::widgets::button(t!("common.save"), if saving { ButtonVariant::Loading } else { ButtonVariant::Contrast })
        .disabled(!can_submit)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.submit_rename_dialog();
        })
        .build(kit)
        .key("rename-dialog-save");
    let card = col()
        .key("rename-dialog")
        .role("dialog")
        .label_static(title)
        .radius(6.0)
        .fill(p.layer1)
        .elevate(p.overlay)
        .clip_soft(0.0)
        .modal_tx(true, |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.rename_dialog_event(Event::Cancelled, "");
        })
        .child(row().key("rename-dialog-header").w_full().padding(16.0).child(text(title).font(crate::session_menu_view::TITLE).color(p.text)))
        .child(col().key("rename-dialog-body").w_full().px(16.0).child(input))
        .child(row().key("rename-dialog-footer").w_full().padding(16.0).gap(8.0).child(spacer()).child(cancel).child(save));
    crate::widgets::sized_modal("rename-dialog", p, crate::widgets::DialogSize::Normal, crate::widgets::Fit::Content, crate::widgets::dialog_window(cx, *s.zoom), card)
}
