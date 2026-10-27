//! The OpenCode server connection: GETs on a worker thread, writes (prompts, replies, stop, new
//! session) on another, `/api/event` on a third, all waking the window when data lands.
//!
//! The password comes from `~/.local/share/opencode-studio-migration/password` (or
//! `OPENCODE_SERVER_PASSWORD`) and only ever goes into the Authorization header.

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use rune::Waker;
use serde_json::Value;

use crate::model::{self, Message, Pending, SessionInfo};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";

pub enum Incoming {
    Sessions(Vec<SessionInfo>),
    Info(SessionInfo),
    Messages { session: String, messages: Vec<Message> },
    Event { ty: String, data: Value },
    Status(String),
    /// A session's open permission requests and forms.
    Pending { session: String, items: Vec<Pending> },
    /// A session this app just created (its first prompt is already sent).
    Created(SessionInfo),
}

pub enum Request {
    Session(String),
    Sessions,
    /// One session's info (`time.idle`/`time.viewed` after it finishes or someone views it).
    Info(String),
    Pending(String),
}

pub enum Write {
    Prompt { session: String, text: String },
    Permission { session: String, request: String, decision: String },
    Form { session: String, form: String, answer: Value },
    CancelForm { session: String, form: String },
    Interrupt(String),
    /// A new session in `directory`, then `text` as its first prompt.
    Create { directory: String, text: String },
    /// `Prompt` with its client id and files.
    Send { session: String, prompt: Prompt },
    /// `Create` with a full first prompt.
    Start { directory: String, prompt: Prompt },
}

/// A prompt to send. With `id` (see [`message_id`]) the server's user message takes that id,
/// so a local copy shown before the server's arrives is replaced rather than doubled.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    pub id: Option<String>,
    pub text: String,
    pub files: Vec<FileInput>,
}

/// One `PromptInput.FileAttachment`: `uri` is a `data:` URL (see [`data_url`]) or `file://`.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInput {
    pub uri: String,
    pub name: String,
}

impl Prompt {
    pub fn body(&self) -> Value {
        let mut body = serde_json::json!({ "text": self.text });
        if let Some(id) = &self.id {
            body["id"] = id.clone().into();
        }
        if !self.files.is_empty() {
            body["files"] = self.files.iter().map(|f| serde_json::json!({ "uri": f.uri, "name": f.name })).collect();
        }
        body
    }
}

/// `data:<mime>;base64,<bytes>`.
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

/// A new `SessionMessage.ID` as the server makes them (`Identifier.ascending`): `msg_`, 12 hex
/// digits of `ms * 0x1000 + counter` (its low 48 bits) and 14 random base62 characters. They
/// sort by time, so a message sent now lands after the transcript's others.
pub fn message_id() -> String {
    message_id_at(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64))
}

fn message_id_at(ms: u64) -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::Mutex;
    static LAST: Mutex<(u64, u64)> = Mutex::new((0, 0));
    let counter = {
        let mut last = LAST.lock().unwrap();
        if last.0 != ms {
            *last = (ms, 0);
        }
        last.1 += 1;
        last.1
    };
    let value = ms.wrapping_mul(0x1000).wrapping_add(counter) & 0xffff_ffff_ffff;
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = format!("msg_{value:012x}");
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(value);
    let mut r = h.finish();
    for k in 0..14 {
        if k == 7 {
            h.write_u64(r);
            r = h.finish();
        }
        out.push(CHARS[(r % 62) as usize] as char);
        r /= 62;
    }
    out
}

