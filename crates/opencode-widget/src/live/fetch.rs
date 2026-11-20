//! `__host.fetch` (`specs/v2/widgets-live.md`, Capabilities): the rules every request keeps,
//! here, and the transfer itself, which the host does ([`Transport`]): `https:` only, to a host
//! the template's granted `https` capability names (checked again on every redirect, at most 5),
//! the methods the contract lists, a request body up to 256 KB and a response up to 2 MB, 15 s
//! in all, at most 4 in flight and 10 a second per widget. A response crosses as
//! `{ status, headers, body | body64 }`; a failure as `{ kind, message }`.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{Map, Value, json};
use url::{Host, Url};

pub const MAX_REQUEST: usize = 256 * 1024;
pub const MAX_RESPONSE: usize = 2 * 1024 * 1024;
pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_REDIRECTS: usize = 5;
pub const MAX_IN_FLIGHT: usize = 4;
pub const MAX_PER_SECOND: usize = 10;

const METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// Headers a template can't set: the transfer's own, and ambient credentials.
const FORBIDDEN: [&str; 9] = ["cookie", "cookie2", "host", "content-length", "connection", "transfer-encoding", "keep-alive", "upgrade", "te"];

/// Why a request failed, as the program sees it (`error.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchErrorKind {
    CapabilityDenied,
    Network,
    Timeout,
    TooLarge,
    RateLimited,
}

impl FetchErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            FetchErrorKind::CapabilityDenied => "CapabilityDenied",
            FetchErrorKind::Network => "Network",
            FetchErrorKind::Timeout => "Timeout",
            FetchErrorKind::TooLarge => "TooLarge",
            FetchErrorKind::RateLimited => "RateLimited",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError {
    pub kind: FetchErrorKind,
    pub message: String,
}

impl FetchError {
    pub fn new(kind: FetchErrorKind, message: impl Into<String>) -> FetchError {
        FetchError { kind, message: message.into() }
    }

    /// What the program's `catch` gets.
    pub fn to_value(&self) -> Value {
        json!({ "kind": self.kind.name(), "message": self.message })
    }
}

/// One hop of a request, checked, for the host to send as is (no redirects followed, no
/// cookies, no stored credentials).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// What one hop answered: its body read up to [`MAX_RESPONSE`] (more is [`FetchErrorKind::TooLarge`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Lowercase names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Sends one hop. The host refuses addresses that resolve to loopback, private or link-local
/// networks, and gives up at `timeout`.
pub trait Transport: Send + Sync {
    fn send(&self, request: &Request, timeout: Duration) -> Result<Response, FetchError>;
}

/// The `https` capability as granted: the hosts a widget may reach (`["*"]`: any).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub https: Option<Vec<String>>,
}

impl Grants {
    /// Whether `host` is one the grant names: exactly, under a `*.` wildcard, or any with `*`.
    pub fn allows(&self, host: &str) -> bool {
        let Some(hosts) = &self.https else { return false };
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        hosts.iter().any(|h| {
            let h = h.to_ascii_lowercase();
            h == "*" || h == host || h.strip_prefix("*.").is_some_and(|suffix| host.len() > suffix.len() && host.ends_with(suffix) && host[..host.len() - suffix.len()].ends_with('.'))
        })
    }

    /// The template's declared `capabilities` (`{ https?: { hosts } }`), all of them.
    pub fn declared(capabilities: &Value) -> Grants {
        let hosts = capabilities["https"]["hosts"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>());
        Grants { https: hosts.filter(|h| !h.is_empty()) }
    }
}

/// A widget's requests over the last second and those in flight.
#[derive(Debug, Default)]
pub struct Rate {
    started: VecDeque<Instant>,
    pub in_flight: usize,
}

impl Rate {
    /// Takes a slot for a request starting at `now`, or says why not.
    pub fn take(&mut self, now: Instant) -> Result<(), FetchError> {
        while self.started.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1)) {
            self.started.pop_front();
        }
        if self.in_flight >= MAX_IN_FLIGHT {
            return Err(FetchError::new(FetchErrorKind::RateLimited, format!("at most {MAX_IN_FLIGHT} requests may be in flight")));
        }
        if self.started.len() >= MAX_PER_SECOND {
            return Err(FetchError::new(FetchErrorKind::RateLimited, format!("at most {MAX_PER_SECOND} requests a second")));
        }
        self.started.push_back(now);
        self.in_flight += 1;
        Ok(())
    }
}

fn denied(message: impl Into<String>) -> FetchError {
    FetchError::new(FetchErrorKind::CapabilityDenied, message)
}

