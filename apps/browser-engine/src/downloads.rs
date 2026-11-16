//! A page's downloads, as Desktop's browser keeps them (`gui-plugins/src/browser/chromium.ts`
//! `will-download`, `files.ts`): each file goes to a folder of its own in the engine's private
//! downloads folder, with no dialog, for the agent's tools to read; one over 5 MiB (Desktop's
//! `MAX_FILE_BYTES`) is cancelled. A tab's files go with it: its folder is removed when it closes
//! (a download still running is cancelled), and the whole folder goes when the engine ends and
//! is emptied again when it starts (after an engine that didn't end cleanly). The app hears each start, step and end (`EngineMsg::Download`).

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

/// Desktop's limit on a file the agent's tools take (`plugin-browser/src/rpc.ts`).
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// The engine's downloads: its folder, emptied when the engine starts (a run's files are that
/// run's, as Desktop's temporary folder is), and each file by Chromium's download id.
pub struct Downloads {
    dir: PathBuf,
    next: AtomicU64,
    files: Mutex<HashMap<u32, DownloadFile>>,
    /// Tabs that closed: a download of theirs still running is cancelled at its next step.
    closed: Mutex<HashSet<u64>>,
}

impl Downloads {
    pub fn open(dir: PathBuf) -> Arc<Self> {
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        Arc::new(Self { dir, next: AtomicU64::new(1), files: Mutex::new(HashMap::new()), closed: Mutex::new(HashSet::new()) })
    }

    /// A tab closed: its files go (Desktop's `files.dispose()` on a page's close).
    pub fn close_tab(&self, tab_id: u64) {
        self.closed.lock().unwrap().insert(tab_id);
        let _ = std::fs::remove_dir_all(self.dir.join(tab_id.to_string()));
    }
}

wrap_download_handler! {
    pub struct TabDownloadHandler {
        tab_id: u64,
        downloads: Arc<Downloads>,
        out_tx: Output,
    }

    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _method: Option<&CefString>) -> ::std::os::raw::c_int {
            1
        }

        // Where it goes, with no dialog. Returning 1 says the callback is taken.
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
            let folder = self.downloads.dir.join(self.tab_id.to_string()).join(&id);
            let path = folder.join(safe_name(&name));
            let mime = CefString::from(&item.mime_type()).to_string();
            let mut file = DownloadFile { id, name, mime, bytes: 0, state: DownloadState::Pending, path: path.to_string_lossy().into_owned(), error: None };
            if std::fs::create_dir_all(&folder).is_err() {
                file.state = DownloadState::Failed;
                file.error = Some("file_failed".to_owned());
                self.send(file);
                return 0;
            }
            callback.cont(Some(&CefString::from(path.to_string_lossy().as_ref())), 0);
            self.downloads.files.lock().unwrap().insert(item.id(), file.clone());
            self.send(file);
            1
        }

        fn on_download_updated(&self, _browser: Option<&mut Browser>, item: Option<&mut DownloadItem>, callback: Option<&mut DownloadItemCallback>) {
            let Some(item) = item else { return };
            if self.downloads.closed.lock().unwrap().contains(&self.tab_id) {
                if let Some(callback) = callback {
                    callback.cancel();
                }
                if let Some(file) = self.downloads.files.lock().unwrap().remove(&item.id()) {
                    let _ = std::fs::remove_file(&file.path);
                    let _ = std::fs::remove_dir_all(self.downloads.dir.join(self.tab_id.to_string()));
                }
                return;
            }
            let mut files = self.downloads.files.lock().unwrap();
            let Some(file) = files.get_mut(&item.id()) else { return };
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
/// accepts replaced, trailing dots and spaces dropped, at most 120 characters (its last ones).
fn safe_name(name: &str) -> PathBuf {
    let cleaned: String = name.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c }).collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']);
    let chars: Vec<char> = trimmed.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(120)..].iter().collect();
    Path::new(if tail.is_empty() { "file" } else { &tail }).to_owned()
}

/// Chromium's interrupt reason, by its `DownloadInterruptReason` name in snake case.
fn reason(reason: DownloadInterruptReason) -> String {
    let debug = format!("{:?}", reason.as_ref());
    let name = debug.rsplit("CEF_DOWNLOAD_INTERRUPT_REASON_").next().unwrap_or(&debug);
    name.to_ascii_lowercase()
}
