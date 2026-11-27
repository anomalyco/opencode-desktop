//! Cached incremental memo-key hashing for collapsed assistant turn groups (`c-*`).
//!
//! A single long turn can span ~100 assistant steps and hundreds of `execute` tool calls.
//! Re-hashing every message's `serde_json::Value` metadata on every `RowSpec::build` costs
//! several milliseconds per rebuild even when `rune::memo` hits. Keeping per-message hashes
//! keyed on the host's transcript version (and narrowing invalidation via
//! `TimelineHost::transcript_changed_from`) makes unchanged groups O(1).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use opencode_kit::model::{Message, Part, Transcript};

use crate::TimelineHost;

/// Sessions kept in the thread-local cache (matches `projection::KEPT`).
const KEPT: usize = 4;

struct SessionCache {
    version: u64,
    edits: bool,
    script_diffs: bool,
    len: usize,
    first_id: Option<String>,
    last_id: Option<String>,
    per_msg: Vec<Option<u64>>,
    ranges: HashMap<(usize, usize), u64>,
}

impl SessionCache {
    fn reset(&mut self, version: u64, edits: bool, script_diffs: bool, t: &Transcript) {
        self.version = version;
        self.edits = edits;
        self.script_diffs = script_diffs;
        self.len = t.messages.len();
        self.first_id = t.messages.first().map(|m| m.id().to_owned());
        self.last_id = t.messages.last().map(|m| m.id().to_owned());
        self.per_msg.clear();
        self.per_msg.resize(t.messages.len(), None);
        self.ranges.clear();
    }

    fn same_shape(&self, t: &Transcript) -> bool {
        self.len == t.messages.len()
            && self.first_id.as_deref() == t.messages.first().map(Message::id)
            && self.last_id.as_deref() == t.messages.last().map(Message::id)
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<String, SessionCache>> = RefCell::new(HashMap::new());
}

/// Hash of the turn slice `range` of `t`, cached per session and incrementally updated when only
/// the tail of `t` changed.
pub(super) fn of<S: TimelineHost>(
    s: &S,
    t: &Transcript,
    range: Range<usize>,
    detail: &crate::detail::Detail,
    script_diffs: bool,
) -> u64 {
    let edits = script_diffs && detail.placement(crate::detail::Category::Edit) != crate::detail::Placement::Hidden;
    let Some(version) = s.transcript_version(&t.session) else {
        return hash_uncached(&t.messages[range], edits, script_diffs);
    };
    CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if !map.contains_key(&t.session) && map.len() >= KEPT {
            map.clear();
        }
        let entry = map.entry(t.session.clone()).or_insert_with(|| SessionCache {
            version: u64::MAX,
            edits,
            script_diffs,
            len: 0,
            first_id: None,
            last_id: None,
            per_msg: Vec::new(),
            ranges: HashMap::new(),
        });

        let same_flags = entry.edits == edits && entry.script_diffs == script_diffs;
        if same_flags && entry.version == version && entry.same_shape(t) {
            if let Some(&v) = entry.ranges.get(&(range.start, range.end)) {
                return v;
            }
        } else if same_flags && entry.version != u64::MAX {
            match s.transcript_changed_from(&t.session, entry.version) {
                Some(usize::MAX) if entry.same_shape(t) => {
                    // Only another session (e.g. a running subagent) streamed between versions.
                    entry.version = version;
                    if let Some(&v) = entry.ranges.get(&(range.start, range.end)) {
                        return v;
                    }
                }
                Some(at) if at == 0 || entry.first_id.as_deref() == t.messages.first().map(Message::id) => {
                    entry.per_msg.truncate(at.min(entry.per_msg.len()));
                    entry.per_msg.resize(t.messages.len(), None);
                    entry.ranges.retain(|&(_, e), _| e <= at);
                    entry.version = version;
                    entry.len = t.messages.len();
                    entry.first_id = t.messages.first().map(|m| m.id().to_owned());
                    entry.last_id = t.messages.last().map(|m| m.id().to_owned());
                    if let Some(&v) = entry.ranges.get(&(range.start, range.end)) {
                        return v;
                    }
                }
                _ => entry.reset(version, edits, script_diffs, t),
            }
        } else {
            entry.reset(version, edits, script_diffs, t);
        }

        let mut h = DefaultHasher::new();
        (range.end - range.start).hash(&mut h);
        for i in range.start..range.end {
            let mh = *entry.per_msg[i].get_or_insert_with(|| message_hash(&t.messages[i], edits, script_diffs));
            mh.hash(&mut h);
        }
        let combined = h.finish();
        entry.ranges.insert((range.start, range.end), combined);
        combined
    })
}

fn hash_uncached(messages: &[Message], edits: bool, script_diffs: bool) -> u64 {
    let mut h = DefaultHasher::new();
    messages.len().hash(&mut h);
    for m in messages {
        message_hash(m, edits, script_diffs).hash(&mut h);
    }
    h.finish()
}

fn message_hash(m: &Message, edits: bool, script_diffs: bool) -> u64 {
    let mut h = DefaultHasher::new();
    if let Message::Notice(n) = m {
        (3u8, &n.id).hash(&mut h);
    }
    let step = match m {
        Message::Assistant(a) => Some(a),
        Message::Note { step, .. } => Some(&**step),
        _ => None,
    };
    if let Some(a) = step {
        (&a.id, a.completed, a.parts.len(), &a.text_phases).hash(&mut h);
        for p in &a.parts {
            match p {
                Part::Tool { id, name, status, metadata, .. } => {
                    (0u8, id, name, std::mem::discriminant(status)).hash(&mut h);
                    // A script's changes name the group as they arrive; progress that adds
                    // none keeps the row.
                    if edits {
                        crate::activity::rules::script_changes(name, status, metadata)
                            .for_each(|(i, f)| (i, f["file"].as_str()).hash(&mut h));
                    }
                    if script_diffs && name == "execute" {
                        metadata["toolCalls"].as_array().into_iter().flatten().for_each(|c| {
                            (
                                c["tool"].as_str(),
                                c["status"].as_str(),
                                c["input"]["method"].as_str(),
                                c["input"]["path"].as_str(),
                            )
                                .hash(&mut h);
                        });
                    }
                }
                Part::Reasoning { text, completed, .. } => (1u8, text.len(), completed).hash(&mut h),
                Part::Text { text } => (2u8, text.len()).hash(&mut h),
            }
        }
    }
    h.finish()
}
