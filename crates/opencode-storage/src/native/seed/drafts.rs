//! Converts Official Desktop prompt drafts (`document` + `blob` tables, `ComposerStore` JSON) and
//! prompt history (`opencode.global.dat:prompt-history{,-shell}`) into Rune's `draft_doc` and
//! `prompt-history.v1` documents.

use std::collections::HashMap;

use base64::Engine as _;
use opencode_kit::model::PromptComment;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::sql;

pub(super) const MAX_SEED_DOC_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_SEED_BLOB_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_SEED_TOTAL_BLOBS_BYTES: usize = 48 * 1024 * 1024;
const MAX_HISTORY_ENTRIES: usize = 100;

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

/// Reads an Official document by key, decoding any chunked text from Official's `blob` table.
/// Fails closed if the document exceeds `MAX_SEED_DOC_BYTES` or a referenced text chunk is missing.
pub(super) fn read_official_doc(official: &Connection, key: &str, has_blobs: bool) -> Result<Option<String>, String> {
    let raw: Option<String> = official
        .prepare_cached("SELECT value FROM document WHERE key = ?1")
        .and_then(|mut s| s.query_row([key], |r| r.get(0)).optional())
        .map_err(|e| format!("read document {key}: {e}"))?;
    let Some(stored) = raw else { return Ok(None) };
    if stored.len() > MAX_SEED_DOC_BYTES {
        return Err(format!("document {key} exceeds {} B cap", MAX_SEED_DOC_BYTES));
    }
    if !stored.contains("\"kind\":\"text\"") {
        return Ok(Some(stored));
    }
    if !has_blobs {
        return Err(format!("document {key} references text chunks without blob table"));
    }
    let decoded = sql::decode_text(official, stored);
    if decoded.contains("\"kind\":\"text\"") {
        return Err(format!("document {key} has missing text chunk blob"));
    }
    if decoded.len() > MAX_SEED_DOC_BYTES {
        return Err(format!("decoded document {key} exceeds {} B cap", MAX_SEED_DOC_BYTES));
    }
    Ok(Some(decoded))
}

/// Loads Official Desktop's `*:session:<sid>:prompt` documents indexed by `session_id`
/// (preferring a non-empty prompt if multiple workspace aliases exist for the same session).
pub(super) fn load_session_docs(official: &Connection) -> Result<HashMap<String, String>, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let mut stmt = official
        .prepare("SELECT key, value FROM document WHERE key LIKE '%:session:%:prompt'")
        .map_err(|e| format!("query session documents: {e}"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("map session documents: {e}"))?;
    for row in rows {
        let (key, val) = row.map_err(|e| format!("read session document row: {e}"))?;
        if val.len() > MAX_SEED_DOC_BYTES {
            continue;
        }
        let Some(rest) = key.split_once(":session:").map(|(_, r)| r) else { continue };
        let Some(sid) = rest.strip_suffix(":prompt") else { continue };
        if sid.is_empty() || sid.contains(':') {
            continue;
        }
        let replace = match out.get(sid) {
            None => true,
            Some(prev) => prev.contains("\"content\":\"\"") && !val.contains("\"content\":\"\""),
        };
        if replace {
            out.insert(sid.to_owned(), val);
        }
    }
    Ok(out)
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
                    let target = match kind {
                        "file" => part["path"].as_str(),
                        "agent" | "skill" => part["name"].as_str(),
                        "session" => part["id"].as_str(),
                        _ => None,
                    };
                    if let Some(id) = target && start < end {
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
                        return Err(format!("blob {blob_id} exceeds {} B cap", MAX_SEED_BLOB_BYTES));
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

/// Converts Official Desktop's `ComposerStore` JSON string into Rune's `draft_doc` JSON string.
/// Returns `Ok(None)` when the draft is genuinely empty, `Err` if malformed or any attachment blob
/// is missing/oversized (caller drops the draft tab rather than opening a blank tab).
pub(super) fn convert_composer_store(
    raw_json: &str,
    official: &Connection,
    has_blobs: bool,
    blobs_out: &mut Vec<(String, Vec<u8>)>,
    blob_budget: &mut usize,
) -> Result<Option<String>, String> {
    let decoded_str = if raw_json.contains("\"kind\":\"text\"") {
        if !has_blobs {
            return Err("draft references text chunks without blob table".into());
        }
        let d = sql::decode_text(official, raw_json.to_owned());
        if d.contains("\"kind\":\"text\"") {
            return Err("draft references missing text chunk blob".into());
        }
        d
    } else {
        raw_json.to_owned()
    };
    if decoded_str.len() > MAX_SEED_DOC_BYTES {
        return Err("decoded draft exceeds cap".into());
    }
    let val: Value = serde_json::from_str(&decoded_str).map_err(|e| format!("invalid draft JSON: {e}"))?;
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
        } else if let Some(comment) = PromptComment::parse(item) {
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

    let mut obj = serde_json::Map::new();
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
        let Ok(Some(raw)) = read_official_doc(official, key, has_blobs) else { return Vec::new() };
        let Ok(val) = serde_json::from_str::<Value>(&raw) else { return Vec::new() };
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
                    let target = match kind {
                        "file" => part["path"].as_str(),
                        "agent" | "skill" => part["name"].as_str(),
                        "session" => part["id"].as_str(),
                        _ => None,
                    };
                    if let Some(id) = target && start < end {
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
