//! Pure server address and pairing-link parsers (Desktop's `servers/connect/pairing.ts`,
//! `normalizeServerUrl`): what the Add server / Edit server form accepts, without network I/O.

use serde_json::Value;

/// `normalizeServerUrl`: trimmed, `http://` when it names no scheme, no trailing slashes.
pub fn normalize(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    Some(with_scheme.trim_end_matches('/').to_owned())
}

/// The parts of an `http(s)` URL this form looks at.
struct Parsed {
    scheme: String,
    /// `host[:port]`, as written.
    authority: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: bool,
}

fn parse(value: &str) -> Option<Parsed> {
    if value.contains('#') || value.chars().any(char::is_whitespace) {
        return None;
    }
    let uri: http::Uri = value.parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = uri.authority()?;
    Some(Parsed {
        scheme,
        authority: authority.as_str().to_owned(),
        host: authority.host().to_ascii_lowercase(),
        port: authority.port_u16(),
        path: uri.path().to_owned(),
        query: uri.query().is_some(),
    })
}

/// `serverAddress`: an `http(s)` address without credentials, query or fragment, normalized.
pub fn server_address(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.contains("://") && !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return None;
    }
    let normalized = normalize(value)?;
    let url = parse(&normalized)?;
    if url.authority.contains('@') || url.query {
        return None;
    }
    Some(normalized)
}

/// `bareServerAddress`: a server address typed where a pairing link was expected. A host with no
/// path, so a bare code or a cut-off link is not mistaken for a server.
pub fn bare_server_address(value: &str) -> Option<String> {
    let address = server_address(value)?;
    let url = parse(&address)?;
    if !(url.path.is_empty() || url.path == "/") {
        return None;
    }
    if url.host != "localhost" && !url.host.contains('.') && !url.host.starts_with('[') && url.port.is_none() {
        return None;
    }
    Some(address)
}

/// A pairing link's code and every address it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub urls: Vec<String>,
    pub code: String,
    pub name: Option<String>,
}

fn valid_code(code: &str) -> bool {
    !code.is_empty() && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `pairingLink`: `opencode pair`'s `<server>/auth/connect/<code>` link, or its QR code's
/// `{"code","urls"}` JSON.
pub fn pairing_link(value: &str) -> Option<Link> {
    let trimmed = value.trim();
    if trimmed.starts_with('{') {
        let v: Value = serde_json::from_str(trimmed).ok()?;
        let code = v["code"].as_str()?;
        let list = v["urls"].as_array()?;
        if !valid_code(code) || list.iter().any(|u| !u.is_string()) {
            return None;
        }
        let mut seen: Vec<&str> = Vec::new();
        for u in list.iter().filter_map(Value::as_str) {
            if !seen.contains(&u) {
                seen.push(u);
            }
        }
        let urls: Vec<String> = seen.iter().map(|u| server_address(u)).collect::<Option<_>>()?;
        if urls.is_empty() {
            return None;
        }
        let name = match v.get("name") {
            None => None,
            Some(name) => Some(name.as_str()?.to_owned()),
        };
        return Some(Link { urls, code: code.to_owned(), name });
    }
    let url = parse(trimmed)?;
    let code = url.path.strip_prefix("/auth/connect/").filter(|c| !c.contains('/'))?;
    let address = server_address(&format!("{}://{}", url.scheme, url.authority))?;
    valid_code(code).then(|| Link { urls: vec![address], code: code.to_owned(), name: None })
}

/// `legacyPairingLink`: servers before one-time links paired with `/connect#<credentials>` or
/// `/connect?data=<credentials>`.
pub fn legacy_pairing_link(value: &str) -> bool {
    let trimmed = value.trim();
    let (base, hash) = trimmed.split_once('#').map_or((trimmed, ""), |(b, h)| (b, h));
    let Some(url) = parse(base) else { return false };
    let query = base.split_once('?').map_or("", |(_, q)| q);
    url.path == "/connect" && (!hash.is_empty() || query.split('&').any(|kv| kv == "data" || kv.starts_with("data=")))
}
