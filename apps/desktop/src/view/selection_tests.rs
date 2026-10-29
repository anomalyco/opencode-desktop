//! Selecting and copying timeline text: a drag across blocks and messages, a word in a tool
//! row, and the composer keeping its own Cmd+A and Cmd+C.

use rune::clipboard::{self, MemoryClipboard};
use rune::{ElementInfo, Env, Key, KeyEvent, Mods, Runtime, TextEdit};

use crate::App;

pub(crate) const CODE: &str = "const QUIET_MS = 25_000";

/// The fixture's open session, finished, tall enough that every row is on screen, with a
/// memory pasteboard.
pub(crate) fn session(draft: &str) -> (Runtime<App>, MemoryClipboard) {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    app.composer = TextEdit::new(draft);
    app.composer.focused = true;
    app.reduced_motion = true;
    let mut rt = Runtime::new(app, super::app, (1280.0, 2400.0), Env::new(true, 1.0));
    rt.on_key = Some(crate::on_key);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    settle(&mut rt);
    (rt, board)
}

pub(crate) fn settle(rt: &mut Runtime<App>) {
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
    }
}

pub(crate) fn find(rt: &Runtime<App>, f: impl Fn(&ElementInfo) -> bool) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && f(e)).expect("element on screen")
}

/// The text element whose words start with `start`.
pub(crate) fn text(rt: &Runtime<App>, start: &str) -> ElementInfo {
    find(rt, |e| e.text.as_deref().is_some_and(|t| t.starts_with(start)))
}

/// A point on `e`'s first line, a fraction `fx` of the way across.
pub(crate) fn first_line(e: &ElementInfo, fx: f32) -> (f32, f32) {
    (e.rect[0] + 1.0 + (e.rect[2] - 2.0) * fx, e.rect[1] + 8.0)
}

/// The end of `e`'s last line.
pub(crate) fn last_line(e: &ElementInfo) -> (f32, f32) {
    (e.rect[0] + e.rect[2] - 1.0, e.rect[1] + e.rect[3] - 6.0)
}

pub(crate) fn drag(rt: &mut Runtime<App>, from: (f32, f32), to: (f32, f32)) {
    rt.pointer_move(from.0, from.1);
    rt.pointer_down(from.0, from.1);
    for k in 1..=8 {
        let t = k as f32 / 8.0;
        rt.pointer_move(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        rt.frame(1.0 / 120.0);
    }
    rt.pointer_up(to.0, to.1);
    rt.frame(1.0 / 120.0);
}

pub(crate) fn click(rt: &mut Runtime<App>, at: (f32, f32)) {
    rt.pointer_move(at.0, at.1);
    rt.pointer_down(at.0, at.1);
    rt.pointer_up(at.0, at.1);
    rt.frame(1.0 / 120.0);
}

fn command(rt: &mut Runtime<App>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.0 / 120.0);
}

fn pasted(board: &MemoryClipboard) -> String {
    board.text().map(|t| t.to_string()).unwrap_or_default()
}

#[test]
fn a_drag_selects_from_a_paragraph_into_a_code_block_and_copies_what_is_drawn() {
    let (mut rt, board) = session("");
    let para = text(&rt, "Treat silence as failure");
    let code = text(&rt, CODE);
    drag(&mut rt, first_line(&para, 0.0), last_line(&code));
    command(&mut rt, 'c');
    let copied = pasted(&board);
    assert!(copied.starts_with("Treat silence as failure."), "{copied:?}");
    assert!(copied.contains(&format!("\n{CODE}\n")), "the code follows on its own line: {copied:?}");
    assert!(copied.trim_end().ends_with("})\n}"), "through the code's last line: {copied:?}");
    assert!(!copied.contains("```"), "no fences: {copied:?}");
}

#[test]
fn a_code_block_copies_its_code_verbatim() {
    let (mut rt, board) = session("");
    let code = text(&rt, CODE);
    let source = code.text.clone().unwrap();
    drag(&mut rt, (code.rect[0] + 0.5, code.rect[1] + 8.0), last_line(&code));
    command(&mut rt, 'c');
    assert_eq!(pasted(&board).trim_end(), source.trim_end());
    assert!(source.contains("\n  let timer = setTimeout(fire, QUIET_MS)\n"), "indentation kept: {source:?}");
}

#[test]
fn a_drag_runs_across_messages_and_list_markers_stay_behind() {
    let (mut rt, board) = session("");
    let prompt = text(&rt, "The event stream drops");
    let bullet = text(&rt, "The sidebar store ignores");
    drag(&mut rt, first_line(&prompt, 0.0), last_line(&bullet));
    command(&mut rt, 'c');
    let copied = pasted(&board);
    assert!(copied.starts_with("The event stream drops"), "{copied:?}");
    assert!(copied.contains("\nI'll start with how the client"), "through the reply: {copied:?}");
    assert!(copied.contains("\nWhat's happening\n"), "into the next message: {copied:?}");
    assert!(copied.contains("\n`backoff.ts` resets") || copied.contains("\nbackoff.ts resets"), "{copied:?}");
    assert!(!copied.contains('•') && !copied.contains("\n- "), "no list markers: {copied:?}");
    assert!(copied.trim_end().ends_with("first list fetch."), "{copied:?}");
}

