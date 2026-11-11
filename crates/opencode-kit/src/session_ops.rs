//! Session revert and fork operations on [`Server`] (`packages/app/src/session/{revert,fork}.ts`,
//! `packages/app/src/composer/submit.ts`).

use serde_json::Value;

use crate::client::{Prompt, Server};
use crate::model::{self, Delivery, SessionInfo};

impl Server {
    /// Marks `session` read up to `idle` (`POST /api/session/:id/view`, Desktop's `session.view`).
    pub fn view(&self, session: &str, idle: i64) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/view"), Some(serde_json::json!({ "idle": idle }))).map(|_| ())
    }

    /// Replaces `session`'s metadata (`PATCH /api/session/:id` with `{ metadata }`).
    pub fn set_metadata(&self, session: &str, metadata: Value) -> Result<(), String> {
        self.write("PATCH", &format!("/api/session/{session}"), Some(serde_json::json!({ "metadata": metadata }))).map(|_| ())
    }

    /// Archives `session` (`PATCH /api/session/:id` with `{ archived: true }`, Desktop's `session.update`).
    pub fn archive(&self, session: &str) -> Result<(), String> {
        self.write("PATCH", &format!("/api/session/{session}"), Some(serde_json::json!({ "archived": true }))).map(|_| ())
    }

    /// Brings `session` back from the archive (`PATCH /api/session/:id` with `{ archived: false }`).
    pub fn unarchive(&self, session: &str) -> Result<(), String> {
        self.write("PATCH", &format!("/api/session/{session}"), Some(serde_json::json!({ "archived": false }))).map(|_| ())
    }

    /// Waits for `session`'s active run to settle (`POST /api/experimental/session/:id/wait`).
    pub fn wait(&self, session: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/experimental/session/{session}/wait"), None).map(|_| ())
    }

    /// Stages a revert at `message` (`POST /api/session/:id/revert/stage`).
    pub fn revert_stage(&self, session: &str, message: &str) -> Result<Value, String> {
        self.write("POST", &format!("/api/session/{session}/revert/stage"), Some(serde_json::json!({ "messageID": message })))
    }

    /// Desktop's rewind sequence: `interrupt`, `wait`, then `revert/stage` at `message`.
    pub fn revert_to(&self, session: &str, message: &str) -> Result<Value, String> {
        self.interrupt(session)?;
        self.wait(session)?;
        self.revert_stage(session, message)
    }

    /// Clears a staged revert (`DELETE /api/session/:id/revert`), restoring reverted messages.
    pub fn revert_clear(&self, session: &str) -> Result<(), String> {
        self.write("DELETE", &format!("/api/session/{session}/revert"), None).map(|_| ())
    }

    /// Commits a staged revert (`POST /api/session/:id/revert/commit`).
    pub fn revert_commit(&self, session: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/revert/commit"), None).map(|_| ())
    }

    /// Sends `prompt` from a staged revert (`composer/submit.ts`): switches agent, commits the
    /// staged revert, switches model, then sends the prompt or command.
    pub fn send_from_revert(&self, session: &str, prompt: &Prompt) -> Result<Option<String>, String> {
        if prompt.delivery != Some(Delivery::Queue)
            && let Some(agent) = &prompt.agent
        {
            self.switch_agent(session, agent)?;
        }
        self.revert_commit(session)?;
        if prompt.delivery != Some(Delivery::Queue)
            && let Some(m) = &prompt.model
        {
            self.switch_model(session, m)?;
        }
        if prompt.command.is_some() {
            return self.write("POST", &format!("/api/session/{session}/command"), Some(prompt.command_body())).map(|_| None);
        }
        let prompt = self.with_mentions(None, session, prompt);
        let v = self.write_bytes(&format!("/api/session/{session}/prompt"), prompt.body_bytes())?;
        Ok(v["data"]["id"].as_str().map(str::to_owned))
    }

    /// Forks `session` before `before` (`POST /api/session/:id/fork`).
    pub fn fork_session(&self, session: &str, before: Option<&str>) -> Result<SessionInfo, String> {
        let body = match before {
            Some(id) => serde_json::json!({ "before": id }),
            None => serde_json::json!({}),
        };
        let path = format!("/api/session/{session}/fork");
        let v = self.write("POST", &path, Some(body))?;
        let info = model::parse_session(v.get("data").unwrap_or(&v));
        if info.id.is_empty() { Err(format!("POST {path}: no session in the reply")) } else { Ok(info) }
    }
}

