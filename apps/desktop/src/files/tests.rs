use rune::{Env, Key, KeyEvent, Mods, Runtime, TextEdit};

use super::data::file_tab_key;
use crate::code_view::{CodeDocState, CodeViewer, FileLineSelection};
use crate::palette::pal;
use crate::{App, FIXTURE, open_scene, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        crate::plugin_host::setup(&mut a, std::sync::Arc::new(|| {}));
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, (1440.0, 900.0), Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

#[test]
fn cmd_p_opens_file_palette_filters_and_jumps_to_line_in_shared_code_viewer() {
    let mut rt = test_runtime();
    // ⌘P opens the quick-open file palette instantly.
    rt.key(KeyEvent {
        key: Key::Command('p'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 120.0);
    assert!(rt.state.switcher.open);
    assert!(rt.find("switcher").is_some());

    // Type `stream.ts:15` and press Enter to open `src/net/stream.ts` at line 15.
    rt.update(|s, _| {
        s.switcher.search = TextEdit::new("stream.ts:15");
        s.switcher.search.focused = true;
    });
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }

    assert!(!rt.state.switcher.open);
    assert!(rt.state.side_open());
    let expected_key = file_tab_key("src/net/stream.ts");
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some(expected_key.as_str())
    );
    let sid = rt.state.active_session_id().unwrap().to_owned();
    let doc = &rt.state.files.sessions[&sid].docs["src/net/stream.ts"];
    assert_eq!(doc.selection(), Some(FileLineSelection { start: 15, end: 15 }));
    assert!(!doc.highlights(1).is_empty(), "TypeScript syntax highlighting active");
}

#[test]
fn preview_tab_replacement_in_file_find_and_reopen_closed_tab() {
    let mut rt = test_runtime();
    let sid = rt.state.active_session_id().unwrap().to_owned();

    // Single-click preview opens `backoff.ts`, then a second preview replaces it in-place.
    rt.update(|s, tx| s.open_workspace_file("src/net/backoff.ts", true, None, tx));
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.side.sessions[&sid].tab_state.preview.as_deref(),
        Some("file://src/net/backoff.ts")
    );

    rt.update(|s, tx| s.open_workspace_file("src/sidebar/store.ts", true, None, tx));
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.side.sessions[&sid].tab_state.preview.as_deref(),
        Some("file://src/sidebar/store.ts")
    );
    assert!(
        !rt.state.side.sessions[&sid]
            .tab_state
            .tabs
            .all
            .contains(&"file://src/net/backoff.ts".to_owned()),
        "preview tab replaced rather than accumulated"
    );

    // Promote to permanent, then press ⌘F inside the file to search for `hydrate`.
    rt.update(|s, tx| s.open_workspace_file("src/sidebar/store.ts", false, None, tx));
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
    }
    rt.key(KeyEvent {
        key: Key::Command('f'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("files-find-bar").is_some());

    rt.update(|s, _| {
        let d = s
            .files
            .sessions
            .get_mut(&sid)
            .unwrap()
            .docs
            .get_mut("src/sidebar/store.ts")
            .unwrap();
        d.find_input = TextEdit::new("hydrate");
        d.sync_find_query();
    });
    rt.frame(1.0 / 60.0);
    assert!(
        rt.state.files.sessions[&sid].docs["src/sidebar/store.ts"].match_count() >= 1
    );

    // Close the file tab with ⌘W, then press ⌘⇧T to reopen `file://src/sidebar/store.ts`.
    rt.update(|s, _| {
        s.files
            .sessions
            .get_mut(&sid)
            .unwrap()
            .docs
            .get_mut("src/sidebar/store.ts")
            .unwrap()
            .find_input
            .focused = false;
    });
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some("review:changes")
    );

    rt.key(KeyEvent {
        key: Key::Command('t'),
        mods: Mods { cmd: true, shift: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some("file://src/sidebar/store.ts")
    );
}

#[test]
fn fifty_thousand_line_source_file_virtualizes_smoothly() {
    let mut src = String::with_capacity(50_000 * 32);
    for i in 1..=50_000 {
        src.push_str(&format!("pub fn item_{i}() -> usize {{ {i} }}\n"));
    }
    let doc = CodeDocState::new("src/generated.rs", &src);
    assert_eq!(doc.line_count(), 50_000);

    let mut rt = Runtime::new(
        doc,
        |d: &CodeDocState, _| {
            CodeViewer::new("big-file", |s: &CodeDocState| s, |s: &mut CodeDocState| s)
                .build(d, pal(true))
        },
        (800.0, 600.0),
        Env::new(true, 2.0),
    );
    rt.frame(0.0);
    rt.wheel(400.0, 300.0, -6400.0);
    let mut settled = false;
    for _ in 0..90 {
        if !rt.frame(1.0 / 120.0) {
            settled = true;
            break;
        }
    }
    assert!(settled, "50k-line CodeViewer settles completely idle");
}
