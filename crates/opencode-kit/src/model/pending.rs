//! What a session waits on a person for: permission requests and forms.

use super::*;

/// Something a session is waiting on a person for.
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    /// A tool wants to run: `action` on `resources`. `input` is the tool's arguments as
    /// `key: value` lines, when the request carries them (`metadata.input`, an MCP tool's).
    Permission { id: String, session: String, action: String, resources: Vec<String>, message: String, input: Vec<String> },
    /// A form, such as the question tool's: one field per question.
    Form { id: String, session: String, title: String, fields: Vec<Field> },
}

impl Pending {
    pub fn id(&self) -> &str {
        match self {
            Pending::Permission { id, .. } | Pending::Form { id, .. } => id,
        }
    }
    pub fn session(&self) -> &str {
        match self {
            Pending::Permission { session, .. } | Pending::Form { session, .. } => session,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    /// Free text, or one of `options` (plus free text when `custom`).
    Text,
    Number,
    Boolean,
    /// Any number of `options`.
    Multi,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub key: String,
    pub title: String,
    pub description: String,
    pub kind: FieldKind,
    pub options: Vec<FieldOption>,
    pub custom: bool,
    pub required: bool,
}

/// `permission.asked`'s payload or one entry of `GET /session/:id/permission`.
pub fn parse_permission(v: &Value) -> Option<Pending> {
    let id = s(&v["id"]);
    if id.is_empty() {
        return None;
    }
    Some(Pending::Permission {
        id,
        session: s(&v["sessionID"]),
        action: s(&v["action"]),
        resources: v["resources"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_owned)).collect(),
        message: s(&v["message"]),
        input: v["metadata"]["input"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, item)| format!("{k}: {}", item.as_str().map_or_else(|| item.to_string(), str::to_owned)))
            .collect(),
    })
}

/// `form.created`'s `form` or one entry of `GET /session/:id/form`.
pub fn parse_form(v: &Value) -> Option<Pending> {
    let v = if v["form"].is_object() { &v["form"] } else { v };
    let id = s(&v["id"]);
    if id.is_empty() || v["state"]["status"].as_str().is_some_and(|st| st != "pending") {
        return None;
    }
    let fields = v["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["hidden"].as_bool() != Some(true) && f["type"] != "external")
        .map(|f| Field {
            key: s(&f["key"]),
            title: s(&f["title"]),
            description: s(&f["description"]),
            kind: match f["type"].as_str().unwrap_or("") {
                "boolean" => FieldKind::Boolean,
                "number" | "integer" => FieldKind::Number,
                "multiselect" => FieldKind::Multi,
                _ => FieldKind::Text,
            },
            options: f["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| FieldOption { value: s(&o["value"]), label: s(&o["label"]), description: s(&o["description"]) })
                .collect(),
            custom: f["custom"].as_bool().unwrap_or(false),
            required: f["required"].as_bool().unwrap_or(false),
        })
        .collect();
    Some(Pending::Form { id, session: s(&v["sessionID"]), title: s(&v["title"]), fields })
}
