//! The Quote button over a timeline selection, its card in the composer, and what a send with
//! quotes carries.

use crate::fixture_ids::{MAIN as SESSION};
use rune::{ElementInfo, Key, KeyEvent, Mods, PhysicalInput, Runtime, Tx};

use opencode_kit::client::{Server, Write};
use opencode_kit::model::{self, Message, Quote};

use crate::view::selection_tests::{click, drag, find, first_line, last_line, session, settle, text};
use crate::{App, Route};


fn quoting(draft: &str) -> Runtime<App> {
    let (mut rt, _) = session(draft);
    rt.poll = Some(crate::poll);
    rt
}

fn shown(rt: &Runtime<App>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key))
}

/// Long enough for the selection to settle and the button to measure itself.
fn wait(rt: &mut Runtime<App>) {
    for _ in 0..40 {
        rt.frame(1.0 / 120.0);
    }
}

fn centre(e: &ElementInfo) -> (f32, f32) {
    (e.rect[0] + e.rect[2] / 2.0, e.rect[1] + e.rect[3] / 2.0)
}

fn escape(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 120.0);
}

#[test]
fn quote_text_trims_lines_and_collapses_blank_runs() {
    use crate::quote::{MAX_LEN, quote_text};
    assert_eq!(quote_text("  one  \n\n\n\ntwo \n"), ("one\n\ntwo".into(), false));
    let (cut, truncated) = quote_text(&"x".repeat(MAX_LEN + 5));
    assert!(truncated && cut.chars().count() == MAX_LEN);
}

#[test]
fn the_button_waits_for_the_release_and_sits_above_a_short_selection() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    let (from, to) = (first_line(&para, 0.0), first_line(&para, 0.4));
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    for k in 1..=8 {
        rt.pointer_move(from.0 + (to.0 - from.0) * k as f32 / 8.0, to.1);
        rt.frame(1.0 / 120.0);
    }
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_none(), "not while the press holds");
    rt.pointer_up(to.0, to.1);
    rt.frame(1.0 / 120.0);
    assert!(shown(&rt, "quote-toolbar").is_none(), "not until the selection settles");
    wait(&mut rt);
    let bar = shown(&rt, "quote-toolbar").expect("the button over the selection");
    let bottom = bar.rect[1] + bar.rect[3];
    assert!((bottom - (para.rect[1] - 8.0)).abs() < 1.5, "8 above the first line: {:?} over {:?}", bar.rect, para.rect);
    let mid = (from.0 + to.0) / 2.0;
    assert!((bar.rect[0] + bar.rect[2] / 2.0 - mid).abs() < 4.0, "centred on the selection: {:?}, {mid}", bar.rect);
    let button = shown(&rt, "quote-add").unwrap();
    assert_eq!((button.rect[3], button.label.as_deref()), (24.0, Some("Quote")));
}

#[test]
fn a_tall_selection_puts_the_button_below_its_last_line() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    let end = text(&rt, crate::view::selection_tests::CODE);
    drag(&mut rt, first_line(&para, 0.0), last_line(&end));
    wait(&mut rt);
    let bar = shown(&rt, "quote-toolbar").expect("the button");
    let last = end.rect[1] + end.rect[3];
    assert!(bar.rect[1] > last && bar.rect[1] - last < 16.0, "under the last line: {:?} below {last}", bar.rect);
}

#[test]
fn quote_puts_a_card_in_the_composer_and_focuses_the_prompt() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.4));
    wait(&mut rt);
    let selected = rt.selected_text().expect("selected");
    let button = shown(&rt, "quote-add").unwrap();
    click(&mut rt, centre(&button));
    settle(&mut rt);
    let items = &rt.state.quote.items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].quote, Quote { text: crate::quote::quote_text(&selected).0, comment: None });
    assert_eq!(rt.selected_text(), None, "the selection goes");
    assert!(rt.state.composer.focused);
    assert!(shown(&rt, "quote-toolbar").is_none());
    let card = shown(&rt, "quote-chip-1").expect("the card in the composer");
    let composer = shown(&rt, "composer").unwrap();
    assert!(card.rect[1] >= composer.rect[1] && card.rect[1] < composer.rect[1] + 16.0, "at the composer's top: {:?}", card.rect);

    // A second quote adds a second card; × takes one out.
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.5), first_line(&para, 0.8));
    wait(&mut rt);
    let button = shown(&rt, "quote-add").expect("offered again");
    click(&mut rt, centre(&button));
    settle(&mut rt);
    assert_eq!(rt.state.quote.items.len(), 2);
    let card = shown(&rt, "quote-chip-2").unwrap();
    rt.pointer_move(card.rect[0] + 20.0, card.rect[1] + 10.0);
    settle(&mut rt);
    let remove = find(&rt, |e| e.path.contains("quote-chip-2") && e.key.as_deref() == Some("remove"));
    click(&mut rt, centre(&remove));
    assert_eq!(rt.state.quote.items.iter().map(|q| q.key).collect::<Vec<_>>(), [1]);
}

