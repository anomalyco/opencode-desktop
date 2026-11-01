use rune::{Key, KeyEvent, Mods, Runtime};

use crate::status_toast::{ToastAction, fail, fail_retry, from_server, plain};
use crate::App;

const DT: f64 = 1.0 / 120.0;

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT) as usize {
        rt.frame(DT);
    }
}

fn title(rt: &Runtime<App>) -> Option<String> {
    rt.state.toast.current.as_ref().map(|e| e.title.clone())
}

fn drawn(rt: &Runtime<App>, key: &str) -> Option<rune::ElementInfo> {
    rt.elements().into_iter().find(|e| !e.exiting && e.path.contains("status-toast") && e.key.as_deref() == Some(key))
}

fn tap(rt: &mut Runtime<App>, key: &str) {
    let el = drawn(rt, key).unwrap_or_else(|| panic!("missing {key}"));
    let (x, y) = (el.rect[0] + el.rect[2] / 2.0, el.rect[1] + el.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.frame(DT);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(DT);
}

#[test]
fn a_message_shows_then_leaves_after_five_seconds() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, tx| {
        s.composer.text = "ls".into();
        s.submit_shell(false, tx);
    });
    play(&mut rt, 0.5);
    assert_eq!(title(&rt).as_deref(), Some("Shell commands need a server"));
    assert!(drawn(&rt, "toast-title").is_some());
    assert_eq!(rt.state.status, "Shell commands need a server: Connect to an OpenCode server to run them.", "the debug bar keeps the line");
    play(&mut rt, 4.0);
    assert!(title(&rt).is_some(), "still there at 4.5 s");
    play(&mut rt, 1.0);
    assert!(title(&rt).is_none() && drawn(&rt, "toast-title").is_none(), "gone after 5 s");
}

#[test]
fn a_failure_stays_until_esc_or_its_close_button() {
    let mut rt = crate::fixture_runtime();
    rt.update(|s, _| fail_retry(s, "Upload failed", "Connection refused", ToastAction::Attach("/tmp/a.png".into())));
    play(&mut rt, 12.0);
    assert_eq!(title(&rt).as_deref(), Some("Upload failed"), "a failure doesn't time out");
    assert!(drawn(&rt, "toast-retry").is_some(), "it carries its fix");

    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    rt.frame(DT);
    assert!(title(&rt).is_none(), "Esc dismisses it");

    rt.update(|s, _| fail(s, "Upload failed", ""));
    play(&mut rt, 0.2);
    tap(&mut rt, "toast-dismiss");
    assert!(title(&rt).is_none(), "× dismisses it");
}

#[test]
fn server_lines_become_desktops_toasts_and_reads_stay_in_the_debug_bar() {
    let mut rt = crate::fixture_runtime();
    let cases = [
        ("POST /api/session/ses_a/prompt: http status: 500", Some(("Failed to send prompt", "http status: 500"))),
        ("POST /api/session/ses_a/background: Connection refused (os error 61)", Some(("Request failed", "OpenCode isn't running"))),
        ("DELETE /api/session/ses_a: http status: 404", Some(("Failed to delete session", "http status: 404"))),
        ("PATCH /api/session/ses_a: http status: 404", Some(("Failed to rename session", "http status: 404"))),
        ("POST /api/session: http status: 500", Some(("Failed to create session", "http status: 500"))),
        ("Saved session to /tmp/a.json", Some(("Session exported", "Saved session to /tmp/a.json"))),
        ("GET /api/session?limit=120: Connection refused", None),
        ("events: Connection refused", None),
    ];
    for (line, want) in cases {
        rt.update(|s, _| {
            s.toast.current = None;
            from_server(s, line.into());
        });
        let got = rt.state.toast.current.as_ref().map(|e| (e.title.clone(), e.description.clone().unwrap_or_default()));
        assert_eq!(got, want.map(|(t, l)| (t.to_owned(), l.to_owned())), "{line}");
        let kept = want.map_or_else(|| line.split(": ").last().unwrap_or(line).trim_end_matches(" (os error 61)"), |(_, why)| why);
        assert!(rt.state.status.contains(kept), "{line}: the debug bar keeps it");
    }
}

#[test]
fn errors_read_without_the_os_code() {
    assert_eq!(plain("No such file or directory (os error 2)"), "No such file or directory");
    assert_eq!(plain("http status: 500"), "http status: 500");
}

#[test]
fn a_library_error_is_said_in_plain_words() {
    assert_eq!(plain("timeout: global"), "OpenCode didn't answer in time");
    assert_eq!(plain("io: Connection refused (os error 61)"), "OpenCode isn't running");
}
