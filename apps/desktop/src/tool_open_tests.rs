use opencode_kit::model::ToolStatus;
use rune::{Env, Key, KeyEvent, Mods, Runtime};
use rune_kit::diff::{FileDiff, LineRange, LineRef, Side};
use serde_json::json;

use crate::code_view::FileLineSelection;
use crate::files::data::file_tab_key;
use crate::review::REVIEW_KEY;
use crate::tool_open::{Anchor, Located, Target, locate, target};
use crate::{App, FIXTURE, open_scene, spec};

const DIR: &str = "/Users/demo/code/lumen";
const EDIT: &str = "call_edit_50000";
const READ: &str = "call_read_28000";

fn runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        a.expanded.insert(format!("c-{EDIT}"));
        a.expanded.insert("c-call_grep_27000".into());
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, (1440.0, 900.0), Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

/// Scrolls the timeline until `key` is mid-screen, then clicks it. Returns where it was.
fn tap(rt: &mut Runtime<App>, key: &str) -> f32 {
    let (x, y) = rt.find(key).unwrap_or_else(|| panic!("{key} not in the timeline"));
    rt.wheel(x, 450.0, 450.0 - y);
    rt.frame(1.0 / 60.0);
    let (x, y) = rt.find(key).unwrap();
    assert!((0.0..900.0).contains(&y), "{key} scrolled into view");
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    y
}

/// Presses step `id` the way its row does, wherever the timeline has scrolled it.
fn press(rt: &mut Runtime<App>, id: &str) {
    assert!(rt.state.tool_step_target(id).is_some());
    rt.update(|s, tx| s.press_tool_step(id, tx));
    rt.frame(1.0 / 60.0);
}

fn active(rt: &Runtime<App>) -> Option<String> {
    rt.state.resolve_side_region().and_then(|r| r.active)
}

fn esc(rt: &mut Runtime<App>) {
    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn targets_follow_the_tool() {
    let done = ToolStatus::Completed;
    let read = json!({ "filePath": format!("{DIR}/src/a.ts"), "offset": 10, "limit": 5 });
    assert_eq!(target("read", &done, &read, &json!(null), DIR), Some(Target::File { path: "src/a.ts".into(), lines: Some((10, 14)) }));
    let write = json!({ "path": "src/b.ts", "content": "x" });
    assert_eq!(target("write", &done, &write, &json!(null), DIR), Some(Target::File { path: "src/b.ts".into(), lines: None }));
    let edit = json!({ "filePath": format!("{DIR}/src/a.ts"), "oldString": "a\nb", "newString": "a\nc" });
    let anchor = Anchor { side: Side::New, text: "c".into(), near: None };
    assert_eq!(target("edit", &done, &edit, &json!(null), DIR), Some(Target::Change { path: "src/a.ts".into(), anchor: Some(anchor) }));
    // The step's own diff says where, and wins over its input.
    let meta = json!({ "files": [{ "file": "src/a.ts", "patch": "--- a/src/a.ts\n+++ b/src/a.ts\n@@ -40,2 +40,2 @@\n a\n-b\n+c\n" }] });
    let Some(Target::Change { anchor: Some(a), .. }) = target("edit", &done, &edit, &meta, DIR) else { panic!() };
    assert_eq!(a.near, Some(41));
    let patch = json!({ "patchText": "*** Begin Patch\n*** Update File: src/z.rs\n@@\n fn a() {}\n-old\n+new\n*** End Patch" });
    let Some(Target::Change { path, anchor: Some(a) }) = target("apply_patch", &done, &patch, &json!(null), DIR) else { panic!() };
    assert_eq!((path.as_str(), a.text.as_str(), a.side), ("src/z.rs", "new", Side::New));
    // Outside the session's folder, failed, or still running: nothing to open.
    assert_eq!(target("read", &done, &json!({ "filePath": "/etc/hosts" }), &json!(null), DIR), None);
    assert_eq!(target("edit", &ToolStatus::Error, &edit, &json!(null), DIR), None);
    assert_eq!(target("read", &ToolStatus::Running, &read, &json!(null), DIR), None);
    assert_eq!(target("shell", &done, &json!({ "command": "ls" }), &json!(null), DIR), None);
}

#[test]
fn locate_finds_the_run_nearest_the_hint() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\n";
    let new = "a\nb\nc\nd\nX\nY\ne\nf\ng\nh\ni\nj\nX\nk\nl\n";
    let fd = FileDiff::from_texts("f", old, new);
    let first = Anchor { side: Side::New, text: "X".into(), near: None };
    assert_eq!(locate(&fd, &first), Some(Located { side: Side::New, start: 5, end: 6, top: 2 }));
    let later = Anchor { near: Some(12), ..first };
    assert_eq!(locate(&fd, &later).map(|l| (l.start, l.end)), Some((13, 13)));
    let gone = Anchor { side: Side::New, text: "nowhere".into(), near: None };
    assert_eq!(locate(&fd, &gone), None);
}