#[test]
fn a_double_click_in_a_tool_row_selects_a_word_and_leaves_the_row_open() {
    let (mut rt, _) = session("");
    let group = rt.state.expanded.len();
    let trigger = find(&rt, |e| e.key.as_deref() == Some("trigger") && e.label.as_deref().is_some_and(|l| l.contains("Grep")));
    click(&mut rt, first_line(&trigger, 0.1));
    assert_eq!(rt.state.expanded.len(), group + 1, "a click on the disclosure opens it");
    settle(&mut rt);
    let steps = format!("{}/steps/", trigger.path.trim_end_matches("/trigger"));
    let tool = |rt: &Runtime<App>, name: &str| find(rt, |e| e.path.starts_with(&steps) && e.text.as_deref() == Some(name));
    let name = tool(&rt, "Read");
    let word = (name.rect[0] + name.rect[2] / 2.0, name.rect[1] + name.rect[3] / 2.0);
    click(&mut rt, word);
    click(&mut rt, word);
    assert_eq!(rt.selected_text().as_deref(), Some("Read"));
    assert_eq!(rt.state.expanded.len(), group + 1, "the group stays open");
    // A drag across two tools selects them without closing the group.
    let grep = tool(&rt, "Grep");
    drag(&mut rt, (grep.rect[0] + 1.0, grep.rect[1] + grep.rect[3] / 2.0), (name.rect[0] + name.rect[2] - 1.0, word.1));
    assert_eq!(rt.selected_text().as_deref(), Some("Grep\tEventSource|heartbeat\nRead"));
    assert_eq!(rt.state.expanded.len(), group + 1);
}

#[test]
fn the_composer_keeps_its_own_select_all_and_copy() {
    let (mut rt, board) = session("half a thought");
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    assert_eq!(pasted(&board), "half a thought");
    assert_eq!(rt.selected_text(), None, "Cmd+A in the composer selects nothing in the timeline");

    // Text selected in the timeline copies; a click back in the composer makes it its own again.
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.3));
    command(&mut rt, 'c');
    assert!(pasted(&board).starts_with("Treat"), "{:?}", pasted(&board));
    let prompt = find(&rt, |e| e.key.as_deref() == Some("prompt"));
    click(&mut rt, first_line(&prompt, 0.5));
    assert!(rt.state.composer.focused);
    assert_eq!(rt.selected_text(), None, "a press in the composer clears the timeline's selection");
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    assert_eq!(pasted(&board), "half a thought");
}

#[test]
fn a_link_still_opens_on_a_click() {
    let (mut rt, _) = session("");
    rt.update(|s, _| {
        let t = s.transcripts.get_mut(crate::fixture_ids::MAIN).unwrap();
        let Some(opencode_kit::model::Message::Assistant(a)) = t.messages.iter_mut().rev().find(|m| matches!(m, opencode_kit::model::Message::Assistant(_))) else { unreachable!() };
        a.parts.push(opencode_kit::model::Part::Text { text: "[the reconnect docs](https://example.com/docs)".into() });
    });
    settle(&mut rt);
    let link = text(&rt, "the reconnect docs");
    click(&mut rt, (link.rect[0] + 12.0, link.rect[1] + link.rect[3] / 2.0));
    assert_eq!(rt.state.opened, ["https://example.com/docs"]);
    assert_eq!(rt.selected_text(), None);
}

#[test]
fn a_selection_survives_streaming_below_it() {
    let (mut rt, _) = session("");
    let para = text(&rt, "Treat silence as failure");
    drag(&mut rt, first_line(&para, 0.0), first_line(&para, 0.4));
    let before = rt.selected_text().expect("selected");
    rt.update(|s, _| {
        s.apply_event("session.prompted", &serde_json::json!({"sessionID": crate::fixture_ids::MAIN, "messageID": "msg_new", "text": "One more thing"}));
    });
    settle(&mut rt);
    assert_eq!(rt.selected_text(), Some(before));
}

/// The fixture's open session in a short window, pinned to its end, so its first rows are
/// scrolled out and never built.
fn short_session() -> (Runtime<App>, MemoryClipboard) {
    short_session_with(false)
}

/// `capsule`: a subagent runs, so its capsule floats over the timeline's bottom edge.
fn short_session_with(capsule: bool) -> (Runtime<App>, MemoryClipboard) {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    if capsule {
        let parent = crate::stories::sid(&app);
        app.sessions.push(opencode_kit::model::SessionInfo { id: "sub".into(), parent: Some(parent), ..Default::default() });
        app.running.insert("sub".into());
    }
    app.reduced_motion = true;
    let mut rt = Runtime::new(app, super::app, (1280.0, 520.0), Env::new(true, 1.0));
    rt.on_key = Some(crate::on_key);
    let board = clipboard::memory();
    rt.use_clipboard(&board);
    settle(&mut rt);
    (rt, board)
}

