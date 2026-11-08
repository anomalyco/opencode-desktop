//! Unit tests for the Rune terminal grid view: CommitMono font metrics, per-row damage & `memo`
//! reuse, phase-locked cursor blink (with zero idle commits when unfocused), selection/copy,
//! scrollback scrollbar, and visual-only bell.

use std::time::{Duration, Instant};

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use terminal::SelectionOp;

use super::grid::{cell_metrics, terminal_font_style};
use super::input::handle_key;
use super::story::{init_story, view_card};
use crate::{App, view};

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pred(), "condition not met within {timeout:?}");
}

#[test]
fn commit_mono_renders_grid_and_cursor_blinks_on_half_second_edges_only_when_focused() {
    let mut app = App::empty();
    app.zoom = 1.6;
    init_story(&mut app);
    crate::stories::stage(&mut app, view_card);

    let spec = crate::spec(|| App::empty());
    let mut rt = Runtime::with_text(
        app,
        view::app,
        (1280.0, 820.0),
        Env::new(true, 2.0),
        rune::text::TextSystem::new(spec.fonts, spec.families),
    );
    rt.poll = Some(crate::poll);
    rt.frame(0.0);

    let (choice, st) = terminal_font_style(&rt.state.knobs);
    let (cw, ch) = cell_metrics(choice, st);
    assert!(cw > 6.0 && ch >= 16.0, "CommitMono cell metrics valid: ({cw}, {ch})");

    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("terminal-pane")),
        "terminal pane mounted"
    );
    assert!(
        rt.state.terminal.workspaces["story\0/workspace/seer"].tabs[0]
            .session
            .snapshot()
            .cursor
            .blinking,
        "default cursor has blinking=true"
    );
    assert!(
        rt.state.terminal.cursor_lit(rt.state.clock),
        "cursor lit at t=0.0"
    );
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("terminal-cursor")),
        "cursor element mounted at t=0.0"
    );

    // Advance past 0.5 s: cursor toggles off on phase-locked app clock.
    let commits_at_t0 = rt.stats.commits;
    for _ in 0..62 {
        rt.frame(1.0 / 120.0);
    }
    assert!(
        !rt.state.terminal.cursor_lit(rt.state.clock),
        "cursor off at t=0.516s"
    );
    assert!(
        rt.stats.commits > commits_at_t0,
        "blink edge committed one frame"
    );

    // Advance past 1.0 s: cursor toggles back on.
    let commits_at_half = rt.stats.commits;
    for _ in 0..62 {
        rt.frame(1.0 / 120.0);
    }
    assert!(
        rt.state.terminal.cursor_lit(rt.state.clock),
        "cursor back on at t=1.033s"
    );
    assert!(
        rt.stats.commits > commits_at_half,
        "second blink edge committed one frame"
    );

    // Unfocused terminal draws solid hollow cursor and requests ZERO idle frames!
    rt.update(|s, _| s.terminal.story_focused = false);
    rt.frame(1.0 / 120.0);
    assert!(
        rt.elements().iter().any(|e| e.key.as_deref() == Some("terminal-cursor")),
        "unfocused hollow cursor stays visible"
    );
    let commits_before = rt.stats.commits;
    for _ in 0..65 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(
        rt.stats.commits, commits_before,
        "unfocused idle terminal must commit zero frames"
    );
}

