//! The OpenCode server connection: GETs on a worker thread, writes (prompts, replies, stop, new
//! session) on another, `/api/event` on a third, all waking the window when data lands.
//!
//! The password comes from `OPENCODE_SERVER_PASSWORD`, the local OpenCode `service.json`,
//! or a connect link (`Server::with_password`, pairing.rs), and only ever goes into the
//! Authorization header.

use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use rune::Waker;
use serde_json::Value;

use crate::model::{self, Delivery, Pending, SessionInfo};

mod downloads;
mod ids;
mod wire;
mod worker;

pub use downloads::{export_filename, save_download, set_download_dir};
pub use ids::{message_id, session_id};
#[cfg(test)]
use ids::{id_at, message_id_at};
pub use wire::{Incoming, Request, Write};
pub use worker::{connect, connect_with_idle};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:4096";

/// The server to use when no `--server` was passed: `OPENCODE_SERVER_URL` when set (a Mac that
/// reaches another Mac's server through a tunnel sets it with `launchctl setenv`, so opening the
/// app from Spotlight or a launcher still finds it), else [`DEFAULT_SERVER`].
pub fn default_server() -> String {
    server_or_default(std::env::var("OPENCODE_SERVER_URL").ok())
}

#[cfg(test)]
#[path = "interrupt_tests.rs"]
mod interrupt_tests;

fn server_or_default(env: Option<String>) -> String {
    env.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()).unwrap_or_else(|| DEFAULT_SERVER.to_owned())
}

/// Sent as `X-OpenCode-Capabilities` on message and inbox reads by a [`Server`] built with
/// [`Server::with_media_refs`]: the server may answer with `media:<hash>` refs (see
/// [`model::Attachment`]) instead of inline bytes. Servers without it ignore the header.
pub const MEDIA_CAPABILITY: &str = "experimental-media-uri";

pub use crate::prompt::{AgentInput, FileInput, Mention, PathAttachment, Prompt, SkillInput};

/// `encodeURIComponent`, for a query value.
pub(crate) fn encode_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `data:<mime>;base64,<bytes>`.
pub fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64(bytes))
}

fn list(v: &Value) -> Vec<Value> {
    v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default()
}

/// When a read's response started (headers in), its body's size, and when JSON parsing began.
#[derive(Clone, Copy, Debug)]
pub struct ReadTiming {
    pub first_byte: Instant,
    pub bytes: usize,
    pub parsing: Instant,
}

