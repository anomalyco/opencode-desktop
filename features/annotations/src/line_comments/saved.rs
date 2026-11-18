//! Each session's line comments, kept in the app's database (`opencode.comments`, one row per
//! session, Desktop's `CommentStore` shape: `{ comments: Record<file, LineComment[]> }`), so the
//! comments under a file's lines come back after a restart with their cards in the composer
//! (`restore::draft_doc`). Written behind when a session's list changes, never per keystroke in a
//! comment box; read once after the storage opens, off the UI thread.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use rune_kit::diff::Side;
use serde_json::{Value, json};

use super::{LineComment, LineCommentsHost, Range};

pub const NAMESPACE: &str = "opencode.comments";
pub const KEY_PREFIX: &str = "comments:";

pub fn key(session: &str) -> String {
    format!("{KEY_PREFIX}{session}")
}

/// What's kept of the sessions' comments.
#[derive(Default)]
pub struct Saved {
    /// Sessions whose list changed since it was written.
    dirty: HashSet<String>,
    phase: Phase,
}

type LoadedComments = Vec<(String, Vec<LineComment>)>;
type SavedSlot = Arc<Mutex<Option<LoadedComments>>>;

#[derive(Default)]
enum Phase {
    /// Nothing read yet.
    #[default]
    Unread,
    /// The rows are being read on the storage thread.
    Reading(SavedSlot),
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

pub fn encode(list: &[LineComment]) -> Value {
    let mut by_file: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for c in list {
        let mut selection = json!({ "start": c.selection.start, "end": c.selection.end });
        if c.selection.side.is_some() {
            selection["side"] = side(c.selection.side);
        }
        if c.selection.end_side.is_some() {
            selection["endSide"] = side(c.selection.end_side);
        }
        by_file.entry(&c.file).or_default().push(json!({
            "id": c.id,
            "file": c.file,
            "selection": selection,
            "comment": c.comment,
            "time": c.time,
        }));
    }
    json!({ "comments": by_file })
}

pub fn decode(v: &Value) -> Vec<LineComment> {
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
                selection: Range {
                    start: s["start"].as_u64()? as u32,
                    end: s["end"].as_u64()? as u32,
                    side: side_of(&s["side"]),
                    end_side: side_of(&s["endSide"]),
                },
                comment: c["comment"].as_str()?.to_owned(),
                time: c["time"].as_i64().unwrap_or(0),
            })
        })
        .collect();
    list.sort_by_key(|c| c.time);
    list
}

/// Every poll of a window that keeps state: the stored lists are read once, and lists that
/// changed are written. True when stored comments came in.
pub fn poll(host: &mut dyn LineCommentsHost) -> bool {
    let mut landed = false;
    enum Action {
        None,
        StartRead,
        Land(Vec<(String, Vec<LineComment>)>),
    }
    let action = match &host.line_comments().saved.phase {
        Phase::Unread if host.storage_ready() => Action::StartRead,
        Phase::Reading(slot) => match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(lists) => Action::Land(lists),
            None => Action::None,
        },
        _ => Action::None,
    };
    match action {
        Action::StartRead => {
            let slot: SavedSlot = Arc::default();
            let (out, wake) = (Arc::clone(&slot), host.waker());
            host.load_saved_comments(
                NAMESPACE,
                Box::new(move |rows| {
                    let lists = rows
                        .into_iter()
                        .filter_map(|(k, v)| Some((k.strip_prefix(KEY_PREFIX)?.to_owned(), decode(&serde_json::from_str(&v).ok()?))))
                        .collect();
                    *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(lists);
                    if let Some(w) = wake {
                        w();
                    }
                }),
            );
            host.line_comments_mut().saved.phase = Phase::Reading(slot);
        }
        Action::Land(lists) => {
            let lc = host.line_comments_mut();
            for (sid, list) in lists {
                let session = lc.sessions.entry(sid).or_default();
                // Comments written meanwhile keep their place after the stored ones.
                let mine = std::mem::replace(&mut session.list, list);
                let fresh: Vec<LineComment> = mine.into_iter().filter(|c| !session.list.iter().any(|x| x.id == c.id)).collect();
                session.list.extend(fresh);
            }
            lc.saved.phase = Phase::Read;
            landed = true;
        }
        Action::None => {}
    }
    // Written only once the stored ones are in, so a write never drops what's on disk.
    let should_flush = matches!(host.line_comments().saved.phase, Phase::Read) && !host.line_comments().saved.dirty.is_empty();
    if should_flush {
        let writes: Vec<(String, Option<Value>)> = {
            let lc = host.line_comments_mut();
            std::mem::take(&mut lc.saved.dirty)
                .into_iter()
                .map(|sid| {
                    let payload = lc.sessions.get(&sid).map(|s| &s.list).filter(|l| !l.is_empty()).map(|l| encode(l));
                    (sid, payload)
                })
                .collect()
        };
        for (sid, payload) in writes {
            match payload {
                Some(val) => host.save_comments_json(NAMESPACE, &key(&sid), &val),
                None => host.delete_saved_comments(NAMESPACE, &key(&sid)),
            }
        }
    }
    landed
}
