//! The names a server and the things on it go by: [`ServerKey`] (Desktop's `ServerConnection.Key`,
//! `runtime/server/registry.tsx:190-202`), [`ServerScope`] (`runtime/server/scope.ts:23-31`), a
//! tab's [`TabKey`] (`shell/tabs/tabs.tsx:69-71`) and the routes and storage keys built from them
//! (`shell/routes/session.ts:5-15`, `scope.ts:33-69`).

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A server's identity, the same in every window and every file:
/// - `"sidecar"` for the app's own local service;
/// - an HTTP server's normalized address (`http://host:4096`, [`normalize_server_url`]);
/// - `"<plugin>:<id>"` for a server a plugin contributes (`wsl:Ubuntu`, `ssh:prod`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerKey(pub Arc<str>);

impl ServerKey {
    pub const SIDECAR: &'static str = "sidecar";

    pub fn new(key: impl AsRef<str>) -> ServerKey {
        ServerKey(Arc::from(key.as_ref()))
    }

    pub fn sidecar() -> ServerKey {
        ServerKey::new(Self::SIDECAR)
    }

    /// A contributed server's key, `<plugin>:<id>`.
    pub fn plugin(plugin: &str, id: &str) -> ServerKey {
        ServerKey::new(format!("{plugin}:{id}"))
    }

    pub fn is_sidecar(&self) -> bool {
        &*self.0 == Self::SIDECAR
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Where this server's client-side state lives (`ServerScope.fromServerKey`, `scope.ts:25-30`).
    pub fn scope(&self) -> ServerScope {
        ServerScope::from_key(self, None)
    }
}

impl fmt::Display for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ServerKey {
    fn from(s: &str) -> ServerKey {
        ServerKey::new(s)
    }
}

/// The namespace a server's client-side state is stored under: `"local"` for the app's own
/// server (and the web app's canonical one), the key itself for every other server
/// (`scope.ts:23-31`). Project use, drafts and side-panel state key by it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerScope(pub Arc<str>);

impl ServerScope {
    pub const LOCAL: &'static str = "local";

    pub fn local() -> ServerScope {
        ServerScope(Arc::from(Self::LOCAL))
    }

    /// `ServerScope.fromServerKey(key, canonicalLocalServer)` (`scope.ts:25-30`).
    pub fn from_key(key: &ServerKey, canonical_local: Option<&ServerKey>) -> ServerScope {
        if key.is_sidecar() || canonical_local == Some(key) {
            return ServerScope::local();
        }
        ServerScope(key.0.clone())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The separator of scoped storage keys (`scope.ts:11`).
pub const SCOPE_SEPARATOR: char = '\u{0}';

/// `SessionRouteKey.fromRoute(dir, sessionID)` (`scope.ts:33-37`): `dir`, then `/<session>`.
pub fn session_route_key(dir: Option<&str>, session_id: Option<&str>) -> String {
    let mut key = dir.unwrap_or_default().replace(SCOPE_SEPARATOR, "");
    if let Some(id) = session_id.filter(|id| !id.is_empty()) {
        key.push('/');
        key.push_str(id);
    }
    key
}

/// `SessionStateKey.from(scope, route)` (`scope.ts:39-45`): `<scope>\0<route>`, the key a
/// session's side tabs and scroll keep in the global layout.
pub fn session_state_key(scope: &ServerScope, route: &str) -> String {
    format!("{}{SCOPE_SEPARATOR}{route}", scope.as_str())
}

/// `ScopedKey.from(scope, ...parts)` (`scope.ts:62-65`).
pub fn scoped_key(scope: &ServerScope, parts: &[&str]) -> String {
    std::iter::once(scope.as_str()).chain(parts.iter().copied()).collect::<Vec<_>>().join(&SCOPE_SEPARATOR.to_string())
}

/// A tab's identity across every server a window shows (`tabKey`, `tabs.tsx:70-71`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TabKey {
    Session { server: ServerKey, session_id: String },
    Draft(String),
}

impl TabKey {
    pub fn session(server: ServerKey, session_id: impl Into<String>) -> TabKey {
        TabKey::Session { server, session_id: session_id.into() }
    }

    pub fn draft(id: impl Into<String>) -> TabKey {
        TabKey::Draft(id.into())
    }

    pub fn server(&self) -> Option<&ServerKey> {
        match self {
            TabKey::Session { server, .. } => Some(server),
            TabKey::Draft(_) => None,
        }
    }

