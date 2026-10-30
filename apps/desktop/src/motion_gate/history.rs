//! Motion-gate scenario: older messages land above the top of the timeline while you read
//! there (`session_pages::load_older`), and nothing on screen moves.

use super::{Run, SID};
use crate::session_pages::{Key, Page};

fn older_page(tag: &str) -> Page {
    let data: Vec<serde_json::Value> = (0..8).map(|i| if i % 2 == 0 {
        serde_json::json!({"id": format!("msg_0older{tag}{i}"), "type": "user", "time": {"created": 1 + i}, "text": format!("Earlier prompt {i}: where does the reconnect start?")})
    } else {
        serde_json::json!({"id": format!("msg_0older{tag}{i}"), "type": "assistant", "time": {"created": 1 + i, "completed": 2 + i}, "finish": "stop",
            "content": [{"type": "text", "text": "It starts in the socket's close handler, which schedules the first retry. ".repeat(4)}]})
    }).collect();
    let messages = opencode_kit::model::parse_messages(&serde_json::json!({"data": data}));
    Page { key: Key { id: SID.into(), updated: 0 }, messages, cursor: Some(format!("before-{tag}")), cached: false, history: true, images: vec![], bytes: 0 }
}

/// From the top (reached unrecorded), a page lands as a scroll comes to rest, and another at rest.
pub fn older(r: &mut Run) {
    r.rt.state.pages.set_older(SID, Some("first".into()));
    let scroll = format!("scroll-{SID}");
    let top = |r: &Run| {
        let rect = |key: &str| r.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.rect[1]);
        rect("timeline") >= rect(&scroll)
    };
    for _ in 0..40 {
        if top(r) { break; }
        r.scroll_up(4000.0);
    }
    assert!(top(r), "at the top");
    // Frame 0 is the settled top the first action changes.
    r.rec = super::recording(r.circ.size());
    r.play(0.1);
    r.swipe_up(600.0, 0.05);
    r.act("older page lands while scrolling", |rt| rt.update(|s, tx| s.show_page(&older_page("a"), tx)));
    r.play(0.8);
    let again = older_page("b");
    r.act("older page lands at rest", |rt| rt.update(move |s, tx| s.show_page(&again, tx)));
    r.play(0.8);
}
