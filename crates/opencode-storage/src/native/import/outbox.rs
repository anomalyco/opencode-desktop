//! `outbox-<sha256>.json`, the outbox's journal (`apps/desktop/src/send_failed/journal.rs`): a
//! JSON array of rows, each attachment's bytes inline as an integer array and again as a
//! base64 `data:` URL inside the prompt. Rows become `outbox` rows; the bytes are stored once,
//! in `blob`, by SHA-256.
//!
//! The file name is `sha256("server\n<base>")` (or an older build's `sha256("<window>\n<base>")`),
//! so the server is found by hashing the addresses the profile knows; a file no known address
//! matches keeps its rows under `sha256:<hex>` until the app opens that server's outbox.

use std::path::Path;

use base64::Engine;
use serde_json::{Value, json};

use super::ImportConfig;
use super::shapes::{Row, Source};
use crate::chunk::blob_id;
use crate::model::{OutboxAttachment, OutboxRow};

pub fn source(cfg: &ImportConfig, windows: &[String], path: &Path, raw: &str, value: &Value) -> Option<Source> {
    let hash = path.file_stem()?.to_str()?.strip_prefix("outbox-")?.to_owned();
    let server = server_of(cfg, windows, &hash).unwrap_or_else(|| format!("sha256:{hash}"));
    let mut source = Source { paths: vec![path.to_path_buf()], ..Source::default() };
    let _ = raw;
    for (seq, row) in value.as_array()?.iter().enumerate() {
        let (out, blobs) = convert(&server, i64::try_from(seq).unwrap_or(i64::MAX), row);
        for (id, data) in blobs {
            if !source.blobs.iter().any(|(b, _)| *b == id) {
                source.blobs.push((id, data));
            }
        }
        source.rows.push(Row::Outbox(Box::new(out)));
    }
    Some(source)
}

fn server_of(cfg: &ImportConfig, windows: &[String], hash: &str) -> Option<String> {
    let ctx = &cfg.context;
    let mut bases: Vec<&str> = cfg.servers.iter().chain(&ctx.sidecar).chain(&ctx.saved).map(String::as_str).collect();
    bases.extend(ctx.aliases.iter().map(|(u, _)| u.as_str()));
    let hex = |s: String| blob_id(s.as_bytes());
    bases.iter().find(|b| hex(format!("server\n{b}")) == hash || windows.iter().any(|w| hex(format!("{w}\n{b}")) == hash)).map(|b| (*b).to_owned())
}

/// One journal row as an outbox row and the blobs it needs. A row the columns can't give back
/// exactly also keeps its JSON verbatim (`legacy_json`).
fn convert(server: &str, seq: i64, row: &Value) -> (OutboxRow, Vec<(String, Vec<u8>)>) {
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
    let attachments = row["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            let bytes: Vec<u8> = f["bytes"].as_array().into_iter().flatten().filter_map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok())).collect();
            let blob = (!bytes.is_empty()).then(|| {
                let id = blob_id(&bytes);
                blobs.push((id.clone(), bytes));
                id
            });
            OutboxAttachment { name: str_of(&f["name"]), mime: str_of(&f["mime"]), blob, media: f["media"].as_str().map(str::to_owned) }
        })
        .collect();
    let prompt = (!row["prompt"].is_null()).then(|| {
        let mut p = row["prompt"].clone();
        for file in p["files"].as_array_mut().into_iter().flatten() {
            externalize_uri(file, &mut blobs);
        }
        p.to_string()
    });
    let out = OutboxRow {
        server: server.to_owned(),
        id: str_of(&row["id"]),
        seq,
        session: str_of(&row["session"]),
        state: row["state"].as_str().unwrap_or("Failed").to_owned(),
        operation_json: row.get("operation").cloned().unwrap_or(Value::Null).to_string(),
        draft_json: row["draft"].to_string(),
        prompt_json: prompt,
        attachments,
        paths_json: row.get("paths").cloned().unwrap_or_else(|| json!([])).to_string(),
        inbox: row["inbox"].as_bool().unwrap_or(false),
        why: str_of(&row["why"]),
        after_check: row["after_check"].as_str().map(str::to_owned),
        restored: row["restored_to_composer"].as_bool().unwrap_or(false),
        durable: row["durable"].as_bool().unwrap_or(false),
        legacy_json: None,
    };
    let exact = rebuild(&out, &blobs) == *row;
    (OutboxRow { legacy_json: (!exact).then(|| row.to_string()), ..out }, blobs)
}

/// `data:<mime>;base64,<bytes>` becomes `blob:<sha256>` with the prefix kept beside it, when the
/// base64 re-encodes to the same text.
fn externalize_uri(file: &mut Value, blobs: &mut Vec<(String, Vec<u8>)>) {
    let Some(uri) = file["uri"].as_str() else { return };
    let Some((prefix, data)) = uri.split_once(";base64,").filter(|(p, _)| p.starts_with("data:")) else { return };
    let engine = base64::engine::general_purpose::STANDARD;
    let Ok(bytes) = engine.decode(data) else { return };
    if engine.encode(&bytes) != data {
        return;
    }
    let id = blob_id(&bytes);
    let prefix = format!("{prefix};base64,");
    if !blobs.iter().any(|(b, _)| *b == id) {
        blobs.push((id.clone(), bytes));
    }
    file["uri"] = json!(format!("blob:{id}"));
    file["blobPrefix"] = json!(prefix);
}

/// The journal row the columns give back (`journal.rs` `Row`), bytes put back inline.
pub fn rebuild(row: &OutboxRow, blobs: &[(String, Vec<u8>)]) -> Value {
    let bytes = |id: &Option<String>| -> Vec<u8> { id.as_ref().and_then(|id| blobs.iter().find(|(b, _)| b == id)).map(|(_, d)| d.clone()).unwrap_or_default() };
    let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap_or(Value::Null);
    let files: Vec<Value> = row.attachments.iter().map(|a| json!({ "mime": a.mime, "name": a.name, "bytes": bytes(&a.blob), "media": a.media })).collect();
    let prompt = row.prompt_json.as_deref().map(parse).map(|mut p| {
        for file in p["files"].as_array_mut().into_iter().flatten() {
            let (Some(uri), Some(prefix)) = (file["uri"].as_str(), file["blobPrefix"].as_str()) else { continue };
            let Some(id) = uri.strip_prefix("blob:") else { continue };
            let data = base64::engine::general_purpose::STANDARD.encode(bytes(&Some(id.to_owned())));
            let uri = format!("{prefix}{data}");
            if let Some(m) = file.as_object_mut() {
                m.remove("blobPrefix");
                m.insert("uri".into(), json!(uri));
            }
        }
        p
    });
    json!({
        "session": row.session,
        "id": row.id,
        "draft": parse(&row.draft_json),
        "files": files,
        "paths": parse(&row.paths_json),
        "inbox": row.inbox,
        "why": row.why,
        "state": row.state,
        "prompt": prompt,
        "durable": row.durable,
        "operation": parse(&row.operation_json),
        "after_check": row.after_check,
        "restored_to_composer": row.restored,
    })
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
