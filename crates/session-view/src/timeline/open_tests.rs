//! Opening a long history must only construct the viewport's rows, in either host app.
use super::*;
use crate::{fixture::SESSION_ID, story::{Host, view}};
use rune::{Env, Runtime};

#[test]
fn opening_thousands_of_messages_builds_only_visible_rows() {
    let messages = (0..13_222).map(|i| Message::User {
        id: format!("open_{i:05}"), text: format!("Prompt {i}: inspect the next boundary."),
    }).collect();
    let host = Host { talk: std::cell::RefCell::new(None), ..Host::default() };
    let mut rt = Runtime::new(host, view, (1280.0, 820.0), Env::new(true, 2.0));
    rt.frame(0.0);
    timeline_reset_row_builds();
    rt.update(|h, tx| {
        *h.talk.borrow_mut() = Some(Transcript { session: SESSION_ID.into(), messages });
        tx.settle();
    });
    rt.frame(0.0);
    // Includes both estimate/measure passes and the viewport's overscan.
    assert!(timeline_row_builds() <= 64, "built {} rows on open", timeline_row_builds());
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Prompt 13221: inspect the next boundary.")), "the newest message must be mounted on the first frame");
    assert_eq!(rt.scroll_at_bottom(&Host::scroll_key()), Some(true));
}
