//! Wire types, constants, identifiers, and SSE control frame decoder for the OpenCode
//! `experimental.browser` v4 RPC protocol (`packages/plugin-browser/src/rpc.ts`,
//! `packages/gui-plugins/src/browser/pane-machine.ts`).

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::wire_error::WireError;

/// RPC identifier registered by `@opencode/plugin-browser`.
pub const RPC_ID: &str = "experimental.browser";

/// SSE event type emitted by `@opencode/plugin-browser` for attachment handshakes, commands, and cancellations.
pub const CONTROL_EVENT: &str = "rpc.experimental.browser.control";

/// Protocol version spoken by this client (`Browser.Control` `version: 4`).
pub const PROTOCOL_VERSION: u32 = 4;

/// Maximum combined bytes for upload/download/capture transfers (`Browser.MAX_FILE_BYTES`, 5 MiB).
pub const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;

/// Maximum bytes per `tunnel.read` / `tunnel.write` frame (`Browser.TUNNEL_CHUNK_BYTES`, 64 KiB).
pub const TUNNEL_CHUNK_BYTES: usize = 64 * 1024;

/// Maximum text length for snapshot/console/URL wire fields (`Browser.MAX_TEXT`).
pub const MAX_TEXT: usize = 100_000;

/// Maximum duration for a `wait` action (`Browser.MAX_WAIT_MS`, 30 s).
pub const MAX_WAIT_MS: u64 = 30_000;

/// Maximum time a prompt waits for built-in gates before sending anyway (`PROMPT_GATE_TIMEOUT_MS`, 1 s).
pub const PROMPT_GATE_TIMEOUT_MS: u64 = 1_000;

/// Maximum time an attachment waits for the server's `attached` SSE handshake (`REGISTER_TIMEOUT_MS`, 15 s).
pub const REGISTER_TIMEOUT_MS: u64 = 15_000;

/// Base reconnect backoff after attachment failure (`RETRY_BASE_MS`, 1 s).
pub const RETRY_BASE_MS: u64 = 1_000;

/// Maximum reconnect backoff (`RETRY_MAX_MS`, 30 s).
pub const RETRY_MAX_MS: u64 = 30_000;

/// Maximum concurrent server-side TCP tunnels per attachment (`BrowserTunnelMachine.MAX_TUNNELS`).
pub const MAX_TUNNELS: usize = 64;

/// Failure codes acted on by the server (`Browser.FailureCode` in `packages/plugin-browser/src/rpc.ts:12-23`).
pub mod failure_code {
    pub const CONDITION_UNMET: &str = "condition_unmet";
    pub const DOCUMENT_CHANGED: &str = "document_changed";
    pub const STALE_REF: &str = "stale_ref";
    pub const PREVIEW_UNAVAILABLE: &str = "preview_unavailable";
    pub const SIGN_IN_REQUIRED: &str = "sign_in_required";
    pub const NAVIGATION_FAILED: &str = "navigation_failed";
    pub const OPERATION_FAILED: &str = "operation_failed";
    pub const UNSUPPORTED: &str = "unsupported";
}

