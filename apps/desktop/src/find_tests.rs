use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::{App, Route, view};

const DT: f64 = 1.0 / 120.0;

fn settle(rt: &mut Runtime<App>) {
    for _ in 0..180 {
        if !rt.frame(DT) {
            break;
        }
    }
}

fn cmd(rt: &mut Runtime<App>, ch: char, shift: bool) {
    let mut mods = Mods::default();
    mods.cmd = true;
    mods.shift = shift;
    rt.key(KeyEvent { key: Key::Command(ch), mods });
    rt.frame(DT);
}

fn key(rt: &mut Runtime<App>, k: Key, shift: bool) {
    let mut mods = Mods::default();
    mods.shift = shift;
    rt.key(KeyEvent { key: k, mods });
    rt.frame(DT);
}

fn type_str(rt: &mut Runtime<App>, s: &str) {
    for ch in s.chars() {
        rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        rt.frame(DT);
    }
}

fn count_text(rt: &Runtime<App>) -> Option<String> {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-count")).and_then(|e| e.text)
}

#[test]
fn cmd_f_highlights_every_match_steps_wraps_centres_and_closes_on_escape() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| s.composer.focused = true);
    settle(&mut rt);
    assert!(rt.state.composer.focused);
    assert!(rt.find("find-bar").is_none());

    // ⌘F opens the find bar and focuses its input.
    cmd(&mut rt, 'f', false);
    assert!(rt.state.find.open && rt.state.find.input.focused);
    assert!(!rt.state.composer.focused);
    assert!(rt.find("find-bar").is_some());
    assert!(count_text(&rt).is_none(), "empty query shows no count");

    // Typing highlights matches across user bubbles, Markdown prose, inline code and code blocks.
    type_str(&mut rt, "stream");
    settle(&mut rt);
    let total = rt.state.find.count();
    assert!(total >= 8, "matches across user and assistant messages: {total}");
    assert_eq!(count_text(&rt).as_deref(), Some(format!("1 of {total}").as_str()));

    let active_mark = |rt: &Runtime<App>| {
        rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-active")).expect("active highlight")
    };
    let scroll_rect = |rt: &Runtime<App>| {
        rt.elements().into_iter().find(|e| e.path.contains("scroll-")).expect("scroll viewport").rect
    };

    // Enter steps forward; Shift+Enter and ⇧⌘G step backward and wrap.
    key(&mut rt, Key::Enter, false);
    settle(&mut rt);
    assert_eq!(count_text(&rt).as_deref(), Some(format!("2 of {total}").as_str()));
    let m2 = active_mark(&rt);
    let sv = scroll_rect(&rt);
    let mid = m2.rect[1] + m2.rect[3] * 0.5;
    let sv_mid = sv[1] + sv[3] * 0.5;
    assert!((mid - sv_mid).abs() < 40.0 || rt.scroll_at_bottom(&format!("scroll-{}", crate::fixture_ids::MAIN)) == Some(true) || mid < sv_mid, "stepped match revealed/centred: mark={:?} in {sv:?}", m2.rect);

    // ⌘G steps to 3, ⇧⌘G steps back to 2, Shift+Enter back to 1, and once more wraps to `total`.
    cmd(&mut rt, 'g', false);
    settle(&mut rt);
    assert_eq!(count_text(&rt).as_deref(), Some(format!("3 of {total}").as_str()));
    cmd(&mut rt, 'g', true);
    settle(&mut rt);
    assert_eq!(count_text(&rt).as_deref(), Some(format!("2 of {total}").as_str()));
    key(&mut rt, Key::Enter, true);
    settle(&mut rt);
    assert_eq!(count_text(&rt).as_deref(), Some(format!("1 of {total}").as_str()));
    key(&mut rt, Key::Enter, true);
    settle(&mut rt);
    assert_eq!(count_text(&rt).as_deref(), Some(format!("{total} of {total}").as_str()));

    // Esc closes Find and clears highlights, leaving nothing focused as Desktop's does (`esc-find`);
    // the bar's close button is the one that hands the composer its focus back.
    key(&mut rt, Key::Escape, false);
    settle(&mut rt);
    assert!(!rt.state.find.open);
    assert!(rt.find("find-bar").is_none());
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("find-active")));
    assert!(!rt.state.composer.focused, "Escape leaves nothing focused");
}

