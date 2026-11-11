//! Large strings leave a document as content-addressed text chunks, as Desktop's draft store
//! keeps them (`packages/app/src/runtime/persistence/drafts.ts:40-44, 288-297, 518-532`): a
//! string of at least [`TEXT_THRESHOLD`] UTF-16 units becomes `{ "blob": { "kind": "text",
//! "ids": [...] } }`, each id the SHA-256 of a [`TEXT_CHUNK`]-unit piece. Typing after a large
//! paste changes only the last chunk.

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// `draftTextThreshold`, in UTF-16 code units as JavaScript counts them.
pub const TEXT_THRESHOLD: usize = 16 * 1024;
/// `draftTextChunk`, in UTF-16 code units.
pub const TEXT_CHUNK: usize = 64 * 1024;

/// A blob's id: the lowercase hex SHA-256 of its bytes.
pub fn blob_id(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
        out.push(char::from(b"0123456789abcdef"[usize::from(b & 15)]));
    }
    out
}

/// Fixed-size pieces of `text`; a piece never ends between the halves of a surrogate pair
/// (`split`, `drafts.ts:520-532`), which in Rust is a char boundary.
pub fn split(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let (mut start, mut units) = (0, 0);
    for (at, c) in text.char_indices() {
        if units >= TEXT_CHUNK {
            pieces.push(&text[start..at]);
            (start, units) = (at, 0);
        }
        units += c.len_utf16();
    }
    if start < text.len() || pieces.is_empty() {
        pieces.push(&text[start..]);
    }
    pieces
}

fn utf16_len_at_least(text: &str, n: usize) -> bool {
    // UTF-8 is at most 3 bytes per UTF-16 unit, and at least 1.
    if text.len() < n {
        return false;
    }
    if text.len() >= 3 * n {
        return true;
    }
    text.chars().map(char::len_utf16).sum::<usize>() >= n
}

/// `value` with every long string externalized; `chunks` receives each new chunk's id and bytes.
pub fn encode(value: &Value, chunks: &mut Vec<(String, Vec<u8>)>) -> Value {
    match value {
        Value::String(s) if utf16_len_at_least(s, TEXT_THRESHOLD) => {
            let ids: Vec<String> = split(s)
                .into_iter()
                .map(|piece| {
                    let id = blob_id(piece.as_bytes());
                    if !chunks.iter().any(|(c, _)| *c == id) {
                        chunks.push((id.clone(), piece.as_bytes().to_vec()));
                    }
                    id
                })
                .collect();
            json!({ "blob": { "kind": "text", "ids": ids } })
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| encode(v, chunks)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), encode(v, chunks))).collect::<Map<_, _>>()),
        other => other.clone(),
    }
}

/// The text chunk ids `value` references, in order.
pub fn text_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(ids) = text_chunk_ids(value) {
                out.extend(ids.map(str::to_owned));
                return;
            }
            map.values().for_each(|v| text_refs(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| text_refs(v, out)),
        _ => {}
    }
}

fn text_chunk_ids(value: &Value) -> Option<impl Iterator<Item = &str>> {
    let blob = value.as_object().filter(|m| m.len() == 1)?.get("blob")?;
    (blob["kind"] == "text").then_some(())?;
    Some(blob["ids"].as_array()?.iter().filter_map(Value::as_str))
}

/// The inverse of [`encode`]: `chunk` gives a chunk's text; a missing chunk loses that text but
/// keeps the rest of the document (`loadChunk`, `drafts.ts:271-279`).
pub fn decode(value: &Value, chunk: &mut dyn FnMut(&str) -> Option<String>) -> Value {
    if let Some(ids) = text_chunk_ids(value) {
        return Value::String(ids.map(|id| chunk(id).unwrap_or_default()).collect());
    }
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|v| decode(v, chunk)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), decode(v, chunk))).collect()),
        other => other.clone(),
    }
}
