//! Rewriting a session's queued prompts, as Desktop's `session/composer/queue.ts` does. The
//! server's inbox can't reorder, so a reorder or a queued edit re-admits the changed suffix under
//! new ids and only then cancels the originals (`rewrite`, `queue.ts:318-419`); a steered edit
//! admits its replacement and then cancels the original (`sendEdit`, `queue.ts:275-314`). Each
//! step reports its progress, so the app's queue machine knows how far a failed or timed-out
//! change got.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use super::{Prompt, Server, list, message_id};
use crate::wire_error::WireError;

/// How far a rewrite got (`MutationProgress` and the `mutation.*` events).
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Each inbox id the rewrite replaces, with the id admitted in its place (`mutation.moved`).
    Moved(Vec<(String, String)>),
    /// A step it can't take back is in flight (`stage = "committed"`).
    Committed,
    /// The edited prompt's replacement is admitted (`mutation.admitted`).
    Admitted,
    /// An inbox id the rewrite withdrew or cancelled (`progress.hidden`).
    Hidden(String),
}

/// An edit's replacement for `original`, in the rewrite of its queue position.
pub struct Replace {
    pub original: String,
    /// The admission body ([`edited_admission`]), without its id or delivery.
    pub admission: Value,
    /// The ids minted when the edit was confirmed, by the inbox id each replaces.
    pub moved: HashMap<String, String>,
}

impl Server {
    /// The session's inbox as the server lists it now, raw.
    pub fn inbox_items(&self, session: &str) -> Result<Vec<Value>, WireError> {
        self.read(&format!("/api/session/{session}/inbox")).map(|v| list(&v))
    }

    /// Whether the server holds prompt `id`: in the inbox, or delivered among the newest messages.
    fn holds_prompt(&self, session: &str, id: &str) -> bool {
        let listed = self.inbox_items(session).is_ok_and(|items| items.iter().any(|i| i["id"] == id));
        listed || self.read(&format!("/api/session/{session}/message?limit=20&order=desc")).is_ok_and(|v| list(&v).iter().any(|m| m["id"] == id))
    }

    /// Admits `body` under `id` (`data.session.prompt`).
    fn admit(&self, session: &str, id: &str, body: &Value) -> Result<(), WireError> {
        let mut body = body.clone();
        body["id"] = id.into();
        self.write("POST", &format!("/api/session/{session}/prompt"), Some(body)).map(|_| ())
    }

    /// Re-admits the session's queued prompts from the first that moves, in `order` (the server's
    /// own order when `None`), with `replace` substituted for its original, then cancels the
    /// originals. A copy that fails withdraws the copies already in, so the queue is left as it
    /// was; once every copy is in, the originals go whatever it takes.
    pub fn rewrite_queue(
        &self,
        session: &str,
        order: Option<&[String]>,
        replace: Option<&Replace>,
        abort: &AtomicBool,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), WireError> {
        let aborted = || abort.load(Ordering::SeqCst);
        if aborted() {
            return Err("aborted".into());
        }
        let pending = self.inbox_items(session)?;
        if aborted() {
            return Err("aborted".into());
        }
        if pending.iter().any(|i| i["delivery"] == "queue" && i["type"] != "user") {
            return Err("Queued control items block reordering".into());
        }
        let current: Vec<&Value> = pending.iter().filter(|i| i["type"] == "user" && i["delivery"] == "queue").collect();
        let ordered: Vec<&Value> = match order {
            Some(ids) => ids.iter().filter_map(|id| current.iter().find(|i| i["id"] == id.as_str()).copied()).collect(),
            None => current.clone(),
        };
        let original = replace.map(|r| r.original.as_str());
        // An edited prompt delivered meanwhile is no longer queued; fail so the edit draft stays.
        if ordered.len() != current.len() || original.is_some_and(|o| !current.iter().any(|i| i["id"] == o)) {
            return Err("Queued prompts changed before reordering".into());
        }
        let Some(changed) = ordered.iter().enumerate().position(|(k, i)| i["id"] != current[k]["id"] || original.is_some_and(|o| i["id"] == o)) else {
            return Ok(());
        };
        let moved: Vec<(String, String)> = ordered[changed..]
            .iter()
            .map(|i| {
                let id = id_of(i);
                let next = replace.and_then(|r| r.moved.get(&id).cloned()).unwrap_or_else(message_id);
                (id, next)
            })
            .collect();
        progress(Progress::Moved(moved.clone()));
        let mut replacements: Vec<String> = Vec::new();
        let withdraw = |replacements: &[String], progress: &mut dyn FnMut(Progress)| {
            for id in replacements {
                if self.cancel_inbox(session, id).is_ok() {
                    progress(Progress::Hidden(id.clone()));
                }
            }
        };
        for (item, (old, id)) in ordered[changed..].iter().zip(&moved) {
            let admission = match replace.filter(|r| r.original == *old) {
                Some(r) => with_delivery(&r.admission, "queue"),
                None => copy(session, item),
            };
            // A rewrite stopped by its bound withdraws what it sent, as a failed one does.
            if aborted() {
                withdraw(&replacements, progress);
                return Err("aborted".into());
            }
            // Recorded before sending: the server can admit a prompt whose response then fails.
            replacements.push(id.clone());
            if let Err(e) = self.admit(session, id, &admission) {
                withdraw(&replacements, progress);
                return Err(e);
            }
        }
        if aborted() {
            withdraw(&replacements, progress);
            return Err("aborted".into());
        }
        // Every copy is in, so the originals must go, however long that takes, or the queue
        // holds both.
        progress(Progress::Committed);
        if replace.is_some() {
            progress(Progress::Admitted);
        }
        // An edit cancels last first: the strip shows each original until it goes, so its copies
        // never show out of order.
        let mut replaced: Vec<String> = current[changed..].iter().map(|i| id_of(i)).collect();
        if replace.is_some() {
            replaced.reverse();
        }
        for id in replaced {
            self.cancel_inbox(session, &id)?;
            progress(Progress::Hidden(id));
        }
        Ok(())
    }