#[test]
fn an_edit_opens_its_change_and_the_same_click_shuts_it() {
    let mut rt = runtime();
    assert!(!rt.state.side_open());
    let key = format!("open-{EDIT}");
    let y0 = tap(&mut rt, &key);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!((rt.find(&key).unwrap().1 - y0).abs() < 1.0, "the pressed row stays where it was");
    assert!(rt.state.side_open(), "the side region opens");
    assert_eq!(active(&rt).as_deref(), Some(REVIEW_KEY));
    assert!(!rt.state.side.focused, "focus stays in the timeline");
    let sid = rt.state.active_session_id().unwrap().to_owned();
    let sr = &rt.state.review.sessions[&sid];
    assert_eq!(sr.active_path.as_deref(), Some("src/net/stream.ts"));
    let file = sr.diffs.file_index("src/net/stream.ts").unwrap();
    let want = LineRange { file, start: LineRef::new(Side::New, 14), end: LineRef::new(Side::New, 15) };
    assert_eq!(sr.diffs.selection(), Some(want), "the edit's new lines are selected");
    tap(&mut rt, &key);
    assert!(!rt.state.side_open(), "the same click shuts what it opened");
}

#[test]
fn a_read_opens_its_lines_and_esc_goes_back_to_the_tab_before() {
    let mut rt = runtime();
    rt.update(|s, tx| s.toggle_side_region(tx));
    rt.frame(1.0 / 60.0);
    let before = active(&rt);
    press(&mut rt, READ);
    let key = file_tab_key("src/net/stream.ts");
    assert_eq!(active(&rt).as_deref(), Some(key.as_str()));
    let sid = rt.state.active_session_id().unwrap().to_owned();
    let doc = &rt.state.files.sessions[&sid].docs["src/net/stream.ts"];
    assert_eq!(doc.selection(), Some(FileLineSelection { start: 11, end: 30 }));
    // Opening another step from there still goes back to the first tab.
    press(&mut rt, EDIT);
    assert_eq!(active(&rt).as_deref(), Some(REVIEW_KEY));
    esc(&mut rt);
    assert!(rt.state.side_open());
    assert_eq!(active(&rt), before);
    assert!(rt.state.tool_open.is_none());
    // Nothing left to undo: Esc does what it did before (here, back home).
    esc(&mut rt);
    assert_eq!(rt.state.route, crate::Route::Home);
}

#[test]
fn esc_shuts_a_side_region_the_step_opened() {
    let mut rt = runtime();
    press(&mut rt, READ);
    assert!(rt.state.side_open());
    esc(&mut rt);
    assert!(!rt.state.side_open());
    assert!(matches!(rt.state.route, crate::Route::Session { .. }), "Esc closed the side region, not the session");
}

#[test]
fn a_double_click_leaves_the_step_open() {
    let mut rt = runtime();
    let key = format!("open-{EDIT}");
    let (x, y) = rt.find(&key).unwrap();
    rt.wheel(x, 450.0, 450.0 - y);
    rt.frame(1.0 / 60.0);
    let (x, y) = rt.find(&key).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open(), "a double-click opens it, and leaves it open");
    assert_eq!(active(&rt).as_deref(), Some(REVIEW_KEY));
}