fn fill_random(out: &mut [u8]) {
    if getrandom::fill(out).is_err() {
        use std::hash::{BuildHasher as _, Hasher as _};
        let state = std::collections::hash_map::RandomState::new();
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = state.build_hasher();
            h.write_usize(i);
            let bytes = h.finish().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

/// Cryptographically random lowercase hex bytes (`bytes * 2` hex chars). Fails closed if the
/// OS CSPRNG (`getrandom::fill`) fails — never falls back to a non-cryptographic hash.
pub fn try_random_hex(bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf)
        .map_err(|e| format!("OS cryptographic RNG unavailable for browser tunnel credentials: {e}"))?;
    let mut out = String::with_capacity(bytes * 2);
    for b in buf {
        out.push_str(&format!("{b:02x}"));
    }
    Ok(out)
}

/// Cryptographically random RFC 4122 version-4 UUID (`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`).
pub fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    fill_random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Allocates a wire-valid browser tab ID (`^tab_[a-f0-9-]{36}$`).
pub fn new_tab_id() -> String {
    format!("tab_{}", uuid_v4())
}

/// Allocates a wire-valid browser file ID (`^file_[a-f0-9-]{36}$`).
pub fn new_file_id() -> String {
    format!("file_{}", uuid_v4())
}

/// Allocates a random attachment connection ID.
pub fn new_connection_id() -> String {
    uuid_v4()
}

/// Allocates `n` cryptographically random bytes encoded as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    fill_random(&mut buf);
    let mut out = String::with_capacity(bytes * 2);
    for b in buf {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Whether `id` matches `Browser.TabID` (`^tab_[a-f0-9-]{36}$`).
pub fn is_valid_tab_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("tab_") else { return false };
    rest.len() == 36 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Whether `id` matches `Browser.FileID` (`^file_[a-f0-9-]{36}$`).
pub fn is_valid_file_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("file_") else { return false };
    rest.len() == 36 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Normalizes `@e12` or `e12` into canonical `e12` if valid (`^@?e[1-9][0-9]*$`).
pub fn normalize_ref(raw: &str) -> Option<String> {
    let stripped = raw.strip_prefix('@').unwrap_or(raw);
    let digits = stripped.strip_prefix('e')?;
    let first = digits.as_bytes().first().copied()?;
    if !matches!(first, b'1'..=b'9') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(stripped.to_owned())
}

/// Server location query parameters required on every `/api/rpc/experimental.browser/*` call.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocationRef {
    pub directory: String,
    pub workspace_id: Option<String>,
}

impl LocationRef {
    pub fn new(directory: impl Into<String>, workspace_id: Option<String>) -> Self {
        Self { directory: directory.into(), workspace_id }
    }
}

/// One browser tab in the wire inventory (`Browser.Tab`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
    #[serde(rename = "loadError", default, skip_serializing_if = "Option::is_none")]
    pub load_error: Option<String>,
    #[serde(rename = "canGoBack")]
    pub can_go_back: bool,
    #[serde(rename = "canGoForward")]
    pub can_go_forward: bool,
    pub generation: u64,
}

/// Session browser inventory sent via `experimental.browser/state` (`Browser.State`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserState {
    pub tabs: Vec<Tab>,
    #[serde(rename = "focusedTabID")]
    pub focused_tab_id: Option<String>,
}

/// File transferred between desktop and server (`Browser.File`, base64-encoded bytes <= 5 MiB).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireFile {
    pub id: String,
    pub name: String,
    pub mime: String,
    #[serde(rename = "data")]
    pub data_base64: String,
}

impl WireFile {
    pub fn from_bytes(id: String, name: String, mime: String, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!("File {name} exceeds the 5 MiB transfer limit."));
        }
        Ok(Self {
            id,
            name,
            mime,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    pub fn decode_bytes(&self) -> Result<Vec<u8>, String> {
        let trimmed = self.data_base64.trim();
        let max_b64 = MAX_FILE_BYTES.div_ceil(3) * 4 + 4;
        if trimmed.len() > max_b64 {
            return Err(format!("File {} exceeds the 5 MiB transfer limit.", self.name));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(trimmed)
            .map_err(|e| format!("Invalid base64 payload for {}: {e}", self.name))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!("File {} exceeds the 5 MiB transfer limit.", self.name));
        }
        Ok(bytes)
    }

    /// Validates that a batch of wire files respects both the 8-file count cap and the combined
    /// 5 MiB transfer limit before decoding.
    pub fn validate_batch(files: &[Self]) -> Result<(), String> {
        if files.len() > 8 {
            return Err("Too many files in transfer (maximum 8).".into());
        }
        let mut total = 0usize;
        for f in files {
            let trimmed = f.data_base64.trim();
            let est = (trimmed.len() / 4).saturating_mul(3);
            if est > MAX_FILE_BYTES {
                return Err(format!("File {} exceeds the 5 MiB transfer limit.", f.name));
            }
            total = total.saturating_add(est);
            if total > MAX_FILE_BYTES + 24 {
                return Err("Combined transfer files exceed the 5 MiB limit.".into());
            }
        }
        Ok(())
    }
}

/// Target metadata returned for `inspect: true` authorization probes (`Browser.Target`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetMeta {
    pub resources: Vec<String>,
    pub key: String,
}

/// Command fetched from `experimental.browser/command` (`Browser.Command`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub action: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default)]
    pub files: Vec<WireFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetMeta>,
}

impl Command {
    pub fn action_type(&self) -> Option<&str> {
        self.action.get("type").and_then(Value::as_str)
    }

