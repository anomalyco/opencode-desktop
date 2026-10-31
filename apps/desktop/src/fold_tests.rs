use rune::Runtime;

use crate::App;
use crate::fold::{self, OVER, SHOW};
use crate::fold_stories::LONG;
use crate::view::BUBBLE;

fn runtime(fold_user: bool) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.settings.fold_user = fold_user;
    let mut rt = app.into_runtime();
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new(LONG);
        s.submit(false, tx);
    });
    for _ in 0..240 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

fn rect(rt: &Runtime<App>, key: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.rect)
}

fn sent(rt: &Runtime<App>) -> String {
    let t = &rt.state.transcripts[crate::motion_gate::SID];
    t.messages.iter().rev().find(|m| matches!(m, opencode_kit::model::Message::User { .. })).unwrap().id().to_owned()
}

fn press(rt: &mut Runtime<App>) {
    let (x, y) = rt.find("fold-toggle").unwrap();
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
}

#[test]
fn a_bubble_wraps_at_the_column_desktop_gives_it() {
    // 64ch of a wide timeline, less the bubble's sides; 82% of a narrow one.
    assert_eq!(fold::text_width(1200.0, false), 565.0 - 24.0);
    assert!((fold::text_width(548.0, false) - (500.0 * 0.82 - 24.0)).abs() < 0.01);
    assert_eq!(fold::text_width(1200.0, true), 342.0 - 24.0);
}

#[test]
fn lines_count_hard_breaks_and_wrapping() {
    assert_eq!(fold::lines("one\ntwo\nthree", 541.0), 3);
    assert_eq!(fold::lines(LONG, 541.0), LONG.lines().count(), "past 12 hard lines it's long already");
    let wrapping: String = LONG.lines().take(10).collect::<Vec<_>>().join("\n");
    assert!(fold::lines(&wrapping, 300.0) > 10, "long lines wrap");
    let paragraph = "word ".repeat(400);
    assert!(fold::lines(&paragraph, 541.0) > OVER, "one long paragraph folds too");
    assert!(fold::lines(&paragraph, 300.0) > fold::lines(&paragraph, 541.0));
}

#[test]
fn a_long_prompt_lands_folded_and_opens() {
    let mut rt = runtime(true);
    let folded = rect(&rt, "fold").expect("the prompt folds");
    assert_eq!(folded[3], SHOW as f32 * BUBBLE.line_px());
    let toggle = rect(&rt, "fold-toggle").expect("Read more");
    assert!(toggle[1] >= folded[1] + folded[3], "Read more sits under the words");
    let words = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&format!("text-{}", sent(&rt)))).unwrap();
    assert_eq!(words.text.as_deref(), Some(LONG), "the folded words are all there to copy");

    let top = rect(&rt, &format!("u-{}", sent(&rt))).unwrap()[1];
    press(&mut rt);
    let open = rect(&rt, "fold").unwrap();
    assert!(open[3] > 2.0 * folded[3], "Read more shows every line");
    assert!((rect(&rt, &format!("u-{}", sent(&rt))).unwrap()[1] - top).abs() < 1.0, "the prompt's top holds while it opens");
}

#[test]
fn closing_a_prompt_read_past_its_top_holds_show_less_still() {
    let mut rt = runtime(true);
    press(&mut rt);
    let view = rect(&rt, &format!("scroll-{}", crate::motion_gate::SID)).unwrap();
    rt.wheel(view[0] + view[2] / 2.0, view[1] + view[3] / 2.0, -2000.0);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rect(&rt, &format!("u-{}", sent(&rt))).unwrap()[1] < view[1], "the prompt's top is read past");
    let at = rect(&rt, "fold-toggle").unwrap()[1];
    press(&mut rt);
    assert!(rect(&rt, &format!("text-{}", sent(&rt))).is_some());
    assert!((rect(&rt, "fold-toggle").unwrap()[1] - at).abs() < 1.0, "Read more is where Show less was pressed");
}

#[test]
fn short_prompts_and_the_setting_off_never_fold() {
    let rt = runtime(false);
    assert!(rect(&rt, "fold-toggle").is_none());
    let rt = runtime(true);
    assert_eq!(rt.elements().iter().filter(|e| !e.exiting && e.key.as_deref() == Some("fold-toggle")).count(), 1, "only the long prompt folds");
}

fn link_runtime(viewport: (f32, f32)) -> Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = 1.6;
    let mut rt = app.into_runtime();
    rt.set_viewport(viewport, 1.6);
    rt.frame(1.0 / 60.0);
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new(crate::fold_stories::LINK);
        s.submit(false, tx);
    });
    for _ in 0..240 {
        rt.frame(1.0 / 60.0);
    }
    rt
}

#[test]
fn a_long_link_breaks_inside_its_bubble() {
    // Rects are in window points, 1.6 times the text's own points at this zoom.
    for viewport in [(1280.0, 820.0), (900.0, 820.0), (700.0, 820.0)] {
        let rt = link_runtime(viewport);
        let words = rect(&rt, &format!("text-{}", sent(&rt))).expect("the link's words");
        let (wide, _) = rune::text::measure(crate::fold_stories::LINK, &[], BUBBLE, None);
        let lines = (words[3] / 1.6 / BUBBLE.line_px()).round();
        let least = (wide / (words[2] / 1.6)).ceil();
        assert!(lines >= least, "{viewport:?}: {lines} lines can't hold a {wide} pt link in {} pt", words[2] / 1.6);
    }
}

#[test]
fn ordinary_words_still_break_at_spaces() {
    let mut rt = App::fixture_session().into_runtime();
    rt.frame(1.0 / 60.0);
    let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
    rt.update(|s, tx| {
        s.composer = rune::TextEdit::new(text);
        s.submit(false, tx);
    });
    for _ in 0..240 {
        rt.frame(1.0 / 60.0);
    }
    let words = rect(&rt, &format!("text-{}", sent(&rt))).unwrap();
    let (_, h) = rune::text::measure(text, &[], BUBBLE, Some(words[2]));
    assert_eq!(words[3], h, "laid out as the space-only measure says");
}
