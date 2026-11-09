//! The servers the app knows, as plain values (`runtime/server/registry.tsx`, `persistence.ts`):
//! each [`ServerConn`]'s key, endpoint and credentials, which of them is this computer's, the
//! names lists show, and how the provided and saved lists merge into the ones Home shows.

use serde_json::{Value, json};

use super::key::{ServerKey, normalize_server_url};
use super::plugin_server::PluginStatus;

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

/// One server (`ServerConnection.Any`, `registry.tsx:151-188`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ServerConn {
    /// The app's own local service (`ServerConnection.Sidecar`, `variant: "base"`).
    Sidecar { http: HttpBase, display_name: Option<String>, label: Option<String> },
    /// A server added by address (`ServerConnection.Http`, `persistence.ts:11-17`).
    Http { http: HttpBase, auth_token: Option<bool>, display_name: Option<String>, label: Option<String> },
    /// A server a plugin contributes (`ServerConnection.Plugin`, `registry.tsx:168-183`).
    Plugin { key: ServerKey, plugin: String, status: PluginStatus, managed: bool, display_name: Option<String>, label: Option<String> },
}

impl ServerConn {
    pub fn http_server(url: &str) -> Option<ServerConn> {
        Some(ServerConn::Http { http: HttpBase::new(normalize_server_url(url)?), auth_token: None, display_name: None, label: None })
    }

    /// `ServerConnection.key` (`registry.tsx:190-199`).
    pub fn key(&self) -> ServerKey {
        match self {
            ServerConn::Sidecar { .. } => ServerKey::sidecar(),
            ServerConn::Http { http, .. } => ServerKey::new(&http.url),
            ServerConn::Plugin { key, .. } => key.clone(),
        }
    }

    /// `ServerConnection.builtin` (`registry.tsx:204`).
    pub fn builtin(&self) -> bool {
        matches!(self, ServerConn::Sidecar { .. })
    }

    /// The endpoint to probe, show or call; None while a contributed server isn't ready
    /// (`ServerConnection.http`, `registry.tsx:209-214`).
    pub fn http(&self) -> Option<&HttpBase> {
        match self {
            ServerConn::Sidecar { http, .. } | ServerConn::Http { http, .. } => Some(http),
            ServerConn::Plugin { status: PluginStatus::Ready { http }, .. } => Some(http),
            ServerConn::Plugin { .. } => None,
        }
    }

    pub fn display_name(&self) -> Option<&str> {
        match self {
            ServerConn::Sidecar { display_name, .. } | ServerConn::Http { display_name, .. } | ServerConn::Plugin { display_name, .. } => display_name.as_deref(),
        }
    }

    /// The badge a row shows beside the name (`WSL`, `SSH`).
    pub fn label(&self) -> Option<&str> {
        match self {
            ServerConn::Sidecar { label, .. } | ServerConn::Http { label, .. } | ServerConn::Plugin { label, .. } => label.as_deref(),
        }
    }

    /// The plugin re-resolves the endpoint (a tunnel), so the connection may drop and return.
    pub fn managed(&self) -> bool {
        matches!(self, ServerConn::Plugin { managed: true, .. })
    }

    /// `ServerConnection.local(conn, ownUrl)` (`registry.tsx:216-227`): the sidecar, or an HTTP
    /// server on the same loopback port as the app's own. Other loopback ports may be a WSL
    /// server, a tunnel or a container with its own filesystem, so they aren't.
    pub fn local(&self, own_url: Option<&str>) -> bool {
        if self.builtin() {
            return true;
        }
        let (ServerConn::Http { http, .. }, Some(own)) = (self, own_url) else { return false };
        loopback_origin(&http.url).is_some_and(|origin| Some(origin) == loopback_origin(own))
    }

    /// `serverCredentials` (`runtime/server/machine.ts:96-103`).
    pub fn credentials(&self) -> Credentials {
        match self {
            ServerConn::Plugin { status, .. } => Credentials { controller: String::new(), auth: status.http().map(HttpBase::credentials) },
            ServerConn::Http { http, .. } => Credentials { controller: http.credentials(), auth: Some(http.credentials()) },
            ServerConn::Sidecar { http, .. } => Credentials { controller: String::new(), auth: Some(http.credentials()) },
        }
    }
}

/// What a server's controller is bound to (`ServerCredentials`, `machine.ts:4-13`). An HTTP
/// controller keeps the endpoint it started with (`controller` is its credentials), so new
/// credentials need a new one; the sidecar and contributed servers move their transport in place
/// (`controller` is empty). `auth` is what health checks probe; None while a plugin server isn't
/// ready.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Credentials {
    pub controller: String,
    pub auth: Option<String>,
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

/// `serverName` (`registry.tsx:21-27`): the custom name, else the address without its scheme.
pub fn server_name(conn: &ServerConn, ignore_display_name: bool) -> String {
    if let Some(name) = conn.display_name().filter(|_| !ignore_display_name) {
        return name.to_owned();
    }
    let url = conn.http().map(|h| h.url.as_str()).unwrap_or("");
    url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_owned()
}