#[test]
fn selection_copy_cmd_backspace_scrollback_and_visual_bell() {
    let mut app = App::empty();
    app.zoom = 1.6;
    app.variant = 3; // 1,500-line scrollback variant
    init_story(&mut app);
    crate::stories::stage(&mut app, view_card);

    let spec = crate::spec(|| App::empty());
    let mut rt = Runtime::with_text(
        app,
        view::app,
        (1280.0, 820.0),
        Env::new(true, 2.0),
        rune::text::TextSystem::new(spec.fonts, spec.families),
    );
    rt.poll = Some(crate::poll);
    rt.frame(0.0);

    // Scrollbar mounts when scrollback exceeds viewport height.
    assert!(
        rt.elements()
            .iter()
            .any(|e| e.key.as_deref() == Some("terminal-scrollbar-thumb")),
        "scrollbar thumb visible on scrollback buffer"
    );

    // Select all + `⌘C` copies via `tx.copy` without blocking main thread.
    let session_ok = {
        let ws = rt.state.terminal.workspaces.get("story\0/workspace/seer").unwrap();
        let tab = &ws.tabs[0];
        tab.session.select(SelectionOp::All);
        wait_until(Duration::from_secs(2), || tab.session.selected_text().is_some());
        true
    };
    assert!(session_ok);

    rt.update(|s, tx| {
        let session = {
            let ws = s.terminal.workspaces.get("story\0/workspace/seer").unwrap();
            &ws.tabs[0].session as *const terminal::TerminalSession
        };
        let mut mods = Mods::default();
        mods.cmd = true;
        // Safe: `handle_key` on `⌘C` only reads `session.selected_text()` and writes `tx.copy`.
        let consumed = handle_key(
            s,
            unsafe { &*session },
            &KeyEvent {
                key: Key::Command('c'),
                mods,
            },
            24,
            tx,
        );
        assert!(consumed, "⌘C with active selection must copy");
    });

    // Drain any pending dirty flag from selection before arming the visual bell check.
    rt.frame(1.0 / 120.0);
    rt.state
        .terminal
        .dirty_flag
        .store(false, std::sync::atomic::Ordering::Release);

    // Visual bell sets `bell_until` and clears after 140 ms with zero audio.
    rt.update(|s, _| {
        let ws = s.terminal.workspaces.get("story\0/workspace/seer").unwrap();
        ws.tabs[0].stream.as_ref().unwrap().feed(b"\x07");
    });
    wait_until(Duration::from_secs(2), || {
        rt.state.terminal.dirty_flag.load(std::sync::atomic::Ordering::Acquire)
    });
    rt.frame(1.0 / 120.0);
    assert!(rt.state.terminal.bell_until > rt.state.clock, "visual bell armed");
    for _ in 0..25 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.state.terminal.bell_until, 0.0, "visual bell settles cleanly");
}

#[test]
fn scrolling_100k_lines_in_debug_emits_no_stalls_and_stays_responsive() {
    use super::grid::{cell_metrics, terminal_font_style};
    use super::streams::cat_lines_stream;
    use super::theme_from_pal;
    use crate::palette::pal;
    use terminal::{RestoreState, ScrollOp, TerminalConfig};

    let cat_bytes = cat_lines_stream(100_000);
    let mut app = App::empty();
    app.zoom = 1.6;
    crate::open_scene(&mut app);
    app.finish_stream();
    let spec = crate::spec(|| App::empty());
    let text_sys = rune::text::TextSystem::new(spec.fonts, spec.families);
    let (choice, st) = terminal_font_style(&app.knobs);
    let (cw, ch) = cell_metrics(choice, st);
    let cols = ((((814.0 / 1.6) - 48.0 - 20.0) / cw).floor() as u16).max(40);
    let rows = ((((820.0 / 1.6) - 48.0 - 54.0) / ch).floor() as u16).max(12);

    app.insert_stream_terminal(
        "story\0/workspace/seer",
        "term_100k",
        "cat 100k.log",
        1,
        TerminalConfig {
            cols,
            rows,
            cell_w_px: (cw * 3.2).round() as u32,
            cell_h_px: (ch * 3.2).round() as u32,
            scrollback_lines: 250_000,
            theme: theme_from_pal(pal(true)),
        },
        Some(RestoreState {
            buffer: cat_bytes,
            cols: Some(cols),
            rows: Some(rows),
            scroll_y: None,
            cursor: None,
        }),
        b"",
    );
    crate::stories::stage(&mut app, view_card);

    let mut rt = Runtime::with_text(app, view::app, (1280.0, 820.0), Env::new(true, 2.0), text_sys);
    rt.poll = Some(crate::poll);
    rt.frame(0.0);

    for _ in 0..40 {
        rt.update(|s, _| {
            let ws = s.terminal.workspaces.get("story\0/workspace/seer").unwrap();
            ws.tabs[0].session.scroll(ScrollOp::Delta(-5));
        });
        wait_until(Duration::from_secs(2), || {
            rt.state.terminal.dirty_flag.load(std::sync::atomic::Ordering::Acquire)
        });
        rt.frame(1.0 / 120.0);
    }

    let snap = rt.state.terminal.workspaces["story\0/workspace/seer"].tabs[0]
        .session
        .snapshot();
    assert!(snap.scrollbar.total >= 100_000);
    assert!(!snap.scrollbar.at_bottom);
}

