//! The health probe: `GET /api/info` with the server's password (`checkServerHealth`,
//! `runtime/server/health.ts:97-143`), plus its plain [`HttpBase`] endpoint and [`ProbeResult`]
//! value types. Blocking, so off the app's thread only.

use std::time::Duration;

use rune::time::Instant;
use serde_json::Value;

/// An endpoint and its optional password (`ServerConnection.HttpBase`, `persistence.ts:6-9`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct HttpBase {
    pub url: String,
    pub password: Option<String>,
}

impl HttpBase {
    pub fn new(url: impl Into<String>) -> HttpBase {
        HttpBase { url: url.into(), password: None }
    }

    pub fn with_password(mut self, password: impl Into<String>) -> HttpBase {
        self.password = Some(password.into());
        self
    }

    /// `endpointCredentials` (`runtime/server/machine.ts:92-94`): `<url>\n<password>`, what a
    /// probe answered for and what a controller is bound to.
    pub fn credentials(&self) -> String {
        format!("{}\n{}", self.url, self.password.as_deref().unwrap_or(""))
    }
}

/// `loopbackOrigin` (`registry.tsx:41-48`): `<scheme>//127.0.0.1:<port>` for a loopback address.
pub fn loopback_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, after) = v6.split_once(']')?;
            (format!("[{host}]"), after.strip_prefix(':').unwrap_or(""))
        }
        None => match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p),
            None => (authority.to_owned(), ""),
        },
    };
    let host = host.to_ascii_lowercase();
    if host != "localhost" && host != "127.0.0.1" && host != "[::1]" {
        return None;
    }
    let default = if scheme == "https" { "443" } else { "80" };
    let port = if port.is_empty() || port == default { default } else { port };
    Some(format!("{scheme}://127.0.0.1:{port}"))
}

/// The answer of one `/api/info` probe (`ServerProbeResult`, `health-machine.ts:7-11`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProbeResult {
    Healthy { version: Option<String>, hostname: Option<String> },
    Unauthorized,
    Unreachable,
}

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
/// A redirect or an answer that isn't OpenCode's (a captive portal's page) is `Body`: whatever
/// answered, it wasn't this server.
pub(super) fn get_json(http: &HttpBase, path: &str, timeout: Duration) -> Result<Value, RequestError> {
    use crate::wire_error::WireKind;
    let base = http.url.trim_end_matches('/');
    let server = crate::client::Server::anonymous(base);
    let server = match http.password.as_deref().filter(|p| !p.is_empty()) {
        Some(p) => server.with_password(p),
        None => server,
    };
    server.get_json(path, None, timeout).map(|(v, _)| v).map_err(|e| match e.kind {
        WireKind::TimedOut => RequestError::Timeout,
        WireKind::Status { status, .. } => RequestError::Status(status),
        WireKind::Unreachable(_) | WireKind::Dropped(_) | WireKind::Tls(_) | WireKind::Other(_) => RequestError::Transport(e.to_string()),
        WireKind::Redirected { .. } | WireKind::NotOpenCode { .. } | WireKind::TooLarge { .. } | WireKind::PartialOutput { .. } | WireKind::Shape(_) => RequestError::Body(e.to_string()),
    })
}
