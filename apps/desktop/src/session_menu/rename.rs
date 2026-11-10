//! A session's title edited in place, as Desktop's tab rename machine runs it
//! (`shell/titlebar/tab-rename-machine.ts`): `Arming` makes the title editable while focus
//! settles (the closing menu or the double-click still move it, so a blur then doesn't end the
//! edit), `Editing` ends on Enter, Escape or a blur. A changed title saves; while saves overlap
//! only the latest decides, and one that fails or takes longer than 15 s puts back the title the
//! server last accepted (`confirmed`) and says so.

use rune::machine::{Attempt, Effects, Seq};

use super::{Spot, Target};

/// `TAB_RENAME_SAVE_TIMEOUT_MS`.
pub(crate) const SAVE_TIMEOUT: f64 = 15.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Edit {
    #[default]
    Idle,
    /// The field shows; it hasn't had focus yet.
    Arming { target: Target, initial: String },
    Editing { target: Target, initial: String },
}

/// The title's save (`TabRenameSave`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Save {
    #[default]
    Idle,
    Saving { attempt: Attempt, session: String, optimistic: String, confirmed: String, due: f64 },
}

pub(crate) enum Event<'a> {
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
    /// The server answered a save of `session` to `title`.
    Replied { session: &'a str, title: &'a str, ok: bool, current: Option<&'a str> },
    Tick { now: f64, current: Option<&'a str> },
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    /// The field opens on `title`, all of it selected, and takes focus.
    Arm(String),
    /// The field goes.
    Disarm,
    /// Show `title` for `session` now (`session.remember`).
    Remember { session: String, title: String },
    /// Send the save (`save.start`).
    Save { session: String, title: String },
    WakeAt(f64),
    Failed,
}

pub(crate) fn step(seq: &mut Seq, edit: &mut Edit, save: &mut Save, event: Event<'_>) -> Effects<Effect> {
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
        Event::Replied { session, title, ok, current } => match save {
            Save::Saving { session: s, optimistic, .. } if s == session && optimistic == title => {
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
    let (Edit::Arming { target, initial } | Edit::Editing { target, initial }) = std::mem::take(edit) else { return Effects::none() };
    let next = draft.trim();
    if next.is_empty() || next == initial {
        return Effects::one(Effect::Disarm);
    }
    // An overlapping save keeps the last title the server accepted, never the previous optimistic one.
    let confirmed = match save {
        Save::Saving { session, confirmed, .. } if *session == target.id => confirmed.clone(),
        _ => initial,
    };
    let due = now + SAVE_TIMEOUT;
    *save = Save::Saving { attempt: seq.next(), session: target.id.clone(), optimistic: next.to_owned(), confirmed, due };
    Effects::Many(vec![
        Effect::Disarm,
        Effect::Remember { session: target.id.clone(), title: next.to_owned() },
        Effect::Save { session: target.id, title: next.to_owned() },
        Effect::WakeAt(due),
    ])
}

/// A failed or timed-out latest save puts back the accepted title, unless the server pushed
/// another meanwhile.
fn failed(save: &Save, ok: bool, current: Option<&str>) -> Effects<Effect> {
    let Save::Saving { session, optimistic, confirmed, .. } = save else { return Effects::none() };
    if ok {
        return Effects::none();
    }
    let restore = (current == Some(optimistic.as_str())).then(|| Effect::Remember { session: session.clone(), title: confirmed.clone() });
    match restore {
        Some(r) => Effects::two(r, Effect::Failed),
        None => Effects::one(Effect::Failed),
    }
}
