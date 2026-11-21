//! Settings > Browser's "Import cookie file" (official `settings.tsx:56-61,230-255`,
//! `profile/index.ts:503-516`): the app opens the OS file picker itself, so a plugin never names a
//! path. `#286`'s `read_cookie_file` reads and parses it (regular files only, at most 32 MiB,
//! Netscape `cookies.txt`, Chrome JSON or Playwright storage state) on a worker thread, and
//! `prepare_for_import` drops what no browser would send (expired, nameless, or a host that is not a
//! plain hostname or IP literal: `valid_cookie_host`). The cookies go to the profile's store through
//! `BrowserHost::import_profile_cookies` in batches of at most 1,000 cookies / 2 MiB (one wire line
//! each, far under the engine's line cap), starting the engine if it is idle. Like official's
//! `writeCookies` (`profile/import.ts:10-38`) it writes every cookie with no per-site cap; Chromium
//! evicts past its own limits. One import runs at a time; it fails after 60 s without an answer
//! (`jobs-machine.ts:87`; counted from the last batch answered), or at once when the engine goes away.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use browser_pane::cookies::database::SnapshotScratch;
use browser_pane::cookies::format::prepare_for_import;
use browser_pane::cookies::sources::read_source_cookies;
use browser_pane::cookies::{CookieRead, ImportFailure, ProfileEvent, read_cookie_file};
use browser_pane::protocol::WireCookie;
use rune::time::Instant;
use rune::{FileDialogOptions, Tx};
use rune_luau::{Completer, Data};

use crate::App;
use crate::plugin_host::services::map;

const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
/// One `ProfileCookiesSet` line: at most this many cookies ...
const BATCH_COOKIES: usize = 1_000;
/// ... and about this many bytes of cookie text.
const BATCH_BYTES: usize = 2 * 1024 * 1024;
/// A picker title longer than this is cut.
const MAX_TITLE: usize = 200;

type Read = Result<CookieRead, ImportFailure>;

/// What the import brings: still being read on the worker, or read and waiting to be sent.
enum Reading {
    Pending(Arc<Mutex<Option<Read>>>),
    Ready(CookieRead),
}

/// Settings > Browser's work on the profile's cookie store: one import, one Clear browsing data,
/// and the cookie count its Browser data row shows.
#[derive(Default)]
pub struct CookieJobs {
    pub(super) import: Option<CookieImport>,
    clear: Option<ClearJob>,
    count: CookieCount,
}

/// Clear browsing data's engine half (`profile/index.ts:518-540` `clear`): every cookie and the
/// open sites' stored data, through `#286`'s `ClearProfileData`.
struct ClearJob {
    done: Completer,
    asked: Option<u64>,
    deadline: Instant,
}

/// The profile's cookie count (`profile/index.ts:517-522` `summary`): read from the engine while
/// it runs, when Settings > Browser asks and the last read is older than `COUNT_FRESH` or an
/// import or clear changed the store. `None` until a read lands.
#[derive(Default)]
struct CookieCount {
    value: Option<usize>,
    asked: Option<u64>,
    read_at: Option<Instant>,
    stale: bool,
}

const COUNT_FRESH: Duration = Duration::from_secs(5);

impl CookieJobs {
    /// The last count read: part of `Dep::Browser`'s version, so Settings > Browser draws it once it lands.
    pub fn counted(&self) -> Option<usize> {
        self.count.value
    }
}

/// The import the engine has yet to answer.
pub struct CookieImport {
    done: Completer,
    read: Reading,
    /// Another browser's sign-ins (its read failures keep their own names), not a file.
    browser: bool,
    /// Batches still to send, the requests the engine has yet to answer, and its answers so far.
    batches: Vec<Vec<WireCookie>>,
    waiting: HashSet<u64>,
    sent: bool,
    imported: u32,
    failed: u32,
    deadline: Instant,
}

fn failure(reason: &str) -> Data {
    map([("error", Data::String(reason.into()))])
}

