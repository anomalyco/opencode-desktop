//! OpenCode's session terminals: persistent PTYs that live on the server's terminal service and
//! outlast every client (`packages/protocol/src/groups/persistent-pty.ts`,
//! `packages/client/src/solid/pty.ts`). Desktop lists and creates them per session, restores a
//! screen from a snapshot's checkpoint, then attaches over a WebSocket.

use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use serde_json::{Value, json};

use crate::client::{Server, encode_component};
use crate::pty::PtyStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistentPtyInfo {
    pub id: String,
    pub session_id: String,
    /// The shell's own title (OSC 0/2), or the one a user gave it.
    pub title: String,
    /// The title a user gave it, if any (`userTitle`).
    pub user_title: Option<String>,
    pub cwd: String,
    pub status: PtyStatus,
    /// The canonical grid every attachment renders at.
    pub size: (u16, u16),
    /// The byte offset just past the last output, where an attachment's replay ends.
    pub tail: i64,
}

/// What `GET /api/experimental/persistent-pty/:id/snapshot` restores a screen from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistentPtySnapshot {
    pub info: PersistentPtyInfo,
    /// VT bytes that redraw the screen and scrollback at `info.size`.
    pub checkpoint: Vec<u8>,
}

/// What a new terminal starts with (`PersistentPty.CreateInput`): its title, folder, grid, and the
/// program it runs instead of the user's shell.
#[derive(Clone, Debug, Default)]
pub struct TerminalCreate {
    title: String,
    cwd: Option<String>,
    size: Option<(u16, u16)>,
    command: Option<String>,
    args: Vec<String>,
}

impl TerminalCreate {
    pub fn new(title: impl Into<String>) -> Self {
        TerminalCreate { title: title.into(), ..TerminalCreate::default() }
    }

    /// The shell's working folder.
    pub fn cwd(mut self, cwd: Option<String>) -> Self {
        self.cwd = cwd;
        self
    }

    /// Its first grid, in columns and rows.
    pub fn size(mut self, size: Option<(u16, u16)>) -> Self {
        self.size = size;
        self
    }

    /// Runs `command` with `args` instead of the user's shell.
    pub fn command(mut self, command: Option<String>, args: Vec<String>) -> Self {
        self.command = command;
        self.args = args;
        self
    }
}

pub fn parse_persistent_pty_info(v: &Value) -> Option<PersistentPtyInfo> {
    let d = v.get("data").filter(|d| d.is_object()).unwrap_or(v);
    let id = d["id"].as_str().filter(|id| !id.is_empty())?.to_owned();
    let dim = |k: &str| d["size"][k].as_u64().and_then(|n| u16::try_from(n).ok()).filter(|&n| n > 0);
    Some(PersistentPtyInfo {
        id,
        session_id: d["sessionID"].as_str().unwrap_or_default().to_owned(),
        title: d["title"].as_str().unwrap_or_default().to_owned(),
        user_title: d["userTitle"].as_str().map(str::to_owned),
        cwd: d["cwd"].as_str().unwrap_or_default().to_owned(),
        status: if d["status"].as_str() == Some("exited") { PtyStatus::Exited } else { PtyStatus::Running },
        size: (dim("cols").unwrap_or(80), dim("rows").unwrap_or(24)),
        tail: d["output"]["tail"].as_i64().unwrap_or(0),
    })
}

/// One attachment's id; the server tells attachments apart by it when they take control.
pub fn attachment_id() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    #[cfg(not(target_arch = "wasm32"))]
    h.write_u32(std::process::id());
    // A page has no process id, and its hasher's keys can be the same in every page.
    #[cfg(target_arch = "wasm32")]
    h.write_u64(js_sys::Math::random().to_bits());
    format!("rune-{:016x}", h.finish())
}

impl Server {
    /// A session's terminals (`GET /api/experimental/session/:id/terminal`).
    pub fn terminal_list(&self, session_id: &str) -> Result<Vec<PersistentPtyInfo>, String> {
        let v = self.get(&format!("/api/experimental/session/{}/terminal", encode_component(session_id)))?;
        let items = v["data"].as_array().or_else(|| v.as_array());
        Ok(items.into_iter().flatten().filter_map(parse_persistent_pty_info).collect())
    }