    /// A steered edit: `admission` steers under `id` in place of `original`, which then goes. An
    /// original the server delivered meanwhile keeps the edit draft instead of sending twice.
    pub fn steer_edit(&self, session: &str, original: &str, id: &str, admission: &Value, abort: &AtomicBool, progress: &mut dyn FnMut(Progress)) -> Result<(), WireError> {
        if abort.load(Ordering::SeqCst) {
            return Err("aborted".into());
        }
        let pending = self.inbox_items(session)?;
        if !pending.iter().any(|i| i["id"] == original && i["type"] == "user" && i["delivery"] == "queue") {
            return Err("Queued prompt was delivered before the edit".into());
        }
        if abort.load(Ordering::SeqCst) {
            return Err("aborted".into());
        }
        // A steered replacement can be delivered before a withdrawal reaches it, so the bound
        // waits for the admission and the edit returns only if it really did not go out.
        progress(Progress::Committed);
        // An answer can be lost after the server admitted the steer: its receipt decides, so the
        // edit never both goes out and comes back to the composer.
        if let Err(e) = self.admit(session, id, &with_delivery(admission, "steer"))
            && !self.holds_prompt(session, id)
        {
            return Err(e);
        }
        progress(Progress::Admitted);
        // Cancelling is a no-op once the original is gone, so one retry is safe.
        self.cancel_inbox(session, original).or_else(|_| self.cancel_inbox(session, original))
    }
}

fn id_of(item: &Value) -> String {
    item["id"].as_str().unwrap_or_default().to_owned()
}

fn with_delivery(admission: &Value, delivery: &str) -> Value {
    let mut body = admission.clone();
    body["delivery"] = delivery.into();
    body["resume"] = false.into();
    body
}

/// A queued prompt re-admitted as it is (`queue.ts:375-393`).
fn copy(session: &str, item: &Value) -> Value {
    let payload = &item["payload"];
    let mut body = json!({ "sessionID": session, "text": payload["text"], "delivery": "queue", "resume": false });
    if let Some(files) = payload["files"].as_array() {
        body["files"] = files.iter().map(|f| file_input(f, f.get("mention").cloned())).collect();
    }
    for key in ["agents", "skills", "metadata"] {
        if !payload[key].is_null() {
            body[key] = payload[key].clone();
        }
    }
    body
}

fn file_input(file: &Value, mention: Option<Value>) -> Value {
    let mut v = json!({ "uri": stored_uri(file), "name": file["name"] });
    if !file["description"].is_null() {
        v["description"] = file["description"].clone();
    }
    if let Some(m) = mention.filter(|m| !m.is_null()) {
        v["mention"] = m;
    }
    v
}

