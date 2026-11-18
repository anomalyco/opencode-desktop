//! Images and files on prompts: upload/drop glue over
//! [`session_view::composer::attachments`] and [`session_view::images`].

pub mod reply_image;
pub mod toast;
pub mod uploads;

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::{PathAttachment, Server, UploadError};
use opencode_kit::model::Attachment;
use rune::Tx;
use rune::machine::{Attempt, Effects, Seq};

pub use opencode_runtime::ContextFile;
pub use reply_image::add_reply_image;
pub use session_view::composer::attachments::{
    MAX_BYTES, from_path, head, inputs, mime_for, name_of, native_mime, path_lines, push_unique,
    reconcile, remove_button,
};
pub use session_view::images::{
    MediaStore, PreparedImage, identity, inline_images, prepare_images, view_image, viewer,
};

/// Running remote upload tasks and channel bridge owned by [`Attach`].
pub(crate) struct UploadRunner {
    pub(crate) seq: Seq,
    pub(crate) uploads_tx: Sender<Copied>,
    pub(crate) uploads_rx: Receiver<Copied>,
    /// Each running upload's stop (Desktop's `runs`, its `AbortController`) and the draft it lands in.
    pub(crate) runs: HashMap<Attempt, (Arc<AtomicBool>, Option<String>)>,
}

impl Default for UploadRunner {
    fn default() -> Self {
        let (uploads_tx, uploads_rx) = channel();
        Self {
            seq: Seq::default(),
            uploads_tx,
            uploads_rx,
            runs: HashMap::new(),
        }
    }
}

/// Composer attachment queue, remote upload runner, file dropzone state, optimistic prompt echoes,
/// and shared [`MediaStore`].
///
/// Defaults `C` to the canonical [`ContextFile`] (`opencode_runtime::ContextFile`) shared with
/// `opencode-feature-restore`.
pub struct Attach<C = ContextFile> {
    /// Waiting in the composer for the next send, inline.
    pub files: Vec<Attachment>,
    /// Waiting in the composer for the next send, by path on the server.
    pub paths: Vec<PathAttachment>,
    /// Files' selected lines waiting for the next send as context (Add selection to context).
    pub context: Vec<C>,
    /// Uploads still copying to a remote server; sending waits for them.
    pub uploads: uploads::Uploads,
    pub(crate) runner: UploadRunner,
    /// A file drag over the page, which shows the drop overlay.
    pub drop: crate::dropzone::Dropzone,
    /// Prompts shown before the server's copy: (session, local id, text).
    pub echoes: Vec<(String, String, String)>,
    /// Shared image cache, `media:` ref fetcher, and active viewer state.
    pub store: MediaStore,
}

impl<C> Default for Attach<C> {
    fn default() -> Self {
        Attach {
            files: Vec::new(),
            paths: Vec::new(),
            context: Vec::new(),
            uploads: uploads::Uploads::default(),
            runner: UploadRunner::default(),
            drop: Default::default(),
            echoes: Vec::new(),
            store: MediaStore::default(),
        }
    }
}

impl<C> Deref for Attach<C> {
    type Target = MediaStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl<C> DerefMut for Attach<C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

/// Non-generic read-only projection of [`Attach<C>`] for object-safe [`AttachmentsHost`] access.
pub struct AttachRef<'a> {
    pub files: &'a [Attachment],
    pub paths: &'a [PathAttachment],
    pub uploads: &'a uploads::Uploads,
    pub drop: crate::dropzone::Dropzone,
    pub store: &'a MediaStore,
    pub(crate) runner: &'a UploadRunner,
}

/// Non-generic mutable projection of [`Attach<C>`] for object-safe [`AttachmentsHost`] access.
pub struct AttachMut<'a> {
    pub files: &'a mut Vec<Attachment>,
    pub paths: &'a mut Vec<PathAttachment>,
    pub uploads: &'a mut uploads::Uploads,
    pub drop: &'a mut crate::dropzone::Dropzone,
    pub echoes: &'a mut Vec<(String, String, String)>,
    pub store: &'a mut MediaStore,
    pub(crate) runner: &'a mut UploadRunner,
}