/// A browser's read failure as official names it (`ReadFailure`: `key`, `access`, `locked`, `read`).
fn browser_reason(f: ImportFailure) -> &'static str {
    match f {
        ImportFailure::Key => "key",
        ImportFailure::Access => "access",
        ImportFailure::Locked => "locked",
        ImportFailure::Storage => "storage",
        ImportFailure::Read | ImportFailure::Format => "read",
    }
}

fn reason(f: ImportFailure) -> &'static str {
    match f {
        ImportFailure::Read | ImportFailure::Key | ImportFailure::Access | ImportFailure::Locked => "read",
        ImportFailure::Format => "format",
        ImportFailure::Storage => "storage",
    }
}

/// Splits `cookies` into batches of at most `BATCH_COOKIES` cookies and about `BATCH_BYTES` bytes.
fn batches(cookies: Vec<WireCookie>) -> Vec<Vec<WireCookie>> {
    let mut out: Vec<Vec<WireCookie>> = Vec::new();
    let mut current: Vec<WireCookie> = Vec::new();
    let mut bytes = 0usize;
    for c in cookies {
        let size = c.host.len() + c.name.len() + c.value.len() + c.path.len() + 128;
        if !current.is_empty() && (current.len() >= BATCH_COOKIES || bytes + size > BATCH_BYTES) {
            out.push(std::mem::take(&mut current));
            bytes = 0;
        }
        bytes += size;
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

impl App {
    /// Opens the picker titled `title` and imports the chosen file; `done` gets
    /// `{ imported, failed, protected, skipped }`, `{ error }` or `{ cancelled = true }`.
    pub(crate) fn start_cookie_file_import(&mut self, title: String, done: Completer, tx: &mut Tx) {
        if self.workspace_panes.browser.cookies.import.is_some() {
            return done.complete(failure("busy"));
        }
        let title: String = title.chars().filter(|c| !c.is_control()).take(MAX_TITLE).collect();
        let options = FileDialogOptions { title, ..Default::default() }.filter("Cookies", &["txt", "json"]);
        tx.open_file(options, move |s: &mut App, picked, _tx| {
            let path = match picked {
                Ok(Some(path)) => path,
                Ok(None) => return done.complete(map([("cancelled", Data::Bool(true))])),
                Err(_) => return done.complete(failure("read")),
            };
            if s.workspace_panes.browser.cookies.import.is_some() {
                return done.complete(failure("busy"));
            }
            // Reading and parsing up to 32 MiB happens off the UI thread.
            let slot: Arc<Mutex<Option<Read>>> = Arc::new(Mutex::new(None));
            let filled = slot.clone();
            let waker = (*s.waker).clone();
            let spawned = std::thread::Builder::new().name("browser-cookie-file".into()).spawn(move || {
                let read = read_cookie_file(&path);
                if let Ok(mut s) = filled.lock() {
                    *s = Some(read);
                }
                if let Some(w) = &waker {
                    w();
                }
            });
            if spawned.is_err() {
                return done.complete(failure("read"));
            }
            let start = !s.use_mock_browser() && !s.browser_unsupported();
            let pane = &mut s.workspace_panes.browser;
            if !pane.host.engine_cookies() && start {
                pane.host.prepare(ANSWER_TIMEOUT);
            }
            pane.cookies.import = Some(CookieImport {
                done,
                read: Reading::Pending(slot),
                browser: false,
                batches: Vec::new(),
                waiting: HashSet::new(),
                sent: false,
                imported: 0,
                failed: 0,
                deadline: Instant::now() + ANSWER_TIMEOUT,
            });
        });
    }

    /// Imports the sign-ins of browser `source`'s profile `profile` (ids from `list_sources`, never
    /// a path), or with `sites` only those sites' (`profile/index.ts:478-486` `importBrowser`): `#286`'s
    /// `read_source_cookies` reads it on a worker thread (it copies databases and may ask the OS key
    /// store), then the cookies take the file's path: `prepare_for_import`, batches, the engine.
    /// `done` gets what a file import's does, with `error` one of `key`, `access`, `locked`, `read`,
    /// `storage`, `busy`, `failed`.
    pub(crate) fn start_browser_import(&mut self, source: String, profile: String, sites: Option<Vec<String>>, done: Completer) {
        if self.workspace_panes.browser.cookies.import.is_some() {
            return done.complete(failure("busy"));
        }
        let slot: Arc<Mutex<Option<Read>>> = Arc::new(Mutex::new(None));
        let filled = slot.clone();
        let waker = (*self.waker).clone();
        let spawned = std::thread::Builder::new().name("browser-signin-import".into()).spawn(move || {
            let scratch = SnapshotScratch::new(SnapshotScratch::default_dir());
            let read = read_source_cookies(&source, &profile, &scratch, sites.as_deref());
            if let Ok(mut s) = filled.lock() {
                *s = Some(read);
            }
            if let Some(w) = &waker {
                w();
            }
        });
        if spawned.is_err() {
            return done.complete(failure("failed"));
        }
        let start = !self.use_mock_browser() && !self.browser_unsupported();
        let pane = &mut self.workspace_panes.browser;
        if !pane.host.engine_cookies() && start {
            pane.host.prepare(ANSWER_TIMEOUT);
        }
        pane.cookies.import = Some(CookieImport {
            done,
            read: Reading::Pending(slot),
            browser: true,
            batches: Vec::new(),
            waiting: HashSet::new(),
            sent: false,
            imported: 0,
            failed: 0,
            deadline: Instant::now() + ANSWER_TIMEOUT,
        });
    }

    /// The profile's cookie count for Settings > Browser, `None` while the engine is idle or before
    /// the first read lands. Asks the engine again (on the next turn) when the count is old.
    pub(crate) fn browser_cookie_count(&self) -> Option<usize> {
        let jobs = &self.workspace_panes.browser.cookies;
        let host = &self.workspace_panes.browser.host;
        let old = jobs.count.stale || jobs.count.read_at.is_none_or(|at| at.elapsed() >= COUNT_FRESH);
        if old && jobs.count.asked.is_none() && host.engine_cookies() {
            self.later(Box::new(|s: &mut App, _: &mut Tx| {
                let pane = &mut s.workspace_panes.browser;
                if pane.cookies.count.asked.is_none() {
                    pane.cookies.count.asked = pane.host.request_profile_cookies(None);
                    pane.cookies.count.stale = false;
                }
            }));
        }
        jobs.count.value.filter(|_| host.engine_cookies())
    }

    /// Clears every cookie and the open sites' stored data, starting the engine if it is idle;
    /// `done` gets whether the engine did. One clear runs at a time.
    pub(crate) fn start_profile_clear(&mut self, done: Completer) {
        if self.workspace_panes.browser.cookies.clear.is_some() {
            return done.complete(Data::Bool(false));
        }
        let start = !self.use_mock_browser() && !self.browser_unsupported();
        let pane = &mut self.workspace_panes.browser;
        if !pane.host.engine_cookies() && start {
            pane.host.prepare(ANSWER_TIMEOUT);
        }
        pane.cookies.clear = Some(ClearJob { done, asked: None, deadline: Instant::now() + ANSWER_TIMEOUT });
    }

    /// Moves the cookie import, the clear and the count on. True when one of them changed.
    pub(crate) fn pump_cookie_import(&mut self) -> bool {
        let imported = self.pump_import();
        let cleared = self.pump_clear();
        if imported || cleared {
            self.workspace_panes.browser.cookies.count.stale = true;
        }
        let counted = self.pump_count();
        imported || cleared || counted
    }

    fn pump_count(&mut self) -> bool {
        let pane = &mut self.workspace_panes.browser;
        let Some(asked) = pane.cookies.count.asked else { return false };
        let listed = pane.host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::CookiesListed { request_id, .. } if *request_id == asked));
        if let Some(ProfileEvent::CookiesListed { cookies, .. }) = listed.into_iter().next() {
            pane.cookies.count = CookieCount { value: Some(cookies.len()), asked: None, read_at: Some(Instant::now()), stale: pane.cookies.count.stale };
            return true;
        }
        if !pane.host.has_live_sidecar() {
            pane.cookies.count.asked = None;
        }
        false
    }

    fn pump_clear(&mut self) -> bool {
        let pane = &mut self.workspace_panes.browser;
        let Some(mut job) = pane.cookies.clear.take() else { return false };
        let host = &mut pane.host;
        if job.asked.is_none() && host.engine_cookies() {
            job.asked = host.clear_profile_data();
        }
        let done = job.asked.and_then(|asked| {
            host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::ActionDone { request_id, .. } if *request_id == asked))
                .into_iter()
                .find_map(|ev| if let ProfileEvent::ActionDone { ok, .. } = ev { Some(ok) } else { None })
        });
        let result = match done {
            Some(ok) => ok,
            None if job.asked.is_none() && host.engine_caps_known() && !host.engine_cookies() => false,
            None if (job.asked.is_some() && !host.has_live_sidecar()) || Instant::now() >= job.deadline => false,
            None => {
                pane.cookies.clear = Some(job);
                return false;
            }
        };
        job.done.complete(Data::Bool(result));
        true
    }

    /// Moves the waiting import on: takes the worker's read, sends its batches once the engine's
    /// cookie store is up, and answers once every batch is answered. Takes only its own profile
    /// events; others stay for whoever asked. True when an import ended.
    fn pump_import(&mut self) -> bool {
        let pane = &mut self.workspace_panes.browser;
        let Some(mut job) = pane.cookies.import.take() else { return false };
        let host = &mut pane.host;
        if let Reading::Pending(slot) = &job.read {
            match slot.lock().ok().and_then(|mut s| s.take()) {
                Some(Ok(read)) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                    let (valid, skipped) = prepare_for_import(&read.cookies, now);
                    job.batches = batches(valid);
                    job.batches.reverse();
                    job.read = Reading::Ready(CookieRead { cookies: Vec::new(), skipped: read.skipped + skipped, ..read });
                }
                Some(Err(f)) => {
                    job.done.complete(failure(if job.browser { browser_reason(f) } else { reason(f) }));
                    return true;
                }
                None if Instant::now() >= job.deadline => {
                    job.done.complete(failure("failed"));
                    return true;
                }
                None => {
                    pane.cookies.import = Some(job);
                    return false;
                }
            }
        }
        let Reading::Ready(read) = &job.read else { return false };
        if !job.sent && host.engine_cookies() {
            while let Some(batch) = job.batches.pop() {
                match host.import_profile_cookies(None, batch) {
                    Some(id) => {
                        job.waiting.insert(id);
                    }
                    None => break,
                }
            }
            job.sent = job.batches.is_empty();
        }
        let waiting = &job.waiting;
        for ev in host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::CookiesImported { request_id, .. } if waiting.contains(request_id))) {
            if let ProfileEvent::CookiesImported { request_id, imported, failed } = ev {
                job.waiting.remove(&request_id);
                job.imported += imported;
                job.failed += failed;
                // A large file answers batch by batch: the 60 s run from the last answer.
                job.deadline = Instant::now() + ANSWER_TIMEOUT;
            }
        }
        let result = if job.sent && job.waiting.is_empty() {
            map([
                ("imported", Data::Number(f64::from(job.imported))),
                ("failed", Data::Number(f64::from(job.failed + read.failed))),
                ("protected", Data::Number(f64::from(read.protected))),
                ("skipped", Data::Number(f64::from(read.skipped))),
            ])
        } else if !job.sent && host.engine_caps_known() && !host.engine_cookies() {
            failure("failed")
        } else if (job.sent && !host.has_live_sidecar()) || Instant::now() >= job.deadline {
            // The engine went away with batches unanswered, or it never answered.
            failure("failed")
        } else {
            pane.cookies.import = Some(job);
            return false;
        };
        job.done.complete(result);
        true
    }
}
