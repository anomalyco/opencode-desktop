//! Agent widgets' endpoints (`specs/v2/widgets.md`, the `experimental.widget.*` routes): a
//! template version's program, and an open widget's answer and messages.

use serde_json::Value;

use super::{Server, encode_component};
use crate::wire_error::{WireError, WireKind};

impl Server {
    /// Widget template `name` at `version`'s runnable program (`experimental.widget.template.get`):
    /// a version never changes, so callers cache it by `name@version` for good.
    pub fn widget_program(&self, name: &str, version: &str) -> Result<String, WireError> {
        let path = format!("/api/experimental/widget/template/{}/{}", encode_component(name), encode_component(version));
        let v = self.get(&path)?;
        v["data"]["program"].as_str().map(str::to_owned).ok_or_else(|| WireError::new("GET", &path, WireKind::Shape("no program in the reply".into())))
    }

    /// Answers open widget `message` (`experimental.session.widget.respond`): `{ value }` or
    /// `{ dismiss: true }`. A 409 means it was already answered.
    pub fn respond_widget(&self, session: &str, message: &str, body: Value) -> Result<(), WireError> {
        self.write("POST", &format!("/api/experimental/session/{session}/widget/{message}/respond"), Some(body)).map(|_| ())
    }

    /// Sends a non-final message from open widget `message` to the agent
    /// (`experimental.session.widget.message`): `{ value, wake }`. A 409 means it isn't open.
    pub fn send_widget_message(&self, session: &str, message: &str, body: Value) -> Result<(), WireError> {
        self.write("POST", &format!("/api/experimental/session/{session}/widget/{message}/message"), Some(body)).map(|_| ())
    }
}