#[test]
fn alt_click_opens_the_comment_editor_and_enter_keeps_the_comment() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.4));
    wait(&mut rt);
    let button = shown(&rt, "quote-add").unwrap();
    rt.physical_input(PhysicalInput::Modifiers(Mods { alt: true, ..Mods::default() }));
    click(&mut rt, centre(&button));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    settle(&mut rt);
    assert_eq!(rt.state.quote.editing, Some(1));
    assert!(!rt.state.composer.focused && rt.state.quote.draft.focused);
    let editor = shown(&rt, "quote-comment").expect("the editor");
    let card = shown(&rt, "quote-chip-1").unwrap();
    assert!(editor.rect[1] + editor.rect[3] <= card.rect[1] - 7.0, "above its card: {:?} {:?}", editor.rect, card.rect);
    rt.update(|s, _| s.quote.draft.text = "why this?".into());
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    settle(&mut rt);
    assert_eq!(rt.state.quote.editing, None);
    assert_eq!(rt.state.quote.items[0].quote.comment.as_deref(), Some("why this?"));
    assert!(rt.state.composer.focused);
}

#[test]
fn escape_hides_the_button_until_the_selection_changes_and_a_click_elsewhere_closes_it() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.4));
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_some());
    escape(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_none());
    assert!(rt.selected_text().is_some(), "Escape leaves the selection");
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_none(), "the same selection stays dismissed");

    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.6));
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_some(), "a new selection offers it again");
    let other = text(&rt, "The event stream drops");
    click(&mut rt, (other.rect[0] + other.rect[2] - 2.0, other.rect[1] + 8.0));
    wait(&mut rt);
    assert_eq!(rt.selected_text(), None);
    assert!(shown(&rt, "quote-toolbar").is_none(), "a click elsewhere clears the selection and the button");
}

#[test]
fn typing_in_the_composer_closes_the_button() {
    let mut rt = quoting("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.4));
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_some());
    rt.update(|s, _| s.composer.focused = true);
    rt.frame(1.0 / 120.0);
    rt.key(KeyEvent { key: Key::Text("x".into()), mods: Mods::default() });
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_none());
}

#[test]
fn a_selection_in_the_composer_never_shows_it() {
    let mut rt = quoting("half a thought about the reconnect loop");
    let prompt = find(&rt, |e| e.key.as_deref() == Some("prompt"));
    drag(&mut rt, first_line(&prompt, 0.0), first_line(&prompt, 0.6));
    wait(&mut rt);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    wait(&mut rt);
    assert!(shown(&rt, "quote-toolbar").is_none());
    assert!(!rt.state.quote.offered());
}

fn live(typed: &str, quotes: &[Quote]) -> (App, std::sync::mpsc::Receiver<Write>) {
    let (w, writes) = std::sync::mpsc::channel();
    let mut app = App::empty();
    app.source = crate::Source::Live { server: Server::new("http://127.0.0.1:9"), requests: None, writes: Some(w), allow_send: true };
    app.route = Route::Session("ses_a".into());
    app.composer = rune::TextEdit::new(typed);
    for q in quotes {
        app.quote.push(q.clone());
    }
    (app, writes)
}

#[test]
fn a_send_carries_its_quotes_as_desktop_does() {
    let quote = Quote { text: "one\ntwo".into(), comment: Some("why?".into()) };
    let (mut app, writes) = live("look", std::slice::from_ref(&quote));
    app.send(&mut Tx::default());
    let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no prompt") };
    assert_eq!(
        prompt.text,
        "look\nThe user quoted the following from earlier in the conversation:\n> one\n> two\n\nThe user made the following comment regarding this quote: why?"
    );
    assert_eq!(
        prompt.body()["metadata"],
        serde_json::json!({"displayText": "look", "comments": [{"type": "quote", "text": "one\ntwo", "comment": "why?"}], "attachments": []})
    );
    assert!(!app.quote.any(), "the composer lets them go");
    let id = prompt.id.unwrap();
    assert_eq!(model::user_quotes(&id).as_ref(), [quote]);
    let t = &app.transcripts["ses_a"];
    assert!(matches!(t.messages.last(), Some(Message::User { text, .. }) if text == "look"), "the echo shows the typed words");

    // Quotes alone are enough to send.
    let only = Quote { text: "alone".into(), comment: None };
    let (mut app, writes) = live("", std::slice::from_ref(&only));
    app.send(&mut Tx::default());
    let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("quotes alone send") };
    assert_eq!(prompt.text, only.prompt_text());
    assert_eq!(prompt.body()["metadata"]["comments"], serde_json::json!([{"type": "quote", "text": "alone"}]));
}

#[test]
fn a_prompt_of_only_quotes_shows_its_cards() {
    let mut rt = quoting("");
    model::set_user_quotes("msg_quote_only", vec![Quote { text: "alone".into(), comment: None }]);
    rt.update(|s, _| {
        let t = s.transcripts.get_mut(SESSION).unwrap();
        t.messages.push(Message::User { id: "msg_quote_only".into(), text: String::new() });
    });
    settle(&mut rt);
    let card = find(&rt, |e| e.path.contains("u-msg_quote_only") && e.key.as_deref() == Some("quote-0"));
    assert!(card.rect[2] > 100.0, "{:?}", card.rect);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("text-msg_quote_only")), "no empty bubble");
    assert!(card.path.contains("/bubble/"), "the cards stand in for the bubble: {}", card.path);
    model::set_user_quotes("msg_quote_only", Vec::new());
}