#[derive(Clone)]
pub struct Server {
    pub base: String,
    pub(crate) auth: Option<String>,
    pub(crate) media_refs: bool,
    /// Its files aren't this machine's even on a loopback address (a WSL distro's server, an SSH
    /// tunnel), as Desktop treats a contributed server.
    pub(crate) remote_files: bool,
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
        let clean = base.trim_end_matches('/').to_owned();
        let password = std::env::var("OPENCODE_SERVER_PASSWORD")
            .ok()
            .or_else(|| crate::local_service::password_for_base(&clean));
        let auth = password
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .map(|p| format!("Basic {}", base64(format!("opencode:{p}").as_bytes())));
        Server { base: clean, auth, media_refs: false, remote_files: false }
    }

    /// Uses `password` (a token from a connect link, say) in place of the one `new` found.
    pub fn with_password(mut self, password: &str) -> Server {
        self.auth = Some(format!("Basic {}", base64(format!("opencode:{}", password.trim()).as_bytes())));
        self
    }

    /// Marks its files as another machine's though it answers on a loopback address.
    pub fn with_remote_files(mut self) -> Server {
        self.remote_files = true;
        self
    }

    /// Opts `connect`'s message and inbox reads into `media:` refs ([`MEDIA_CAPABILITY`]). Off by
    /// default: only an app that fetches refs (`Server::media`) should ask for them.
    pub fn with_media_refs(mut self) -> Server {
        self.media_refs = true;
        self
    }

    /// A read `connect` makes: with the capability header when opted in.
    pub fn read(&self, path: &str) -> Result<Value, String> {
        self.get_with(path, self.media_refs.then_some(MEDIA_CAPABILITY))
    }

    fn addr(&self) -> Option<SocketAddr> {
        let (scheme, rest) = self.base.split_once("://")?;
        let host = rest.split('/').next()?.replace("localhost", "127.0.0.1");
        if let Ok(a) = host.parse() {
            return Some(a);
        }
        // A name, as a remote server behind a tunnel has.
        let port = if scheme == "https" { 443 } else { 80 };
        let host = if host.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) { host } else { format!("{host}:{port}") };
        std::net::ToSocketAddrs::to_socket_addrs(&host).ok()?.next()
    }

    /// A quick TCP check, so a missing server falls back to the fixture without waiting.
    pub fn reachable(&self) -> bool {
        self.addr().is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_millis(250)).is_ok())
    }

    /// The server shares this machine's filesystem, so a local path means the same file there.
    pub fn is_local(&self) -> bool {
        if self.remote_files {
            return false;
        }
        let host = self.base.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
        let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
        matches!(host, "127.0.0.1" | "localhost" | "[::1]")
    }

    pub(crate) fn agent(timeout: Option<Duration>) -> ureq::Agent {
        ureq::Agent::config_builder().timeout_global(timeout).http_status_as_error(true).build().into()
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.get_with(path, None)
    }

    fn get_with(&self, path: &str, capabilities: Option<&str>) -> Result<Value, String> {
        self.get_timed(path, capabilities).map(|(v, _)| v)
    }

    /// `GET path`'s body as the server wrote it, unparsed (its keys in the server's order).
    pub fn get_text(&self, path: &str) -> Result<String, String> {
        let mut req = Self::agent(Some(Duration::from_secs(15))).get(format!("{}{path}", self.base));
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        resp.body_mut().with_config().limit(64 << 20).read_to_string().map_err(|e| format!("GET {path}: {e}"))
    }

    /// `read`, with when its response started, its body size and when parsing started.
    pub fn read_timed(&self, path: &str) -> Result<(Value, ReadTiming), String> {
        self.get_timed(path, self.media_refs.then_some(MEDIA_CAPABILITY))
    }

    fn get_timed(&self, path: &str, capabilities: Option<&str>) -> Result<(Value, ReadTiming), String> {
        let mut req = Self::agent(Some(Duration::from_secs(15))).get(format!("{}{path}", self.base));
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        if let Some(c) = capabilities {
            req = req.header("X-OpenCode-Capabilities", c);
        }
        let mut resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        let first_byte = Instant::now();
        let text = resp
            .body_mut()
            .with_config()
            .limit(64 << 20)
            .read_to_string()
            .map_err(|e| format!("GET {path}: {e}"))?;
        let parsing = Instant::now();
        let value = serde_json::from_str(&text).map_err(|e| format!("GET {path}: {e}"))?;
        Ok((value, ReadTiming { first_byte, bytes: text.len(), parsing }))
    }

    /// The bytes behind a `media:<hash>` ref (`GET /api/experimental/media/:hash`). They never
    /// change, so callers cache them by hash; a 404 means nothing references them anymore.
    pub fn media(&self, hash: &str) -> Result<Vec<u8>, String> {
        self.get_bytes(&format!("/api/experimental/media/{hash}"))
    }

    /// Raw bytes from `path` (such as `/api/fs/read/<path>?location[directory]=…`).
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut req = Self::agent(Some(Duration::from_secs(60))).get(format!("{}{path}", self.base));
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.call().map_err(|e| format!("GET {path}: {e}"))?;
        resp.body_mut().with_config().limit(64 << 20).read_to_vec().map_err(|e| format!("GET {path}: {e}"))
    }

    /// A write: `method` with an optional JSON body; an empty (204) reply is `Value::Null`.
    pub fn write(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
        self.write_within(method, path, body, Duration::from_secs(15))
    }

    /// `write`, waiting up to `timeout` for the reply.
    fn write_within(&self, method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<Value, String> {
        let url = format!("{}{path}", self.base);
        let agent = Self::agent(Some(timeout));
        let auth = self.auth.as_deref();
        let resp = match method {
            "DELETE" => {
                let mut req = agent.delete(url);
                if let Some(a) = auth {
                    req = req.header("Authorization", a);
                }
                req.call()
            }
            "PATCH" => {
                let mut req = agent.patch(url).header("Content-Type", "application/json");
                if let Some(a) = auth {
                    req = req.header("Authorization", a);
                }
                req.send(body.unwrap_or(Value::Object(Default::default())).to_string().as_bytes())
            }
            "PUT" => {
                let mut req = agent.put(url).header("Content-Type", "application/json");
                if let Some(a) = auth {
                    req = req.header("Authorization", a);
                }
                req.send(body.unwrap_or(Value::Object(Default::default())).to_string().as_bytes())
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
        self.apply_choice(session, prompt)?;
        if prompt.command.is_some() {
            return self.write("POST", &format!("/api/session/{session}/command"), Some(prompt.command_body())).map(|_| None);
        }
        let v = self.write("POST", &format!("/api/session/{session}/prompt"), Some(prompt.body()))?;
        Ok(v["data"]["id"].as_str().map(str::to_owned))
    }

    /// Desktop's `applySelection`: a prompt that runs now (a steer, or any prompt to an idle
    /// session) switches the session's agent, then its model, before it's admitted. A queued one
    /// mustn't reconfigure the turn it waits behind, so it only records its choice.
    fn apply_choice(&self, session: &str, prompt: &Prompt) -> Result<(), String> {
        if prompt.delivery == Some(Delivery::Queue) {
            return Ok(());
        }
        if let Some(agent) = &prompt.agent {
            self.switch_agent(session, agent)?;
        }
        if let Some(m) = &prompt.model {
            self.switch_model(session, m)?;
        }
        Ok(())
    }

    /// `session.switchAgent`: `{"agent": …}`.
    pub fn switch_agent(&self, session: &str, agent: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/agent"), Some(serde_json::json!({ "agent": agent }))).map(|_| ())
    }

    /// `session.switchModel`: `{"model": {id, providerID, variant?}}`, no variant for the model's
    /// default.
    pub fn switch_model(&self, session: &str, m: &model::ModelRef) -> Result<(), String> {
        let mut v = serde_json::json!({ "id": m.id, "providerID": m.provider });
        if !m.variant.is_empty() {
            v["variant"] = m.variant.clone().into();
        }
        self.write("POST", &format!("/api/session/{session}/model"), Some(serde_json::json!({ "model": v }))).map(|_| ())
    }

    /// Runs `command` in the session's directory; the server reports it as `shell.*` events.
    pub fn shell(&self, session: &str, id: Option<&str>, command: &str) -> Result<(), String> {
        let mut body = serde_json::json!({ "command": command });
        if let Some(id) = id {
            body["id"] = id.into();
        }
        self.write("POST", &format!("/api/session/{session}/shell"), Some(body)).map(|_| ())
    }

    /// Copies `bytes` into the server's temporary directory as `name` (one folder per upload,
    /// as Desktop does) and returns the path there.
    pub fn upload(&self, directory: &str, name: &str, bytes: &[u8]) -> Result<String, String> {
        let info = self.get("/api/info")?;
        let tmp = info["paths"]["tmp"].as_str().ok_or("GET /api/info: no paths.tmp")?;
        let target = format!("{}/uploads/{}/{name}", tmp.trim_end_matches('/'), message_id());
        let path = format!(
            "/api/experimental/fs/write?location%5Bdirectory%5D={}&path={}",
            encode_component(directory),
            encode_component(&target)
        );
        let mut req = Self::agent(Some(Duration::from_secs(120))).post(format!("{}{path}", self.base)).header("Content-Type", "application/octet-stream");
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.send(bytes).map_err(|e| format!("upload {name}: {e}"))?;
        let text = resp.body_mut().read_to_string().map_err(|e| format!("upload {name}: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("upload {name}: {e}"))?;
        v["data"]["path"].as_str().map(str::to_owned).ok_or_else(|| format!("upload {name}: no path in the reply"))
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

    /// Like `interrupt`, preserving whether a run was interrupted. `false` means idle or
    /// already stopping; `None` is an older server that doesn't return an acknowledgement.
    pub fn interrupt_ack(&self, session: &str) -> Result<Option<bool>, String> {
        self.write("POST", &format!("/api/session/{session}/interrupt"), None).map(|v| v["interrupted"].as_bool())
    }

    /// `session.background`: moves the session's running shells and subagents that block its run
    /// to the background, so the run goes on to its next step. The server moves each job once
    /// and ignores the call when nothing blocks.
    pub fn background(&self, session: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/background"), None).map(|_| ())
    }

    /// `session.compact`: asks the server to summarize the session now. It admits the compaction
    /// to the session's inbox, steering a running turn as Desktop's does.
    pub fn compact(&self, session: &str) -> Result<(), String> {
        self.write("POST", &format!("/api/session/{session}/compact"), Some(serde_json::json!({}))).map(|_| ())
    }

    pub fn create_session(&self, directory: &str) -> Result<SessionInfo, String> {
        self.create(serde_json::json!({ "location": { "directory": directory } }))
    }

    /// `create_session` under `id`, running `agent` and `model` from the start (Desktop's
    /// `session.create` from the new-session page), so its first prompt needs no switches.
    pub fn create_session_as(&self, id: &str, directory: &str, agent: Option<&str>, model: Option<&model::ModelRef>) -> Result<SessionInfo, String> {
        let mut body = serde_json::json!({ "id": id, "location": { "directory": directory } });
        if let Some(agent) = agent {
            body["agent"] = agent.into();
        }
        if let Some(m) = model {
            body["model"] = serde_json::json!({ "id": m.id, "providerID": m.provider });
            if !m.variant.is_empty() {
                body["model"]["variant"] = m.variant.clone().into();
            }
        }
        self.create(body)
    }

    fn create(&self, body: Value) -> Result<SessionInfo, String> {
        let v = self.write("POST", "/api/session", Some(body))?;
        let info = model::parse_session(v.get("data").unwrap_or(&v));
        if info.id.is_empty() { Err("POST /api/session: no session in the reply".into()) } else { Ok(info) }
    }

    /// `Write::StartAs`: the create and the first prompt. When the prompt fails, or the create
    /// may have arrived, the session is deleted so a failed start leaves nothing behind.
    pub fn start_as(&self, id: &str, directory: &str, prompt: &Prompt) -> Result<SessionInfo, String> {
        let created = self.create_session_as(id, directory, prompt.agent.as_deref(), prompt.model.as_ref());
        // The create already chose the agent and model, so no switches before the prompt.
        let sent = created.as_ref().map_err(String::clone).and_then(|_| self.write("POST", &format!("/api/session/{id}/prompt"), Some(prompt.body())));
        match sent {
            Ok(_) => created,
            Err(e) => {
                if created.is_ok() || !crate::plain_error::failed_for_sure(&e) {
                    let _ = self.delete_session(id);
                }
                Err(e)
            }
        }
    }

    /// `worktree.create`: a new worktree of `project` from the checkout `from`, on `branch` or
    /// the checkout's current one; returns its directory. The server runs the project's setup
    /// script before it answers (often a dependency install), so this waits up to ten minutes.
    pub fn create_worktree(&self, project: &str, from: &str, branch: Option<&str>) -> Result<String, String> {
        let mut body = serde_json::json!({ "projectID": project, "from": from });
        if let Some(b) = branch {
            body["branch"] = b.into();
        }
        let v = self.write_within("POST", "/api/worktree", Some(body), Duration::from_secs(600))?;
        v.get("data").unwrap_or(&v)["directory"].as_str().map(str::to_owned).ok_or_else(|| "POST /api/worktree: no directory in the reply".into())
    }

    /// `StartInNewWorktree`: the worktree, then `start_as` in it.
    pub fn start_in_new_worktree(&self, id: &str, project: &str, from: &str, branch: Option<&str>, prompt: &Prompt, made: impl FnOnce()) -> Result<SessionInfo, String> {
        let directory = self.create_worktree(project, from, branch)?;
        made();
        self.start_as(id, &directory, prompt)
    }

    /// `vcs.branch.list`: up to 50 branches at `directory` whose names hold `search`.
    pub fn branches(&self, directory: &str, search: &str) -> Result<Vec<String>, String> {
        let path = format!("/api/vcs/branch?location%5Bdirectory%5D={}&search={}&limit=50", encode_component(directory), encode_component(search));
        let v = self.get(&path)?;
        Ok(list(&v).iter().filter_map(|b| b.as_str().map(str::to_owned)).collect())
    }

    /// Resolves `directory` (`GET /api/location?location[directory]=…`), registering its project
    /// on the server as Desktop's `projects.add` does.
    pub fn resolve_location(&self, directory: &str) -> Result<model::LocationInfo, String> {
        let path = format!("/api/location?location%5Bdirectory%5D={}", encode_component(directory));
        let v = self.get(&path)?;
        model::parse_location(&v).ok_or_else(|| format!("GET {path}: no location in the reply"))
    }

    /// Lists the server's non-archived projects (`GET /api/project`).
    pub fn projects(&self) -> Result<Vec<model::Project>, String> {
        self.get("/api/project").map(|v| model::parse_projects(&v))
    }

    /// Registers `directory` as a project on the server and returns its resolved location and the
    /// refreshed project list.
    pub fn open_project(&self, directory: &str) -> Result<(model::LocationInfo, Vec<model::Project>), String> {
        let location = self.resolve_location(directory)?;
        let projects = self.projects()?;
        Ok((location, projects))
    }

    /// `session.update` with a title: the body is `{"title": …}` and nothing else.
    pub fn rename_session(&self, session: &str, title: &str) -> Result<(), String> {
        self.write("PATCH", &format!("/api/session/{session}"), Some(serde_json::json!({ "title": title }))).map(|_| ())
    }

    pub fn delete_session(&self, session: &str) -> Result<(), String> {
        self.write("DELETE", &format!("/api/session/{session}"), None).map(|_| ())
    }

    /// What Desktop's Export saves: `{ info, messages }`, the messages oldest first in pages of
    /// 200, with the file name it gives it.
    pub fn export_session(&self, session: &str) -> Result<(String, String), String> {
        let v = self.get(&format!("/api/session/{session}"))?;
        let info = v.get("data").unwrap_or(&v).clone();
        let mut messages = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = format!("/api/session/{session}/message?limit=200&order=asc");
            if let Some(c) = &cursor {
                path.push_str(&format!("&cursor={}", encode_component(c)));
            }
            let page = self.get(&path)?;
            messages.extend(page["data"].as_array().cloned().unwrap_or_default());
            cursor = page["cursor"]["next"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        let name = export_filename(&info, session);
        let json = serde_json::to_string_pretty(&serde_json::json!({ "info": info, "messages": messages })).map_err(|e| e.to_string())?;
        Ok((name, json))
    }

    pub fn cancel_inbox(&self, session: &str, item: &str) -> Result<(), String> {
        self.write("DELETE", &format!("/api/session/{session}/inbox/{item}"), None).map(|_| ())
    }

    pub fn set_delivery(&self, session: &str, item: &str, delivery: Delivery) -> Result<(), String> {
        let body = serde_json::json!({ "delivery": delivery.as_str() });
        self.write("PATCH", &format!("/api/session/{session}/inbox/{item}"), Some(body)).map(|_| ())
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

    /// Reads the event stream until it ends or goes silent for `idle`; `opened` says whether it
    /// got as far as the server's reply.
    fn events(&self, idle: Duration, inbox: &Sender<Incoming>, wake: &Waker, resumed: bool, opened: &mut bool) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_per_read(Some(idle)).http_status_as_error(true).build().into();
        let mut req = agent.get(format!("{}/api/event", self.base)).header("Accept", "text/event-stream");
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let resp = req.call().map_err(|e| format!("events: {e}"))?;
        *opened = true;
        if resumed {
            if inbox.send(Incoming::Reconnected).is_err() {
                return Ok(());
            }
            wake();
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        for line in reader.lines() {
            let line = line.map_err(|e| format!("events: {e}"))?;
            let Some(data) = line.strip_prefix("data:") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            let ty = v["type"].as_str().unwrap_or("").to_owned();
            if ty.is_empty() || ty == "server.heartbeat" {
                continue;
            }
            let mut data = v["data"].clone();
            // A compaction message, and a notice's, takes its id from the event that started it.
            if (ty.starts_with("session.compaction.") || crate::model::is_notice_event(&ty))
                && let Some(id) = v["id"].as_str()
            {
                data["eventID"] = id.into();
            }
            // When a request was asked, for the inbox's waiting age; and a branch change's folder.
            if matches!(ty.as_str(), "permission.asked" | "form.created") {
                data["eventCreated"] = v["created"].clone();
            }
            if ty == "vcs.branch.updated" {
                data["eventLocation"] = v["location"].clone();
            }
            if inbox.send(Incoming::Event { ty, data }).is_err() {
                return Ok(());
            }
            wake();
        }
        Err("events: stream ended".into())
    }
}

/// A one-request HTTP server for tests: answers 200 and returns what it received.
#[cfg(test)]
pub fn mock_once() -> (String, std::thread::JoinHandle<String>) {
    let (base, h) = mock(1);
    (base, std::thread::spawn(move || h.join().unwrap().remove(0)))
}

/// [`mock_once`] for `n` requests in a row, returned in the order they came.
#[cfg(test)]
pub fn mock(n: usize) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    (base, std::thread::spawn(move || (0..n).map(|_| mock_reply(&listener)).collect()))
}

/// [`mock`] answering each request in turn with its status (`"200 OK"`) and JSON body.
#[cfg(test)]
pub fn mock_replies(replies: Vec<(&'static str, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let base = format!("http://{}", listener.local_addr().unwrap());
    (base, std::thread::spawn(move || replies.into_iter().map(|(status, body)| mock_reply_with(&listener, status, &body)).collect()))
}

#[cfg(test)]
fn mock_reply(listener: &std::net::TcpListener) -> String {
    mock_reply_with(listener, "200 OK", "{}")
}

#[cfg(test)]
fn mock_reply_with(listener: &std::net::TcpListener, status: &str, body: &str) -> String {
    use std::io::{Read, Write};
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
    let _ = s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}", body.len()).as_bytes());
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b"opencode:pw"), "b3BlbmNvZGU6cHc=");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn opencode_server_url_overrides_the_default() {
        assert_eq!(server_or_default(None), DEFAULT_SERVER);
        assert_eq!(server_or_default(Some("  ".into())), DEFAULT_SERVER);
        assert_eq!(server_or_default(Some("http://127.0.0.1:14096\n".into())), "http://127.0.0.1:14096");
    }

    #[test]
    fn connect_with_idle_reports_a_stalled_stream() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // A tunnel that accepts and answers, then never sends another byte.
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").unwrap();
            std::thread::sleep(Duration::from_secs(2));
        });
        let start = std::time::Instant::now();
        let (_r, _w, inbox) = connect_with_idle(Server::new(&base), std::sync::Arc::new(|| {}), Duration::from_millis(300));
        let got = std::iter::from_fn(|| inbox.recv_timeout(Duration::from_millis(1500)).ok())
            .find_map(|m| if let Incoming::Status(e) = m { Some(e) } else { None });
        assert!(got.is_some_and(|e| e.starts_with("events:")), "the stalled stream is reported");
        assert!(start.elapsed() < Duration::from_millis(1500));
        h.join().unwrap();
    }

    #[test]
    fn a_silent_event_stream_is_dropped() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        // A stream that sends its reply and one heartbeat, then goes quiet without closing, as a
        // connection that died while the Mac slept does.
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").unwrap();
            s.write_all(b"d\r\n: heartbeat\n\n\r\n").unwrap();
            std::thread::sleep(Duration::from_secs(2));
        });
        let (inbox, _rx) = channel();
        let wake: Waker = std::sync::Arc::new(|| {});
        let mut opened = false;
        let start = std::time::Instant::now();
        let ended = Server { base, auth: None, media_refs: false, remote_files: false }.events(Duration::from_millis(300), &inbox, &wake, false, &mut opened);
        assert!(ended.is_err() && opened, "{ended:?}");
        assert!(start.elapsed() < Duration::from_millis(1500), "it gave up after the idle limit, not when the socket closed");
        h.join().unwrap();
    }

    #[test]
    fn send_posts_prompt_to_mock() {
        let (base, h) = mock_once();
        let server = Server { base, auth: Some("Basic dGVzdA==".into()), media_refs: false, remote_files: false };
        server.send_prompt("ses_test", "hello there").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_test/prompt HTTP/1.1"), "{req}");
        assert!(req.contains("authorization: Basic dGVzdA==") || req.contains("Authorization: Basic dGVzdA=="));
        assert!(req.ends_with(r#"{"text":"hello there"}"#), "{req}");
    }

    #[test]
    fn rename_patches_the_title_alone_and_delete_removes_the_session() {
        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.rename_session("ses_a", "Fix \"the\" jitter").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("PATCH /api/session/ses_a HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"title":"Fix \"the\" jitter"}"#), "{req}");

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.delete_session("ses_a").unwrap();
        assert!(h.join().unwrap().starts_with("DELETE /api/session/ses_a HTTP/1.1"));
    }

    #[test]
    fn background_posts_to_the_session() {
        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.background("ses_a").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/background HTTP/1.1"), "{req}");
    }

    #[test]
    fn compact_posts_to_the_session() {
        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.compact("ses_a").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/compact HTTP/1.1"), "{req}");
    }

    #[test]
    fn a_quiet_background_drops_its_failure_and_a_loud_one_reports_it() {
        let (_, writes, inbox) = connect(Server::new("http://127.0.0.1:9"), std::sync::Arc::new(|| {}));
        writes.send(Write::Background { session: "ses_quiet".into(), quiet: true }).unwrap();
        writes.send(Write::Background { session: "ses_loud".into(), quiet: false }).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut failed = Vec::new();
        while failed.is_empty() && std::time::Instant::now() < deadline {
            if let Ok(Incoming::Status(e)) = inbox.recv_timeout(Duration::from_millis(100))
                && e.contains("/background")
            {
                failed.push(e);
            }
        }
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].starts_with("POST /api/session/ses_loud/background"), "{failed:?}");
    }

    #[test]
    fn export_files_are_named_as_desktop_names_them() {
        let name = |title: &str| export_filename(&serde_json::json!({ "title": title, "slug": "swift-owl" }), "ses_1");
        assert_eq!(name("Fix the Jitter!"), "fix-the-jitter.json");
        assert_eq!(name("  --a__b--  "), "a__b.json");
        assert_eq!(name(""), "swift-owl.json");
        assert_eq!(name("日本語"), "ses_1.json");
    }

    #[test]
    fn replies_to_a_permission_and_a_form() {
        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.reply_permission("ses_a", "per_b", "once").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/permission/per_b/reply HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"decision":"once"}"#), "{req}");

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.reply_form("ses_a", "frm_c", serde_json::json!({"q0": "dev"})).unwrap();
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
    fn session_ids_match_the_servers_and_sort_newest_first() {
        let a = id_at("ses", 1_793_064_618_118, true);
        let b = id_at("ses", 1_793_064_618_119, true);
        assert_eq!(a.len(), 4 + 26, "{a}");
        // `Identifier.descending`: the inverted digits of the message id at that millisecond.
        assert_eq!(&a[..16], format!("ses_{:012x}", !(1_793_064_618_118u64 * 0x1000 + 1) & 0xffff_ffff_ffff));
        assert!(b < a, "{a} {b}");
        assert!(session_id().starts_with("ses_"));
    }

    #[test]
    fn a_start_creates_under_its_id_with_the_choice_then_prompts_without_switching() {
        let info = serde_json::json!({ "data": { "id": "ses_new", "location": { "directory": "/r" } } }).to_string();
        let (base, h) = mock_replies(vec![("200 OK", info), ("200 OK", r#"{"data":{"id":"msg_1"}}"#.into())]);
        let prompt = Prompt {
            id: Some("msg_1".into()),
            text: "go".into(),
            agent: Some("build".into()),
            model: Some(model::ModelRef { id: "opus".into(), provider: "opencode".into(), variant: "high".into() }),
            ..Default::default()
        };
        let made = Server { base, auth: None, media_refs: false, remote_files: false }.start_as("ses_new", "/r", &prompt).unwrap();
        assert_eq!(made.id, "ses_new");
        let reqs = h.join().unwrap();
        let body = |r: &str| serde_json::from_str::<Value>(&r[r.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert!(reqs[0].starts_with("POST /api/session HTTP/1.1"), "{}", reqs[0]);
        assert_eq!(body(&reqs[0]), serde_json::json!({ "id": "ses_new", "location": { "directory": "/r" }, "agent": "build",
            "model": { "id": "opus", "providerID": "opencode", "variant": "high" } }));
        assert!(reqs[1].starts_with("POST /api/session/ses_new/prompt HTTP/1.1"), "{}", reqs[1]);
        assert_eq!(body(&reqs[1]), prompt.body());
    }

    #[test]
    fn a_start_whose_prompt_fails_deletes_the_session() {
        let info = serde_json::json!({ "data": { "id": "ses_new" } }).to_string();
        let (base, h) = mock_replies(vec![("200 OK", info), ("500 Internal Server Error", "{}".into()), ("200 OK", "{}".into())]);
        let prompt = Prompt { id: Some("msg_1".into()), text: "go".into(), ..Default::default() };
        assert!(Server { base, auth: None, media_refs: false, remote_files: false }.start_as("ses_new", "/r", &prompt).is_err());
        let reqs = h.join().unwrap();
        assert!(reqs[2].starts_with("DELETE /api/session/ses_new HTTP/1.1"), "{}", reqs[2]);
    }

    #[test]
    fn a_refused_start_deletes_nothing() {
        let (base, h) = mock_replies(vec![("400 Bad Request", "{}".into())]);
        let prompt = Prompt { id: Some("msg_1".into()), text: "go".into(), ..Default::default() };
        let e = Server { base, auth: None, media_refs: false, remote_files: false }.start_as("ses_new", "/r", &prompt).unwrap_err();
        assert!(crate::plain_error::failed_for_sure(&e), "{e}");
        assert_eq!(h.join().unwrap().len(), 1);
    }

    #[test]
    fn sends_a_prompt_with_its_id_and_an_image() {
        let (base, h) = mock_once();
        let prompt = Prompt {
            id: Some("msg_17afac822001abcdefghijklmn".into()),
            text: "see this".into(),
            files: vec![FileInput { uri: data_url("image/png", b"ab"), name: "shot.png".into(), ..Default::default() }],
            ..Default::default()
        };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
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
    fn queues_moves_and_withdraws_a_prompt() {
        let (base, h) = mock_once();
        let prompt = Prompt { text: "after this".into(), delivery: Some(Delivery::Queue), ..Default::default() };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
        let req = h.join().unwrap();
        let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body, serde_json::json!({"text": "after this", "delivery": "queue"}));

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.set_delivery("ses_a", "msg_1", Delivery::Steer).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("PATCH /api/session/ses_a/inbox/msg_1 HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"delivery":"steer"}"#), "{req}");

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.cancel_inbox("ses_a", "msg_1").unwrap();
        assert!(h.join().unwrap().starts_with("DELETE /api/session/ses_a/inbox/msg_1 HTTP/1.1"));
    }

    #[test]
    fn mentions_go_out_as_beta_sends_them() {
        let m = |start, end, text: &str| Some(Mention { start, end, text: text.into() });
        let prompt = Prompt {
            text: "@explore read @src/a.rs with @opencode, see @session:ses_x".into(),
            files: vec![FileInput {
                uri: "file:///repo/src/a.rs".into(),
                name: "a.rs".into(),
                mime: Some("text/plain".into()),
                mention: m(14, 23, "@src/a.rs"),
            }],
            agents: vec![AgentInput { name: "explore".into(), mention: m(0, 8, "@explore") }],
            skills: vec![SkillInput { id: "opencode".into(), name: "OpenCode".into(), mention: m(29, 38, "@opencode") }],
            display_text: Some("@explore read @src/a.rs with @opencode, see @Fix it".into()),
            ..Default::default()
        };
        assert_eq!(
            prompt.body(),
            serde_json::json!({
                "text": "@explore read @src/a.rs with @opencode, see @session:ses_x",
                "files": [{"uri": "file:///repo/src/a.rs", "name": "a.rs", "mime": "text/plain",
                    "mention": {"start": 14, "end": 23, "text": "@src/a.rs"}}],
                "agents": [{"name": "explore", "mention": {"start": 0, "end": 8, "text": "@explore"}}],
                "skills": [{"id": "opencode", "name": "OpenCode", "mention": {"start": 29, "end": 38, "text": "@opencode"}}],
                "metadata": {"displayText": "@explore read @src/a.rs with @opencode, see @Fix it", "comments": [], "attachments": []},
            })
        );
        let same = Prompt { text: "hi".into(), display_text: Some("hi".into()), ..Default::default() };
        assert_eq!(same.body(), serde_json::json!({"text": "hi"}));
    }

    #[test]
    fn message_reads_opt_into_media_refs_and_media_reads_bytes() {
        let (base, h) = mock_once();
        Server::new(&base).with_media_refs().read("/api/session/ses_a/message").unwrap();
        let req = h.join().unwrap().to_ascii_lowercase();
        assert!(req.contains("x-opencode-capabilities: experimental-media-uri"), "{req}");

        // Off by default, and never on other reads.
        let (base, h) = mock_once();
        Server::new(&base).read("/api/session/ses_a/message").unwrap();
        assert!(!h.join().unwrap().to_ascii_lowercase().contains("x-opencode-capabilities"));
        let (base, h) = mock_once();
        Server::new(&base).with_media_refs().get("/api/session").unwrap();
        assert!(!h.join().unwrap().to_ascii_lowercase().contains("x-opencode-capabilities"));

        let hash = "a".repeat(64);
        let (base, h) = mock_once();
        assert_eq!(Server { base, auth: None, media_refs: false, remote_files: false }.media(&hash).unwrap(), b"{}");
        assert!(h.join().unwrap().starts_with(&format!("GET /api/experimental/media/{hash} HTTP/1.1")));
    }

    #[test]
    fn a_media_ref_goes_back_as_its_uri() {
        let hash = "b".repeat(64);
        let prompt = Prompt {
            text: "again".into(),
            files: vec![FileInput { uri: format!("media:{hash}"), name: "again.png".into(), mime: Some("image/png".into()), mention: None }],
            delivery: Some(Delivery::Queue),
            ..Default::default()
        };
        assert_eq!(
            prompt.body(),
            serde_json::json!({"text": "again", "delivery": "queue",
                "files": [{"uri": format!("media:{hash}"), "name": "again.png", "mime": "image/png"}]})
        );
    }

    #[test]
    fn path_attachments_and_commands() {
        let attached = Prompt {
            text: "look\nAttached file: `/tmp/a.log`".into(),
            display_text: Some("look".into()),
            attachments: vec![PathAttachment { name: "a.log".into(), mime: "text/plain".into(), path: "/tmp/a.log".into() }],
            ..Default::default()
        };
        assert_eq!(
            attached.body(),
            serde_json::json!({
                "text": "look\nAttached file: `/tmp/a.log`",
                "metadata": {"displayText": "look", "comments": [], "attachments": [{"name": "a.log", "mime": "text/plain", "path": "/tmp/a.log"}]},
            })
        );

        let quote = crate::quote::Quote { text: "a\nb".into(), comment: Some("why".into()) };
        let quoted = Prompt { text: format!("look\n{}", quote.prompt_text()), display_text: Some("look".into()), quotes: vec![quote], ..Default::default() };
        assert_eq!(
            quoted.body()["metadata"],
            serde_json::json!({"displayText": "look", "comments": [{"type": "quote", "text": "a\nb", "comment": "why"}], "attachments": []})
        );

        let (base, h) = mock_once();
        let prompt = Prompt { id: Some("msg_x".into()), text: "HEAD~1".into(), command: Some("review".into()), delivery: Some(Delivery::Queue), ..Default::default() };
        assert_eq!(Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap(), None);
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/command HTTP/1.1"), "{req}");
        let body: Value = serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(body, serde_json::json!({"name": "review", "text": "HEAD~1", "delivery": "queue"}));

        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.shell("ses_a", Some("msg_y"), "ls -la").unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/shell HTTP/1.1"), "{req}");
        assert!(req.ends_with(r#"{"command":"ls -la","id":"msg_y"}"#), "{req}");
    }

    #[test]
    fn local_servers() {
        let s = |b: &str| Server { base: b.into(), auth: None, media_refs: false, remote_files: false };
        assert!(s("http://127.0.0.1:4096").is_local());
        assert!(s("http://localhost:4096").is_local());
        assert!(!s("https://host.example.ts.net:4096").is_local());
        assert_eq!(encode_component("/a b/é?&"), "%2Fa%20b%2F%C3%A9%3F%26");
    }

    fn body_of(req: &str) -> Value {
        serde_json::from_str(&req[req.find("\r\n\r\n").unwrap() + 4..]).unwrap()
    }

    #[test]
    fn a_choice_switches_the_agent_then_the_model_before_the_prompt() {
        let (base, h) = mock(3);
        let model = model::ModelRef { id: "gpt-6-astra".into(), provider: "openai".into(), variant: "high".into() };
        let prompt = Prompt { id: Some("msg_a".into()), text: "go".into(), agent: Some("plan".into()), model: Some(model), ..Default::default() };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("POST /api/session/ses_a/agent HTTP/1.1"), "{}", reqs[0]);
        assert_eq!(body_of(&reqs[0]), serde_json::json!({"agent": "plan"}));
        assert!(reqs[1].starts_with("POST /api/session/ses_a/model HTTP/1.1"), "{}", reqs[1]);
        assert_eq!(body_of(&reqs[1]), serde_json::json!({"model": {"id": "gpt-6-astra", "providerID": "openai", "variant": "high"}}));
        assert!(reqs[2].starts_with("POST /api/session/ses_a/prompt HTTP/1.1"), "{}", reqs[2]);
        assert_eq!(
            body_of(&reqs[2]),
            serde_json::json!({"id": "msg_a", "text": "go",
                "metadata": {"agent": "plan", "model": {"providerID": "openai", "modelID": "gpt-6-astra", "variant": "high"}}})
        );
    }

    #[test]
    fn a_default_effort_sends_no_variant_and_a_model_alone_leaves_the_agent() {
        let (base, h) = mock(2);
        let model = model::ModelRef { id: "claude-sonnet-5-5".into(), provider: "anthropic".into(), variant: String::new() };
        let prompt = Prompt { text: "go".into(), display_text: Some("go!".into()), model: Some(model), ..Default::default() };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("POST /api/session/ses_a/model HTTP/1.1"), "{}", reqs[0]);
        assert_eq!(body_of(&reqs[0]), serde_json::json!({"model": {"id": "claude-sonnet-5-5", "providerID": "anthropic"}}));
        assert_eq!(
            body_of(&reqs[1])["metadata"],
            serde_json::json!({"displayText": "go!", "comments": [], "attachments": [],
                "model": {"providerID": "anthropic", "modelID": "claude-sonnet-5-5"}})
        );
    }

    #[test]
    fn a_queued_prompt_only_records_its_choice_and_no_choice_sends_nothing_extra() {
        let (base, h) = mock_once();
        let prompt = Prompt { text: "later".into(), delivery: Some(Delivery::Queue), agent: Some("plan".into()), ..Default::default() };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
        let req = h.join().unwrap();
        assert!(req.starts_with("POST /api/session/ses_a/prompt HTTP/1.1"), "{req}");
        assert_eq!(body_of(&req), serde_json::json!({"text": "later", "delivery": "queue", "metadata": {"agent": "plan"}}));

        let plain = Prompt { text: "hi".into(), delivery: Some(Delivery::Steer), ..Default::default() };
        assert_eq!(plain.body(), serde_json::json!({"text": "hi", "delivery": "steer"}));
    }

    #[test]
    fn a_steered_command_switches_first() {
        let (base, h) = mock(2);
        let prompt = Prompt { text: "HEAD".into(), command: Some("review".into()), delivery: Some(Delivery::Steer), agent: Some("plan".into()), ..Default::default() };
        Server { base, auth: None, media_refs: false, remote_files: false }.send("ses_a", &prompt).unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("POST /api/session/ses_a/agent HTTP/1.1"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("POST /api/session/ses_a/command HTTP/1.1"), "{}", reqs[1]);
        assert_eq!(body_of(&reqs[1]), serde_json::json!({"name": "review", "text": "HEAD", "delivery": "steer"}));
    }

    #[test]
    fn interrupts() {
        let (base, h) = mock_once();
        Server { base, auth: None, media_refs: false, remote_files: false }.interrupt("ses_a").unwrap();
        assert!(h.join().unwrap().starts_with("POST /api/session/ses_a/interrupt HTTP/1.1"));
    }

    #[test]
    fn open_project_resolves_location_refreshes_projects_and_creates_session() {
        use std::io::{Read, Write as IoWrite};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let replies = [
            r#"{"directory":"/Users/demo/code/my repo","project":{"id":"proj_1","directory":"/Users/demo/code/my repo","canonical":"/Users/demo/code/my repo"}}"#,
            r#"[{"id":"proj_1","canonical":"/Users/demo/code/my repo","name":"My Repo","time":{"active":42},"sandboxes":[]}]"#,
            r#"{"data":{"id":"ses_new_proj","projectID":"proj_1","location":{"directory":"/Users/demo/code/my repo"},"time":{"created":42,"updated":42}}}"#,
        ];
        let h = std::thread::spawn(move || {
            replies
                .into_iter()
                .map(|body| {
                    let (mut s, _) = listener.accept().unwrap();
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}", body.len());
                    let _ = s.write_all(head.as_bytes());
                    String::from_utf8_lossy(&buf[..n]).into_owned()
                })
                .collect::<Vec<_>>()
        });
        let server = Server { base, auth: Some("Basic dGVzdA==".into()), media_refs: false, remote_files: false };
        let (loc, projects) = server.open_project("/Users/demo/code/my repo").unwrap();
        let created = server.create_session(&loc.directory).unwrap();
        let reqs = h.join().unwrap();
        assert!(reqs[0].starts_with("GET /api/location?location%5Bdirectory%5D=%2FUsers%2Fdemo%2Fcode%2Fmy%20repo HTTP/1.1"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("GET /api/project HTTP/1.1"), "{}", reqs[1]);
        assert!(reqs[2].starts_with("POST /api/session HTTP/1.1"), "{}", reqs[2]);
        assert_eq!(body_of(&reqs[2]), serde_json::json!({ "location": { "directory": "/Users/demo/code/my repo" } }));
        assert_eq!(loc.project_id, "proj_1");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "My Repo");
        assert_eq!(created.id, "ses_new_proj");
    }
}
