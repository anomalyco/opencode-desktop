use opencode_kit::model::{Attachment, Message};
use rune::Runtime;
use session_view::images::Dragged;
use session_view::images::menu::{COPY, MenuAt, OPEN, SAVE};

use super::menu;
use crate::{App, Route};

const DT: f64 = 1.0 / 120.0;

fn session() -> (Runtime<App>, String) {
    let app = App::fixture_session();
    let Route::Session { id: sid, .. } = app.route.clone() else { panic!("no open session") };
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt.wheel(640.0, 400.0, 520.0);
    for _ in 0..45 {
        rt.frame(DT);
    }
    (rt, sid)
}

fn ids(m: &crate::popup::Menu) -> Vec<String> {
    let native = m.native();
    [COPY, SAVE, OPEN, "copy", "fork", "revert", "selection-copy", "selection-quote"].into_iter().filter(|id| native.find(id).is_some()).map(str::to_owned).collect()
}

/// Kit right-clicked his own message and nothing happened: a right-click on the bubble opens
/// the menu its chevron opens, at the pointer.
#[test]
fn right_clicking_a_user_bubble_opens_its_message_menu() {
    let (mut rt, sid) = session();
    let last = rt.state.visible_user_ids(&sid).last().unwrap().clone();
    let bubble = rt
        .elements()
        .into_iter()
        .find(|e| !e.exiting && e.path.contains(&format!("u-{last}")) && e.key.as_deref() == Some("bubble"))
        .expect("the newest prompt's bubble");
    let (x, y) = (bubble.rect[0] + bubble.rect[2] / 2.0, bubble.rect[1] + bubble.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(DT);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    rt.frame(DT);
    let fork = opencode_i18n::t!("ui.message.forkMessage");
    let opened = match rt.popup_menu() {
        Some(native) => native.find("fork").is_some() && native.find("copy").is_some(),
        None => rt.elements().into_iter().any(|e| !e.exiting && e.text.as_deref() == Some(fork)),
    };
    assert!(opened, "the message's menu opens on a right-click");
}

#[test]
fn a_reply_offers_copy_message() {
    let (rt, sid) = session();
    let reply = rt.state.transcripts[&sid].messages.iter().rev().find_map(|m| match m {
        Message::Assistant(a) => Some(a.id.clone()),
        _ => None,
    });
    let at = MenuAt { session: Some(sid), message: reply, image: None };
    assert_eq!(ids(&menu(&rt.state, &at).expect("a reply has a menu")), ["copy"]);
}

/// An image in a message: its own items first, then the message's.
#[test]
fn an_image_in_a_message_leads_with_its_own_items() {
    let (rt, sid) = session();
    let last = rt.state.visible_user_ids(&sid).last().unwrap().clone();
    let image = Dragged::File(Attachment { mime: "image/png".into(), name: "a.png".into(), ..Default::default() });
    let at = MenuAt { session: Some(sid.clone()), message: Some(last), image: Some(image.clone()) };
    let got = ids(&menu(&rt.state, &at).unwrap());
    assert_eq!(&got[..3], [COPY, SAVE, OPEN]);
    assert!(got.contains(&"copy".to_owned()) && got.contains(&"fork".to_owned()), "{got:?}");
    let alone = MenuAt { session: Some(sid), message: None, image: Some(image) };
    assert_eq!(ids(&menu(&rt.state, &alone).unwrap()), [COPY, SAVE, OPEN]);
}