/// What switchers and titles call a server (`serverLabel`, `registry.tsx:29-39`, and
/// `servers.label`, `:325-331`): its custom name; for the app's own server, the hostname it last
/// reported or `this_computer`; for others, the address.
pub fn server_label(conn: &ServerConn, current: bool, hostname: Option<&str>, this_computer: &str) -> String {
    if let Some(name) = conn.display_name() {
        return name.to_owned();
    }
    if current {
        return hostname.filter(|h| !h.is_empty()).unwrap_or(this_computer).to_owned();
    }
    server_name(conn, false)
}

/// The name a list row shows beside its item: none for the app's own server or when it is the
/// only one (`servers.tag`, `registry.tsx:356-359`).
pub fn server_tag(all: usize, current: bool, label: impl FnOnce() -> String) -> Option<String> {
    (all >= 2 && !current).then(label)
}

/// "This computer" beside the app's own server, unless its name already says it
/// (`servers.hint`, `registry.tsx:360-365`).
pub fn server_hint(current: bool, label: &str, this_computer: &str) -> Option<String> {
    (current && label != this_computer).then(|| this_computer.to_owned())
}

/// `resolveServerList` (`registry.tsx:101-124`): the provided servers (the sidecar, then the
/// plugins'), each overlaid by a saved server with its key, then the other saved servers.
pub fn resolve_server_list(provided: &[ServerConn], stored: &[ServerConn]) -> Vec<ServerConn> {
    let mut list: Vec<ServerConn> = Vec::new();
    for conn in provided {
        match list.iter_mut().find(|c| c.key() == conn.key()) {
            Some(slot) => *slot = conn.clone(),
            None => list.push(conn.clone()),
        }
    }
    for conn in stored {
        match list.iter_mut().find(|c| c.key() == conn.key()) {
            Some(slot) => *slot = overlay(slot, conn),
            None => list.push(conn.clone()),
        }
    }
    list
}

/// `{ ...existing, ...conn, http: { ...http(existing), ...conn.http } }`: the saved server's
/// fields win, a missing password keeps the provided one.
fn overlay(existing: &ServerConn, stored: &ServerConn) -> ServerConn {
    let ServerConn::Http { http, auth_token, display_name, label } = stored else { return stored.clone() };
    let base = existing.http().cloned().unwrap_or_default();
    let http = HttpBase { url: http.url.clone(), password: http.password.clone().or(base.password) };
    let display_name = display_name.clone().or_else(|| existing.display_name().map(str::to_owned));
    let label = label.clone().or_else(|| existing.label().map(str::to_owned));
    ServerConn::Http { http, auth_token: *auth_token, display_name, label }
}

/// `canRemoveServer` (`registry.tsx:126-134`): only a saved HTTP server no source provides.
pub fn can_remove_server(key: &ServerKey, provided: &[ServerConn], stored: &[ServerConn]) -> bool {
    if provided.iter().any(|c| c.key() == *key) {
        return false;
    }
    stored.iter().any(|c| matches!(c, ServerConn::Http { http, .. } if http.url == key.as_str()))
}

/// `resolveVisibleServers` (`registry.tsx:136-149`): the servers not hidden; when every one is,
/// the built-in (else the first) shows anyway, as Home always lists one.
pub fn resolve_visible_servers(servers: &[ServerConn], hidden: impl Fn(&ServerKey) -> bool) -> Vec<ServerConn> {
    let visible: Vec<ServerConn> = servers.iter().filter(|c| !hidden(&c.key())).cloned().collect();
    if !visible.is_empty() || servers.is_empty() {
        return visible;
    }
    vec![servers.iter().find(|c| c.builtin()).unwrap_or(&servers[0]).clone()]
}

/// A saved HTTP server as `server.list` keeps it (`ServerHttp`, `persistence.ts:11-30`): a bare
/// address and a bare `{ url, password }` from older builds read as one too.
pub fn http_from_json(v: &Value) -> Option<ServerConn> {
    if let Some(url) = v.as_str() {
        return ServerConn::http_server(url);
    }
    let text = |v: &Value| v.as_str().map(str::to_owned);
    let http = if v.get("http").is_some() { &v["http"] } else { v };
    let url = normalize_server_url(http["url"].as_str()?)?;
    let http = HttpBase { url, password: text(&http["password"]) };
    Some(ServerConn::Http { http, auth_token: v["authToken"].as_bool(), display_name: text(&v["displayName"]), label: text(&v["label"]) })
}

/// The inverse of [`http_from_json`] for an HTTP server; None for the others, which no file keeps.
pub fn http_to_json(conn: &ServerConn) -> Option<Value> {
    let ServerConn::Http { http, auth_token, display_name, label } = conn else { return None };
    let mut base = json!({ "url": http.url });
    if let Some(p) = &http.password {
        base["password"] = json!(p);
    }
    let mut v = json!({ "type": "http", "http": base });
    if let Some(t) = auth_token {
        v["authToken"] = json!(t);
    }
    if let Some(n) = display_name {
        v["displayName"] = json!(n);
    }
    if let Some(l) = label {
        v["label"] = json!(l);
    }
    Some(v)
}
