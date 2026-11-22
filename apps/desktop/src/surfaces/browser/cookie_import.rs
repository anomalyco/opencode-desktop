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

/// Clear browsing data's engine half (`profile/index.ts:523-540` `clear`): every cookie, HTTP sign-in,
/// site's stored data and the HTTP cache, through `ClearProfileData`.
/// First the profile's cookies are listed (`listed`), so the engine can check afterwards that none of
/// those sites keeps stored data (`sites`); then the clear is asked (`asked`).
struct ClearJob {
    done: Completer,
    listed: Option<u64>,
    sites: Option<Vec<String>>,
    asked: Option<u64>,
    /// The engine's clear went; the local-storage check (`local_storage`) runs on a worker.
    checking: Option<Arc<Mutex<Option<bool>>>>,
    deadline: Instant,
}

/// The origins a site's cookies belong to: `http://` and `https://` of each host (an IPv6 host
/// in brackets), once each; a host that makes no origin is left out.
fn cookie_sites(cookies: &[browser_pane::protocol::WireCookie]) -> Vec<String> {
    let mut sites: Vec<String> = cookies
        .iter()
        .map(|c| c.host.trim_start_matches('.').to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .map(|h| if h.contains(':') && !h.starts_with('[') { format!("[{h}]") } else { h })
        .flat_map(|h| [format!("https://{h}"), format!("http://{h}")])
        .filter(|s| browser_policy::destination_origin(s).as_deref() == Some(s.as_str()))
        .collect();
    sites.sort();
    sites.dedup();
    sites
}

/// The profile's cookie count (`profile/index.ts:517-522` `summary`: every cookie of the shared
/// profile, session cookies included, never a remote server's). While the engine runs it is asked,
/// when Settings > Browser asks and the last read is older than `COUNT_FRESH` or an import or clear
/// changed the store. While it is idle the count comes from the profile's own cookie store on disk
/// (`disk`), read once and kept until the engine next runs or a clear. `None` until a read lands.
#[derive(Default)]
struct CookieCount {
    value: Option<usize>,
    asked: Option<u64>,
    read_at: Option<Instant>,
    stale: bool,
    disk: Arc<Mutex<DiskCount>>,
}

/// The on-disk count: whether a read finished since the engine last ran or a clear (`done`),
/// what it found (`None` when the store could not be read), and whether one is on its worker now.
/// `generation` moves at each of those, so a read started before one never lands after it.
#[derive(Default)]
struct DiskCount {
    done: bool,
    value: Option<usize>,
    reading: bool,
    generation: u64,
}

const COUNT_FRESH: Duration = Duration::from_secs(5);

impl CookieJobs {
    /// The last count read: part of `Dep::Browser`'s version, so Settings > Browser draws it once it lands.
    pub fn counted(&self) -> (Option<usize>, Option<usize>) {
        (self.count.value, self.count.disk.lock().ok().and_then(|d| d.value))
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
        if host.engine_cookies() {
            return jobs.count.value;
        }
        self.disk_cookie_count()
    }

    /// The count from the profile's cookie store while the engine is idle: a read-only snapshot of
    /// `Default/Network/Cookies` (`snapshot_database`: no lock on the live file, no `-shm` beside
    /// it) and `SELECT COUNT(*)`, as no value needs decrypting; the engine's own persistence probe
    /// cookie is not counted, as its listing leaves it out. Read on a worker, once.
    fn disk_cookie_count(&self) -> Option<usize> {
        let disk = self.workspace_panes.browser.cookies.count.disk.clone();
        let mut d = disk.lock().ok()?;
        if d.done || d.reading {
            return d.value;
        }
        let profile = self.workspace_panes.browser.host.engine_profile_dir()?;
        d.reading = true;
        let generation = d.generation;
        drop(d);
        let waker = (*self.waker).clone();
        let filled = disk.clone();
        let spawned = std::thread::Builder::new().name("browser-cookie-count".into()).spawn(move || {
            let count = read_disk_count(&profile);
            if let Ok(mut d) = filled.lock() {
                d.reading = false;
                if d.generation == generation {
                    d.done = true;
                    d.value = count;
                }
            }
            if let Some(w) = &waker {
                w();
            }
        });
        if spawned.is_err()
            && let Ok(mut d) = disk.lock()
        {
            d.reading = false;
        }
        None
    }

    /// Clears every site's cookies, sign-ins, stored data and cache, starting the engine if it is
    /// idle; `done` gets whether the engine did and found nothing left. One clear runs at a time.
    /// With no engine to run it (none in this build, not installed, or failed to start) it answers
    /// at once: done when the profile was never made on disk (nothing to clear), else failed.
    pub(crate) fn start_profile_clear(&mut self, done: Completer) {
        if self.workspace_panes.browser.cookies.clear.is_some() {
            return done.complete(Data::Bool(false));
        }
        if self.use_mock_browser() || self.browser_unsupported() {
            return done.complete(Data::Bool(false));
        }
        let pane = &mut self.workspace_panes.browser;
        if !pane.host.engine_cookies() {
            if let Some(cleared) = self.clear_without_engine() {
                return done.complete(Data::Bool(cleared));
            }
            self.workspace_panes.browser.host.prepare(ANSWER_TIMEOUT);
        }
        self.workspace_panes.browser.cookies.clear =
            Some(ClearJob { done, listed: None, sites: None, asked: None, checking: None, deadline: Instant::now() + ANSWER_TIMEOUT });
    }

    /// When no engine can run the clear: `Some(true)` if the profile was never made on disk,
    /// `Some(false)` if it was, `None` when the engine can be started for it.
    fn clear_without_engine(&self) -> Option<bool> {
        let host = &self.workspace_panes.browser.host;
        if !matches!(host.status(), browser_pane::EngineStatus::NotInstalled | browser_pane::EngineStatus::Failed(_)) {
            return None;
        }
        Some(host.engine_profile_dir().is_none_or(|p| !p.join("Default").exists()))
    }

    /// When the clear or an import next needs a look without anything else waking the app.
    pub(crate) fn cookie_jobs_deadline(&self) -> Option<Instant> {
        let jobs = &self.workspace_panes.browser.cookies;
        [jobs.clear.as_ref().map(|j| j.deadline), jobs.import.as_ref().map(|j| j.deadline)].into_iter().flatten().min()
    }

    /// Moves the cookie import, the clear and the count on. True when one of them changed.
    pub(crate) fn pump_cookie_import(&mut self) -> bool {
        let imported = self.pump_import();
        let cleared = self.pump_clear();
        let running = self.workspace_panes.browser.host.engine_cookies();
        let count = &mut self.workspace_panes.browser.cookies.count;
        if imported || cleared {
            count.stale = true;
        }
        // The engine runs (or a clear ran): what the disk said is old by the time it is idle again.
        if (cleared || running)
            && let Ok(mut d) = count.disk.lock()
            && (d.done || d.reading)
        {
            d.generation += 1;
            d.done = false;
            d.value = None;
        }
        let counted = self.pump_count();
        imported || cleared || counted
    }

    fn pump_count(&mut self) -> bool {
        let pane = &mut self.workspace_panes.browser;
        let Some(asked) = pane.cookies.count.asked else { return false };
        let listed = pane.host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::CookiesListed { request_id, .. } if *request_id == asked));
        if let Some(ProfileEvent::CookiesListed { cookies, .. }) = listed.into_iter().next() {
            let count = &mut pane.cookies.count;
            count.value = Some(cookies.len());
            count.asked = None;
            count.read_at = Some(Instant::now());
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
        // The sites the profile has cookies for, so the engine can check none keeps stored data.
        if job.sites.is_none() && job.listed.is_none() && host.engine_cookies() {
            job.listed = host.request_profile_cookies(None);
        }
        if let Some(listed) = job.listed {
            let answer = host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::CookiesListed { request_id, .. } if *request_id == listed));
            if let Some(ProfileEvent::CookiesListed { cookies, .. }) = answer.into_iter().next() {
                job.sites = Some(cookie_sites(&cookies));
                job.listed = None;
            }
        }
        if job.asked.is_none()
            && let Some(sites) = job.sites.clone()
        {
            job.asked = host.clear_profile_data(sites);
        }
        let done = job.asked.and_then(|asked| {
            host.take_profile_events_matching(|ev| matches!(ev, ProfileEvent::ActionDone { request_id, .. } if *request_id == asked))
                .into_iter()
                .find_map(|ev| if let ProfileEvent::ActionDone { ok, .. } = ev { Some(ok) } else { None })
        });
        // DevTools reports no size for local storage: the engine's check cannot see a site that
        // kept only that, so the profile's own local-storage store is read before "Cleared".
        if done == Some(true) {
            let slot = Arc::new(Mutex::new(None));
            match host.engine_profile_dir() {
                Some(profile) => {
                    let filled = slot.clone();
                    let waker = (*self.waker).clone();
                    let spawned = std::thread::Builder::new().name("browser-clear-check".into()).spawn(move || {
                        let clean = local_storage_cleared(&profile);
                        if let Ok(mut s) = filled.lock() {
                            *s = Some(clean);
                        }
                        if let Some(w) = &waker {
                            w();
                        }
                    });
                    if spawned.is_err() {
                        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(false);
                    }
                }
                None => *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(false),
            }
            job.checking = Some(slot);
            job.deadline = Instant::now() + ANSWER_TIMEOUT;
        }
        let checked = job.checking.as_ref().and_then(|slot| slot.lock().ok().and_then(|s| *s));
        let result = match done {
            Some(false) => false,
            // No profile folder to read, or no worker to read it: not cleared, at once.
            Some(true) => match checked {
                Some(clean) => clean,
                None => {
                    pane.cookies.clear = Some(job);
                    return false;
                }
            },
            None if job.checking.is_some() => match checked {
                Some(clean) => clean,
                None if Instant::now() >= job.deadline => false,
                None => {
                    pane.cookies.clear = Some(job);
                    return false;
                }
            },
            None if job.asked.is_none() && host.engine_caps_known() && !host.engine_cookies() => false,
            // No engine after all: done when the profile was never made on disk, as on a first click.
            None if matches!(host.status(), browser_pane::EngineStatus::NotInstalled | browser_pane::EngineStatus::Failed(_)) => {
                host.engine_profile_dir().is_none_or(|p| !p.join("Default").exists())
            }
            None if ((job.asked.is_some() || job.listed.is_some()) && !host.has_live_sidecar()) || Instant::now() >= job.deadline => false,
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

/// The shared profile's cookie count on disk (`Default/Network/Cookies`, else Chromium's older
/// `Default/Cookies`), expired ones left out: 0 when there is no store yet, `None` when it cannot be read.
fn read_disk_count(profile: &std::path::Path) -> Option<usize> {
    let default = profile.join("Default");
    let Some(file) = [default.join("Network").join("Cookies"), default.join("Cookies")].into_iter().find(|f| f.is_file()) else {
        return Some(0);
    };
    // Chromium's time: microseconds since 1601-01-01; an expired cookie is not counted, as the
    // engine's listing leaves it out.
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
    let now = unix + 11_644_473_600 * 1_000_000;
    let scratch = SnapshotScratch::new(SnapshotScratch::default_dir());
    browser_pane::cookies::database::snapshot_database(&file, &scratch, |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM cookies WHERE NOT (trim(host_key, '.') = ?1 AND name = ?2) AND (has_expires = 0 OR expires_utc > CAST(?3 AS INTEGER))",
            [PROBE_COOKIE_HOST, PROBE_COOKIE_NAME, now.to_string().as_str()],
            |r| r.get::<_, i64>(0),
        )
    })
    .ok()
    .and_then(|n| usize::try_from(n).ok())
}

/// The engine's persistence probe cookie (`apps/browser-engine/src/cookies.rs`), never counted.
const PROBE_COOKIE_HOST: &str = "cookie-check.opencode.invalid";
const PROBE_COOKIE_NAME: &str = "opencode_cookie_check";

/// Whether no site keeps local storage in the shared profile after a clear: read up to three
/// times, 1.5 s apart, as the storage service may still be writing its deletions or compacting
/// (a torn copy); a store still unread after the third counts as not cleared.
fn local_storage_cleared(profile: &std::path::Path) -> bool {
    let scratch = SnapshotScratch::new(SnapshotScratch::default_dir());
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(1500));
        }
        // The reader may meet a file torn by the copy (Chromium compacting mid-read): a panic there is
        // one failed attempt, never a worker that dies without an answer.
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            browser_pane::cookies::local_storage::sites_with_local_storage(profile, &scratch)
        }));
        match read.unwrap_or(Err(browser_pane::cookies::ImportFailure::Read)) {
            Ok(sites) if sites.is_empty() => return true,
            Ok(_) | Err(_) => {}
        }
    }
    false
}
