//! A prompt as its outbox row (`opencode_storage::OutboxRow`), built and read on the storage
//! thread, never the UI's. The draft, prompt, paths and operation keep the shapes the file
//! journal had, so rows imported from it read the same. Each file's bytes are one blob, stored
//! once by SHA-256: an attachment names it, and the prompt's inline file is `blob:<id>` beside
//! its `data:` prefix (`blobPrefix`), as the importer writes them.

use std::sync::Arc;

use opencode_kit::client::{Inline, Prompt};
use opencode_kit::model::Attachment;
use opencode_storage::chunk::blob_id;
use opencode_storage::{OutboxAttachment, OutboxEntry, OutboxPut, OutboxRow, OutboxUpdate};
use serde_json::{Value, json};

use super::machine::Phase;
use super::{AfterCheck, Sent, State};

/// The prompt's first row and its blobs. `s` is the outbox's copy, its bytes shared.
pub fn build(server: String, s: Sent) -> OutboxPut {
    let mut blobs: Vec<(String, Arc<[u8]>)> = Vec::new();
    // An attachment and the prompt's inline file share their bytes: one hash, one blob.
    let mut blob = |bytes: &Arc<[u8]>| -> String {
        if let Some((id, _)) = blobs.iter().find(|(_, b)| Arc::ptr_eq(b, bytes)) {
            return id.clone();
        }
        let id = blob_id(bytes);
        if !blobs.iter().any(|(b, _)| *b == id) {
            blobs.push((id.clone(), bytes.clone()));
        }
        id
    };
    let attachments = s
        .files
        .iter()
        .map(|f| OutboxAttachment {
            name: f.name.clone(),
            mime: f.mime.clone(),
            blob: (!f.bytes.is_empty()).then(|| blob(&f.bytes)),
            media: f.media.clone(),
        })
        .collect();
    let prompt_json = s.prompt.as_ref().map(|p| {
        let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
        for (file, input) in v["files"].as_array_mut().into_iter().flatten().zip(&p.files) {
            if let Some(inline) = &input.inline {
                file["uri"] = json!(format!("blob:{}", blob(&inline.bytes)));
                file["blobPrefix"] = json!(format!("data:{};base64,", inline.mime));
            }
        }
        v.to_string()
    });
    let text = |v: Result<String, serde_json::Error>| v.unwrap_or_else(|_| "null".into());
    let state = update(&s);
    let row = OutboxRow {
        server,
        id: s.id.clone(),
        seq: s.seq,
        session: s.session.clone(),
        state: state.state,
        operation_json: state.operation_json,
        draft_json: text(serde_json::to_string(&s.draft)),
        prompt_json,
        attachments,
        paths_json: text(serde_json::to_string(&s.paths)),
        inbox: s.inbox,
        why: state.why,
        after_check: state.after_check,
        restored: state.restored,
        durable: s.journaled,
        legacy_json: None,
    };
    let clear_documents = match s.left_draft {
        super::LeftDraft::None => Vec::new(),
        super::LeftDraft::Pending { key, rev } => vec![(key, rev)],
    };
    OutboxPut { row, blobs, clear_documents }
}

/// The columns a later step changes: where it is, why, and what waits on it.
pub fn update(s: &Sent) -> OutboxUpdate {
    let state = match s.state() {
        State::Sending => "Sending",
        State::Unconfirmed => "Unconfirmed",
        State::Failed => "Failed",
        State::Accepted => "Accepted",
        State::Waiting => "Waiting",
    };
    OutboxUpdate {
        state: state.into(),
        why: s.why().to_owned(),
        after_check: s.then().map(|a| match a {
            AfterCheck::Edit => "Edit".into(),
            AfterCheck::Delete => "Delete".into(),
        }),
        restored: matches!(s.phase, Phase::Failed { restored: true, .. }),
        operation_json: serde_json::to_string(&s.operation).unwrap_or_else(|_| "\"Send\"".into()),
    }
}

/// A stored row as the prompt it holds, its bytes back from their blobs. A row whose bytes are
/// gone comes back failed, never sent without them.
pub fn sent(entry: OutboxEntry) -> Sent {
    let OutboxEntry { row, blobs } = entry;
    let bytes = |id: &str| blobs.iter().find(|(b, _)| b == id).map(|(_, d)| d.clone());
    let mut missing = false;
    let files: Vec<Attachment> = row
        .attachments
        .iter()
        .map(|a| {
            let data = a.blob.as_deref().map(|id| {
                bytes(id).unwrap_or_else(|| {
                    missing = true;
                    Arc::from([])
                })
            });
            Attachment { mime: a.mime.clone(), name: a.name.clone(), bytes: data.unwrap_or_else(|| Arc::from([])), media: a.media.clone() }
        })
        .collect();
    let prompt = row.prompt_json.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|mut v| {
        let mut inline: Vec<Option<Inline>> = Vec::new();
        for file in v["files"].as_array_mut().into_iter().flatten() {
            let id = file["uri"].as_str().and_then(|u| u.strip_prefix("blob:")).map(str::to_owned);
            let mime = file["blobPrefix"].as_str().and_then(|p| p.strip_prefix("data:")).and_then(|p| p.strip_suffix(";base64,")).map(str::to_owned);
            inline.push(match (id, mime) {
                (Some(id), Some(mime)) => {
                    file["uri"] = json!("");
                    match bytes(&id) {
                        Some(bytes) => Some(Inline { mime, bytes }),
                        None => {
                            missing = true;
                            None
                        }
                    }
                }
                _ => None,
            });
            if let Some(m) = file.as_object_mut() {
                m.remove("blobPrefix");
            }
        }
        let mut p: Prompt = serde_json::from_value(v).ok()?;
        for (f, i) in p.files.iter_mut().zip(inline) {
            f.inline = i;
        }
        Some(p)
    });
    let (why, then) = (
        row.why,
        row.after_check.as_deref().and_then(|a| match a {
            "Edit" => Some(AfterCheck::Edit),
            "Delete" => Some(AfterCheck::Delete),
            _ => None,
        }),
    );
    let phase = match row.state.as_str() {
        _ if missing => Phase::Failed { why: "Couldn't read its attachment".into(), restored: false },
        "Waiting" => Phase::Waiting { why },
        "Sending" => Phase::Sending { by: 0, then, handoff: 0, slow: super::SlowSend::Unstamped },
        "Unconfirmed" => Phase::Unconfirmed { why, then },
        "Accepted" => Phase::Accepted,
        _ => Phase::Failed { why, restored: row.restored },
    };
    Sent {
        session: row.session,
        id: row.id,
        draft: serde_json::from_str(&row.draft_json).unwrap_or_default(),
        files,
        paths: serde_json::from_str(&row.paths_json).unwrap_or_default(),
        inbox: row.inbox,
        prompt,
        phase,
        journaled: row.durable,
        operation: serde_json::from_str(&row.operation_json).unwrap_or_default(),
        seq: row.seq,
        row: super::store::Row::Written,
        handoffs: 0,
        left_draft: super::LeftDraft::None,
    }
}
