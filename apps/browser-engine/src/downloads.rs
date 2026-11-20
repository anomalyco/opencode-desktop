//! A page's downloads, as Desktop's browser keeps them (`gui-plugins/src/browser/chromium.ts`
//! `will-download`, `files.ts`): each file goes to a folder of its own in the engine's private
//! downloads folder, with no dialog, for the agent's tools to read; one over 5 MiB (Desktop's
//! `MAX_FILE_BYTES`) is cancelled, and a tab keeps at most 64 (a 65th is refused as `too_many`:
//! Desktop has no limit). A tab's files go when it closes (one still running is cancelled), and
//! the engine's go when it ends and again when it starts (after an engine that didn't end cleanly).
//! The app hears each start, step and end (`EngineMsg::Download`).
//!
//! Only what the engine made is ever deleted: the folder is `opencode-page-downloads`, a name no
//! person's folder has, and it holds a marker the engine writes when it makes it. A folder of that
//! name without the marker (or one the engine can't make) is left alone and downloads fail as
//! `file_failed`; a tab's files are removed by the folders the engine handed out, nothing else.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cef::rc::Rc as _;
use cef::{
    BeforeDownloadCallback, Browser, CefString, DownloadHandler, DownloadInterruptReason, DownloadItem, DownloadItemCallback,
    ImplBeforeDownloadCallback, ImplDownloadHandler, ImplDownloadItem, ImplDownloadItemCallback, WrapDownloadHandler, wrap_download_handler,
};

use crate::output::Output;
use crate::wire::{DownloadFile, DownloadState, EngineMsg};

/// The downloads folder's name in the engine's profile.
pub const DIR_NAME: &str = "opencode-page-downloads";
/// The file that says the engine made the folder: without it nothing in the folder is deleted.
const MARKER: &str = ".opencode-page-downloads";
/// Desktop's limit on a file the agent's tools take (`plugin-browser/src/rpc.ts`).
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// A tab's files at most: a page that keeps starting downloads stops at this many.
pub const MAX_TAB_FILES: usize = 64;

/// The engine's downloads: its folder (when it's the engine's own), each file by Chromium's
/// download id with its tab, and the tabs that closed.
pub struct Downloads {
    /// None when the folder isn't the engine's to use (it exists without the marker, or can't be
    /// made): downloads then fail, and nothing is deleted.
    dir: Option<PathBuf>,
    next: AtomicU64,
    files: Mutex<HashMap<u32, (u64, DownloadFile)>>,
    /// Tabs that closed: a download of theirs still running is cancelled at its next step.
    closed: Mutex<HashSet<u64>>,
}

impl Downloads {
    /// `profile`'s downloads folder, emptied when it's the engine's, made (with its marker) when
    /// there's none.
    pub fn open(profile: &Path) -> Arc<Self> {
        let dir = profile.join(DIR_NAME);
        remove_owned(profile);
        let made = !dir.exists() && std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join(MARKER), b"").is_ok();
        Arc::new(Self { dir: made.then_some(dir), next: AtomicU64::new(1), files: Mutex::new(HashMap::new()), closed: Mutex::new(HashSet::new()) })
    }

    /// A tab closed: its files go (Desktop's `files.dispose()` on a page's close), by the folders
    /// the engine made for them.
    pub fn close_tab(&self, tab_id: u64) {
        self.closed.lock().unwrap().insert(tab_id);
        let files = self.files.lock().unwrap();
        for (_, file) in files.values().filter(|(tab, _)| *tab == tab_id) {
            remove_file_folder(&file.path);
        }
        if let Some(dir) = &self.dir {
            // Only when empty: a folder the engine didn't fill stays.
            let _ = std::fs::remove_dir(dir.join(tab_id.to_string()));
        }
    }
}