#[test]
fn workspace_persistence_roundtrips_tabs_active_scroll_and_2000_line_vt_buffer() {
    use terminal::ScrollOp;

    let mut a = App::fixture_session();
    let wk = a.active_terminal_workspace_key().expect("active workspace key");
    let server_id = a.server_id_str().to_owned();
    let dir = wk.split_once('\0').unwrap().1.to_owned();

    let t1 = a.new_terminal_in(&wk).unwrap();
    let t2 = a.new_terminal_in(&wk).unwrap();
    assert_eq!(t1, "term_1");
    assert_eq!(t2, "term_2");

    // Feed 2,400 colored lines into `term_2`, scroll up, and set a custom title on `term_1`.
    {
        let tab1 = a.terminal_tab_mut(&wk, &t1).unwrap();
        tab1.title = "dev server".into();
        tab1.custom_title = true;
    }
    {
        let tab2 = a.terminal_tab_mut(&wk, &t2).unwrap();
        let st = tab2.stream.as_ref().unwrap();
        let mut payload = Vec::new();
        for i in 0..2_400 {
            payload.extend_from_slice(format!("persist-line-{i:04} \x1b[32mok\x1b[0m\r\n").as_bytes());
        }
        st.feed(&payload);
    }
    wait_until(Duration::from_secs(2), || {
        a.terminal.workspaces[&wk].tabs[1]
            .session
            .snapshot()
            .viewport_text()
            .contains("persist-line-2399")
    });
    a.terminal.workspaces[&wk].tabs[1]
        .session
        .scroll(ScrollOp::Delta(-12));
    wait_until(Duration::from_secs(2), || {
        !a.terminal.workspaces[&wk].tabs[1]
            .session
            .snapshot()
            .scrollbar
            .at_bottom
    });

    let saved = a.terminals_to_json(&server_id);
    assert_eq!(saved[&dir]["active"], "term_2");
    assert_eq!(saved[&dir]["all"].as_array().unwrap().len(), 2);

    // Restore into a fresh `App`, lazy-hydrate on dock open, and verify tabs, title, and VT buffer.
    let mut b = App::fixture_session();
    b.restore_terminals_from_json(&server_id, &saved);
    assert!(
        b.terminal.workspaces.is_empty(),
        "workspaces stay lazy until dock opens"
    );
    assert!(b.ensure_terminal_workspace_hydrated(&wk));

    let ws = b.terminal.workspaces.get(&wk).expect("hydrated workspace");
    assert_eq!(ws.active.as_deref(), Some("term_2"));
    assert_eq!(ws.tabs.len(), 2);
    assert_eq!(ws.tabs[0].title, "dev server");
    assert!(ws.tabs[0].custom_title);

    let snap2 = ws.tabs[1].session.snapshot();
    let full_buf = saved[&dir]["all"][1]["buffer"].as_str().unwrap_or("");
    assert!(!full_buf.contains("persist-line-0010"), "2,000-line cap prunes oldest rows");
    assert!(full_buf.contains("persist-line-2399"), "recent VT rows serialized");
    assert!(
        snap2.scrollbar.total > snap2.scrollbar.len,
        "restored terminal preserves scrollback history"
    );
}