impl AttachMut<'_> {
    pub fn add(&mut self, a: Attachment) {
        push_unique(self.files, a);
    }

    pub fn add_path(&mut self, r: PathAttachment) {
        if !self.paths.iter().any(|x| x.path == r.path) {
            self.paths.push(r);
        }
    }
}

impl<C> Attach<C> {
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

    /// Read-only non-generic projection for [`AttachmentsHost::attach_ref`].
    pub fn as_ref_parts(&self) -> AttachRef<'_> {
        AttachRef {
            files: &self.files,
            paths: &self.paths,
            uploads: &self.uploads,
            drop: self.drop,
            store: &self.store,
            runner: &self.runner,
        }
    }

    /// Mutable non-generic projection for [`AttachmentsHost::attach_mut`].
    pub fn as_mut_parts(&mut self) -> AttachMut<'_> {
        AttachMut {
            files: &mut self.files,
            paths: &mut self.paths,
            uploads: &mut self.uploads,
            drop: &mut self.drop,
            echoes: &mut self.echoes,
            store: &mut self.store,
            runner: &mut self.runner,
        }
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
    pub error: String,
}

/// Object-safe host boundary for attaching files from disk, running/polling/cancelling remote
/// uploads, the upload progress toast, and the system file picker.
pub trait AttachmentsHost: opencode_kernel::AppFeedback {
    /// Read-only projection of the window's [`Attach`] state (must not dirty `Tracked` cells).
    fn attach_ref(&self) -> AttachRef<'_>;
    /// Mutable projection of the window's [`Attach`] state.
    fn attach_mut(&mut self) -> AttachMut<'_>;

    /// Directory of the active session or draft where uploads and file pickers open.
    fn composer_directory(&self) -> String;
    /// Active draft key so remote uploads land in the draft they started from.
    fn draft_key(&self) -> Option<String> {
        None
    }
    /// Live OpenCode server connection, when connected (remote servers trigger uploads).
    fn live_server(&self) -> Option<Server> {
        None
    }

    /// Notifies the host that the draft changed (for example, ending prompt-history recall).
    fn on_draft_edited(&mut self) {}
    /// Ensures the persistent upload progress toast is visible in the toast stack.
    fn on_uploads_shown(&mut self) {}
    /// Lands a completed remote upload in `draft` (or the shown composer when it matches).
    fn land_upload(&mut self, draft: Option<&str>, attachment: PathAttachment) {
        let _ = draft;
        self.attach_mut().add_path(attachment);
    }

    /// Whether picking a file from the dialog should focus the composer.
    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        !session_view::style::touch_controls() || tx.by_keys()
    }
    /// Moves keyboard focus into the composer editor.
    fn focus_composer(&mut self) {}
}

