//! Converts Official Desktop prompt drafts (`document` + `blob` tables, `ComposerStore` JSON) and
//! prompt history (`opencode.global.dat:prompt-history{,-shell}`) into Rune's `draft_doc` and
//! `prompt-history.v1` documents.

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use opencode_kit::model::PromptComment;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub(super) const MAX_SEED_DOC_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_SEED_BLOB_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_SEED_TOTAL_BLOBS_BYTES: usize = 48 * 1024 * 1024;
/// Matches `crates/opencode-ui/src/prompt_history.rs::MAX` (`#594`) and Official `entry.ts:12`.
const MAX_HISTORY_ENTRIES: usize = 200;

/// FNV-1a 32-bit hash over UTF-16 code units, base-36 formatted — exact match for Official Desktop's
/// `packages/util/src/encode.ts::checksum`.
pub(super) fn official_checksum(content: &str) -> String {
    if content.is_empty() {
        return "0".into();
    }
    let mut hash: u32 = 0x811c_9dc5;
    for unit in content.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    base36(hash)
}

fn base36(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::with_capacity(8);
    while n > 0 {
        buf.push(DIGITS[(n % 36) as usize] as char);
        n /= 36;
    }
    buf.into_iter().rev().collect()
}

/// Official Desktop document key for a draft tab (`packages/app/src/runtime/persistence/storage.ts:359-364`).
pub(super) fn official_draft_doc_key(draft_id: &str) -> String {
    let raw_head: String = draft_id.chars().take(12).collect();
    let head = if raw_head.is_empty() {
        "draft".to_owned()
    } else {
        raw_head
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
            .collect()
    };
    let sum = official_checksum(draft_id);
    format!("opencode.draft.{head}.{sum}.dat:draft:prompt")
}

/// Maps a UTF-16 code-unit offset in `text` to a UTF-8 byte offset.
fn utf16_to_byte(text: &str, u16_pos: usize) -> usize {
    let mut units = 0usize;
    for (idx, ch) in text.char_indices() {
        if units >= u16_pos {
            return idx;
        }
        units += ch.len_utf16();
    }
    text.len()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    if !meta.ends_with(";base64") || payload.len() > (MAX_SEED_BLOB_BYTES * 4) / 3 + 16 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()?;
    (bytes.len() <= MAX_SEED_BLOB_BYTES).then_some(bytes)
}

