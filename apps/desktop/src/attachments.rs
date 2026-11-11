//! Images and files on prompts: Desktop's upload/drop glue over
//! [`session_view::composer::attachments`] and [`session_view::images`].

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::PathAttachment;
use opencode_kit::model::Attachment;
use rune::machine::Attempt;
use rune::{El, Tx};

use crate::App;
use crate::palette::Pal;

pub use session_view::composer::attachments::{
    MAX_BYTES, from_path, head, inputs, mime_for, name_of, native_mime, path_lines, push_unique,
    reconcile,
};
pub use session_view::images::{
    MediaStore, PreparedImage, identity, inline_images, prepare_images, view_image, viewer,
};

pub struct Attach {
    /// Waiting in the composer for the next send, inline.
    pub files: Vec<Attachment>,
    /// Waiting in the composer for the next send, by path on the server.
    pub paths: Vec<PathAttachment>,
    /// Files' selected lines waiting for the next send as context (Add selection to context).
    pub context: Vec<crate::add_selection::ContextFile>,
    /// Uploads still copying to a remote server; sending waits for them.
    pub uploads: uploads::Uploads,
    seq: rune::machine::Seq,
    uploads_tx: Sender<Copied>,
    uploads_rx: Receiver<Copied>,
    /// Each running upload's stop (Desktop's `runs`, its `AbortController`).
    runs: std::collections::HashMap<Attempt, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// A file drag over the page, which shows the drop overlay.
    pub drop: crate::dropzone::Dropzone,
    /// Prompts shown before the server's copy: (session, local id, text).
    pub echoes: Vec<(String, String, String)>,
    /// Shared image cache, `media:` ref fetcher, and active viewer state.
    pub store: MediaStore,
}

impl Default for Attach {
    fn default() -> Self {
        let (uploads_tx, uploads_rx) = channel();
        Attach {
            files: Vec::new(),
            paths: Vec::new(),
            context: Vec::new(),
            uploads: uploads::Uploads::default(),
            seq: rune::machine::Seq::default(),
            uploads_tx,
            uploads_rx,
            runs: Default::default(),
            drop: Default::default(),
            echoes: Vec::new(),
            store: MediaStore::default(),
        }
    }
}

impl Deref for Attach {
    type Target = MediaStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DerefMut for Attach {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

impl Attach {
    /// Something to send besides text.
    pub fn any(&self) -> bool {
        !self.files.is_empty() || !self.paths.is_empty()
    }

    pub fn add_path(&mut self, r: PathAttachment) {
        if !self.paths.iter().any(|x| x.path == r.path) {
            self.paths.push(r);
        }
    }

    pub fn add(&mut self, a: Attachment) {
        push_unique(&mut self.files, a);
    }
}

/// How often a moving body's progress reaches the window.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

/// What an upload's copy reports: how far its body has gone, then its answer (`Err(None)` when
/// it was stopped).
pub enum Copied {
    Progress(Attempt, u64),
    Done(Attempt, Result<PathAttachment, Option<Failed>>),
}

/// An upload that didn't reach the server, and why.
pub struct Failed {
    error: String,
}

impl App {
    /// Attaches a file from disk: inline when the model reads it natively and it fits, else by
    /// path, uploading a copy first when the server is on another machine.
    pub fn attach_path(&mut self, path: &Path) {
        self.suggest_edited();
        let name = name_of(path);
        let size = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len() as usize,
            Ok(_) => {
                return crate::status_toast::say(
                    self,
                    opencode_i18n::t!("rune.attach.failed"),
                    opencode_i18n::t!("rune.attach.folder", name = name),
                );
            }
            Err(e) => {
                return crate::status_toast::fail(
                    self,
                    opencode_i18n::t!("rune.attach.failed"),
                    format!("{name}: {}", crate::status_toast::plain(&e.to_string())),
                );
            }
        };
        if native_mime(path).is_some() && size <= MAX_BYTES {
            match from_path(path) {
                Ok(a) => self.attach.add(a),
                Err(e) => crate::status_toast::fail(self, opencode_i18n::t!("rune.attach.failed"), crate::status_toast::plain(&e)),
            }
            return;
        }
        let mime = match head(path) {
            Ok(h) => mime_for(path, &h).to_owned(),
            Err(e) => {
                return crate::status_toast::fail(self, opencode_i18n::t!("rune.attach.failed"), crate::status_toast::plain(&e));
            }
        };
        let server = match &self.source {
            crate::Source::Live { server, .. } if !server.is_local() => server.clone(),
            _ => {
                return self.attach.add_path(PathAttachment {
                    name,
                    mime,
                    path: path.to_string_lossy().into_owned(),
                });
            }
        };
        // Each upload is its own (`attachments.ts:132`, a fresh id), so the same file twice
        // copies twice and lands once (`addPath` keeps one per path).
        let id = opencode_kit::client::message_id();
        let size = size as u64;
        let a = &mut self.attach;
        let Some(attempt) = uploads::step(&mut a.seq, &mut a.uploads, uploads::Event::Started { id: &id, name: &name, mime: &mime, size }).into_iter().find_map(|e| match e {
            uploads::Effect::Run(attempt) => Some(attempt),
            _ => None,
        }) else {
            return;
        };
        // The upload toast comes up with the first upload (`UploadToast`).
        crate::status_toast::uploads_shown(self);
        let a = &mut self.attach;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        a.runs.insert(attempt, stop.clone());
        let (out, wake, directory, path) = (a.uploads_tx.clone(), a.waker.clone(), self.composer_directory(), path.to_owned());
        std::thread::spawn(move || {
            let woke = || {
                if let Some(w) = &wake {
                    w();
                }
            };
            // The body's progress reaches the window about as often as an XHR's does (every 50 ms
            // while it moves), and once all of it has gone.
            let mut last: Option<std::time::Instant> = None;
            let mut sent = |n: u64| {
                if last.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) || n >= size {
                    last = Some(std::time::Instant::now());
                    let _ = out.send(Copied::Progress(attempt, n));
                    woke();
                }
                !stop.load(std::sync::atomic::Ordering::Relaxed)
            };
            let copied = std::fs::File::open(&path)
                .map_err(|e| opencode_kit::client::UploadError::Failed(format!("{name}: {e}")))
                .and_then(|mut file| server.upload(&directory, &name, &mut file, size, &mut sent));
            let done = match copied {
                Ok(at) => Ok(PathAttachment { name, mime, path: at }),
                Err(opencode_kit::client::UploadError::Aborted) => Err(None),
                Err(opencode_kit::client::UploadError::Failed(error)) => Err(Some(Failed { error })),
            };
            let _ = out.send(Copied::Done(attempt, done));
            woke();
        });
    }

