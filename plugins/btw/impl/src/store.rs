//! Each session's chats as the host keeps them (`Store.session(Chats)`): the JSON Desktop's
//! store holds, `{ "chats": [{ id, question, answer?, agent?, model?, created?, duration? }] }`.

use opencode_plugin::Store;
use serde_json::{Value, json};

use crate::machine::{ChatModel, Info, Saved};

const CHATS: Store<String> = Store::new("chats");

pub fn load(session: &str) -> Vec<Saved> {
    let Some(text) = CHATS.get_in(session) else { return Vec::new() };
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    v["chats"].as_array().into_iter().flatten().filter_map(decode).collect()
}

pub fn update(session: &str, f: impl FnOnce(&mut Vec<Saved>)) {
    let mut chats = load(session);
    f(&mut chats);
    CHATS.set_in(session, &json!({ "chats": chats.iter().map(encode).collect::<Vec<_>>() }).to_string());
}

fn decode(v: &Value) -> Option<Saved> {
    let s = |k: &str| v[k].as_str().map(str::to_owned);
    let model = v["model"].as_object().map(|m| ChatModel {
        provider: m.get("providerID").and_then(Value::as_str).unwrap_or_default().to_owned(),
        id: m.get("id").and_then(Value::as_str).unwrap_or_default().to_owned(),
        variant: m.get("variant").and_then(Value::as_str).unwrap_or_default().to_owned(),
    });
    Some(Saved {
        id: s("id")?,
        info: Info { question: s("question")?, created: v["created"].as_i64(), agent: s("agent").unwrap_or_default(), model },
        answer: s("answer"),
        duration: v["duration"].as_i64(),
    })
}

fn encode(c: &Saved) -> Value {
    let mut v = json!({ "id": c.id, "question": c.info.question });
    if let Some(a) = &c.answer {
        v["answer"] = a.clone().into();
    }
    if !c.info.agent.is_empty() {
        v["agent"] = c.info.agent.clone().into();
    }
    if let Some(m) = &c.info.model {
        v["model"] = json!({ "providerID": m.provider, "id": m.id });
        if !m.variant.is_empty() {
            v["model"]["variant"] = m.variant.clone().into();
        }
    }
    if let Some(t) = c.info.created {
        v["created"] = t.into();
    }
    if let Some(d) = c.duration {
        v["duration"] = d.into();
    }
    v
}
