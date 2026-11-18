//! Each session's line comments, kept in the app's database (`opencode.comments`, one row per
//! session, Desktop's `CommentStore` shape: `{ comments: Record<file, LineComment[]> }`), so the
//! comments under a file's lines come back after a restart with their cards in the composer
//! (`restore::draft_doc`). Written behind when a session's list changes, never per keystroke in a
//! comment box; read once after the storage opens, off the UI thread.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use rune_kit::diff::Side;
use serde_json::{Value, json};

use super::{LineComment, Range};
use crate::App;

const NAMESPACE: &str = "opencode.comments";

fn key(session: &str) -> String {
    format!("comments:{session}")
}

/// What's kept of the sessions' comments.
#[derive(Default)]
pub struct Saved {
    /// Sessions whose list changed since it was written.
    dirty: HashSet<String>,
    phase: Phase,
}

#[derive(Default)]
enum Phase {
    /// Nothing read yet.
    #[default]
    Unread,
    /// The rows are being read on the storage thread.
    Reading(Arc<Mutex<Option<Vec<(String, Vec<LineComment>)>>>>),
    /// The stored lists are in.
    Read,
}

impl Saved {
    /// Session `sid`'s list changed.
    pub fn touch(&mut self, sid: &str) {
        self.dirty.insert(sid.to_owned());
    }

}

fn side(s: Option<Side>) -> Value {
    match s {
        Some(Side::Old) => "deletions".into(),
        Some(Side::New) => "additions".into(),
        None => Value::Null,
    }
}

fn side_of(v: &Value) -> Option<Side> {
    match v.as_str()? {
        "deletions" => Some(Side::Old),
        "additions" => Some(Side::New),
        _ => None,
    }
}

fn encode(list: &[LineComment]) -> Value {
    let mut by_file: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for c in list {
        let mut selection = json!({ "start": c.selection.start, "end": c.selection.end });
        if c.selection.side.is_some() {
            selection["side"] = side(c.selection.side);
        }
        if c.selection.end_side.is_some() {
            selection["endSide"] = side(c.selection.end_side);
        }
        by_file.entry(&c.file).or_default().push(json!({ "id": c.id, "file": c.file, "selection": selection, "comment": c.comment, "time": c.time }));
    }
    json!({ "comments": by_file })
}

fn decode(v: &Value) -> Vec<LineComment> {
    let mut list: Vec<LineComment> = v["comments"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .flat_map(|l| l.as_array().into_iter().flatten())
        .filter_map(|c| {
            let s = &c["selection"];
            Some(LineComment {
                id: c["id"].as_str()?.to_owned(),
                file: c["file"].as_str()?.to_owned(),
                selection: Range { start: s["start"].as_u64()? as u32, end: s["end"].as_u64()? as u32, side: side_of(&s["side"]), end_side: side_of(&s["endSide"]) },
                comment: c["comment"].as_str()?.to_owned(),
                time: c["time"].as_i64().unwrap_or(0),
            })
        })
        .collect();
    list.sort_by_key(|c| c.time);
    list
}

impl App {
    /// Every poll of a window that keeps state: the stored lists are read once, and lists that
    /// changed are written. True when stored comments came in.
    pub(crate) fn poll_line_comments(&mut self) -> bool {
        let mut landed = false;
        match &self.line_comments.saved.phase {
            Phase::Unread if crate::storage::is_ready() => {
                let slot: Arc<Mutex<Option<Vec<(String, Vec<LineComment>)>>>> = Arc::default();
                let (out, wake) = (slot.clone(), (*self.waker).clone());
                crate::storage::with_storage(move |s| {
                    let store = s.clone();
                    s.state().load(&[NAMESPACE], Some(Box::new(move || {
                        let rows = store.state().items(NAMESPACE).unwrap_or_default();
                        let lists = rows
                            .into_iter()
                            .filter_map(|(k, v)| Some((k.strip_prefix("comments:")?.to_owned(), decode(&serde_json::from_str(&v).ok()?))))
                            .collect();
                        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(lists);
                        if let Some(w) = wake {
                            w();
                        }
                    })));
                });
                self.line_comments.saved.phase = Phase::Reading(slot);
            }
            Phase::Reading(slot) => {
                let lists = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(lists) = lists {
                    for (sid, list) in lists {
                        let session = self.line_comments.sessions.entry(sid).or_default();
                        // Comments written meanwhile keep their place after the stored ones.
                        let mine = std::mem::replace(&mut session.list, list);
                        let fresh: Vec<LineComment> = mine.into_iter().filter(|c| !session.list.iter().any(|x| x.id == c.id)).collect();
                        session.list.extend(fresh);
                    }
                    self.line_comments.saved.phase = Phase::Read;
                    landed = true;
                }
            }
            _ => {}
        }
        // Written only once the stored ones are in, so a write never drops what's on disk.
        if matches!(self.line_comments.saved.phase, Phase::Read) && !self.line_comments.saved.dirty.is_empty() {
            for sid in std::mem::take(&mut self.line_comments.saved.dirty) {
                match self.line_comments.sessions.get(&sid).map(|s| &s.list).filter(|l| !l.is_empty()) {
                    Some(list) => crate::storage::set_json(NAMESPACE, &key(&sid), &encode(list)),
                    None => crate::storage::delete(NAMESPACE, &key(&sid)),
                }
            }
        }
        landed
    }
}