    pub fn tab_id(&self) -> Option<&str> {
        self.action.get("tabID").and_then(Value::as_str)
    }

    pub fn is_inspect(&self) -> bool {
        self.inspect == Some(true)
    }
}

/// Success payload sent inside `Outcome::Success` (`Browser.Result`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultPayload {
    pub value: Value,
    pub files: Vec<WireFile>,
}

/// Result reported via `experimental.browser/result` (`Browser.Outcome`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Outcome {
    Success { result: ResultPayload },
    Failure { code: String, message: String },
}

impl Outcome {
    pub fn ok(value: Value, files: Vec<WireFile>) -> Self {
        Self::Success { result: ResultPayload { value, files } }
    }

    pub fn fail(code: impl Into<String>, message: impl Into<String>) -> Self {
        let mut msg = message.into();
        if msg.len() > 2_048 {
            let mut cut = 2_048;
            while cut > 0 && !msg.is_char_boundary(cut) {
                cut -= 1;
            }
            msg.truncate(cut);
        }
        Self::Failure { code: code.into(), message: msg }
    }
}

/// Decoded SSE `rpc.experimental.browser.control` frame for one `connectionID`
/// (`pane-machine.ts:65-73, 167-182`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlFrame {
    Foreign,
    Attached,
    Command { request_id: String },
    Cancel { request_id: String },
    Incompatible,
    Unrecognized { control_type: Option<String> },
}

/// Decodes an SSE `rpc.experimental.browser.control` payload against `connection_id`.
pub fn decode_control(connection_id: &str, data: &Value) -> ControlFrame {
    if data.get("connectionID").and_then(Value::as_str) != Some(connection_id) {
        return ControlFrame::Foreign;
    }
    let kind = data.get("type").and_then(Value::as_str);
    match kind {
        Some("attached") => {
            if data.get("version").and_then(Value::as_u64) == Some(u64::from(PROTOCOL_VERSION)) {
                ControlFrame::Attached
            } else {
                ControlFrame::Incompatible
            }
        }
        Some("command") => match data.get("requestID").and_then(Value::as_str) {
            Some(req) if !req.is_empty() => ControlFrame::Command { request_id: req.to_owned() },
            _ => ControlFrame::Unrecognized { control_type: Some("command".to_owned()) },
        },
        Some("cancel") => match data.get("requestID").and_then(Value::as_str) {
            Some(req) if !req.is_empty() => ControlFrame::Cancel { request_id: req.to_owned() },
            _ => ControlFrame::Unrecognized { control_type: Some("cancel".to_owned()) },
        },
        other => ControlFrame::Unrecognized { control_type: other.map(str::to_owned) },
    }
}

/// Validated remote TCP destination passed to `tunnel.open` (`Browser.TunnelTarget`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelTarget {
    pub host: String,
    pub port: u16,
}

impl TunnelTarget {
    pub fn parse(host: &str, port: u16) -> Option<Self> {
        let host = host.trim().trim_start_matches('[').trim_end_matches(']');
        if host.is_empty() || host.len() > 253 || port == 0 {
            return None;
        }
        if !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'%' | b'-'))
        {
            return None;
        }
        Some(Self { host: host.to_owned(), port })
    }
}

/// One chunk returned by `tunnel.read` (`Browser.TunnelRead`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelRead {
    pub data: Vec<u8>,
    pub eof: bool,
}

/// Terminal outcome of one held `experimental.browser/attach` call (`pane-machine.ts:62`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachOutcome {
    Closed,
    Replaced,
    Missing,
    Unavailable,
    Unsupported,
    Transport(String),
}

/// Typed error for one `experimental.browser/*` RPC call.
#[derive(Clone, Debug)]
pub enum RpcCallError {
    Unavailable(String),
    Unsupported(String),
    Missing(String),
    InvalidInput(String),
    Internal(String),
    Transport(WireError),
    Aborted,
}

impl std::fmt::Display for RpcCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m)
            | Self::Unsupported(m)
            | Self::Missing(m)
            | Self::InvalidInput(m)
            | Self::Internal(m) => f.write_str(m),
            Self::Transport(e) => write!(f, "{e}"),
            Self::Aborted => f.write_str("Browser RPC request was aborted."),
        }
    }
}
