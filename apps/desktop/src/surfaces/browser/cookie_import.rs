//! Settings > Browser's "Import cookie file" (official `settings.tsx:56-61,230-255`,
//! `profile/index.ts:503-516`): the app opens the OS file picker itself, so a plugin never names a
//! path; `#286`'s `read_cookie_file` reads and parses it (regular files only, at most 32 MiB,
//! Netscape `cookies.txt`, Chrome JSON or Playwright storage state); the cookies go to the
//! profile's store through `BrowserHost::import_profile_cookies`, starting the engine if it is idle.
//! One import runs at a time; it fails after 60 s without an answer (`jobs-machine.ts:87`).

use std::time::Duration;

use browser_pane::cookies::{CookieRead, ImportFailure, ProfileEvent, read_cookie_file};
use rune::time::Instant;
use rune::{FileDialogOptions, Tx};
use rune_luau::{Completer, Data};

use crate::App;
use crate::plugin_host::services::map;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// The import the engine has yet to answer: before its cookie store is known, `request` is `None`.
pub struct CookieImport {
    done: Completer,
    read: CookieRead,
    request: Option<u64>,
    deadline: Instant,
}

fn failure(reason: &str) -> Data {
    map([("error", Data::String(reason.into()))])
}

fn reason(f: ImportFailure) -> &'static str {
    match f {
        ImportFailure::Read | ImportFailure::Key | ImportFailure::Access | ImportFailure::Locked => "read",
        ImportFailure::Format => "format",
        ImportFailure::Storage => "storage",
    }
}

impl App {
    /// Opens the picker titled `title` and imports the chosen file; `done` gets
    /// `{ imported, failed, protected, skipped }`, `{ error }` or `{ cancelled = true }`.
    pub(crate) fn start_cookie_file_import(&mut self, title: String, done: Completer, tx: &mut Tx) {
        if self.workspace_panes.browser.cookie_import.is_some() {
            return done.complete(failure("busy"));
        }
        let options = FileDialogOptions { title, ..Default::default() }.filter("Cookies", &["txt", "json"]);
        tx.open_file(options, move |s: &mut App, picked, _tx| {
            let path = match picked {
                Ok(Some(path)) => path,
                Ok(None) => return done.complete(map([("cancelled", Data::Bool(true))])),
                Err(_) => return done.complete(failure("read")),
            };
            let read = match read_cookie_file(&path) {
                Ok(read) => read,
                Err(f) => return done.complete(failure(reason(f))),
            };
            if s.workspace_panes.browser.cookie_import.is_some() {
                return done.complete(failure("busy"));
            }
            let start = !s.use_mock_browser() && !s.browser_unsupported();
            let pane = &mut s.workspace_panes.browser;
            if !pane.host.engine_cookies() && start {
                pane.host.prepare(ANSWER_TIMEOUT);
            }
            pane.cookie_import = Some(CookieImport { done, read, request: None, deadline: Instant::now() + ANSWER_TIMEOUT });
            s.pump_cookie_import();
        });
    }

    /// Sends the waiting import once the engine's cookie store is up and answers it; drains the
    /// profile events so none pile up. True when an import ended.
    pub(crate) fn pump_cookie_import(&mut self) -> bool {
        let pane = &mut self.workspace_panes.browser;
        let events = pane.host.take_profile_events();
        let Some(mut job) = pane.cookie_import.take() else { return false };
        let host = &mut pane.host;
        if job.request.is_none() && host.engine_cookies() {
            job.request = host.import_profile_cookies(None, std::mem::take(&mut job.read.cookies));
        }
        let answered = events.into_iter().find_map(|ev| match ev {
            ProfileEvent::CookiesImported { request_id, imported, failed } if Some(request_id) == job.request => Some((imported, failed)),
            _ => None,
        });
        let result = if let Some((imported, failed)) = answered {
            map([
                ("imported", Data::Number(imported as f64)),
                ("failed", Data::Number((failed + job.read.failed) as f64)),
                ("protected", Data::Number(job.read.protected as f64)),
                ("skipped", Data::Number(job.read.skipped as f64)),
            ])
        } else if job.request.is_none() && host.engine_caps_known() && !host.engine_cookies() {
            failure("failed")
        } else if Instant::now() >= job.deadline {
            failure("failed")
        } else {
            pane.cookie_import = Some(job);
            return false;
        };
        job.done.complete(result);
        true
    }
}