/// A stored file re-admits by the URI it came from, so a `file://` reference keeps its
/// provenance instead of turning into an inline snapshot (`storedFileUri`).
fn stored_uri(file: &Value) -> String {
    if file["source"]["type"] == "uri"
        && let Some(uri) = file["source"]["uri"].as_str()
    {
        return uri.to_owned();
    }
    if let Some(uri) = file["uri"].as_str() {
        return uri.to_owned();
    }
    format!("data:{};base64,{}", file["mime"].as_str().unwrap_or_default(), file["data"].as_str().unwrap_or_default())
}

/// Whether a stored file is one the composer added itself (`isComposerAttachment`): those come
/// back to the composer when the prompt is edited; mentions and context files stay in the payload.
pub fn is_composer_file(file: &Value) -> bool {
    file.get("mention").is_none_or(Value::is_null) && (file["source"].is_null() || file["source"]["type"] == "inline")
}

/// The text the queue shows and an edit loads (`queuedPromptText`).
pub fn queued_text(payload: &Value) -> String {
    match payload["metadata"]["displayText"].as_str().filter(|t| !t.is_empty()) {
        Some(t) => t.to_owned(),
        None => payload["text"].as_str().unwrap_or_default().to_owned(),
    }
}

/// The admission that replaces a queued prompt with what the composer holds (`editedPromptInput`,
/// `queue.ts:524-608`). Removed attachments drop and added ones join; stored mentions and context
/// files the composer can't show stay while their mention text does, and the comment notes
/// appended to the original's model-visible text survive.
pub fn edited_admission(session: &str, payload: &Value, prompt: &Prompt) -> Value {
    let request = prompt.body();
    let text = prompt.display_text.clone().unwrap_or_else(|| prompt.text.clone());
    let display = queued_text(payload);
    let original = payload["text"].as_str().unwrap_or_default();
    let notes = if !display.is_empty() && original.starts_with(&display) { &original[display.len()..] } else { "" };
    let mention = |m: &Value| -> Value {
        let Some(t) = m["text"].as_str() else { return Value::Null };
        match text.find(t) {
            Some(at) => {
                let start = text[..at].encode_utf16().count();
                json!({ "text": t, "start": start, "end": start + t.encode_utf16().count() })
            }
            None => Value::Null,
        }
    };
    let mentioned = |v: &Value| v["mention"]["text"].as_str().is_none_or(|t| text.contains(t));
    let empty = Vec::new();
    let listed = |v: &Value, key: &str| -> Vec<Value> { v[key].as_array().unwrap_or(&empty).clone() };
    let merge = |key: &str, same: &dyn Fn(&Value, &Value) -> bool, out: &dyn Fn(&Value) -> Value| -> Vec<Value> {
        let typed = listed(&request, key);
        listed(payload, key)
            .iter()
            .filter(|o| mentioned(o) && !typed.iter().any(|t| same(o, t)))
            .chain(typed.iter())
            .map(out)
            .collect()
    };
    let agents = merge("agents", &|a, b| a["name"] == b["name"], &|a| json!({ "name": a["name"], "mention": mention(&a["mention"]) }));
    let skills = merge("skills", &|a, b| a["id"] == b["id"], &|s| json!({ "id": s["id"], "mention": mention(&s["mention"]) }));
    let typed = listed(&request, "files");
    let files: Vec<Value> = listed(payload, "files")
        .iter()
        .filter(|f| !is_composer_file(f) && mentioned(f) && !typed.iter().any(|t| t["uri"] == stored_uri(f).as_str()))
        .map(|f| file_input(f, Some(mention(&f["mention"]))))
        .chain(typed.iter().map(|f| {
            let mut v = json!({ "uri": f["uri"], "name": f["name"] });
            if !f["mention"].is_null() {
                v["mention"] = f["mention"].clone();
            }
            v
        }))
        .collect();
    let mut metadata = payload["metadata"].as_object().cloned().unwrap_or_else(Map::new);
    // Presentation metadata reads only with a comments list; when the editor showed the notes as
    // text, the edit owns them and the comments go.
    metadata.insert("comments".into(), if notes.is_empty() { json!([]) } else { payload["metadata"]["comments"].clone() });
    metadata.insert("displayText".into(), text.clone().into());
    metadata.insert("attachments".into(), prompt.attachments.iter().map(|a| json!({ "name": a.name, "mime": a.mime, "path": a.path })).collect());
    json!({
        "sessionID": session,
        "text": format!("{}{notes}", request["text"].as_str().unwrap_or_default()),
        "files": files,
        "agents": agents,
        "skills": skills,
        "metadata": metadata,
    })
}
