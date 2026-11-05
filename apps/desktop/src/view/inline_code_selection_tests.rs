//! Selecting inside inline code in a reply: a press in `code` anchors the selection there, and
//! a double-click selects the word under it, as in plain text (t1399).

use rune::{ElementInfo, Runtime};

use crate::App;
use crate::view::selection_tests::{click, session, settle, text};

const REPLY: &str = "Nothing urgent in the overnight digest. I acted on three items:\n\n\
- **Docker VM memory warning:** this is a Docker VM that tests use. Its memory has a hard cap of 4 GB, so it can't grow without limit.\n\
- **Tests skipping the pool:** the new alert caught a second team doing it, at 3:36 AM.\n\
- **Long build waits:** the waits of 30–40 minutes overnight were mostly the speed-test problems, which are fixed now.\n\n\
Still waiting on you:\n\
- **fseventsd on the devbox:** `ssh -t devbox sudo killall fseventsd`. That might also help the devbox's swap, which has been above 14 GB for 31 hours.\n\
- **Public ports on the rented Macs:** host-one and build-node still have SSH, Kerberos and screen sharing open to the internet.";

/// The fixture's session with `REPLY` as its last reply under a long, folded prompt whose
/// hidden lines run on under the reply, and the item holding the inline code.
fn reply() -> (Runtime<App>, ElementInfo) {
    let (mut rt, _) = session("");
    rt.update(|s, _| {
        s.composer.focused = false;
        let t = s.transcripts.get_mut(crate::fixture_ids::MAIN).unwrap();
        let Some(opencode_kit::model::Message::User { text, .. }) = t.messages.iter_mut().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. })) else { unreachable!() };
        *text = (1..=120).map(|k| format!("- {k}:32 AM alert: a long digest line folded under the first lines of the prompt\n")).collect();
        let Some(opencode_kit::model::Message::Assistant(a)) = t.messages.iter_mut().rev().find(|m| matches!(m, opencode_kit::model::Message::Assistant(_))) else { unreachable!() };
        a.parts.push(opencode_kit::model::Part::Text { text: REPLY.into() });
    });
    settle(&mut rt);
    let item = text(&rt, "fseventsd on the devbox");
    (rt, item)
}

/// Waits out the double-click interval, so the next press is a first click.
fn pause(rt: &mut Runtime<App>) {
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
    }
}

/// The word a double-click at `at` selects.
fn word_at(rt: &mut Runtime<App>, at: (f32, f32)) -> Option<String> {
    pause(rt);
    click(rt, at);
    click(rt, at);
    rt.selected_text()
}

/// A point on the item's first line over `word` of the inline code, found by double-clicking
/// across the line.
fn over(rt: &mut Runtime<App>, item: &ElementInfo, word: &str) -> (f32, f32) {
    let y = item.rect[1] + 10.0;
    let mut seen = Vec::new();
    let mut x = item.rect[0] + 2.0;
    while x < item.rect[0] + item.rect[2] {
        let got = word_at(rt, (x, y));
        if got.as_deref() == Some(word) {
            return (x, y);
        }
        seen.push((x as i32, got));
        x += 5.0;
    }
    panic!("no point on the first line selects {word:?}: {seen:?}");
}

#[test]
fn a_double_click_in_inline_code_selects_its_word() {
    let (mut rt, item) = reply();
    over(&mut rt, &item, "fseventsd");
    over(&mut rt, &item, "killall");
}

#[test]
fn a_drag_from_inline_code_starts_where_it_was_pressed() {
    let (mut rt, item) = reply();
    let from = over(&mut rt, &item, "killall");
    pause(&mut rt);
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    for k in 1..=6 {
        rt.pointer_move(from.0 + 4.0 * k as f32, from.1);
        rt.frame(1.0 / 120.0);
    }
    rt.pointer_up(from.0 + 24.0, from.1);
    rt.frame(1.0 / 120.0);
    let got = rt.selected_text().unwrap_or_default();
    assert!(!got.contains("Nothing urgent") && !got.contains("Still waiting"), "only from the press: {got:?}");
    assert!(!got.is_empty() && "killall fseventsd".contains(got.trim()), "inside the code: {got:?}");

    // And leftwards back over the code.
    pause(&mut rt);
    rt.pointer_down(from.0, from.1);
    for k in 1..=6 {
        rt.pointer_move(from.0 - 4.0 * k as f32, from.1);
        rt.frame(1.0 / 120.0);
    }
    rt.pointer_up(from.0 - 24.0, from.1);
    rt.frame(1.0 / 120.0);
    let got = rt.selected_text().unwrap_or_default();
    assert!(!got.contains("Nothing urgent") && !got.contains("Still waiting"), "only to the press: {got:?}");
    settle(&mut rt);
}

#[test]
fn the_quote_button_sits_over_a_drag_in_inline_code() {
    let (mut rt, item) = reply();
    rt.poll = Some(crate::poll);
    let from = over(&mut rt, &item, "killall");
    pause(&mut rt);
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    for k in 1..=6 {
        rt.pointer_move(from.0 + 4.0 * k as f32, from.1);
        rt.frame(1.0 / 120.0);
    }
    rt.pointer_up(from.0 + 24.0, from.1);
    for _ in 0..40 {
        rt.frame(1.0 / 120.0);
    }
    let bar = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("quote-toolbar")).expect("the Quote button");
    let bottom = bar.rect[1] + bar.rect[3];
    assert!(bottom < item.rect[1] && item.rect[1] - bottom < 14.0, "just above the code's line: {:?} over {:?}", bar.rect, item.rect);
    let mid = from.0 + 12.0;
    assert!((bar.rect[0] + bar.rect[2] / 2.0 - mid).abs() < 12.0, "centred on the drag: {:?}, {mid}", bar.rect);
}
