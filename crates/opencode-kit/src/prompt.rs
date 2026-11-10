//! What a prompt carries and how it goes on the wire: `POST …/prompt`'s body, or a command's.

use serde_json::Value;
use serde::{Deserialize, Serialize};

use crate::model::{self, Delivery};

/// A prompt to send. With `id` (see [`message_id`]) the server's user message takes that id,
/// so a local copy shown before the server's arrives is replaced rather than doubled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub id: Option<String>,
    pub text: String,
    pub files: Vec<FileInput>,
    /// How to deliver it while the session is busy; None lets the server steer.
    pub delivery: Option<Delivery>,
    /// `@agent` mentions (`PromptInput.AgentAttachment`).
    pub agents: Vec<AgentInput>,
    /// `@skill` mentions (`PromptInput.SkillAttachment`).
    pub skills: Vec<SkillInput>,
    /// `metadata.displayText`: the text as typed when `text` differs (session mentions go out as
    /// `@session:ses_…` tokens but read as their titles).
    pub display_text: Option<String>,
    /// Files the server reads by path, which reach the model as `Attached file:` lines in
    /// `text` and the message view through `metadata.attachments`.
    pub attachments: Vec<PathAttachment>,
    /// A slash command to run with `text` as its arguments (`POST …/command`) instead of a
    /// prompt.
    pub command: Option<String>,
    /// Text quoted from the conversation and comments on lines of files. `text` already
    /// carries each one's [`PromptComment::prompt_text`](crate::quote::PromptComment::prompt_text),
    /// and `files` a file comment's lines; these go in `metadata.comments`.
    #[serde(alias = "quotes")]
    pub comments: Vec<crate::quote::PromptComment>,
    /// The agent to switch the session to first (`POST …/agent`), as Desktop applies the
    /// composer's choice; None leaves it as it is. Also kept in `metadata.agent`.
    pub agent: Option<String>,
    /// The model and effort (its `variant`, empty for the model's default) to switch the session
    /// to first (`POST …/model`, which takes them together); None leaves them as they are. Also
    /// kept in `metadata.model`.
    pub model: Option<model::ModelRef>,
}

/// A file attached by its path on the server (`PromptAttachmentReference`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathAttachment {
    pub name: String,
    pub mime: String,
    pub path: String,
}

/// One `PromptInput.FileAttachment`: `uri` is a `data:` URL (see [`data_url`]), `file://`, or a
/// `media:<hash>` ref the server sent, which it re-admits from its stored bytes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileInput {
    pub uri: String,
    pub name: String,
    pub mime: Option<String>,
    pub mention: Option<Mention>,
}

/// Where a mention sits in the prompt's text: UTF-16 offsets, as the server's JavaScript
/// counts them, and the mention's text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mention {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentInput {
    pub name: String,
    pub mention: Option<Mention>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillInput {
    pub id: String,
    pub name: String,
    pub mention: Option<Mention>,
}

impl Mention {
    fn json(&self) -> Value {
        serde_json::json!({ "start": self.start, "end": self.end, "text": self.text })
    }
}

impl Prompt {
    pub fn body(&self) -> Value {
        let mut body = self.input();
        if let Some(id) = &self.id {
            body["id"] = id.clone().into();
        }
        let display = self.display_text.as_ref().filter(|d| **d != self.text);
        let mut metadata = serde_json::Map::new();
        if display.is_some() || !self.attachments.is_empty() || !self.comments.is_empty() {
            // Desktop reads `displayText` only beside a `comments` list.
            let attachments: Vec<Value> =
                self.attachments.iter().map(|a| serde_json::json!({ "name": a.name, "mime": a.mime, "path": a.path })).collect();
            let comments: Vec<Value> = self.comments.iter().map(crate::quote::PromptComment::json).collect();
            metadata.insert("displayText".into(), display.unwrap_or(&self.text).clone().into());
            metadata.insert("comments".into(), comments.into());
            metadata.insert("attachments".into(), attachments.into());
        }
        // The choice the prompt was sent with, as Desktop records it: a queued prompt runs with
        // the session's choice when it's delivered, so only here does it say what was meant.
        if let Some(agent) = &self.agent {
            metadata.insert("agent".into(), agent.clone().into());
        }
        if let Some(m) = &self.model {
            let mut v = serde_json::json!({ "providerID": m.provider, "modelID": m.id });
            if !m.variant.is_empty() {
                v["variant"] = m.variant.clone().into();
            }
            metadata.insert("model".into(), v);
        }
        if !metadata.is_empty() {
            body["metadata"] = Value::Object(metadata);
        }
        if let Some(d) = self.delivery {
            body["delivery"] = d.as_str().into();
        }
        body
    }

    /// `POST …/command`'s body: the command's name, with `text` as its arguments.
    pub fn command_body(&self) -> Value {
        let mut body = self.input();
        body["name"] = self.command.clone().unwrap_or_default().into();
        if let Some(d) = self.delivery {
            body["delivery"] = d.as_str().into();
        }
        body
    }

    /// `PromptInput.Prompt`'s fields: text, files, agents and skills.
    fn input(&self) -> Value {
        let mut body = serde_json::json!({ "text": self.text });
        if !self.files.is_empty() {
            body["files"] = self
                .files
                .iter()
                .map(|f| {
                    let mut v = serde_json::json!({ "uri": f.uri, "name": f.name });
                    if let Some(mime) = &f.mime {
                        v["mime"] = mime.clone().into();
                    }
                    if let Some(m) = &f.mention {
                        v["mention"] = m.json();
                    }
                    v
                })
                .collect();
        }
        if !self.agents.is_empty() {
            body["agents"] = self
                .agents
                .iter()
                .map(|a| {
                    let mut v = serde_json::json!({ "name": a.name });
                    if let Some(m) = &a.mention {
                        v["mention"] = m.json();
                    }
                    v
                })
                .collect();
        }
        if !self.skills.is_empty() {
            body["skills"] = self
                .skills
                .iter()
                .map(|s| {
                    let mut v = serde_json::json!({ "id": s.id, "name": s.name });
                    if let Some(m) = &s.mention {
                        v["mention"] = m.json();
                    }
                    v
                })
                .collect();
        }
        body
    }
}