/// Attaches a file from disk: inline when the model reads it natively and it fits, else by
/// path, uploading a copy first when the server is on another machine.
pub fn attach_path(host: &mut dyn AttachmentsHost, path: &Path) {
    host.on_draft_edited();
    let name = name_of(path);
    let size = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m.len() as usize,
        Ok(_) => {
            host.toast_say(
                opencode_i18n::t!("rune.attach.failed").into(),
                opencode_i18n::t!("rune.attach.folder", name = name),
            );
            return;
        }
        Err(e) => {
            host.toast_fail(
                opencode_i18n::t!("rune.attach.failed").into(),
                format!("{name}: {}", opencode_ui::toast::plain(&e.to_string())),
            );
            return;
        }
    };
    if native_mime(path).is_some() && size <= MAX_BYTES {
        match from_path(path) {
            Ok(a) => host.attach_mut().add(a),
            Err(e) => host.toast_fail(opencode_i18n::t!("rune.attach.failed").into(), opencode_ui::toast::plain(&e)),
        }
        return;
    }
    let mime = match head(path) {
        Ok(h) => mime_for(path, &h).to_owned(),
        Err(e) => {
            host.toast_fail(opencode_i18n::t!("rune.attach.failed").into(), opencode_ui::toast::plain(&e));
            return;
        }
    };
    let Some(server) = host.live_server().filter(|s| !s.is_local()) else {
        host.attach_mut().add_path(PathAttachment {
            name,
            mime,
            path: path.to_string_lossy().into_owned(),
        });
        return;
    };
    // Each upload is its own (`attachments.ts:132`, a fresh id), so the same file twice
    // copies twice and lands once (`add_path` keeps one per path).
    let id = opencode_kit::client::message_id();
    let size = size as u64;
    let attempt = {
        let a = host.attach_mut();
        uploads::step(
            &mut a.runner.seq,
            a.uploads,
            uploads::Event::Started { id: &id, name: &name, mime: &mime, size },
        )
        .into_iter()
        .find_map(|e| match e {
            uploads::Effect::Run(attempt) => Some(attempt),
            _ => None,
        })
    };
    let Some(attempt) = attempt else {
        return;
    };
    // The upload toast comes up with the first upload (`UploadToast`).
    host.on_uploads_shown();
    let draft = host.draft_key();
    let directory = host.composer_directory();
    let stop = Arc::new(AtomicBool::new(false));
    let (out, wake) = {
        let a = host.attach_mut();
        a.runner.runs.insert(attempt, (stop.clone(), draft));
        (a.runner.uploads_tx.clone(), a.store.waker.clone())
    };
    let path = path.to_owned();
    rune::task::spawn("attachment-upload", move || {
        let woke = || {
            if let Some(w) = &wake {
                w();
            }
        };
        // The body's progress reaches the window about as often as an XHR's does (every 50 ms
        // while it moves), and once all of it has gone.
        let mut last: Option<rune::time::Instant> = None;
        let mut sent = |n: u64| {
            if last.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) || n >= size {
                last = Some(rune::time::Instant::now());
                out.send(Copied::Progress(attempt, n)).ok();
                woke();
            }
            !stop.load(Ordering::Relaxed)
        };
        let copied = std::fs::File::open(&path)
            .map_err(|e| UploadError::Failed(format!("{name}: {e}")))
            .and_then(|mut file| server.upload(&directory, &name, &mut file, size, &mut sent));
        let done = match copied {
            Ok(at) => Ok(PathAttachment { name, mime, path: at }),
            Err(UploadError::Aborted) => Err(None),
            Err(UploadError::Failed(error)) => Err(Some(Failed { error })),
        };
        out.send(Copied::Done(attempt, done)).ok();
        woke();
    })
    .ok();
}

/// Takes in the uploads' progress and answers, and stalls that came due; an upload cancelled
/// or stalled since lands nowhere.
pub fn poll_uploads(host: &mut dyn AttachmentsHost, tx: &mut Tx) -> bool {
    let now = tx.clock();
    let (events, stall_due) = {
        let a = host.attach_ref();
        let mut events = Vec::new();
        while let Ok(copied) = a.runner.uploads_rx.try_recv() {
            events.push(copied);
        }
        let stall_due = a.uploads.next_stall().is_some_and(|due| now + 1e-9 >= due);
        (events, stall_due)
    };
    if events.is_empty() && !stall_due {
        return false;
    }
    let mut changed = false;
    for copied in events {
        let (event, done) = match copied {
            Copied::Progress(attempt, loaded) => (uploads::Event::Progress { attempt, loaded, now }, None),
            Copied::Done(attempt, done) => {
                let event = if done.is_ok() {
                    uploads::Event::Succeeded { attempt }
                } else {
                    uploads::Event::Failed { attempt }
                };
                (event, Some(done))
            }
        };
        let (effects, step_changed) = {
            let a = host.attach_mut();
            let before = a.uploads.clone();
            let effects = uploads::step(&mut a.runner.seq, a.uploads, event);
            (effects, *a.uploads != before)
        };
        changed |= step_changed;
        upload_effects(host, effects, done, tx);
    }
    if host.attach_ref().uploads.next_stall().is_some_and(|due| now + 1e-9 >= due) {
        let effects = {
            let a = host.attach_mut();
            uploads::step(&mut a.runner.seq, a.uploads, uploads::Event::Tick { now })
        };
        changed |= !effects.is_empty();
        upload_effects(host, effects, None, tx);
    }
    changed
}

