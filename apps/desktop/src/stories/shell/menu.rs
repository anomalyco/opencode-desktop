//! The storybook's Popover › Menus: each menu that opens from a control over the timeline or
//! composer (the + menu, a message's ⋯ menu, the subagents list, the todo list), growing from
//! it as the Popover knobs say. O opens, C closes, R replays. Components › Toast shows a toast
//! arriving and leaving: S says something, F fails, D dismisses.

use opencode_kit::model::Message;
use rune::Play;

use crate::App;
use crate::popover::motion;
use crate::stories::{Story, Trigger, replay, sid};

const VARIANTS: &[(&str, usize)] = &[("+ menu", 0), ("⋯ menu", 1), ("Subagents list", 2), ("Todo list", 3), ("Right-click an image", 4)];

const KEYS: &[Trigger] = &[
    replay(start),
    Trigger { key: 'o', label: "Open", run: |s, tx| if !is_open(s) { set(s, true, tx) } },
    Trigger { key: 'c', label: "Close", run: |s, tx| if is_open(s) { set(s, false, tx) } },
];

fn last_prompt(s: &App) -> Option<String> {
    let t = s.transcripts.get(&sid(s))?;
    t.messages.iter().rev().find_map(|m| if let Message::User { id, .. } = m { Some(id.clone()) } else { None })
}

fn is_open(s: &App) -> bool {
    match *s.fixture_run.variant {
        0 => s.kernel.slots.get(&crate::add_menu::ADD_MENU).open(),
        1 => s.kernel.slots.get(&crate::rewind::REWIND).menu.is_some() && s.popup.is_open(),
        4 => s.popup.is_open(),
        2 => s.kernel.slots.get(&crate::subagents::SHELLS).activity.is_open(),
        _ => *s.kernel.slots.get(&crate::todos::TODO_OPEN),
    }
}

/// Opens or closes the variant's menu, with the motion its control uses.
fn set(s: &mut App, open: bool, tx: &mut Play<App>) {
    match *s.fixture_run.variant {
        0 if open => s.open_add_menu(tx),
        0 => s.close_add_menu(crate::add_menu::Close::Outside, tx),
        1 if open => {
            if let Some(mid) = last_prompt(s) {
                s.toggle_message_menu(&sid(s), &mid, tx);
            }
        }
        4 if open => right_click(s, tx),
        1 | 4 => crate::popup::dismiss(s, tx),
        2 => crate::subagents::shells::set_open(s, open),
        _ => *s.kernel.slots.get_mut(&crate::todos::TODO_OPEN) = open,
    }
    motion::toggled(&s.diag.knobs, tx, open);
}

/// A right-click on an image in the newest prompt, drawn as on Windows and Linux: the image's
/// items, then the message's.
fn right_click(s: &mut App, tx: &mut Play<App>) {
    use session_view::images::{Dragged, menu::MenuAt};
    let image = Dragged::File(opencode_kit::model::Attachment { mime: "image/png".into(), name: "screenshot.png".into(), ..Default::default() });
    let at = MenuAt { session: Some(sid(s)), message: last_prompt(s), image: Some(image) };
    crate::popup::context::show(s, at, (300.0, 120.0).into(), true, tx);
}

fn start(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    s.close_add_menu(crate::add_menu::Close::Outside, tx);
    s.close_message_menu();
    crate::subagents::shells::set_open(s, false);
    *s.kernel.slots.get_mut(&crate::todos::TODO_OPEN) = false;
    match *s.fixture_run.variant {
        2 => crate::stories::composer::capsule::run_thirty(s),
        3 => {
            let id = sid(s);            s.running.insert(id);
            crate::stories::composer::todo::set_step(s, 2);
        }
        _ => {}
    }
    tx.instant();
    tx.after(0.5, |s, tx| set(s, true, tx));
}

const TOAST_KEYS: &[Trigger] = &[
    replay(toast_start),
    Trigger { key: 's', label: "Say something", run: |s, _| crate::status_toast::say(s, "Copied the session link", "") },
    Trigger { key: 'f', label: "Fail", run: |s, _| crate::status_toast::fail(s, "Couldn't stop the session", "The server didn't answer in 5 s") },
    Trigger { key: 'd', label: "Dismiss", run: |s, _| { crate::status_toast::dismiss(s); } },
];

fn toast_start(s: &mut App, tx: &mut Play<App>) {
    s.finish_stream();
    crate::status_toast::dismiss(s);
    tx.instant();
    tx.after(0.5, |s, _| crate::status_toast::succeed(s, "Copied", "~/code/lumen"));
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story::session("Popover/Menus", KEYS).knobs(&[motion::GROUP]).variants(VARIANTS),
    Story::session("Components/Toast", TOAST_KEYS),
]);
