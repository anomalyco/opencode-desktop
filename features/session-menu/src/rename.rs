//! A session's title edited in place, as Desktop's tab rename machine runs it
//! (`shell/titlebar/tab-rename-machine.ts`): `Arming` makes the title editable while focus
//! settles (the closing menu or the double-click still move it, so a blur then doesn't end the
//! edit), `Editing` ends on Enter, Escape or a blur. A changed title saves; while saves overlap
//! only the latest decides, and one that fails or takes longer than 15 s puts back the title the
//! server last accepted (`confirmed`) and says so.

use opencode_kit::client::Write;
use opencode_kit::servers::ServerKey;
use rune::machine::{Attempt, Effects, Seq};
use rune::{TextEdit, Tx};

use crate::host::{SessionMenuHost, on_server};
use crate::{Spot, Target};

/// `TAB_RENAME_SAVE_TIMEOUT_MS`.
pub const SAVE_TIMEOUT: f64 = 15.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Edit {
    #[default]
    Idle,
    /// The field shows; it hasn't had focus yet.
    Arming { target: Target, initial: String },
    Editing { target: Target, initial: String },
}

/// The title's save (`TabRenameSave`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Save {
    #[default]
    Idle,
    Saving { attempt: Attempt, session: String, server: ServerKey, optimistic: String, confirmed: String, due: f64 },
}