/// `url`, if the grant lets a widget reach it: `https:`, a granted host, not an address on this
/// machine or its networks.
fn check_url(url: &str, grants: &Grants) -> Result<Url, FetchError> {
    let parsed = Url::parse(url).map_err(|e| denied(format!("not a URL: {url} ({e})")))?;
    if parsed.scheme() != "https" {
        return Err(denied(format!("only https: URLs may be fetched, not {}:", parsed.scheme())));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(denied("URLs with credentials may not be fetched"));
    }
    let host = match parsed.host() {
        Some(Host::Domain(d)) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") {
                return Err(denied(format!("{d} is on this machine or its network")));
            }
            d
        }
        Some(Host::Ipv4(ip)) => {
            if !public(&IpAddr::V4(ip)) {
                return Err(denied(format!("{ip} is a private address")));
            }
            ip.to_string()
        }
        Some(Host::Ipv6(ip)) => {
            if !public(&IpAddr::V6(ip)) {
                return Err(denied(format!("{ip} is a private address")));
            }
            ip.to_string()
        }
        None => return Err(denied("the URL has no host")),
    };
    if grants.https.is_none() {
        return Err(denied("this widget has no https capability"));
    }
    if !grants.allows(&host) {
        return Err(denied(format!("{host} isn't among the hosts this widget may reach")));
    }
    Ok(parsed)
}

/// Whether `ip` is on the public internet: not loopback, private, link-local, unique-local,
/// shared (CGNAT), unspecified, multicast or documentation space.
pub fn public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_multicast() || v.is_documentation() || o[0] == 0 || (o[0] == 100 && (64..128).contains(&o[1])) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || o[0] >= 240)
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return public(&IpAddr::V4(v4));
            }
            let s = v.segments();
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || s[0] == 0x2001 && s[1] == 0x0db8)
        }
    }
}

/// The program's request (`{ url, method, headers, body? }`), checked.
pub fn request(v: &Value, grants: &Grants) -> Result<Request, FetchError> {
    let url = v["url"].as_str().ok_or_else(|| denied("a request needs a url"))?;
    let method = v["method"].as_str().unwrap_or("GET").to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return Err(denied(format!("{method} requests aren't allowed")));
    }
    let url = check_url(url, grants)?;
    let headers: Vec<(String, String)> = v["headers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())))
        .filter(|(k, _)| !FORBIDDEN.contains(&k.as_str()) && !k.starts_with("proxy-") && !k.starts_with("sec-"))
        .collect();
    let body = v["body"].as_str().map(str::to_owned);
    if body.as_ref().is_some_and(|b| b.len() > MAX_REQUEST) {
        return Err(FetchError::new(FetchErrorKind::TooLarge, format!("a request body may be at most {} KB", MAX_REQUEST / 1024)));
    }
    Ok(Request { url: url.into(), method, headers, body })
}

/// Sends `request` through `transport`, following up to [`MAX_REDIRECTS`] redirects that stay
/// within the grant, all within [`TIMEOUT`]: the response as the program gets it.
pub fn run(transport: &dyn Transport, mut request: Request, grants: &Grants) -> Result<Value, FetchError> {
    let deadline = Instant::now() + TIMEOUT;
    for _ in 0..=MAX_REDIRECTS {
        let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or_else(|| FetchError::new(FetchErrorKind::Timeout, "the request took longer than 15 s"))?;
        let response = transport.send(&request, left)?;
        if response.body.len() > MAX_RESPONSE {
            return Err(FetchError::new(FetchErrorKind::TooLarge, format!("a response may be at most {} MB", MAX_RESPONSE / 1024 / 1024)));
        }
        let location = response.headers.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone());
        match (response.status, location) {
            (301 | 302 | 303 | 307 | 308, Some(location)) => {
                let base = Url::parse(&request.url).map_err(|e| FetchError::new(FetchErrorKind::Network, e.to_string()))?;
                let next = base.join(&location).map_err(|e| FetchError::new(FetchErrorKind::Network, format!("a bad redirect: {e}")))?;
                let next = check_url(next.as_str(), grants)?;
                // A 303, and a 301 or 302 after a POST, go on as a GET without a body, as fetch does.
                if response.status == 303 || (matches!(response.status, 301 | 302) && request.method == "POST") {
                    request.method = "GET".into();
                    request.body = None;
                    request.headers.retain(|(k, _)| !k.starts_with("content-"));
                }
                // Credentials the template set stay with the host they were meant for.
                if next.host_str() != base.host_str() {
                    request.headers.retain(|(k, _)| k != "authorization");
                }
                request.url = next.into();
            }
            _ => return Ok(answer(response)),
        }
    }
    Err(FetchError::new(FetchErrorKind::Network, format!("more than {MAX_REDIRECTS} redirects")))
}

fn answer(r: Response) -> Value {
    let mut headers = Map::new();
    for (k, v) in r.headers {
        match headers.get_mut(&k) {
            Some(Value::String(prev)) => {
                prev.push_str(", ");
                prev.push_str(&v);
            }
            _ => {
                headers.insert(k, Value::String(v));
            }
        }
    }
    match String::from_utf8(r.body) {
        Ok(text) => json!({ "status": r.status, "headers": headers, "body": text }),
        Err(e) => json!({ "status": r.status, "headers": headers, "body64": base64::engine::general_purpose::STANDARD.encode(e.as_bytes()) }),
    }
}
