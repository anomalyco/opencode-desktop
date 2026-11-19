//! Storybook stories for Rewind (`Rewind to here`, `Rewound` toast with `Undo`, `/undo` and
//! `/redo`) and Fork (`Fork from here` in the message menu and the `Fork from message` dialog).

use rune::Play;
use rune_register::register;

use crate::stories::{ROW_KNOBS, STORY_LIST, Story, Trigger, replay, session_view, sid, stage, ticking_view};
use crate::{App, status_toast};

const REWIND_VARIANTS: &[(&str, usize)] = &[("Rewound", 0), ("Message menu", 1)];
const FORK_VARIANTS: &[(&str, usize)] = &[("Dialog", 0), ("Forked session", 1)];

const REWIND_KEYS: &[Trigger] = &[
    replay(rewind_open),
    Trigger { key: 'u', label: "Undo last prompt", run: |s, tx| { s.undo_last(tx); } },
    Trigger { key: 'd', label: "Redo all", run: |s, tx| { s.redo_revert(tx); } },
    Trigger { key: 'z', label: "Undo on toast", run: toast_undo },
];

const FORK_KEYS: &[Trigger] = &[
    replay(fork_open),
    Trigger { key: 'o', label: "Open fork dialog", run: |s, tx| { s.open_fork_dialog(tx); } },
    Trigger { key: 'f', label: "Fork from latest", run: fork_latest },
];

fn rewind_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    status_toast::dismiss(s);
    s.close_message_menu();
    let id = sid(s);
    s.set_session_revert(&id, None);
    match *s.fixture_run.variant {
        1 => {
            if let Some(last) = s.visible_user_ids(&id).pop() {
                tx.instant();
                s.toggle_message_menu(&id, &last, tx);
            }
        }
        _ => {
            tx.after(0.35, |s, tx| {
                s.undo_last(tx);
            });
        }
    }
}

fn toast_undo(s: &mut App, tx: &mut Play<App>) {
    let undo = s.dialogs.toast.current().and_then(|e| match &e.action {
        Some(status_toast::ToastAction::Custom(status_toast::DesktopToastAction::UndoRewind(u))) => Some(u.clone()),
        _ => None,
    });
    if let Some(undo) = undo {
        status_toast::dismiss(s);
        s.undo_rewind(undo, tx);
    }
}

fn fork_scene(s: &mut App) {
    stage(s, ticking_view);
}

fn fork_open(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.close_fork_dialog(false);
    match *s.fixture_run.variant {
        1 => fork_latest(s, tx),
        _ => {
            s.open_fork_dialog(tx);
        }
    }
}

fn fork_latest(s: &mut App, tx: &mut Play<App>) {
    let id = sid(s);
    if let Some(last) = s.visible_user_ids(&id).pop() {
        s.fork_from(&id, &last, tx);
    }
}

register!(STORY_LIST: Story = [
    Story { path: "Timeline/Rewind", scene: |s| stage(s, session_view), knobs: ROW_KNOBS, keys: REWIND_KEYS, variants: REWIND_VARIANTS },
    Story { path: "Timeline/Fork", scene: fork_scene, knobs: &[], keys: FORK_KEYS, variants: FORK_VARIANTS },
]);