const PROMPT_ROW: &str = "u-msg_001a162c32a80001fixture00";

/// The timeline's last reply text on screen, and the key of the row holding it.
fn last_text(rt: &Runtime<App>) -> (ElementInfo, String) {
    let texts = rt.elements().into_iter().filter(|e| !e.exiting && e.visible[3] > 0.0 && e.text.as_deref().is_some_and(|t| !t.trim().is_empty()) && e.path.contains("/timeline/t-msg_"));
    let last = texts.max_by(|a, b| (a.rect[1] + a.rect[3]).total_cmp(&(b.rect[1] + b.rect[3]))).expect("reply text");
    let row = last.path.split("/timeline/").nth(1).and_then(|r| r.split('/').next()).expect("row").to_owned();
    (last, row)
}

fn on_screen(rt: &Runtime<App>, start: &str) -> bool {
    rt.elements().iter().any(|e| !e.exiting && e.visible[3] > 0.0 && e.text.as_deref().is_some_and(|t| t.starts_with(start)))
}

#[test]
fn a_selection_from_a_row_scrolled_out_copies_every_row_to_the_last() {
    scrolled_out_selection(false);
}

#[test]
fn under_the_capsule_a_selection_still_copies_every_row() {
    scrolled_out_selection(true);
}

fn scrolled_out_selection(capsule: bool) {
    // Every row built: the drag from the prompt to the last line copies what all rows hold.
    let (mut tall, board) = session("");
    tall.update(|s, _| s.composer.focused = false);
    let (end, _) = last_text(&tall);
    let from = first_line(&text(&tall, "The event stream drops"), 0.0);
    drag(&mut tall, from, last_line(&end));
    command(&mut tall, 'c');
    let whole = pasted(&board);
    assert!(whole.starts_with("The event stream drops") && whole.contains(CODE), "{whole:?}");

    // The same selection made a screen at a time: pressed at the prompt, scrolled to the end,
    // then Shift+clicked there, with the prompt's row scrolled out.
    let (mut rt, board) = short_session_with(capsule);
    let (_, last_row) = last_text(&rt);
    rt.update(|_, tx| tx.scroll_into_view(PROMPT_ROW));
    settle(&mut rt);
    let from = first_line(&text(&rt, "The event stream drops"), 0.0);
    click(&mut rt, from);
    rt.update(move |_, tx| tx.scroll_into_view(last_row));
    settle(&mut rt);
    // At the very end, so the last line stands clear of the capsule's lane.
    if capsule {
        rt.update(|s, tx| tx.scroll_to_bottom_instant(format!("scroll-{}", crate::stories::sid(s))));
        settle(&mut rt);
    }
    assert!(!on_screen(&rt, "The event stream drops"), "the prompt's row is scrolled out");
    let (end, _) = last_text(&rt);
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, last_line(&end));
    rt.physical_input(rune::PhysicalInput::Modifiers(Mods::default()));
    command(&mut rt, 'c');
    assert_eq!(pasted(&board), whole, "every row between the ends, as when all were built");
}

#[test]
fn select_all_copies_rows_never_built() {
    let (mut rt, board) = short_session();
    // Thirty earlier prompts arrive above the timeline's end, where it stays: most are never built.
    rt.update(|s, _| {
        let t = s.transcripts.get_mut(crate::fixture_ids::MAIN).unwrap();
        for n in (0..30).rev() {
            t.messages.insert(0, opencode_kit::model::Message::User { id: format!("msg_000early{n:03}"), text: format!("Earlier prompt {n}") });
        }
    });
    // Which of them any frame builds.
    let mut built = std::collections::HashSet::new();
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        built.extend(rt.elements().into_iter().filter_map(|e| e.text.filter(|t| t.starts_with("Earlier prompt"))));
    }
    let never: Vec<String> = (0..30).map(|n| format!("Earlier prompt {n}")).filter(|t| !built.contains(t)).collect();
    assert!(never.len() >= 5, "some earlier rows are never built: {never:?}");
    let (end, _) = last_text(&rt);
    let end_words = end.text.clone().unwrap();
    click(&mut rt, first_line(&end, 0.5));
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    let copied = pasted(&board);
    let at = |w: &str| copied.find(w).unwrap_or_else(|| panic!("{w:?} not in {copied:?}"));
    let earlier: Vec<String> = (0..30).map(|n| format!("Earlier prompt {n}")).collect();
    assert!(copied.starts_with(&format!("{}\n", earlier.join("\n"))), "every earlier row, built or not, in order: {copied:?}");
    assert!(never.iter().all(|t| copied.contains(&format!("{t}\n"))));
    assert!(at("The event stream drops") < at(CODE));
    assert!(copied.contains(end_words.trim_end()), "to the last reply: {copied:?}");
}
