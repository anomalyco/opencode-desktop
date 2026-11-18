//! Pure headless prompt draft data types shared by queue, fork, and rewind state machines.

use std::ops::Range;

use opencode_kit::client::PathAttachment;
use opencode_kit::model::{Attachment, PromptComment};
use serde_json::{Value, json};

/// A prompt as typed: its text and its `@` pills (byte range, id).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub text: String,
    pub pills: Vec<(Range<usize>, String)>,
}

impl Entry {
    pub fn json(&self) -> Value {
        json!({
            "text": self.text,
            "pills": self.pills.iter().map(|(r, id)| json!([r.start, r.end, id])).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<Entry> {
        let text = v["text"].as_str()?.to_owned();
        let pills = v["pills"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                let (s, e) = (p[0].as_u64()? as usize, p[1].as_u64()? as usize);
                (s < e && e <= text.len() && text.is_char_boundary(s) && text.is_char_boundary(e))
                    .then(|| (s..e, p[2].as_str().unwrap_or_default().to_owned()))
            })
            .collect();
        Some(Entry { text, pills })
    }
}

/// Captured composer draft with caret, file/path attachments, and quote comments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedDraft {
    pub entry: Entry,
    pub caret: usize,
    pub files: Vec<Attachment>,
    pub paths: Vec<PathAttachment>,
    pub quotes: Vec<PromptComment>,
}

impl SavedDraft {
    /// The same words, pills, files, paths and quotes, wherever the caret is.
    pub fn same_words(&self, other: &SavedDraft) -> bool {
        self.entry == other.entry && self.files == other.files && self.paths == other.paths && self.quotes == other.quotes
    }

    pub fn is_empty(&self) -> bool {
        self.entry.text.trim().is_empty() && self.files.is_empty() && self.paths.is_empty() && self.quotes.is_empty()
    }
}