fn list(v: &Value) -> Vec<Value> {
    v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default()
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    auth: Option<String>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("base", &self.base).finish_non_exhaustive()
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Server {
    pub fn new(base: &str) -> Server {
        let password = std::env::var("OPENCODE_SERVER_PASSWORD").ok().or_else(|| {
            let home = std::env::var("HOME").ok()?;
            std::fs::read_to_string(format!("{home}/.local/share/opencode-studio-migration/password")).ok()
        });
        let auth = password.map(|p| format!("Basic {}", base64(format!("opencode:{}", p.trim()).as_bytes())));
        Server { base: base.trim_end_matches('/').to_owned(), auth }
    }

    fn addr(&self) -> Option<SocketAddr> {
        let host = self.base.strip_prefix("http://")?.split('/').next()?;
        let host = host.replace("localhost", "127.0.0.1");
        host.parse().ok()
    }

    /// A quick TCP check, so a missing server falls back to the fixture without waiting.
    pub fn reachable(&self) -> bool {
        self.addr().is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_millis(250)).is_ok())
    }

    fn agent(timeout: Option<Duration>) -> ureq::Agent {
        ureq::Agent::config_builder().timeout_global(timeout).http_status_as_error(true).build().into()
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        let mut req = Self::agent(Some(Duration::from_secs(15))).get(format!("{}{path}", self.base));
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        let text = resp
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_string()
            .map_err(|e| format!("GET {path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("GET {path}: {e}"))
    }

    /// A write: `method` with an optional JSON body; an empty (204) reply is `Value::Null`.
    pub fn write(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let agent = Self::agent(Some(Duration::from_secs(15)));
        let auth = self.auth.as_deref();
        let resp = match method {
            "DELETE" => {
                let mut req = agent.delete(url);
                if let Some(a) = auth {
                    req = req.header("Authorization", a);
                }
                req.call()
            }
            _ => {
                let mut req = agent.post(url).header("Content-Type", "application/json");
                if let Some(a) = auth {
                    req = req.header("Authorization", a);
                }
                req.send(body.unwrap_or(Value::Object(Default::default())).to_string().as_bytes())
            }
        };
        let mut resp = resp.map_err(|e| format!("{method} {path}: {e}"))?;
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// `POST /session/:id/prompt`. Tests point this at a local mock, never the live server.
    pub fn send_prompt(&self, session: &str, text: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/prompt"), Some(serde_json::json!({ "text": text }))).map(|_| ())
    }

    /// `POST /session/:id/prompt` with an id and files; returns the id the server gave the
    /// message (`data.id`), which is `prompt.id` when that was set.
    pub fn send(&self, session: &str, prompt: &Prompt) -> Result<Option<String>, String> {
        let v = self.write("POST", &format!("/api/session/{session}/prompt"), Some(prompt.body()))?;
        Ok(v["data"]["id"].as_str().map(str::to_owned))
    }

    /// `decision`: `once`, `always` or `reject`.
    pub fn reply_permission(&self, session: &str, request: &str, decision: &str) -> Result<(), String> {
        let path = format!("/api/session/{session}/permission/{request}/reply");
        self.write("POST", &path, Some(serde_json::json!({ "decision": decision }))).map(|_| ())
    }

    /// `answer` maps each field key to a string, number, bool or list of strings.
    pub fn reply_form(&self, session: &str, form: &str, answer: Value) -> Result<(), String> {
        let path = format!("/api/session/{session}/form/{form}/reply");
        self.write("POST", &path, Some(serde_json::json!({ "answer": answer }))).map(|_| ())
    }

    pub fn cancel_form(&self, session: &str, form: &str) -> Result<(), String> {
        self.write("DELETE", &format!("/api/session/{session}/form/{form}"), None).map(|_| ())
    }

    /// Stops the session's current run.
    pub fn interrupt(&self, session: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/interrupt"), None).map(|_| ())
    }

    pub fn create_session(&self, directory: &str) -> Result<SessionInfo, String> {
        let v = self.write("POST", "/api/session", Some(serde_json::json!({ "location": { "directory": directory } })))?;
        let info = model::parse_session(v.get("data").unwrap_or(&v));
        if info.id.is_empty() { Err("POST /api/session: no session in the reply".into()) } else { Ok(info) }
    }

    /// The session's open permission requests and forms.
    pub fn pending(&self, session: &str) -> Vec<Pending> {
        let mut out = Vec::new();
        if let Ok(v) = self.get(&format!("/api/session/{session}/permission")) {
            out.extend(list(&v).iter().filter_map(model::parse_permission));
        }
        if let Ok(v) = self.get(&format!("/api/session/{session}/form")) {
            out.extend(list(&v).iter().filter_map(model::parse_form));
        }
        out
    }

    fn events(&self, inbox: &Sender<Incoming>, wake: &Waker) -> Result<(), String> {
        let mut req = Self::agent(None).get(format!("{}/api/event", self.base)).header("Accept", "text/event-stream");
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let resp = req.call().map_err(|e| format!("events: {e}"))?;
        let reader = BufReader::new(resp.into_body().into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| format!("events: {e}"))?;
            let Some(data) = line.strip_prefix("data:") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            let ty = v["type"].as_str().unwrap_or("").to_owned();
            if ty.is_empty() || ty == "server.heartbeat" {
                continue;
            }
            if inbox.send(Incoming::Event { ty, data: v["data"].clone() }).is_err() {
                return Ok(());
            }
            wake();
        }
        Err("events: stream ended".into())
    }
}

/// Starts the worker, writer and event threads (the first loads already happened). Returns the
/// request and write senders and the inbox.
pub fn connect(server: Server, wake: Waker) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    let (inbox_tx, inbox) = channel();
    let (req_tx, req_rx) = channel::<Request>();
    let (write_tx, write_rx) = channel::<Write>();

    {
        let server = server.clone();
        let inbox_tx = inbox_tx.clone();
        let wake = wake.clone();
        std::thread::Builder::new()
            .name("opencode-write".into())
            .spawn(move || {
                for w in write_rx {
                    let result = match w {
                        Write::Prompt { session, text } => server.send_prompt(&session, &text),
                        Write::Permission { session, request, decision } => server.reply_permission(&session, &request, &decision),
                        Write::Form { session, form, answer } => server.reply_form(&session, &form, answer),
                        Write::CancelForm { session, form } => server.cancel_form(&session, &form),
                        Write::Interrupt(session) => server.interrupt(&session),
                        Write::Create { directory, text } => server.create_session(&directory).and_then(|info| {
                            server.send_prompt(&info.id, &text)?;
                            let _ = inbox_tx.send(Incoming::Created(info));
                            Ok(())
                        }),
                        Write::Send { session, prompt } => server.send(&session, &prompt).map(|_| ()),
                        Write::Start { directory, prompt } => server.create_session(&directory).and_then(|info| {
                            server.send(&info.id, &prompt)?;
                            let _ = inbox_tx.send(Incoming::Created(info));
                            Ok(())
                        }),
                    };
                    if let Err(e) = result
                        && inbox_tx.send(Incoming::Status(e)).is_err()
                    {
                        return;
                    }
                    wake();
                }
            })
            .expect("spawn write thread");
    }

    {
        let server = server.clone();
        let inbox_tx = inbox_tx.clone();
        let wake = wake.clone();
        std::thread::Builder::new()
            .name("opencode-fetch".into())
            .spawn(move || {
                let send = |m: Incoming| {
                    let ok = inbox_tx.send(m).is_ok();
                    wake();
                    ok
                };
                let sessions = |server: &Server| server.get("/api/session?limit=120").map(|v| model::parse_sessions(&v));
                for r in req_rx {
                    let ok = match r {
                        Request::Session(id) => match server.get(&format!("/api/session/{id}/message?limit=80&order=desc")) {
                            Ok(v) => send(Incoming::Messages { session: id, messages: model::parse_messages(&v) }),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Sessions => match sessions(&server) {
                            Ok(list) => send(Incoming::Sessions(list)),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Info(id) => match server.get(&format!("/api/session/{id}")) {
                            Ok(v) => send(Incoming::Info(model::parse_session(v.get("data").unwrap_or(&v)))),
                            Err(e) => send(Incoming::Status(e)),
                        },
                        Request::Pending(id) => send(Incoming::Pending { items: server.pending(&id), session: id }),
                    };
                    if !ok {
                        return;
                    }
                    // Gentle: at most a few requests a second.
                    std::thread::sleep(Duration::from_millis(250));
                }
            })
            .expect("spawn fetch thread");
    }

    std::thread::Builder::new()
        .name("opencode-events".into())
        .spawn(move || {
            let mut backoff = 2;
            loop {
                if let Err(e) = server.events(&inbox_tx, &wake) {
                    if inbox_tx.send(Incoming::Status(e)).is_err() {
                        return;
                    }
                    wake();
                }
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(30);
            }
        })
        .expect("spawn event thread");

    (req_tx, write_tx, inbox)
}

/// A one-request HTTP server for tests: answers 200 and returns what it received.
#[cfg(test)]
pub fn mock_once() -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().expect("accept");
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = s.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if let Some(h) = text.find("\r\n\r\n") {
                let len = text[..h]
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0usize);
                if buf.len() >= h + 4 + len {
                    break;
                }
            }
        }
        let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{}");
        String::from_utf8_lossy(&buf).into_owned()
    });
    (base, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b"opencode:pw"), "b3BlbmNvZGU6cHc=");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn send_posts_prompt_to_mock() {
        let (base, h) = mock_once();
        let server = Server { base, auth: Some("Basic dGVzdA==".into()) };
        server.send_prompt("ses_test", "hello there").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_test/prompt HTTP/1.1"), "{req}");
        assert!(req.contains("authorization: Basic dGVzdA==") || req.contains("Authorization: Basic dGVzdA=="));
        assert!(req.ends_with(r#"{"text":"hello there"}"#), "{req}");
    }

    #[test]
    fn replies_to_a_permission_and_a_form() {
        let (base, h) = mock_once();
        Server { base, auth: None }.reply_permission("ses_a", "per_b", "once").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/permission/per_b/reply HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"decision":"once"}"#), "{req}");

        let (base, h) = mock_once();
        Server { base, auth: None }.reply_form("ses_a", "frm_c", serde_json::json!({"q0": "dev"})).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/form/frm_c/reply HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"answer":{"q0":"dev"}}"#), "{req}");
    }

    #[test]
    fn message_ids_match_the_servers_and_sort_by_time() {
        let a = message_id_at(1_793_064_618_018);
        let b = message_id_at(1_793_064_618_018);
        let c = message_id_at(1_793_064_618_019);
        for id in [&a, &b, &c] {
            assert_eq!(id.len(), 4 + 26, "{id}");
            assert!(id.starts_with("msg_") && id[4..16].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "{id}");
            assert!(id[16..].chars().all(|c| c.is_ascii_alphanumeric()), "{id}");
        }
        // The server's id for a message created at that millisecond starts the same way.
        assert_eq!(&a[..16], "msg_17afac822001");
        assert!(a < b && b < c, "{a} {b} {c}");
    }

    #[test]
    fn sends_a_prompt_with_its_id_and_an_image() {
        let (base, h) = mock_once();
        let prompt = Prompt {
            id: Some("msg_17afac822001abcdefghijklmn".into()),
            text: "see this".into(),
            files: vec![FileInput { uri: data_url("image/png", b"ab"), name: "shot.png".into() }],
        };
        Server { base, auth: None }.send("ses_a", &prompt).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/prompt HTTP/1.1"), "{req}");
        let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"id": "msg_17afac822001abcdefghijklmn", "text": "see this",
                "files": [{"uri": "data:image/png;base64,YWI=", "name": "shot.png"}]})
        );
        assert_eq!(Prompt { text: "hi".into(), ..Default::default() }.body(), serde_json::json!({"text": "hi"}));
    }

    #[test]
    fn interrupts() {
        let (base, h) = mock_once();
        Server { base, auth: None }.interrupt("ses_a").unwrap();
        assert!(h.join().unwrap().starts_with("POST /api/session/ses_a/interrupt HTTP/1.1"));
    }
}
