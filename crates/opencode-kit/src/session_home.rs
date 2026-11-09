//! Sending to a session whose folder is gone, as a removed worktree leaves it.
//!
//! The server answers a prompt with 404 both when the session doesn't exist and when its folder
//! doesn't: it opens the session's location before admitting anything. So only a 404 from GET
//! means the session is gone. One whose folder is gone moves to its project's main checkout
//! (`POST /api/session/:id/move`) and then hears the prompt, with a first line saying so.

use serde_json::{Value, json};

use crate::client::{Prompt, Server};

impl Server {
    /// Moves `session` to `directory` (`POST /api/session/:id/move`), waiting up to 30 s
    /// (Desktop's `WORKSPACE_MOVE_TIMEOUT_MS`).
    pub fn move_session(&self, session: &str, directory: &str) -> Result<(), String> {
        self.write_within("POST", &format!("/api/session/{session}/move"), Some(json!({ "directory": directory })), std::time::Duration::from_secs(30)).map(|_| ())
    }

    /// [`Server::send`], and when the prompt 404s because the session's folder is gone, moves the
    /// session to its project's main checkout and sends again, the prompt led by a line telling
    /// the agent where it is now. Every error keeps `send`'s `POST …/prompt:` start.
    pub fn send_moving_home(&self, session: &str, prompt: &Prompt) -> Result<Option<String>, String> {
        let first = match self.send(session, prompt) {
            Err(e) if e.contains("404") => e,
            done => return done,
        };
        let info = match self.get(&format!("/api/session/{session}")) {
            Ok(v) => v,
            Err(e) if e.contains("404") => return Err(format!("POST /api/session/{session}/prompt: {GONE}")),
            Err(_) => return Err(first),
        };
        let info = info.get("data").unwrap_or(&info);
        let was = info["location"]["directory"].as_str().unwrap_or_default();
        let Some(home) = self.project_home(info["projectID"].as_str().unwrap_or_default()).filter(|h| h != was) else {
            return Err(first);
        };
        self.move_session(session, &home).map_err(|e| format!("{first}; moving it to {home}: {e}"))?;
        let mut moved = prompt.clone();
        moved.text = format!("{}\n\n{}", moved_note(was, &home), prompt.text);
        self.send(session, &moved)
    }

    /// A project's main checkout (`canonical` in `/api/project`).
    fn project_home(&self, project: &str) -> Option<String> {
        let list = self.get("/api/project").ok()?;
        let list: &Value = list.get("data").unwrap_or(&list);
        list.as_array()?.iter().find(|p| p["id"] == project).and_then(|p| p["canonical"].as_str()).map(str::to_owned)
    }
}

/// What a send says when the session itself is gone.
pub const GONE: &str = "the session no longer exists on the server";

/// The line a moved session's prompt starts with.
pub fn moved_note(was: &str, home: &str) -> String {
    format!("Your folder {was} was removed, so this session moved to {home} to hear this. Make a new worktree before you change anything.")
}

#[cfg(test)]
#[path = "session_home_tests.rs"]
mod tests;
