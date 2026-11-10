use opencode_kit::client::Server;
use opencode_kit::model::SessionInfo;
use rune::{Key, KeyEvent, Mods, Runtime, TextEdit, Tx};

use crate::{App, FIXTURE, Route, Source};

const DT: f64 = 1.0 / 60.0;

fn home_runtime() -> Runtime<App> {
    let mut app = App::empty();
    app.load_fixture(FIXTURE);
    app.route = Route::Home;
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt
}

fn cmd(rt: &mut Runtime<App>, ch: char, shift: bool) {
    rt.key(KeyEvent { key: Key::Command(ch), mods: Mods { cmd: true, shift, ..Default::default() } });
    rt.frame(DT);
}

fn press_key(rt: &mut Runtime<App>, k: Key) {
    rt.key(KeyEvent { key: k, mods: Mods::default() });
    rt.frame(DT);
}

fn type_str(rt: &mut Runtime<App>, s: &str) {
    for ch in s.chars() {
        rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        rt.frame(DT);
    }
}

#[test]
fn cmd_f_on_home_focuses_search_filters_by_title_and_project_steps_and_opens_on_enter() {
    let mut rt = home_runtime();
    assert!(!rt.state.search.focused);

    // ⌘F on Home focuses the session search input.
    cmd(&mut rt, 'f', false);
    assert!(rt.state.search.focused);

    // Filtering by project name ("harbor") matches Harbor sessions even when their titles lack "harbor".
    type_str(&mut rt, "harbor");
    let matches = rt.state.home_search_results(None);
    assert!(matches.len() >= 2, "matched Harbor project sessions: {}", matches.len());
    assert!(rt.find("search-clear").is_some(), "clear button shows while query is non-empty");

    // Arrow Down steps to the second match, and Enter opens it.
    let second_id = matches[1].id.clone();
    press_key(&mut rt, Key::Down);
    press_key(&mut rt, Key::Enter);
    assert_eq!(rt.state.route, Route::here(second_id));
    assert!(rt.state.search.text.is_empty(), "opening a result clears Home's search");

    // ⌘B back to Home focuses Home's session search, and Esc clears and blurs it.
    cmd(&mut rt, 'b', false);
    assert_eq!(rt.state.route, Route::Home);
    assert!(rt.state.search.focused, "⌘B to Home focuses session search");
    type_str(&mut rt, "lumen");
    assert!(!rt.state.search.text.is_empty());
    press_key(&mut rt, Key::Escape);
    assert!(rt.state.search.text.is_empty());
    assert!(!rt.state.search.focused);
}

#[test]
fn home_search_merges_server_results_and_exact_session_id_lookups() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let remote_id = "ses_remote0000000000000001";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = listener.accept().expect("accept");
            s.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok();
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let line = req.lines().next().unwrap_or("").to_owned();
            let body = if line.contains("/api/session?parentID=null") {
                serde_json::json!({
                    "data": [{
                        "id": remote_id,
                        "title": "Server-indexed archive needle",
                        "projectID": "prj_lumen",
                        "location": { "directory": "/Users/alex/lumen" },
                        "time": { "created": 1_700_000_000_000_i64, "updated": 1_700_000_050_000_i64 }
                    }]
                })
                .to_string()
            } else {
                serde_json::json!({
                    "data": {
                        "id": remote_id,
                        "title": "Exact ID lookup needle",
                        "projectID": "prj_lumen",
                        "location": { "directory": "/Users/alex/lumen" },
                        "time": { "created": 1_700_000_000_000_i64, "updated": 1_700_000_090_000_i64 }
                    }
                })
                .to_string()
            };
            seen.push(line);
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(resp.as_bytes());
        }
        seen
    });

    let mut app = App::empty();
    app.load_fixture(FIXTURE);
    app.route = Route::Home;
    app.source = Source::Live { server: Server::new(&base), requests: None, writes: None, allow_send: false };
    app.home_search.fetch = rune::Latest::default();
    app.search = TextEdit::new(remote_id);

    assert!(app.poll_home_search(&mut Tx::default()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while app.home_search.fetch.is_pending() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.poll_home_search(&mut Tx::default()));
    let _ = handle.join();

    let found: Vec<&SessionInfo> = app.home_search_results(None);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, remote_id);
    assert_eq!(found[0].title, "Exact ID lookup needle", "newest metadata wins across exact + list");
}

#[test]
fn shift_cmd_f_and_sidebar_search_open_sessions_only_dialog() {
    let mut rt = crate::fixture_runtime();

    // ⇧⌘F opens the session search dialog (recent sessions only, no commands).
    cmd(&mut rt, 'f', true);
    assert!(rt.state.switcher.open);
    let entries = rt.state.switcher_entries();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|e| matches!(e.action, crate::switcher::Action::Session(_))), "sessions only: {entries:?}");

    // Escape closes it; clicking Sidebar's "Search sessions" opens the same sessions-only dialog.
    press_key(&mut rt, Key::Escape);
    assert!(!rt.state.switcher.open);
    let (x, y) = rt.find("search").expect("sidebar Search sessions button");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
    assert!(rt.state.switcher.open);
    assert!(rt.state.switcher_entries().iter().all(|e| matches!(e.action, crate::switcher::Action::Session(_))));
}
