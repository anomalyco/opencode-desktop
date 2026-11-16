//! A draft's document: all of it, not only its words. Its text (a plain string when that's all
//! it holds, as before), shell mode, caret, `@` pills, files named by path, context lines,
//! quotes and comment cards, and its images and files by content hash: their bytes go once into
//! the database's `blob` table (hashed on the storage thread, never the UI's) and the document
//! names them as Desktop's drafts do (`{ blob: { id } }`, which the blob sweep counts as
//! referenced). Desktop keeps the same pieces in its `ComposerStore`; Rune kept only the words.
//!
//! A restored draft's files read their bytes back on the storage thread and land in the draft
//! (shown or parked) when they arrive; until then their entries are written through unchanged,
//! so nothing a write makes in between drops them.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment};
use serde_json::{Value, json};

use super::Parked;
use crate::App;
use crate::add_selection::ContextFile;

/// Which blob holds which attachment's bytes, as far as this run knows.
#[derive(Default)]
pub struct Blobs {
    /// By the bytes' address: their blob id, while those bytes live.
    ids: HashMap<usize, (Weak<[u8]>, String)>,
    /// Bytes sent to be stored and hashed, not yet answered.
    asked: HashSet<usize>,
    /// Answers from the storage thread, for the next poll.
    arrived: Arc<Mutex<Inbox>>,
    /// Restored files whose bytes are still on their way, by draft key: their entries as stored.
    pending: HashMap<String, Vec<Value>>,
    /// Grows when what the drafts' documents name changes (an id known, a file in).
    pub revision: u64,
}

enum Arrival {
    Stored { at: usize, bytes: Weak<[u8]>, id: String },
    Read { draft: String, entry: Value, file: Option<Attachment> },
}

#[derive(Default)]
struct Inbox {
    items: Vec<Arrival>,
    #[cfg(windows)]
    after_id: Option<Box<dyn FnOnce()+Send>>,
}

impl Inbox {
    fn stored(inbox:&Mutex<Self>, item:Arrival){
        let mut batch=inbox.lock().unwrap_or_else(|e|e.into_inner());
        batch.items.push(item);
        #[cfg(windows)] {
            let notify=batch.after_id.take();drop(batch);
            if let Some(notify)=notify{notify();}
        }
    }
}

fn address(bytes: &Arc<[u8]>) -> usize {
    Arc::as_ptr(bytes).cast::<u8>() as usize
}

impl Blobs {
    #[cfg(windows)]
    pub(super) fn knows(&self, bytes:&Arc<[u8]>) -> bool { self.id(bytes).is_some() }
    fn id(&self, bytes: &Arc<[u8]>) -> Option<&str> {
        let (weak, id) = self.ids.get(&address(bytes))?;
        weak.upgrade().is_some_and(|b| Arc::ptr_eq(&b, bytes)).then_some(id.as_str())
    }
}

/// A draft's pieces besides its words, borrowed where they live (the composer's own for the
/// shown draft, `Parked` for the rest): a keystroke builds no copy of them.
#[derive(Default)]
pub(super) struct Parts<'a> {
    pub pills: Vec<(usize, usize, &'a str)>,
    pub caret: Option<usize>,
    pub files: &'a [Attachment],
    pub paths: &'a [PathAttachment],
    pub context: &'a [ContextFile],
    pub quotes: Vec<&'a PromptComment>,
}

impl<'a> Parts<'a> {
    pub fn of(p: &'a Parked) -> Parts<'a> {
        Parts {
            pills: p.pills.iter().map(|(r, id)| (r.start, r.end, id.as_str())).collect(),
            caret: p.caret,
            files: &p.files,
            paths: &p.paths,
            context: &p.context,
            quotes: p.quotes.iter().collect(),
        }
    }

    /// Anything to keep beside `text`: pills and a caret only mean something with words.
    fn any(&self, text: &str) -> bool {
        !self.files.is_empty() || !self.paths.is_empty() || !self.context.is_empty() || !self.quotes.is_empty() || (!text.trim().is_empty() && (!self.pills.is_empty() || self.caret.is_some_and(|c| c != text.len())))
    }
}

/// The document as it's written, keys in this order, empty ones left out.
#[derive(serde::Serialize)]
struct Doc<'a> {
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    caret: Option<usize>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pills: &'a [(usize, usize, &'a str)],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    paths: &'a [PathAttachment],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<Value>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    quotes: &'a [&'a PromptComment],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<Value>,
}