/// Strictly decodes Official Desktop's `{ "blob": { "kind": "text", "ids": [...] } }` chunk refs
/// in `value`, failing closed on any missing/non-UTF-8 chunk blob or running size past `budget`.
/// (Unlike `sql::decode_text` / `chunk::decode`, never replaces missing chunks with `""`.)
fn decode_text_chunks_strict(
    value: &Value,
    official: &Connection,
    has_blobs: bool,
    budget: &mut usize,
) -> Result<Value, String> {
    if let Some(map) = value.as_object()
        && map.len() == 1
        && let Some(blob) = map.get("blob").and_then(Value::as_object)
        && blob.get("kind").and_then(Value::as_str) == Some("text")
    {
        if !has_blobs {
            return Err("text chunk reference without blob table".into());
        }
        let ids = blob
            .get("ids")
            .and_then(Value::as_array)
            .filter(|a| !a.is_empty())
            .ok_or_else(|| "text chunk reference missing ids array".to_owned())?;
        let mut joined = String::new();
        for id_val in ids {
            let id = id_val.as_str().filter(|s| !s.is_empty()).ok_or_else(|| "invalid text chunk id".to_owned())?;
            let bytes: Option<Vec<u8>> = official
                .prepare_cached("SELECT data FROM blob WHERE id = ?1")
                .and_then(|mut s| s.query_row([id], |r| r.get(0)).optional())
                .map_err(|e| format!("read text chunk blob {id}: {e}"))?;
            let bytes = bytes.ok_or_else(|| format!("missing text chunk blob {id}"))?;
            if bytes.len() > *budget {
                return Err(format!("decoded document exceeds {MAX_SEED_DOC_BYTES} B cap"));
            }
            let piece = String::from_utf8(bytes).map_err(|_| format!("non-utf8 text chunk blob {id}"))?;
            *budget -= piece.len();
            joined.push_str(&piece);
        }
        return Ok(Value::String(joined));
    }
    match value {
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(decode_text_chunks_strict(item, official, has_blobs, budget)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), decode_text_chunks_strict(v, official, has_blobs, budget)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Parses an Official document JSON string once and strictly resolves all text chunks within
/// `MAX_SEED_DOC_BYTES`.
pub(super) fn parse_official_doc_strict(raw: &str, official: &Connection, has_blobs: bool) -> Result<Value, String> {
    if raw.len() > MAX_SEED_DOC_BYTES {
        return Err(format!("document exceeds {MAX_SEED_DOC_BYTES} B cap"));
    }
    let parsed: Value = serde_json::from_str(raw).map_err(|e| format!("invalid document JSON: {e}"))?;
    if !raw.contains("\"kind\":\"text\"") {
        return Ok(parsed);
    }
    let mut budget = MAX_SEED_DOC_BYTES.saturating_sub(raw.len());
    decode_text_chunks_strict(&parsed, official, has_blobs, &mut budget)
}

/// Reads an Official document by key, parses JSON once, and strictly resolves all text chunks.
pub(super) fn read_official_doc(official: &Connection, key: &str, has_blobs: bool) -> Result<Option<Value>, String> {
    let raw: Option<String> = official
        .prepare_cached("SELECT value FROM document WHERE key = ?1")
        .and_then(|mut s| s.query_row([key], |r| r.get(0)).optional())
        .map_err(|e| format!("read document {key}: {e}"))?;
    let Some(stored) = raw else { return Ok(None) };
    parse_official_doc_strict(&stored, official, has_blobs).map(Some)
}

/// Escapes SQLite `LIKE` wildcards (`\`, `%`, `_`) for use with `ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Loads Official Desktop's `*:session:<sid>:prompt` documents ONLY for `open_sessions` (bounded
/// frame-0 scan; never loads closed historical sessions into RAM).
pub(super) fn load_session_docs(official: &Connection, open_sessions: &HashSet<String>) -> Result<HashMap<String, String>, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    if open_sessions.is_empty() {
        return Ok(out);
    }
    let mut stmt = official
        .prepare("SELECT value FROM document WHERE key LIKE ?1 ESCAPE '\\'")
        .map_err(|e| format!("prepare session documents: {e}"))?;
    for sid in open_sessions {
        if sid.is_empty() || sid.contains(':') {
            continue;
        }
        let pattern = format!("%:session:{}:prompt", escape_like(sid));
        let rows = stmt
            .query_map([&pattern], |r| r.get::<_, String>(0))
            .map_err(|e| format!("query session document {sid}: {e}"))?;
        for row in rows {
            let val = row.map_err(|e| format!("read session document {sid}: {e}"))?;
            if val.len() > MAX_SEED_DOC_BYTES {
                continue;
            }
            let replace = match out.get(sid) {
                None => true,
                Some(prev) => prev.contains("\"content\":\"\"") && !val.contains("\"content\":\"\""),
            };
            if replace {
                out.insert(sid.clone(), val);
            }
        }
    }
    Ok(out)
}

/// Formats Official Desktop's prompt mention part into Rune's mandatory `kind:value` pill id
/// (`crates/opencode-kit/src/mention.rs:195-204`, `features/mentions/src/index.rs:225-290`,
/// `features/mentions/src/wire.rs:72-105`).
fn rune_pill_id(kind: &str, part: &Value) -> Option<String> {
    match kind {
        "file" => {
            let path = part["path"].as_str().filter(|s| !s.is_empty())?;
            let prefix = if part["mime"].as_str() == Some("application/x-directory") { "reference" } else { "file" };
            Some(format!("{prefix}:{path}"))
        }
        "agent" => {
            let name = part["name"].as_str().filter(|s| !s.is_empty())?;
            Some(format!("agent:{name}"))
        }
        "skill" => {
            let id = part["id"].as_str().filter(|s| !s.is_empty()).or_else(|| part["name"].as_str().filter(|s| !s.is_empty()))?;
            Some(format!("skill:{id}"))
        }
        "session" => {
            let id = part["id"].as_str().filter(|s| !s.is_empty())?;
            Some(format!("session:{id}"))
        }
        _ => None,
    }
}

struct PartsOut {
    text: String,
    pills: Vec<Value>,
    paths: Vec<Value>,
    files: Vec<Value>,
}

fn convert_prompt_parts(
    parts: &[Value],
    official: &Connection,
    has_blobs: bool,
    blobs_out: &mut Vec<(String, Vec<u8>)>,
    blob_budget: &mut usize,
) -> Result<PartsOut, String> {
    let mut text = String::new();
    let mut pills = Vec::new();
    let mut paths = Vec::new();
    let mut files = Vec::new();

    for part in parts {
        let kind = part["type"].as_str().unwrap_or("");
        match kind {
            "text" => {
                if let Some(c) = part["content"].as_str() {
                    text.push_str(c);
                }
            }
            "file" | "agent" | "skill" | "session" => {
                if let Some(c) = part["content"].as_str() {
                    let start = text.len();
                    text.push_str(c);
                    let end = text.len();
                    if let Some(id) = rune_pill_id(kind, part) && start < end {
                        pills.push(json!([start, end, id]));
                    }
                }
            }
            "path" => {
                let (Some(name), Some(mime), Some(path)) = (part["filename"].as_str(), part["mime"].as_str(), part["path"].as_str()) else {
                    continue;
                };
                paths.push(json!({ "name": name, "mime": mime, "path": path }));
            }
            "image" => {
                let name = part["filename"].as_str().unwrap_or("image").to_owned();
                let mime = part["mime"].as_str().unwrap_or("application/octet-stream").to_owned();
                let blob_id = part["blob"]["id"].as_str().unwrap_or("");
                let media_ref = part["media"]
                    .as_str()
                    .or_else(|| blob_id.starts_with("media:").then_some(blob_id))
                    .filter(|s| s.starts_with("media:"));
                if let Some(media) = media_ref {
                    files.push(json!({ "name": name, "mime": mime, "media": media }));
                    continue;
                }
                let data_url = part["dataUrl"]
                    .as_str()
                    .or_else(|| part["blob"]["url"].as_str().filter(|u| u.starts_with("data:")))
                    .or_else(|| blob_id.starts_with("data:").then_some(blob_id));

                let (id, bytes) = if let Some(u) = data_url {
                    let b = decode_data_url(u).ok_or_else(|| format!("invalid inline image dataUrl for {name}"))?;
                    let id = hex_sha256(&b);
                    (id, b)
                } else if !blob_id.is_empty() && has_blobs {
                    let b: Option<Vec<u8>> = official
                        .prepare_cached("SELECT data FROM blob WHERE id = ?1")
                        .and_then(|mut s| s.query_row([blob_id], |r| r.get(0)).optional())
                        .map_err(|e| format!("read blob {blob_id}: {e}"))?;
                    let b = b.ok_or_else(|| format!("missing blob {blob_id} for {name}"))?;
                    if b.len() > MAX_SEED_BLOB_BYTES {
                        return Err(format!("blob {blob_id} exceeds {MAX_SEED_BLOB_BYTES} B cap"));
                    }
                    (blob_id.to_owned(), b)
                } else {
                    return Err(format!("image attachment {name} has no resolvable blob"));
                };
                if bytes.len() > *blob_budget {
                    return Err(format!("total draft blob budget ({MAX_SEED_TOTAL_BLOBS_BYTES} B) exceeded"));
                }
                *blob_budget -= bytes.len();
                let size = bytes.len();
                blobs_out.push((id.clone(), bytes));
                files.push(json!({ "name": name, "mime": mime, "size": size, "blob": { "id": id } }));
            }
            _ => {}
        }
    }

    Ok(PartsOut { text, pills, paths, files })
}

fn parse_context_comment(item: &Value) -> Option<PromptComment> {
    // Official's `FileContextItem` stores `commentOrigin` (`schema.ts:158`), while Rune's
    // `FileComment::parse` (`quote.rs:240`) reads `origin`.
    if item["type"].as_str() == Some("file")
        && item.get("origin").is_none()
        && let Some(co) = item.get("commentOrigin")
        && let Some(mut obj) = item.as_object().cloned()
    {
        obj.insert("origin".into(), co.clone());
        return PromptComment::parse(&Value::Object(obj));
    }
    PromptComment::parse(item)
}

/// Converts a strictly decoded Official Desktop `ComposerStore` JSON value into Rune's `draft_doc`
/// JSON string. Returns `Ok(None)` when the draft is genuinely empty, `Err` if malformed or any
/// attachment blob is missing/oversized (caller drops the draft tab rather than opening it blank).
pub(super) fn convert_composer_value(
    val: &Value,
    official: &Connection,
    has_blobs: bool,
    blobs_out: &mut Vec<(String, Vec<u8>)>,
    blob_budget: &mut usize,
) -> Result<Option<String>, String> {
    let parts = val["prompt"].as_array().ok_or_else(|| "draft missing prompt array".to_owned())?;
    let p = convert_prompt_parts(parts, official, has_blobs, blobs_out, blob_budget)?;
    let shell = val["mode"].as_str() == Some("shell");

    let mut context = Vec::new();
    let mut quotes = Vec::new();
    for item in val["context"]["items"].as_array().into_iter().flatten() {
        let kind = item["type"].as_str().unwrap_or("");
        let has_comment = item["comment"].as_str().is_some_and(|c| !c.trim().is_empty());
        if kind == "file" && !has_comment {
            if let (Some(path), Some(start), Some(end)) = (
                item["path"].as_str(),
                item["selection"]["startLine"].as_u64(),
                item["selection"]["endLine"].as_u64(),
            ) {
                context.push(json!({ "path": path, "start": start, "end": end }));
            }
        } else if let Some(comment) = parse_context_comment(item) {
            if let Ok(v) = serde_json::to_value(comment) {
                quotes.push(v);
            }
        }
    }

    let caret = val["cursor"]
        .as_u64()
        .map(|u| utf16_to_byte(&p.text, u as usize))
        .filter(|&c| c < p.text.len());
    let has_rich = !p.pills.is_empty()
        || !p.paths.is_empty()
        || !context.is_empty()
        || !quotes.is_empty()
        || !p.files.is_empty()
        || (!p.text.trim().is_empty() && caret.is_some());

    if !has_rich {
        return match (shell, p.text.trim().is_empty()) {
            (true, _) => Ok(Some(json!({ "text": p.text, "mode": "shell" }).to_string())),
            (false, true) => Ok(None),
            (false, false) => Ok(Some(json!(p.text).to_string())),
        };
    }

    let mut obj = Map::new();
    obj.insert("text".into(), json!(p.text));
    if shell {
        obj.insert("mode".into(), json!("shell"));
    }
    if let Some(c) = caret {
        obj.insert("caret".into(), json!(c));
    }
    if !p.pills.is_empty() {
        obj.insert("pills".into(), Value::Array(p.pills));
    }
    if !p.paths.is_empty() {
        obj.insert("paths".into(), Value::Array(p.paths));
    }
    if !context.is_empty() {
        obj.insert("context".into(), Value::Array(context));
    }
    if !quotes.is_empty() {
        obj.insert("quotes".into(), Value::Array(quotes));
    }
    if !p.files.is_empty() {
        obj.insert("files".into(), Value::Array(p.files));
    }
    Ok(Some(Value::Object(obj).to_string()))
}

/// Converts Official Desktop's `opencode.global.dat:prompt-history` and `prompt-history-shell`
/// into Rune's `opencode.global.dat:prompt-history.v1` JSON (`{"normal":[...],"shell":[...]}`).
pub(super) fn convert_prompt_history(official: &Connection, has_blobs: bool) -> Option<String> {
    let parse_list = |key: &str| -> Vec<Value> {
        let Ok(Some(val)) = read_official_doc(official, key, has_blobs) else { return Vec::new() };
        let mut out = Vec::new();
        for entry in val["entries"].as_array().into_iter().flatten().take(MAX_HISTORY_ENTRIES) {
            let parts = entry["prompt"].as_array().or_else(|| entry.as_array());
            let Some(parts) = parts else { continue };
            let mut text = String::new();
            let mut pills = Vec::new();
            for part in parts {
                let kind = part["type"].as_str().unwrap_or("");
                if let Some(c) = part["content"].as_str() {
                    let start = text.len();
                    text.push_str(c);
                    let end = text.len();
                    if let Some(id) = rune_pill_id(kind, part) && start < end {
                        pills.push(json!([start, end, id]));
                    }
                }
            }
            if !text.trim().is_empty() {
                out.push(json!({ "text": text, "pills": pills }));
            }
        }
        out
    };
    let normal = parse_list("opencode.global.dat:prompt-history");
    let shell = parse_list("opencode.global.dat:prompt-history-shell");
    if normal.is_empty() && shell.is_empty() {
        return None;
    }
    Some(json!({ "normal": normal, "shell": shell }).to_string())
}