fn upload_effects(
    host: &mut dyn AttachmentsHost,
    effects: Effects<uploads::Effect>,
    mut done: Option<Result<PathAttachment, Option<Failed>>>,
    tx: &mut Tx,
) {
    for effect in effects {
        match effect {
            uploads::Effect::Run(_) => {}
            uploads::Effect::WakeAt(due) => tx.wake_at(due),
            uploads::Effect::Abort(attempt) => {
                if let Some((stop, _)) = host.attach_mut().runner.runs.remove(&attempt) {
                    stop.store(true, Ordering::Relaxed);
                }
            }
            uploads::Effect::Resolve(attempt) => {
                let draft = host.attach_mut().runner.runs.remove(&attempt).and_then(|(_, d)| d);
                if let Some(Ok(r)) = done.take() {
                    host.land_upload(draft.as_deref(), r);
                }
            }
            uploads::Effect::Reject(attempt, why) => {
                host.attach_mut().runner.runs.remove(&attempt);
                let failed = match done.take() {
                    Some(Err(Some(f))) => Some(f),
                    _ => None,
                };
                let line = match (why, &failed) {
                    (uploads::Reject::Failed, Some(f)) => opencode_ui::toast::plain(&f.error),
                    _ => opencode_i18n::t!("common.requestFailed").to_owned(),
                };
                // Desktop's `onUploadError` toast: no action, gone after the region's 5 s.
                host.toast_say(opencode_i18n::t!("prompt.toast.uploadFailed.title").into(), line);
            }
        }
    }
}

/// The upload card's cancel (`upload.cancelled`): it leaves the composer, its copy stops,
/// and what the copy answers later is dropped.
pub fn cancel_upload(host: &mut dyn AttachmentsHost, id: &str) {
    let effects = {
        let a = host.attach_mut();
        uploads::step(&mut a.runner.seq, a.uploads, uploads::Event::Cancelled { id })
    };
    for effect in effects {
        if let uploads::Effect::Abort(attempt) = effect
            && let Some((stop, _)) = host.attach_mut().runner.runs.remove(&attempt)
        {
            stop.store(true, Ordering::Relaxed);
        }
    }
}

/// Builds the system file picker options for attaching a file to the composer.
pub fn pick_attachment_options(host: &dyn AttachmentsHost) -> rune::FileDialogOptions {
    rune::FileDialogOptions {
        title: opencode_i18n::t!("rune.attach.pickerTitle").into(),
        directory: Some(host.composer_directory().into()),
        ..Default::default()
    }
}

/// Handles the result of the system file picker opened by [`pick_attachment`].
pub fn handle_picked_attachment(host: &mut dyn AttachmentsHost, result: Result<Option<PathBuf>, String>, tx: &mut Tx) {
    match result {
        Ok(Some(path)) => {
            tx.instant();
            attach_path(host, &path);
            if host.auto_focus_composer(tx) {
                host.focus_composer();
            }
        }
        Ok(None) => {}
        Err(e) => host.toast_fail(opencode_i18n::t!("rune.status.filePickerFailed").into(), e),
    }
}

/// ⌘U and the + menu's first item: the system's file picker, then the file.
pub fn pick_attachment<H: AttachmentsHost + 'static>(host: &mut H, tx: &mut Tx) {
    let options = pick_attachment_options(host);
    tx.open_file(options, |s: &mut H, result, tx: &mut Tx| {
        handle_picked_attachment(s, result.map_err(|e| e.to_string()), tx);
    });
}