    /// Desktop's key string: `draft:<id>`, or `<server>\n/server/<base64 server>/session/<id>`.
    pub fn encode(&self) -> String {
        match self {
            TabKey::Draft(id) => format!("draft:{id}"),
            TabKey::Session { server, session_id } => format!("{server}\n{}", session_href(server, session_id)),
        }
    }

    /// The inverse of [`TabKey::encode`]; None for a string no tab key has.
    pub fn parse(key: &str) -> Option<TabKey> {
        if let Some(id) = key.strip_prefix("draft:") {
            return Some(TabKey::Draft(id.to_owned()));
        }
        let (server, href) = key.split_once('\n')?;
        let route = Route::parse(href)?;
        match route {
            Route::Session { server: s, session_id } if s.as_str() == server => Some(TabKey::Session { server: s, session_id }),
            _ => None,
        }
    }
}

impl fmt::Display for TabKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encode())
    }
}

impl Serialize for TabKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.encode())
    }
}

impl<'de> Deserialize<'de> for TabKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<TabKey, D::Error> {
        let s = String::deserialize(d)?;
        TabKey::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("not a tab key: {s:?}")))
    }
}

/// A window's page as its last active URL keeps it (`LayoutRoute`, `shell/state/layout.tsx:72-78`,
/// `desktop/src/renderer/window/route-storage.ts`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Route {
    #[default]
    Home,
    /// Settings, on the page its `?tab=` names.
    Settings(Option<String>),
    Draft(String),
    Session { server: ServerKey, session_id: String },
}

impl Route {
    /// `/`, `/settings?tab=<page>`, `/new-session?draftId=<id>` or `/server/<base64>/session/<id>`.
    pub fn href(&self) -> String {
        match self {
            Route::Home => "/".to_owned(),
            Route::Settings(None) => "/settings".to_owned(),
            Route::Settings(Some(tab)) => format!("/settings?tab={}", crate::client::encode_component(tab)),
            Route::Draft(id) => draft_href(id),
            Route::Session { server, session_id } => session_href(server, session_id),
        }
    }

    /// The inverse of [`Route::href`]; a server segment that doesn't decode to itself is no
    /// route (`requireServerKey`, `routes/session.ts:9-15`).
    pub fn parse(href: &str) -> Option<Route> {
        let (path, query) = href.split_once('?').unwrap_or((href, ""));
        let param = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('=')).map(decode_component);
        match path {
            "" | "/" => return Some(Route::Home),
            "/settings" => return Some(Route::Settings(param("tab"))),
            "/new-session" => return Some(Route::Draft(param("draftId")?)),
            _ => {}
        }
        let rest = path.strip_prefix("/server/")?;
        let (segment, rest) = rest.split_once('/')?;
        let session_id = rest.strip_prefix("session/")?;
        let server = String::from_utf8(base64_url_decode(segment)?).ok()?;
        if session_id.is_empty() || session_id.contains('/') || base64_url_encode(server.as_bytes()) != segment {
            return None;
        }
        Some(Route::Session { server: ServerKey::new(server), session_id: session_id.to_owned() })
    }
}

/// `sessionHref(server, sessionID)` (`routes/session.ts:5-7`).
pub fn session_href(server: &ServerKey, session_id: &str) -> String {
    format!("/server/{}/session/{session_id}", base64_url_encode(server.as_str().as_bytes()))
}

/// `draftHref(draftID)` (`tabs.tsx:65`).
pub fn draft_href(draft_id: &str) -> String {
    format!("/new-session?draftId={}", crate::client::encode_component(draft_id))
}

/// `normalizeServerUrl` (`registry.tsx:12-19`): trimmed, `http://` when it names no scheme, and
/// no trailing `/`. None for an empty address.
pub fn normalize_server_url(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") { trimmed.to_owned() } else { format!("http://{trimmed}") };
    Some(with_scheme.trim_end_matches('/').to_owned())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `base64Encode` (`util/src/encode.ts:1-5`): URL-safe, unpadded.
pub fn base64_url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for c in input.chunks(3) {
        let n = u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..=c.len() {
            out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    out
}

/// `base64Decode` (`util/src/encode.ts:7-10`), accepting either alphabet and optional padding.
pub fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes = input.trim_end_matches('=').as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= digit(*c)? << (18 - 6 * i);
        }
        out.extend_from_slice(&n.to_be_bytes()[1..chunk.len()]);
    }
    Some(out)
}

/// `decodeURIComponent`, keeping a malformed escape as written.
fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%').then(|| s.get(i + 1..i + 3)).flatten().and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}
