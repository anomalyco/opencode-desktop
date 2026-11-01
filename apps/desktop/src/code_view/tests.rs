use rune::{Env, Key, KeyEvent, Mods, Runtime, col};
use rune_kit::diff::{DiffMode, FileDiff, Side, parse};

use super::{DiffDocState, DiffViewer, ExpandMode};
use crate::palette::pal;

struct Host {
    diff: DiffDocState,
    seen_file: usize,
}

fn view_host(s: &Host, _cx: &mut rune::Cx<Host>) -> rune::El {
    let p = pal(true);
    col().w_full().h_full().child(
        DiffViewer::new("test-diff", |h: &Host| &h.diff, |h: &mut Host| &mut h.diff)
            .on_visible_file(|h: &mut Host, f| h.seen_file = f)
            .build(s, p),
    )
}

#[test]
fn unified_and_split_modes_highlight_syntax_and_jump_files_and_hunks() {
    let f1 = FileDiff::from_texts(
        "src/net/stream.ts",
        "const a = 1\nconst b = 2\nconst c = 3\nconst d = 4\nconst e = 5\nconst f = 6\nconst g = 7\nconst h = 8\nconst i = 9\nconst j = 10\n",
        "const a = 10\nconst b = 2\nconst c = 3\nconst d = 4\nconst e = 5\nconst f = 6\nconst g = 7\nconst h = 8\nconst i = 9\nconst j = 99\n",
    );
    let f2 = FileDiff::from_texts(
        "src/net/backoff.rs",
        "pub fn delay(n: u32) -> u32 {\n    n\n}\n",
        "pub fn delay(n: u32) -> u32 {\n    n.saturating_mul(2)\n}\n",
    );
    let mut diff = DiffDocState::new(vec![f1, f2]);
    diff.attach_file_text(
        "src/net/stream.ts",
        "const a = 10\nconst b = 2\nconst c = 3\nconst d = 4\nconst e = 5\nconst f = 6\nconst g = 7\nconst h = 8\nconst i = 9\nconst j = 99\n",
    );
    assert!(!diff.highlights(0, Side::New, 1).is_empty(), "TypeScript syntax tokens present");
    assert!(!diff.highlights(1, Side::New, 1).is_empty(), "Rust syntax tokens present");

    let unified_rows = diff.rows.len();
    diff.set_mode(DiffMode::Split);
    assert!(diff.rows.len() < unified_rows, "split pairs removed+added lines side by side");
    diff.set_mode(DiffMode::Unified);

    let mut rt = Runtime::new(Host { diff, seen_file: 0 }, view_host, (720.0, 320.0), Env::new(true, 2.0));
    rt.on_key = Some(|s: &mut Host, ev: &KeyEvent, tx| s.diff.handle_key(ev, tx));
    rt.frame(0.0);

    // `j` steps through file 0's two hunks, then the third `j` crosses into file 1's hunk.
    rt.key(KeyEvent { key: Key::Text("j".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.visible_file(), 0);

    rt.key(KeyEvent { key: Key::Text("j".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.visible_file(), 0);

    rt.key(KeyEvent { key: Key::Text("j".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.visible_file(), 1);

    // `[` and `←` jump back to file 0; `→` jumps forward to file 1.
    rt.key(KeyEvent { key: Key::Left, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.visible_file(), 0);

    rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.visible_file(), 1);

    // Expand all unchanged lines via ExpandMode::Expand, then collapse back.
    let before_expand = rt.state.diff.rows.len();
    rt.update(|s, _| s.diff.set_expand_mode(ExpandMode::Expand));
    rt.frame(1.0 / 60.0);
    assert!(rt.state.diff.rows.len() > before_expand);
    rt.update(|s, _| s.diff.set_expand_mode(ExpandMode::Collapse));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.diff.rows.len(), before_expand);
}

#[test]
fn inline_single_file_diff_mounts_without_file_header_for_tool_steps() {
    let f = FileDiff::from_texts("src/lib.rs", "fn old() {}\n", "fn new_fn() {}\n");
    let diff = DiffDocState::inline(f);
    assert!(
        !diff.keys.iter().any(|k| k == "df0"),
        "inline diff omits outer file header row so tool steps can supply their own chrome"
    );
    let mut rt = Runtime::new(
        Host { diff, seen_file: 0 },
        |s: &Host, _| {
            DiffViewer::new("inline-edit-diff", |h: &Host| &h.diff, |h: &mut Host| &mut h.diff)
                .sticky_headers(false)
                .build(s, pal(true))
                .h(120.0)
        },
        (600.0, 140.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    assert!(rt.find("inline-edit-diff").is_some());
}

/// Debug builds check a `Tx::live_only` hover a quarter second after the last commit, and warn
/// when the view built something else: the copy side must commit when it moves, and a hover
/// that keeps it must change nothing the view reads.
#[test]
fn hovering_diff_rows_across_sides_builds_nothing_stale() {
    let old: String = (1..=40).map(|i| format!("let line_{i} = {i};\n")).collect();
    let new: String = (1..=40).map(|i| if i % 3 == 0 { format!("let line_{i} = {};\n", i * 10) } else { format!("let line_{i} = {i};\n") }).collect();
    for mode in [DiffMode::Split, DiffMode::Unified] {
        let mut diff = DiffDocState::new(vec![FileDiff::from_texts("src/a.rs", &old, &new)]);
        diff.set_mode(mode);
        let mut rt = Runtime::new(Host { diff, seen_file: 0 }, view_host, (900.0, 900.0), Env::new(true, 2.0));
        rt.frame(0.0);
        for _ in 0..30 { rt.frame(1.0 / 60.0); }
        let cells = |rt: &Runtime<Host>, side: &str| -> Vec<[f32; 4]> {
            rt.elements()
                .into_iter()
                .filter(|e| e.path.ends_with("/code") && !e.exiting && e.rect[1] > 0.0 && e.rect[1] + e.rect[3] < 900.0)
                .filter(|e| match mode {
                    DiffMode::Split => e.path.contains(&format!("/{side}/")),
                    DiffMode::Unified => true,
                })
                .map(|e| e.rect)
                .collect()
        };
        let (olds, news) = match mode {
            DiffMode::Split => (cells(&rt, "old"), cells(&rt, "new")),
            // Unified rows hover the old side on removed lines and the new side on the rest, so
            // walking every row crosses sides.
            DiffMode::Unified => (cells(&rt, ""), cells(&rt, "")),
        };
        assert!(olds.len() > 4 && news.len() > 4, "{mode:?}: {} old, {} new cells", olds.len(), news.len());
        let mid = |r: [f32; 4]| (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
        let step = |rt: &mut Runtime<Host>, what: &str, moves: &[(f32, f32)]| {
            rt.frame(0.3);
            for &(x, y) in moves { rt.pointer_move(x, y); }
            for _ in 0..3 { rt.frame(1.0 / 60.0); }
            assert!(rt.live_only_warnings().is_empty(), "{mode:?}, {what}: {:#?}", rt.live_only_warnings());
        };
        let (n1, n2, o1, o2) = (mid(news[1]), mid(news[3]), mid(olds[2]), mid(olds[4]));
        step(&mut rt, "into a new-side row", &[n1]);
        step(&mut rt, "onto an old-side row", &[o1]);
        step(&mut rt, "between rows on one side", &[o2]);
        step(&mut rt, "back to the new side", &[n2]);
        step(&mut rt, "out of the code area", &[(2.0, 2.0)]);
        rt.pointer_leave();
        step(&mut rt, "out of the window", &[]);
        step(&mut rt, "two rows in one frame", &[o1, n1]);
        step(&mut rt, "across sides in one frame", &[n2, o2, n1]);
        for (i, r) in news.iter().chain(&olds).enumerate() {
            if i % 2 == 0 { rt.frame(0.3); }
            rt.pointer_move(mid(*r).0, mid(*r).1);
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.live_only_events() > 0, "{mode:?}: some hovers skipped the view");
        assert!(rt.live_only_warnings().is_empty(), "{mode:?}, sweep: {:#?}", rt.live_only_warnings());
    }
}

#[test]
fn fifty_thousand_line_diff_virtualizes_smoothly() {
    let mut patch = String::from(
        "diff --git a/src/large.rs b/src/large.rs\n--- a/src/large.rs\n+++ b/src/large.rs\n@@ -1,25000 +1,25000 @@\n",
    );
    for i in 1..=25_000 {
        patch.push_str(&format!("-let old_{i}: usize = {i};\n+let new_{i}: usize = {};\n", i + 1));
    }
    let parsed = parse(&patch);
    let diff = DiffDocState::new(parsed);
    assert!(diff.rows.len() >= 50_000);

    let mut rt = Runtime::new(Host { diff, seen_file: 0 }, view_host, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // Scroll through the middle of the 50k-line diff and verify it settles completely idle.
    rt.wheel(400.0, 300.0, -4800.0);
    let mut settled = false;
    for _ in 0..90 {
        if !rt.frame(1.0 / 120.0) {
            settled = true;
            break;
        }
    }
    assert!(settled, "idle frame after scroll settles draws nothing");
}

