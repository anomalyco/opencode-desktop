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