/// The document for a draft: `None` when there's nothing to keep.
pub(super) fn encode(text: &str, shell: bool, parts: Option<Parts<'_>>, blobs: &Blobs, pending: Option<&Vec<Value>>) -> Option<String> {
    let rich = parts.filter(|p| p.any(text));
    if rich.is_none() && pending.is_none_or(Vec::is_empty) {
        return match (shell, text.trim().is_empty()) {
            (true, _) => Some(json!({ "text": text, "mode": "shell" }).to_string()),
            (false, true) => None,
            (false, false) => serde_json::to_string(text).ok(),
        };
    }
    let p = rich.unwrap_or_default();
    let mut files: Vec<Value> = p
        .files
        .iter()
        .filter_map(|f| match (&f.media, blobs.id(&f.bytes)) {
            (Some(media), _) => Some(json!({ "name": f.name, "mime": f.mime, "media": media })),
            // Its size, for what a kept closed window holds (`windows::reopen`).
            (None, Some(id)) => Some(json!({ "name": f.name, "mime": f.mime, "size": f.bytes.len(), "blob": { "id": id } })),
            // Its bytes are still being stored: it joins the document once they are.
            (None, None) => None,
        })
        .collect();
    files.extend(pending.into_iter().flatten().cloned());
    let doc = Doc {
        text,
        mode: shell.then_some("shell"),
        caret: p.caret.filter(|&c| c != text.len()),
        pills: &p.pills,
        paths: p.paths,
        context: p.context.iter().map(|c| json!({ "path": c.path, "start": c.start, "end": c.end })).collect(),
        quotes: &p.quotes,
        files,
    };
    serde_json::to_string(&doc).ok()
}
/// A stored draft: its words, shell mode, the rest, and its files whose bytes are in `blob`.
pub(super) struct Decoded {
    pub text: String,
    pub shell: bool,
    pub parked: Parked,
    pub stored: Vec<Value>,
}