pub enum Event<'a> {
    /// `edit.start`: `title` is the title now.
    Start { target: Target, title: String },
    /// The field has focus (`edit.armed`).
    Armed,
    /// The field lost focus after it had it.
    Blurred { draft: &'a str, now: f64 },
    /// Enter (`edit.submit`).
    Submit { draft: &'a str, now: f64 },
    /// Escape (`edit.cancel`).
    Cancel,
    /// `server` answered a save of `session` to `title`.
    Replied { session: &'a str, server: &'a ServerKey, title: &'a str, ok: bool, current: Option<&'a str> },
    Tick { now: f64, current: Option<&'a str> },
}

#[derive(Debug, PartialEq)]
pub enum Effect {
    /// The field opens on `title`, all of it selected, and takes focus.
    Arm(String),
    /// The field goes.
    Disarm,
    /// Show `title` for `session` on `server` now (`session.remember`).
    Remember { session: String, server: ServerKey, title: String },
    /// Send the save to `server` (`save.start`).
    Save { session: String, server: ServerKey, title: String },
    WakeAt(f64),
    Failed,
}

pub fn step(seq: &mut Seq, edit: &mut Edit, save: &mut Save, event: Event<'_>) -> Effects<Effect> {
    match event {
        // A rename already open keeps its title, its draft and its blur handling.
        Event::Start { target, title } => {
            if *edit != Edit::Idle {
                return Effects::none();
            }
            *edit = Edit::Arming { target, initial: title.clone() };
            Effects::one(Effect::Arm(title))
        }
        Event::Armed => {
            if let Edit::Arming { target, initial } = std::mem::take(edit) {
                *edit = Edit::Editing { target, initial };
            }
            Effects::none()
        }
        Event::Cancel => close(edit),
        Event::Blurred { draft, now } => match edit {
            Edit::Editing { target, .. } if target.spot == Spot::Row => close(edit),
            Edit::Editing { .. } => commit(seq, edit, save, draft, now),
            _ => Effects::none(),
        },
        Event::Submit { draft, now } => commit(seq, edit, save, draft, now),
        Event::Replied { session, server, title, ok, current } => match save {
            Save::Saving { session: s, server: k, optimistic, .. } if s == session && k == server && optimistic == title => {
                let restore = failed(save, ok, current);
                *save = Save::Idle;
                restore
            }
            _ => Effects::none(),
        },
        Event::Tick { now, current } => match save {
            Save::Saving { due, .. } if now + 1e-9 >= *due => {
                let restore = failed(save, false, current);
                *save = Save::Idle;
                restore
            }
            _ => Effects::none(),
        },
    }
}

fn close(edit: &mut Edit) -> Effects<Effect> {
    if *edit == Edit::Idle {
        return Effects::none();
    }
    *edit = Edit::Idle;
    Effects::one(Effect::Disarm)
}

/// Ends the edit, saving a changed title that isn't empty.
fn commit(seq: &mut Seq, edit: &mut Edit, save: &mut Save, draft: &str, now: f64) -> Effects<Effect> {
    let (Edit::Arming { target, initial } | Edit::Editing { target, initial }) = std::mem::take(edit) else {
        return Effects::none();
    };
    let next = draft.trim();
    if next.is_empty() || next == initial {
        return Effects::one(Effect::Disarm);
    }
    // An overlapping save keeps the last title the server accepted, never the previous optimistic one.
    let confirmed = match save {
        Save::Saving { session, server, confirmed, .. } if *session == target.id && *server == target.server => confirmed.clone(),
        _ => initial,
    };
    let due = now + SAVE_TIMEOUT;
    *save = Save::Saving { attempt: seq.next(), session: target.id.clone(), server: target.server.clone(), optimistic: next.to_owned(), confirmed, due };
    Effects::Many(vec![
        Effect::Disarm,
        Effect::Remember { session: target.id.clone(), server: target.server.clone(), title: next.to_owned() },
        Effect::Save { session: target.id, server: target.server, title: next.to_owned() },
        Effect::WakeAt(due),
    ])
}

/// A failed or timed-out latest save puts back the accepted title, unless the server pushed
/// another meanwhile.
fn failed(save: &Save, ok: bool, current: Option<&str>) -> Effects<Effect> {
    let Save::Saving { session, server, optimistic, confirmed, .. } = save else {
        return Effects::none();
    };
    if ok {
        return Effects::none();
    }
    let restore = (current == Some(optimistic.as_str())).then(|| Effect::Remember {
        session: session.clone(),
        server: server.clone(),
        title: confirmed.clone(),
    });
    match restore {
        Some(r) => Effects::two(r, Effect::Failed),
        None => Effects::one(Effect::Failed),
    }
}

/// Starts editing `t`'s title in place, all of it selected; one open elsewhere ends first.
pub fn begin_rename(host: &mut dyn SessionMenuHost, t: &Target) {
    let Some(title) = on_server(host, &t.server, |h| h.session(&t.id).map(|i| i.title.clone())).flatten() else {
        return;
    };
    finish_rename(host, true);
    dispatch(host, Event::Start { target: t.clone(), title }, "");
}

/// Ends the edit: with `save`, a changed, non-empty title is sent. Nothing when none is open.
pub fn finish_rename(host: &mut dyn SessionMenuHost, save: bool) {
    if host.menus().renaming().is_none() {
        return;
    }
    let draft = host.menus().edit.text.clone();
    let now = host.clock();
    let event = if save { Event::Submit { draft: &draft, now } } else { Event::Cancel };
    dispatch(host, event, "");
}

/// Runs `event` through the rename machine and carries out what it asks.
pub(crate) fn dispatch(host: &mut dyn SessionMenuHost, event: Event<'_>, why: &str) {
    let waiting = match &host.menus().save {
        Save::Saving { session, optimistic, .. } => Some((session.clone(), optimistic.clone())),
        _ => None,
    };
    let effects = {
        let m = host.menus_mut();
        step(&mut m.seq, &mut m.rename, &mut m.save, event)
    };
    for effect in effects {
        match effect {
            Effect::Arm(title) => {
                let mut edit = TextEdit::new(title);
                // The caret sits at the start, so a long title shows from its first word.
                edit.anchor = edit.text.len();
                edit.focus = 0;
                edit.focused = true;
                host.menus_mut().edit = edit;
                host.on_rename_focus_taken();
            }
            Effect::Disarm => host.menus_mut().edit.focused = false,
            Effect::Remember { session, server, title } => {
                let _ = on_server(host, &server, |h| {
                    if h.session(&session).is_some() {
                        if let Some(sessions) = h.sessions_mut()
                            && let Some(info) = sessions.iter_mut().find(|s| s.id == session)
                        {
                            info.title = title;
                        }
                        h.save_tabs();
                    }
                });
            }
            Effect::Save { session, server, title } => {
                // A story's write goes nowhere and a build that can't write sends none: each
                // answers at once.
                let (sent, current) = on_server(host, &server, |h| {
                    let sent = h.write_server(Write::Rename { session: session.clone(), title: title.clone() });
                    (sent, h.session(&session).map(|i| i.title.clone()))
                })
                .unwrap_or((false, None));
                if host.stage_active() || !sent {
                    let why = if sent { why } else { opencode_i18n::t!("rune.send.disconnected") };
                    return dispatch(
                        host,
                        Event::Replied { session: &session, server: &server, title: &title, ok: sent, current: current.as_deref() },
                        why,
                    );
                }
            }
            // `poll_rename` keeps the save's deadline.
            Effect::WakeAt(_) => {}
            // A refusal in the server's words has its own toast (`from_server`, in
            // `session_write_replied`); this says the rest: not sent, or no answer in time.
            Effect::Failed if why.is_empty() => {
                host.menus_mut().gave_up = waiting.clone();
                host.toast_fail(opencode_i18n::t!("rune.toast.renameFailed"), opencode_i18n::t!("rune.toast.renameNoAnswer"));
            }
            Effect::Failed if why == opencode_i18n::t!("rune.send.disconnected") => {
                host.toast_fail(opencode_i18n::t!("rune.toast.renameFailed"), why);
            }
            Effect::Failed => {}
        }
    }
}

/// Desktop's rename from a dialog or a palette command: the trimmed title, unless it's empty
/// or the same, goes to `session.update`. The list and the tab show it at once; the server's
/// event confirms it.
pub fn rename_session(host: &mut dyn SessionMenuHost, id: &str, title: &str) -> bool {
    let title = title.trim();
    let Some(current) = host.session(id).map(|s| s.title.as_str()) else {
        return false;
    };
    if title.is_empty() || title == current {
        return false;
    }
    if let Some(sessions) = host.sessions_mut()
        && let Some(info) = sessions.iter_mut().find(|s| s.id == id)
    {
        info.title = title.to_owned();
    }
    host.save_tabs();
    host.write_server(Write::Rename { session: id.to_owned(), title: title.to_owned() });
    true
}

/// The field's focus and the save's deadline: focus arms the edit, and losing it afterwards
/// is a click away (Home's row cancels, the tab and the header save, as Desktop's blur
/// handlers do).
pub fn poll_rename(host: &mut dyn SessionMenuHost, now: f64, tx: &mut Tx) -> bool {
    let before = host.menus().renaming();
    let focused = host.menus().edit.focused;
    let arming = matches!(host.menus().rename, Edit::Arming { .. });
    let editing = matches!(host.menus().rename, Edit::Editing { .. });
    if arming && focused {
        dispatch(host, Event::Armed, "");
    } else if editing && !focused {
        let draft = host.menus().edit.text.clone();
        dispatch(host, Event::Blurred { draft: &draft, now }, "");
    }
    if let Save::Saving { session, server, due, .. } = &host.menus().save {
        let (session, server, due) = (session.clone(), server.clone(), *due);
        // Only the deadline needs the title: another server is checked out just for it.
        if now + 1e-9 >= due {
            let title = on_server(host, &server, |h| h.session(&session).map(|i| i.title.clone())).flatten();
            dispatch(host, Event::Tick { now, current: title.as_deref() }, "");
        }
        if matches!(host.menus().save, Save::Saving { .. }) {
            tx.wake_at(due);
        }
    }
    let dialog = crate::rename_dialog::poll(host, now, tx);
    before != host.menus().renaming() || dialog
}