    /// Takes in the uploads' progress and answers, and stalls that came due; an upload cancelled
    /// or stalled since lands nowhere.
    pub fn poll_uploads(&mut self, tx: &mut Tx) -> bool {
        let now = tx.clock();
        let mut changed = false;
        let mut events = Vec::new();
        while let Ok(copied) = self.attach.uploads_rx.try_recv() {
            events.push(copied);
        }
        for copied in events {
            let (event, done) = match copied {
                Copied::Progress(attempt, loaded) => (uploads::Event::Progress { attempt, loaded, now }, None),
                Copied::Done(attempt, done) => {
                    let event = if done.is_ok() { uploads::Event::Succeeded { attempt } } else { uploads::Event::Failed { attempt } };
                    (event, Some(done))
                }
            };
            let a = &mut self.attach;
            let before = a.uploads.clone();
            let effects = uploads::step(&mut a.seq, &mut a.uploads, event);
            changed |= a.uploads != before;
            self.upload_effects(effects, done, tx);
        }
        if self.attach.uploads.next_stall().is_some_and(|due| now + 1e-9 >= due) {
            let a = &mut self.attach;
            let effects = uploads::step(&mut a.seq, &mut a.uploads, uploads::Event::Tick { now });
            changed |= !effects.is_empty();
            self.upload_effects(effects, None, tx);
        }
        changed
    }

    fn upload_effects(&mut self, effects: rune::machine::Effects<uploads::Effect>, mut done: Option<Result<PathAttachment, Option<Failed>>>, tx: &mut Tx) {
        for effect in effects {
            match effect {
                uploads::Effect::Run(_) => {}
                uploads::Effect::WakeAt(due) => tx.wake_at(due),
                uploads::Effect::Abort(attempt) => {
                    if let Some(stop) = self.attach.runs.remove(&attempt) {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                uploads::Effect::Resolve(attempt) => {
                    self.attach.runs.remove(&attempt);
                    if let Some(Ok(r)) = done.take() {
                        self.attach.add_path(r);
                    }
                }
                uploads::Effect::Reject(attempt, why) => {
                    self.attach.runs.remove(&attempt);
                    let failed = match done.take() {
                        Some(Err(Some(f))) => Some(f),
                        _ => None,
                    };
                    let line = match (why, &failed) {
                        (uploads::Reject::Failed, Some(f)) => crate::status_toast::plain(&f.error),
                        _ => opencode_i18n::t!("common.requestFailed").to_owned(),
                    };
                    // Desktop's `onUploadError` toast: no action, gone after the region's 5 s.
                    crate::status_toast::say(self, opencode_i18n::t!("prompt.toast.uploadFailed.title"), line);
                }
            }
        }
    }

    /// The upload card's cancel (`upload.cancelled`): it leaves the composer, its copy stops,
    /// and what the copy answers later is dropped.
    pub fn cancel_upload(&mut self, id: &str) {
        let a = &mut self.attach;
        let effects = uploads::step(&mut a.seq, &mut a.uploads, uploads::Event::Cancelled { id });
        for effect in effects {
            if let uploads::Effect::Abort(attempt) = effect
                && let Some(stop) = self.attach.runs.remove(&attempt)
            {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    /// ⌘U and the + menu's first item: the system's file picker, then the file.
    pub fn pick_attachment(&mut self, tx: &mut Tx) {
        let options = rune::FileDialogOptions {
            title: opencode_i18n::t!("rune.attach.pickerTitle").into(),
            directory: Some(self.composer_directory().into()),
            ..Default::default()
        };
        tx.open_file(options, |s: &mut App, result, tx: &mut Tx| match result {
            Ok(Some(path)) => {
                tx.instant();
                s.attach_path(&path);
                s.composer.focused = true;
            }
            Ok(None) => {}
            Err(e) => crate::status_toast::fail(s, opencode_i18n::t!("rune.status.filePickerFailed"), e.to_string()),
        });
    }
}

pub(crate) fn remove_button(p: &Pal, name: &str, w: f32, on: impl Fn(&mut App) + 'static) -> El {
    session_view::composer::attachments::remove_button(p, name, w, on)
}

mod reply_image;
pub mod toast;
pub mod uploads;