/// Removes `profile`'s downloads folder when the engine made it (its marker is there).
pub fn remove_owned(profile: &Path) {
    let dir = profile.join(DIR_NAME);
    if dir.join(MARKER).is_file() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A downloaded file's own folder (`<dir>/<tab>/<file id>`), with the file in it.
fn remove_file_folder(path: &str) {
    let path = Path::new(path);
    let _ = std::fs::remove_file(path);
    if let Some(folder) = path.parent() {
        let _ = std::fs::remove_dir(folder);
    }
}

wrap_download_handler! {
    pub struct TabDownloadHandler {
        tab_id: u64,
        file_roots: Arc<Mutex<Vec<String>>>,
        is_preview: bool,
        downloads: Arc<Downloads>,
        out_tx: Output,
    }

    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, url: Option<&CefString>, _method: Option<&CefString>) -> ::std::os::raw::c_int {
            if self.is_preview {
                return 0;
            }
            let url = url.map(CefString::to_string).unwrap_or_default();
            let roots = self.file_roots.lock().unwrap().clone();
            if !browser_policy::subresource_allowed(&url, &roots) || browser_policy::is_http_with_credentials(&url) {
                return 0;
            }
            1
        }

        // Where it goes, with no dialog. Returning 1 says the callback is taken; 0 cancels it.
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(item), Some(callback)) = (item, callback) else { return 0 };
            let name = suggested_name.map(|n| n.to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| "file".to_owned());
            let id = format!("file_{}", self.downloads.next.fetch_add(1, Ordering::Relaxed));
            let mime = CefString::from(&item.mime_type()).to_string();
            let mut file = DownloadFile { id, name, mime, bytes: 0, state: DownloadState::Pending, path: String::new(), error: None };
            let held = self.downloads.files.lock().unwrap().values().filter(|(tab, _)| *tab == self.tab_id).count();
            let fail = |mut file: DownloadFile, why: &str| {
                file.state = DownloadState::Failed;
                file.error = Some(why.to_owned());
                self.send(file);
                0
            };
            if held >= MAX_TAB_FILES {
                return fail(file, "too_many");
            }
            let Some(dir) = &self.downloads.dir else { return fail(file, "file_failed") };
            let folder = dir.join(self.tab_id.to_string()).join(&file.id);
            if std::fs::create_dir_all(&folder).is_err() {
                return fail(file, "file_failed");
            }
            let path = folder.join(safe_name(&file.name));
            file.path = path.to_string_lossy().into_owned();
            callback.cont(Some(&CefString::from(file.path.as_str())), 0);
            self.downloads.files.lock().unwrap().insert(item.id(), (self.tab_id, file.clone()));
            self.send(file);
            1
        }

        fn on_download_updated(&self, _browser: Option<&mut Browser>, item: Option<&mut DownloadItem>, callback: Option<&mut DownloadItemCallback>) {
            let Some(item) = item else { return };
            if self.downloads.closed.lock().unwrap().contains(&self.tab_id) {
                if let Some(callback) = callback {
                    callback.cancel();
                }
                if let Some((_, file)) = self.downloads.files.lock().unwrap().remove(&item.id()) {
                    remove_file_folder(&file.path);
                }
                return;
            }
            let mut files = self.downloads.files.lock().unwrap();
            let Some((_, file)) = files.get_mut(&item.id()) else { return };
            if file.state != DownloadState::Pending {
                return;
            }
            let bytes = u64::try_from(item.received_bytes()).unwrap_or_default();
            let step = bytes / (256 * 1024) != file.bytes / (256 * 1024);
            file.bytes = bytes;
            if bytes > MAX_FILE_BYTES {
                if let Some(callback) = callback {
                    callback.cancel();
                }
                file.state = DownloadState::Failed;
                file.error = Some("too_large".to_owned());
            } else if item.is_complete() != 0 {
                file.state = DownloadState::Completed;
            } else if item.is_canceled() != 0 {
                file.state = DownloadState::Failed;
                file.error = Some("canceled".to_owned());
            } else if item.is_interrupted() != 0 {
                file.state = DownloadState::Failed;
                file.error = Some(reason(item.interrupt_reason()));
            } else if !step {
                return;
            }
            let file = file.clone();
            drop(files);
            if file.state == DownloadState::Failed {
                let _ = std::fs::remove_file(&file.path);
            }
            self.send(file);
        }
    }
}

impl TabDownloadHandler {
    fn send(&self, file: DownloadFile) {
        let _ = self.out_tx.send(EngineMsg::Download { tab_id: self.tab_id, file });
    }
}

/// A name every supported filesystem takes, as Desktop's `files.ts` makes it: characters none
/// accepts replaced, trailing dots and spaces dropped, at most 120 characters (its last ones);
/// and a name Windows keeps for a device (`CON`, `NUL.txt`, `COM1`) gets a `_` in front.
fn safe_name(name: &str) -> PathBuf {
    let cleaned: String = name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    let chars: Vec<char> = trimmed.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(120)..].iter().collect();
    let tail = if tail.is_empty() { "file".to_owned() } else { tail };
    let stem = tail.split('.').next().unwrap_or_default().trim_end();
    let device = browser_policy::is_windows_device_name(stem);
    PathBuf::from(if device { format!("_{tail}") } else { tail })
}

/// Chromium's interrupt reason, by its `DownloadInterruptReason` name in snake case.
fn reason(reason: DownloadInterruptReason) -> String {
    let debug = format!("{:?}", reason.as_ref());
    let name = debug.rsplit("CEF_DOWNLOAD_INTERRUPT_REASON_").next().unwrap_or(&debug);
    name.to_ascii_lowercase()
}
