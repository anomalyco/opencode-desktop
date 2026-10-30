//! Session revert and fork operations on [`Server`] (`packages/app/src/session/{revert,fork}.ts`,
//! `packages/app/src/composer/submit.ts`).

use serde_json::Value;

use crate::client::{Prompt, Server};
use crate::model::{self, Delivery, SessionInfo};

impl Server {
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
        let v = self.write("POST", &format!("/api/session/{session}/prompt"), Some(prompt.body()))?;
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::Value;

    use crate::client::{Prompt, Server, mock, mock_once};
    use crate::model;

    fn body_of(req: &str) -> Value {
        serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    #[test]
    fn revert_stages_clears_commits_and_sends_in_desktop_order() {
        let (base, h) = mock(3);
        Server { base, auth: None, media_refs: false }.revert_to("ses_a", "msg_2").unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("POST /api/session/ses_a/interrupt HTTP/1.1"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("POST /api/experimental/session/ses_a/wait HTTP/1.1"), "{}", reqs[1]);
        assert!(reqs[2].starts_with("POST /api/session/ses_a/revert/stage HTTP/1.1"), "{}", reqs[2]);
        assert_eq!(body_of(&reqs[2]), serde_json::json!({ "messageID": "msg_2" }));

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false }.revert_clear("ses_a").unwrap();
        assert!(h.join().unwrap().starts_with("DELETE /api/session/ses_a/revert HTTP/1.1"));

        let (base, h) = mock(4);
        let model = model::ModelRef { id: "claude-sonnet-5-5".into(), provider: "anthropic".into(), variant: "medium".into() };
        let prompt = Prompt { id: Some("msg_3".into()), text: "retry".into(), agent: Some("plan".into()), model: Some(model), ..Default::default() };
        Server { base, auth: None, media_refs: false }.send_from_revert("ses_a", &prompt).unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("POST /api/session/ses_a/agent HTTP/1.1"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("POST /api/session/ses_a/revert/commit HTTP/1.1"), "{}", reqs[1]);
        assert!(reqs[2].starts_with("POST /api/session/ses_a/model HTTP/1.1"), "{}", reqs[2]);
        assert!(reqs[3].starts_with("POST /api/session/ses_a/prompt HTTP/1.1"), "{}", reqs[3]);
    }

    #[test]
    fn fork_session_posts_before_boundary_and_parses_session() {
        use std::io::{Read, Write as IoWrite};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let body = r#"{"data":{"id":"ses_fork_1","projectID":"proj_1","title":"Fix reconnect (fork #1)","agent":"build","location":{"directory":"/Users/demo/code/repo"},"time":{"created":99,"updated":99}}}"#;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}", body.len());
            let _ = s.write_all(head.as_bytes());
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let forked = Server { base, auth: None, media_refs: false }.fork_session("ses_a", Some("msg_2")).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/fork HTTP/1.1"), "{req}");
        assert_eq!(body_of(&req), serde_json::json!({ "before": "msg_2" }));
        assert_eq!(forked.id, "ses_fork_1");
        assert_eq!(forked.title, "Fix reconnect (fork #1)");
        assert_eq!(forked.directory, "/Users/demo/code/repo");
    }
}
