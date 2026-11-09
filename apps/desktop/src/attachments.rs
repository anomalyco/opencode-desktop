//! Images and files on prompts: Desktop's upload/drop glue over
//! [`session_view::composer::attachments`] and [`session_view::images`].

use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(test)]
use std::sync::{Arc, Mutex};

use opencode_kit::client::PathAttachment;
#[cfg(test)]
use opencode_kit::client::FileInput;
use opencode_kit::model::Attachment;
#[cfg(test)]
use opencode_kit::model::{Message, Transcript};
#[cfg(test)]
use rune::{Cx, ImageCache, col};
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
#[cfg(test)]
use session_view::images::{Fetch, Fetched, fit, gallery, shown};

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
    uploads_tx: Sender<(Attempt, Result<PathAttachment, Failed>)>,
    uploads_rx: Receiver<(Attempt, Result<PathAttachment, Failed>)>,
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

/// An upload that didn't reach the server: the file, to try again.
pub struct Failed {
    path: std::path::PathBuf,
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
        let id = path.to_string_lossy().into_owned();
        let a = &mut self.attach;
        let Some(attempt) = uploads::step(&mut a.seq, &mut a.uploads, uploads::Event::Started { id: &id, name: &name }).into_iter().find_map(|e| match e {
            uploads::Effect::Run(attempt) => Some(attempt),
            _ => None,
        }) else {
            return;
        };
        let (out, wake, directory, path) = (a.uploads_tx.clone(), a.waker.clone(), self.composer_directory(), path.to_owned());
        std::thread::spawn(move || {
            let copied = std::fs::read(&path)
                .map_err(|e| format!("{name}: {e}"))
                .and_then(|bytes| server.upload(&directory, &name, &bytes));
            let _ = out.send((attempt, match copied {
                Ok(at) => Ok(PathAttachment { name, mime, path: at }),
                Err(error) => Err(Failed { path, error }),
            }));
            if let Some(w) = wake {
                w();
            }
        });
    }

    /// Takes in finished uploads; one cancelled since lands nowhere.
    pub fn poll_uploads(&mut self) -> bool {
        let mut changed = false;
        while let Ok((attempt, done)) = self.attach.uploads_rx.try_recv() {
            let a = &mut self.attach;
            let event = if done.is_ok() { uploads::Event::Succeeded { attempt } } else { uploads::Event::Failed { attempt } };
            if uploads::step(&mut a.seq, &mut a.uploads, event).is_empty() {
                continue;
            }
            match done {
                Ok(r) => self.attach.add_path(r),
                Err(f) => {
                    let line = crate::status_toast::plain(f.error.trim_start_matches("upload "));
                    crate::status_toast::fail_retry(
                        self,
                        opencode_i18n::t!("prompt.toast.uploadFailed.title"),
                        line,
                        crate::status_toast::ToastAction::Attach(f.path),
                    );
                }
            }
            changed = true;
        }
        changed
    }

    /// The upload card's cancel (`upload.cancelled`): it leaves the composer, and what the copy
    /// answers later is dropped.
    pub fn cancel_upload(&mut self, id: &str) {
        let a = &mut self.attach;
        uploads::step(&mut a.seq, &mut a.uploads, uploads::Event::Cancelled { id });
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
pub mod uploads;

#[cfg(test)]
mod tests;