#[test]
fn virtual_session_finds_unmounted_matches_centres_them_and_skips_rebuilding_unrelated_rows() {
    let mut rt = Runtime::new(crate::perf::fixture(), view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    settle(&mut rt);

    // Open Find on a 1,600-message virtualized session (`bench`).
    cmd(&mut rt, 'f', false);

    // Measure row builds per keystroke on a 1,600-message (2,400+ row) virtualized session:
    // typing 17 characters ("implementation 42") must build only the mounted window per keystroke,
    // never all 2,400 rows.
    for ch in "implementation 42".chars() {
        view::timeline_reset_row_builds();
        rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        rt.frame(DT);
        let builds = view::timeline_row_builds();
        assert!(builds < 64, "keystroke '{ch}' built {builds} rows (must not rebuild all 2,400 rows)");
    }
    settle(&mut rt);

    // Exact match count across unmounted rows ("implementation 42", "implementation 420".."429").
    assert_eq!(rt.state.find.count(), 11);
    assert_eq!(count_text(&rt).as_deref(), Some("1 of 11"));

    // The unmounted row `u-u0042` was mounted and centred in `scroll-bench`.
    let mark = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-active")).expect("u0042 highlight mounted");
    let scroll = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("scroll-bench")).unwrap().rect;
    let mark_mid = mark.rect[1] + mark.rect[3] * 0.5;
    let scroll_mid = scroll[1] + scroll[3] * 0.5;
    assert!((mark_mid - scroll_mid).abs() < 24.0, "active match centred in viewport: mark_mid={mark_mid}, scroll_mid={scroll_mid}");

    // Stepping to another match in an already-mounted or distant row only rebuilds changed rows.
    view::timeline_reset_row_builds();
    key(&mut rt, Key::Enter, false);
    settle(&mut rt);
    let step_builds = view::timeline_row_builds();
    assert!(step_builds < 64, "stepping rebuilt only the target window (built {step_builds})");
    assert_eq!(count_text(&rt).as_deref(), Some("2 of 11"));
    let mark2 = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-active")).expect("second match mounted");
    let mark2_mid = mark2.rect[1] + mark2.rect[3] * 0.5;
    assert!((mark2_mid - scroll_mid).abs() < 24.0, "stepped match centred: mark2_mid={mark2_mid}, scroll_mid={scroll_mid}");

    // Switching sessions keeps each session's Find state separate.
    rt.update(|s, tx| {
        s.route = Route::here("other".into());
        tx.settle();
    });
    rt.frame(DT);
    assert!(!rt.state.find.open, "other session starts with Find closed");
}

#[test]
fn unicode_case_folding_keeps_valid_byte_ranges() {
    let text = "İstanbul STRAẞE τελικός";
    let r1 = crate::find::find_plain(text, "İSTANBUL");
    assert_eq!(r1.len(), 1);
    assert_eq!(&text[r1[0].clone()], "İstanbul");
    let r2 = crate::find::find_plain(text, "straße");
    assert_eq!(r2.len(), 1);
    assert_eq!(&text[r2[0].clone()], "STRAẞE");
    let greek = crate::find::find_plain("τελικός λόγος", "τελικόσ");
    assert_eq!(greek.len(), 1);
    assert_eq!(&"τελικός λόγος"[greek[0].clone()], "τελικός");
}

#[test]
fn the_find_story_opens_with_highlights_and_triggers_step() {
    let mut rt = crate::stories::story("Timeline/Find", None);
    assert!(rt.playground_element("find-bar").is_some(), "Find bar visible on stage");
    assert!(rt.playground_element("find-active").is_some(), "active highlight visible on stage");
    rt.playground_trigger('n');
    for _ in 0..60 {
        rt.frame(DT);
    }
    assert_eq!(crate::stories::world(&rt).find.active_index(), 1);
}

#[test]
fn folded_user_prompt_skips_read_more_and_unfolds_when_match_is_hidden() {
    let mut rt = crate::fixture_runtime();
    let sid = crate::fixture_ids::MAIN.to_owned();
    let msg_id = "msg_zzfolded_find";
    rt.update(|s, tx| {
        s.settings.fold_user = true;
        s.transcripts.get_mut(&sid).unwrap().messages.push(opencode_kit::model::Message::User {
            id: msg_id.into(),
            text: crate::fold_stories::LONG.into(),
        });
        tx.scroll_to_bottom_instant(&format!("scroll-{sid}"));
    });
    settle(&mut rt);
    let fold_key = crate::fold::key(msg_id);
    assert!(!rt.state.expanded.contains(&fold_key), "long prompt starts folded");
    assert!(rt.find("fold-toggle").is_some(), "Read more toggle shown");

    // "Read more" itself is chrome, not message text.
    cmd(&mut rt, 'f', false);
    type_str(&mut rt, "Read more");
    settle(&mut rt);
    assert_eq!(rt.state.find.count(), 0);
    assert!(!rt.state.expanded.contains(&fold_key), "stays folded");

    // A match in the first visible lines ("Notes from the call") highlights without unfolding.
    rt.update(|s, tx| {
        s.find.input = rune::TextEdit::new("Notes from the call");
        crate::find::typed(s, tx);
    });
    settle(&mut rt);
    assert_eq!(rt.state.find.count(), 1);
    assert!(!rt.state.expanded.contains(&fold_key), "visible top-line match keeps fold closed");

    // A match hidden deep inside the fold ("interface change" on line 16) unfolds the prompt and centres it.
    rt.update(|s, tx| {
        s.find.input = rune::TextEdit::new("interface change");
        crate::find::typed(s, tx);
    });
    settle(&mut rt);
    assert_eq!(rt.state.find.count(), 2);
    assert!(rt.state.expanded.contains(&fold_key), "hidden match unfolds the prompt");
    let mark = rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("find-active")).expect("unfolded highlight visible");
    assert!(mark.visible[3] > 0.0, "active match inside unfolded prompt is visible on screen: {mark:?}");
}
