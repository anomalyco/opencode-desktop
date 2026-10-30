//! OpenCode server `/api/pty` REST and WebSocket connect-ticket client (`packages/protocol/src/groups/pty.ts`,
//! `packages/client/src/solid/pty.ts`). All methods are additive on [`Server`].

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{Server, encode_component};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PtyStatus {
    Running,
    Exited,
}

impl PtyStatus {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("exited") => Self::Exited,
            _ => Self::Running,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyInfo {
    pub id: String,
    pub title: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub status: PtyStatus,
    pub pid: u64,
    pub exit_code: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct PtyCreate {
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: BTreeMap<String, String>,
}

fn parse_pty_info(v: &Value) -> Option<PtyInfo> {
    let d = v.get("data").unwrap_or(v);
    let id = d["id"].as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    Some(PtyInfo {
        id,
        title: d["title"].as_str().unwrap_or("").to_owned(),
        command: d["command"].as_str().unwrap_or("").to_owned(),
        args: d["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect(),
        cwd: d["cwd"].as_str().unwrap_or("").to_owned(),
        status: PtyStatus::parse(d["status"].as_str()),
        pid: d["pid"].as_u64().unwrap_or(0),
        exit_code: d["exitCode"].as_u64().and_then(|c| u32::try_from(c).ok()),
    })
}

fn dir_query(directory: &str) -> String {
    if directory.is_empty() {
        String::new()
    } else {
        format!("?location%5Bdirectory%5D={}", encode_component(directory))
    }
}

impl Server {
    /// Lists PTY sessions for `directory` (`GET /api/pty`), including exited ones retained until removal.
    pub fn pty_list(&self, directory: &str) -> Result<Vec<PtyInfo>, String> {
        let path = format!("/api/pty{}", dir_query(directory));
        let v = self.get(&path)?;
        let arr = v["data"].as_array().or_else(|| v.as_array());
        Ok(arr.into_iter().flatten().filter_map(parse_pty_info).collect())
    }

    /// Creates a new PTY session in `directory` (`POST /api/pty`).
    pub fn pty_create(&self, directory: &str, input: &PtyCreate) -> Result<PtyInfo, String> {
        let path = format!("/api/pty{}", dir_query(directory));
        let mut body = serde_json::Map::new();
        if let Some(title) = &input.title {
            body.insert("title".into(), json!(title));
        }
        if let Some(cwd) = &input.cwd {
            body.insert("cwd".into(), json!(cwd));
        }
        if let Some(cmd) = &input.command {
            body.insert("command".into(), json!(cmd));
        }
        if let Some(args) = &input.args {
            body.insert("args".into(), json!(args));
        }
        if !input.env.is_empty() {
            body.insert("env".into(), json!(input.env));
        }
        let v = self.write("POST", &path, Some(Value::Object(body)))?;
        parse_pty_info(&v).ok_or_else(|| format!("POST {path}: missing PTY info in reply"))
    }

    /// Inspects one PTY session (`GET /api/pty/:ptyID`).
    pub fn pty_get(&self, directory: &str, pty_id: &str) -> Result<PtyInfo, String> {
        let path = format!("/api/pty/{}{}", encode_component(pty_id), dir_query(directory));
        let v = self.get(&path)?;
        parse_pty_info(&v).ok_or_else(|| format!("GET {path}: missing PTY info in reply"))
    }

    /// Updates a PTY session's title and/or `(cols, rows)` size (`PUT /api/pty/:ptyID`).
    pub fn pty_update(
        &self,
        directory: &str,
        pty_id: &str,
        title: Option<&str>,
        size: Option<(u16, u16)>,
    ) -> Result<PtyInfo, String> {
        let path = format!("/api/pty/{}{}", encode_component(pty_id), dir_query(directory));
        let mut body = serde_json::Map::new();
        if let Some(t) = title {
            body.insert("title".into(), json!(t));
        }
        if let Some((cols, rows)) = size {
            body.insert("size".into(), json!({ "cols": cols, "rows": rows }));
        }
        let v = self.write("PUT", &path, Some(Value::Object(body)))?;
        parse_pty_info(&v).ok_or_else(|| format!("PUT {path}: missing PTY info in reply"))
    }

    /// Terminates and removes a PTY session (`DELETE /api/pty/:ptyID`).
    pub fn pty_remove(&self, directory: &str, pty_id: &str) -> Result<(), String> {
        let path = format!("/api/pty/{}{}", encode_component(pty_id), dir_query(directory));
        self.write("DELETE", &path, None).map(|_| ())
    }

    /// Mints a single-use connect ticket (`POST /api/pty/:ptyID/connect-token` with
    /// `x-opencode-ticket: 1`) and returns the `ws://` or `wss://` upgrade URL.
    pub fn pty_connect_url(
        &self,
        directory: &str,
        pty_id: &str,
        cursor: Option<i64>,
    ) -> Result<String, String> {
        let path = format!(
            "/api/pty/{}/connect-token{}",
            encode_component(pty_id),
            dir_query(directory)
        );
        let url = format!("{}{path}", self.base);
        let mut req = Self::agent(Some(Duration::from_secs(15)))
            .post(url)
            .header("Content-Type", "application/json")
            .header("x-opencode-ticket", "1");
        if let Some(a) = &self.auth {
            req = req.header("Authorization", a);
        }
        let mut resp = req.send(b"{}").map_err(|e| format!("POST {path}: {e}"))?;
        let text = resp.body_mut().read_to_string().map_err(|e| format!("POST {path}: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("POST {path}: {e}"))?;
        let ticket = v["data"]["ticket"]
            .as_str()
            .or_else(|| v["ticket"].as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("POST {path}: missing ticket in reply"))?;

        let ws_base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            format!("ws://{}", self.base)
        };
        let mut qs = Vec::with_capacity(3);
        if !directory.is_empty() {
            qs.push(format!("location%5Bdirectory%5D={}", encode_component(directory)));
        }
        if let Some(c) = cursor {
            qs.push(format!("cursor={c}"));
        }
        qs.push(format!("ticket={}", encode_component(ticket)));
        Ok(format!(
            "{ws_base}/api/pty/{}/connect?{}",
            encode_component(pty_id),
            qs.join("&")
        ))
    }
}

#[cfg(test)]
#[path = "pty_tests.rs"]
mod tests;