pub(super) fn decode(d: &Value) -> Decoded {
    let text = d.as_str().or(d["text"].as_str()).unwrap_or_default().to_owned();
    let pills = d["pills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p[0].as_u64()? as usize..p[1].as_u64()? as usize, p[2].as_str()?.to_owned())))
        .filter(|(r, _): &(Range<usize>, String)| r.start < r.end)
        .collect();
    let context = d["context"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some(ContextFile { path: c["path"].as_str()?.to_owned(), start: c["start"].as_u64()? as u32, end: c["end"].as_u64()? as u32 }))
        .collect();
    let (stored, media): (Vec<Value>, Vec<Value>) = d["files"].as_array().into_iter().flatten().cloned().partition(|f| f["blob"]["id"].is_string());
    let files = media
        .iter()
        .filter_map(|f| Some(Attachment { mime: f["mime"].as_str()?.to_owned(), name: f["name"].as_str()?.to_owned(), bytes: Arc::from(&[][..]), media: Some(f["media"].as_str()?.to_owned()) }))
        .collect();
    let parked = Parked {
        pills,
        caret: d["caret"].as_u64().map(|c| c as usize),
        files,
        paths: serde_json::from_value::<Vec<PathAttachment>>(d["paths"].clone()).unwrap_or_default(),
        context,
        quotes: serde_json::from_value::<Vec<PromptComment>>(d["quotes"].clone()).unwrap_or_default(),
    };
    Decoded { text, shell: d["mode"].as_str() == Some("shell"), parked, stored }
}

impl App {
    #[cfg(windows)]
    pub(super) fn replace_draft_files(&mut self,key:&str,stored:Vec<Value>){
        self.restore.blobs.pending.remove(key);
        self.read_draft_files(key,stored);
    }
    /// Draft `key`'s restored files whose bytes are in `blob`: read on the storage thread, they
    /// join the draft at a later poll.
    pub(super) fn read_draft_files(&mut self, key: &str, stored: Vec<Value>) {
        if stored.is_empty() {
            return;
        }
        self.restore.blobs.pending.insert(key.to_owned(), stored.clone());
        let (arrived, wake) = (self.restore.blobs.arrived.clone(), (*self.waker).clone());
        let key = key.to_owned();
        crate::storage::with_storage(move |s| {
            for entry in stored {
                let (arrived, wake, draft) = (arrived.clone(), wake.clone(), key.clone());
                let id = entry["blob"]["id"].as_str().unwrap_or_default().to_owned();
                s.documents().get_blob(&id, Box::new(move |bytes| {
                    let file = bytes.and_then(|b| Some(Attachment { mime: entry["mime"].as_str()?.to_owned(), name: entry["name"].as_str()?.to_owned(), bytes: Arc::from(b), media: None }));
                    arrived.lock().unwrap_or_else(|e| e.into_inner()).items.push(Arrival::Read { draft, entry, file });
                    if let Some(w) = wake {
                        w();
                    }
                }));
            }
        });
    }

    /// Every poll: drafts' new files go to be stored (and hashed) off the UI thread, and what the
    /// storage thread answered lands. True when a draft on screen changed.
    pub(crate) fn poll_draft_files(&mut self) -> bool {
        let fresh: Vec<Arc<[u8]>> = self
            .attach
            .files
            .iter()
            .chain(self.restore.parked.values().flat_map(|p| &p.files))
            .filter(|f| f.media.is_none())
            .map(|f| f.bytes.clone())
            .filter(|b| self.restore.blobs.id(b).is_none() && !self.restore.blobs.asked.contains(&address(b)))
            .collect();
        for bytes in fresh {
            self.restore.blobs.asked.insert(address(&bytes));
            let (arrived, wake) = (self.restore.blobs.arrived.clone(), (*self.waker).clone());
            let store=move ||crate::storage::with_storage(move |s| {
                let (at, weak) = (address(&bytes), Arc::downgrade(&bytes));
                let id = s.documents().put_blob(bytes);
                Inbox::stored(&arrived,Arrival::Stored { at, bytes: weak, id });
                if let Some(w) = wake {
                    w();
                }
            });
            #[cfg(not(target_arch="wasm32"))] crate::plugin_host::tasks::run(move ||{
                store();
                // A quit before the first visible frame can park the host before
                // its Presented event. Start adoption off-frame in that case:
                // the hash closure is in `queued.later`; waiting for the final
                // producer before starting adoption would wait for itself.
                #[cfg(windows)] if !crate::storage::is_ready(){let _=crate::storage::barrier_store();}
            });
            #[cfg(target_arch="wasm32")] store();
        }
        let arrived = std::mem::take(&mut self.restore.blobs.arrived.lock().unwrap_or_else(|e| e.into_inner()).items);
        let mut shown = false;
        for a in arrived {
            self.restore.blobs.revision += 1;
            match a {
                Arrival::Stored { at, bytes, id } => {
                    self.restore.blobs.asked.remove(&at);
                    self.restore.blobs.ids.insert(at, (bytes, id));
                }
                Arrival::Read { draft, entry, file } => {
                    // A held or temporarily unreadable store can't return the image bytes.
                    // Keep its persisted reference until we actually have the attachment.
                    let Some(file) = file else { continue };
                    if let Some(list) = self.restore.blobs.pending.get_mut(&draft) {
                        let Some(pos)=list.iter().position(|e| *e==entry) else{continue};
                        list.remove(pos);
                        if list.is_empty() {
                            self.restore.blobs.pending.remove(&draft);
                        }
                    } else { continue; }
                    let id = entry["blob"]["id"].as_str().unwrap_or_default().to_owned();
                    self.restore.blobs.ids.insert(address(&file.bytes), (Arc::downgrade(&file.bytes), id));
                    if self.composer_holds(&draft) {
                        self.attach.files.push(file);
                        shown = true;
                    } else {
                        self.restore.parked.entry(draft).or_default().files.push(file);
                    }
                }
            }
        }
        shown
    }

    /// The save producer isn't finished when it submitted a hash job. It is
    /// finished when the document naming those bytes has entered storage.
    /// Register under the same lock as delivery so no completion can slip
    /// between inspecting the inbox and arming the next UI turn.
    #[cfg(windows)]
    pub(crate) fn quit_files_delivered(&mut self,notify:impl FnOnce()+Send+'static)->bool{
        if self.restore.blobs.asked.is_empty(){return true}
        self.poll_restore();
        if self.restore.blobs.asked.is_empty(){return true}
        let mut inbox=self.restore.blobs.arrived.lock().unwrap_or_else(|e|e.into_inner());
        if inbox.items.iter().any(|i|matches!(i,Arrival::Stored{..})){drop(inbox);notify();}
        else {inbox.after_id=Some(Box::new(notify));}
        false
    }

    /// Draft `key`'s files still on their way from the database, as stored.
    pub(super) fn pending_files(&self, key: &str) -> Option<&Vec<Value>> {
        self.restore.blobs.pending.get(key)
    }
}

impl super::Restore {
    /// The drafts whose files are still on their way.
    pub(super) fn blobs_pending(&self) -> impl Iterator<Item = &String> + '_ {
        self.blobs.pending.keys()
    }
}
