use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::palette::pal;

#[derive(Default)]
struct Mock {
    calls: Mutex<Vec<String>>,
    fail: bool,
}

impl Launcher for Mock {
    fn installed(&self, bundles: &[&'static str]) -> Vec<&'static str> {
        bundles.iter().copied().filter(|b| ["dev.zed.Zed", "com.microsoft.VSCode", "com.apple.Terminal"].contains(b)).collect()
    }
    fn open(&self, path: &str, bundle: Option<&str>) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("open {path} {}", bundle.unwrap_or("default")));
        if self.fail { Err("no such app".into()) } else { Ok(()) }
    }
    fn reveal(&self, path: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("reveal {path}"));
        Ok(())
    }
}

fn app_with(mock: Arc<Mock>) -> App {
    let mut a = App::fixture_session();
    a.open_in = OpenIn::with(mock);
    a.open_in.detect();
    settle(&mut a);
    a
}

/// Polls until the background work has landed.
fn settle(a: &mut App) {
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        a.poll_open_in();
        if !a.open_in.busy() && !a.open_in.found.is_pending() && !a.open_in.done.is_pending() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("open-in work never finished");
}

#[test]
fn only_installed_apps_are_listed_with_finder_first() {
    let a = app_with(Arc::new(Mock::default()));
    let ids: Vec<_> = a.open_in.options().into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, ["finder", "vscode", "zed", "terminal"]);
}

#[test]
fn nothing_is_listed_but_finder_until_the_check_answers() {
    let o = OpenIn::with(Arc::new(Mock::default()));
    assert_eq!(o.options(), [("finder", "Finder")]);
    assert_eq!(o.current(), "finder");
}

#[test]
fn a_pick_opens_the_file_in_that_app_and_is_remembered() {
    let mock = Arc::new(Mock::default());
    let mut a = app_with(mock.clone());
    let dir = a.session(a.active_session_id().unwrap()).unwrap().directory.clone();
    a.open_in_app("zed", "src/main.rs", true);
    settle(&mut a);
    assert_eq!(*mock.calls.lock().unwrap(), [format!("open {dir}/src/main.rs dev.zed.Zed")]);
    assert_eq!(a.open_in.current(), "zed");
    assert_eq!(a.open_in.current_label(), "Zed");
}

#[test]
fn the_main_button_opens_with_the_remembered_app_without_changing_it() {
    let mock = Arc::new(Mock::default());
    let mut a = app_with(mock.clone());
    a.open_in.set_chosen(Some("vscode".into()));
    let app = a.open_in.current().to_owned();
    a.open_in_app(&app, "a.txt", false);
    settle(&mut a);
    assert!(mock.calls.lock().unwrap()[0].ends_with("a.txt com.microsoft.VSCode"));
    assert_eq!(a.open_in.current(), "vscode");
}

#[test]
fn finder_reveals_instead_of_opening() {
    let mock = Arc::new(Mock::default());
    let mut a = app_with(mock.clone());
    a.open_in_app(FINDER, "a.txt", true);
    settle(&mut a);
    assert!(mock.calls.lock().unwrap()[0].starts_with("reveal "));
    assert_eq!(a.open_in.current(), "finder");
}

#[test]
fn a_remembered_app_that_is_gone_falls_back_to_finder() {
    let mut a = app_with(Arc::new(Mock::default()));
    a.open_in.set_chosen(Some("warp".into()));
    assert_eq!(a.open_in.current(), "finder");
}

#[test]
fn a_failed_launch_says_so() {
    let mut a = app_with(Arc::new(Mock { fail: true, ..Default::default() }));
    a.open_in_app("zed", "a.txt", true);
    settle(&mut a);
    assert_eq!(a.status, "Couldn't open: no such app");
}

#[test]
fn a_second_open_waits_for_the_first() {
    let mock = Arc::new(Mock::default());
    let mut a = app_with(mock.clone());
    a.open_in_app("zed", "a.txt", true);
    a.open_in_app("vscode", "b.txt", true);
    settle(&mut a);
    assert_eq!(mock.calls.lock().unwrap().len(), 1);
}

#[test]
fn the_control_is_hidden_when_the_server_is_remote() {
    let mut a = app_with(Arc::new(Mock::default()));
    let p = pal(true);
    assert!(button(&a, &p, "files-open-in", "a.txt").is_some());
    a.source = crate::Source::Live {
        server: opencode_kit::client::Server::new("https://host.example.ts.net:4096"),
        requests: None,
        writes: None,
        allow_send: false,
    };
    assert!(!a.can_open_in());
    assert!(button(&a, &p, "files-open-in", "a.txt").is_none());
}