    /// Starts the server's shell for a session (`POST /api/experimental/session/:id/terminal`).
    pub fn terminal_create(&self, session_id: &str, input: &TerminalCreate) -> Result<PersistentPtyInfo, String> {
        let path = format!("/api/experimental/session/{}/terminal", encode_component(session_id));
        let mut body = json!({ "args": input.args, "title": input.title, "env": {} });
        if let Some(cwd) = input.cwd.as_ref().filter(|c| !c.is_empty()) {
            body["cwd"] = json!(cwd);
        }
        if let Some(command) = input.command.as_ref().filter(|c| !c.is_empty()) {
            body["command"] = json!(command);
        }
        if let Some((cols, rows)) = input.size {
            body["size"] = json!({ "cols": cols.max(1), "rows": rows.max(1) });
        }
        let v = self.write("POST", &path, Some(body))?;
        parse_persistent_pty_info(&v).ok_or_else(|| format!("POST {path}: missing terminal in reply"))
    }

    /// Renames a terminal; `None` gives it back the shell's title (`PUT /api/experimental/persistent-pty/:id`).
    pub fn terminal_rename(&self, pty_id: &str, title: Option<&str>) -> Result<PersistentPtyInfo, String> {
        let path = format!("/api/experimental/persistent-pty/{}", encode_component(pty_id));
        let v = self.write("PUT", &path, Some(json!({ "title": title })))?;
        parse_persistent_pty_info(&v).ok_or_else(|| format!("PUT {path}: missing terminal in reply"))
    }

    /// Ends the shell and forgets the terminal (`DELETE /api/experimental/persistent-pty/:id`).
    pub fn terminal_remove(&self, pty_id: &str) -> Result<(), String> {
        self.write("DELETE", &format!("/api/experimental/persistent-pty/{}", encode_component(pty_id)), None).map(|_| ())
    }

    /// The screen to restore before attaching (`GET /api/experimental/persistent-pty/:id/snapshot`).
    pub fn terminal_snapshot(&self, pty_id: &str) -> Result<PersistentPtySnapshot, String> {
        let path = format!("/api/experimental/persistent-pty/{}/snapshot", encode_component(pty_id));
        let v = self.get(&path)?;
        let d = &v["data"];
        let info = parse_persistent_pty_info(&d["info"]).ok_or_else(|| format!("GET {path}: missing terminal in reply"))?;
        let checkpoint = d["checkpoint"].as_str().and_then(crate::model::base64_decode).unwrap_or_default();
        Ok(PersistentPtySnapshot { info, checkpoint })
    }

    /// Mints a single-use ticket and returns the `ws://` URL that attaches to a terminal from
    /// `cursor`, with framed input (`input_protocol=1`). An observer only watches; a controller
    /// takes over the size and the keyboard.
    pub fn terminal_connect_url(&self, pty_id: &str, cursor: i64, attachment_id: &str, observer: bool) -> Result<String, String> {
        let id = encode_component(pty_id);
        let path = format!("/api/experimental/persistent-pty/{id}/connect-token");
        let (v, ..) = self.exchange("POST", &path, Some(&b"{}"[..]), Some(Duration::from_secs(15)), &[("x-opencode-ticket", "1")])?.json("POST", &path)?;
        let ticket = v["data"]["ticket"]
            .as_str()
            .or_else(|| v["ticket"].as_str())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| format!("POST {path}: missing ticket in reply"))?;
        let ws = match self.base.split_once("://") {
            Some(("https", rest)) => format!("wss://{rest}"),
            Some((_, rest)) => format!("ws://{rest}"),
            None => format!("ws://{}", self.base),
        };
        let role = if observer { "&role=observer" } else { "&takeover=true" };
        Ok(format!(
            "{ws}/api/experimental/persistent-pty/{id}/connect?ticket={}&cursor={}&attachment_id={}{role}&input_protocol=1",
            encode_component(ticket),
            cursor.max(0),
            encode_component(attachment_id),
        ))
    }
}
