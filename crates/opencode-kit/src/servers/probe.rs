//! The health probe: `GET /api/info` with the server's password (`checkServerHealth`,
//! `runtime/server/health.ts:97-143`). Blocking, so off the app's thread only: the caller runs it
//! for each [`super::health::Effect::Probe`] and steps the answer back as `ProbeSettled`.

use std::time::Duration;

use rune::time::Instant;
use serde_json::Value;

use super::health::ProbeResult;
use super::registry::HttpBase;

/// `defaultTimeoutMs` (`health.ts:26`): the whole probe, retries included.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// `defaultRetryCount` and `defaultRetryDelayMs` (`health.ts:28-30`): a transport failure tries
/// again after 100 ms, then 200 ms.
pub const PROBE_RETRIES: u32 = 2;
pub const PROBE_RETRY_DELAY: Duration = Duration::from_millis(100);

/// `ServerInfo` (`/api/info`'s answer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub version: String,
    pub pid: Option<u64>,
    pub hostname: Option<String>,
    pub urls: Vec<String>,
    pub tmp: Option<String>,
    pub persistent_pty: bool,
}

impl ServerInfo {
    pub fn from_json(v: &Value) -> Option<ServerInfo> {
        let text = |v: &Value| v.as_str().map(str::to_owned);
        Some(ServerInfo {
            version: text(&v["version"])?,
            pid: v["pid"].as_u64(),
            hostname: text(&v["hostname"]),
            urls: v["urls"].as_array().into_iter().flatten().filter_map(text).collect(),
            tmp: text(&v["paths"]["tmp"]),
            persistent_pty: v["capabilities"]["persistentPty"].as_bool().unwrap_or(false),
        })
    }
}

/// Why a request didn't answer with JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The connection failed: retryable (`ClientError` reason `Transport`).
    Transport(String),
    /// The time ran out.
    Timeout,
    /// The server answered with this status.
    Status(u16),
    /// The server answered something that isn't the JSON asked for.
    Body(String),
}

/// `checkServerHealth`: healthy with the version and hostname it reports, unauthorized on 401,
/// unreachable otherwise. Only a transport failure tries again, within [`PROBE_TIMEOUT`].
pub fn probe(http: &HttpBase) -> ProbeResult {
    probe_with(http, PROBE_TIMEOUT, PROBE_RETRIES, PROBE_RETRY_DELAY)
}

pub fn probe_with(http: &HttpBase, timeout: Duration, retries: u32, delay: Duration) -> ProbeResult {
    let deadline = Instant::now() + timeout;
    let mut count = 0;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return ProbeResult::Unreachable;
        }
        match get_json(http, "/api/info", left).map(|v| ServerInfo::from_json(&v)) {
            Ok(Some(info)) => return ProbeResult::Healthy { version: Some(info.version), hostname: info.hostname },
            Ok(None) => return ProbeResult::Unreachable,
            Err(RequestError::Status(401)) => return ProbeResult::Unauthorized,
            Err(RequestError::Transport(_)) if count < retries => {
                count += 1;
                let wait = delay * count;
                if Instant::now() + wait >= deadline {
                    return ProbeResult::Unreachable;
                }
                rune::task::sleep(wait);
            }
            Err(_) => return ProbeResult::Unreachable,
        }
    }
}

/// `/api/info` itself, for a caller that wants more than health.
pub fn info(http: &HttpBase, timeout: Duration) -> Result<ServerInfo, RequestError> {
    let v = get_json(http, "/api/info", timeout)?;
    ServerInfo::from_json(&v).ok_or_else(|| RequestError::Body("no version".into()))
}

/// `GET <url><path>` with Basic auth when the endpoint has a password, parsed as JSON.
pub(super) fn get_json(http: &HttpBase, path: &str, timeout: Duration) -> Result<Value, RequestError> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into();
    let mut req = agent.get(format!("{}{path}", http.url.trim_end_matches('/')));
    if let Some(auth) = authorization(http) {
        req = req.header("authorization", auth);
    }
    let mut resp = req.call().map_err(|e| match e {
        ureq::Error::Timeout(_) => RequestError::Timeout,
        e => RequestError::Transport(e.to_string()),
    })?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(RequestError::Status(status));
    }
    let text = resp.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| match e {
        ureq::Error::Timeout(_) => RequestError::Timeout,
        e => RequestError::Body(e.to_string()),
    })?;
    serde_json::from_str(&text).map_err(|e| RequestError::Body(e.to_string()))
}

/// `Basic base64(opencode:<password>)` (`authTokenFromCredentials`), as [`crate::client::Server`]
/// sends it.
fn authorization(http: &HttpBase) -> Option<String> {
    let password = http.password.as_deref().filter(|p| !p.is_empty())?;
    let server = crate::client::Server { base: http.url.clone(), auth: None, media_refs: false, remote_files: false };
    server.with_password(password).auth
}
