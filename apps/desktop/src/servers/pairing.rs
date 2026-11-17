//! Server addresses and pairing links (Desktop's `servers/connect/pairing.ts`,
//! `normalizeServerUrl`): what the Add server form accepts, and redeeming a link's single-use
//! code for the token that signs in on every address of its server.

use std::time::Duration;

use serde_json::Value;

/// `normalizeServerUrl`: trimmed, `http://` when it names no scheme, no trailing slashes.
pub fn normalize(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.starts_with("http://") || trimmed.starts_with("https://") { trimmed.to_owned() } else { format!("http://{trimmed}") };
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
    let uri: ureq::http::Uri = value.parse().ok()?;
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
        let name = match v.get("name") { None => None, Some(name) => Some(name.as_str()?.to_owned()) };
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

/// A redeemed pairing: the address it reached and the token that signs in there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing {
    pub url: String,
    pub password: String,
}

/// What redeeming a link came to.
pub enum Redeemed {
    Paired(Pairing),
    /// A server answered that the code is spent.
    Expired,
    /// No address answered.
    Unreachable,
}

/// `redeemPairingLink`: tries every address at once, 5 s each. The code is single-use, so at most
/// one succeeds; the first address (the one the user chose in Pairing) wins when it answers within
/// a second of another, as a local address stops working away from its network. Blocks: run it
/// off the app's thread. A page has no threads: it tries the addresses one after another, the
/// chosen one first.
pub fn redeem(link: &Link) -> Redeemed {
    #[cfg(target_arch = "wasm32")]
    return redeem_in_turn(link);
    #[cfg(not(target_arch = "wasm32"))]
    redeem_at_once(link)
}

#[cfg(target_arch = "wasm32")]
fn redeem_in_turn(link: &Link) -> Redeemed {
    let mut expired = false;
    for url in &link.urls {
        match connect(url, &link.code) {
            Ok(token) => return Redeemed::Paired(Pairing { url: url.clone(), password: token }),
            Err(true) => expired = true,
            Err(false) => {}
        }
    }
    if expired { Redeemed::Expired } else { Redeemed::Unreachable }
}

#[cfg(not(target_arch = "wasm32"))]
fn redeem_at_once(link: &Link) -> Redeemed {
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Result<String, bool>)>();
    for (i, url) in link.urls.iter().enumerate() {
        let (tx, url, code) = (tx.clone(), url.clone(), link.code.clone());
        std::thread::spawn(move || {
            let _ = tx.send((i, connect(&url, &code)));
        });
    }
    drop(tx);
    let mut paired: Option<(usize, String)> = None;
    let mut expired = false;
    let mut first_answered: Option<bool> = None;
    let mut deadline: Option<rune::time::Instant> = None;
    loop {
        let next = match deadline {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(rune::time::Instant::now())).ok(),
            None => rx.recv().ok(),
        };
        let Some((i, result)) = next else { break };
        if i == 0 {
            first_answered = Some(matches!(result, Ok(_) | Err(true)));
        }
        match result {
            Ok(token) if paired.is_none() => {
                paired = Some((i, token));
                if i == 0 {
                    break;
                }
                deadline = Some(rune::time::Instant::now() + Duration::from_secs(1));
            }
            Err(true) => expired = true,
            _ => {}
        }
        if paired.is_some() && first_answered.is_some() {
            break;
        }
    }
    let Some((i, token)) = paired else { return if expired { Redeemed::Expired } else { Redeemed::Unreachable } };
    // The token works on every address: keep the first one when it answers.
    if i != 0 && first_answered == Some(true) {
        let first = Pairing { url: link.urls[0].clone(), password: token.clone() };
        if crate::servers::check(&first.url, Some(&first.password), Duration::from_secs(5)) == crate::servers::Health::Healthy {
            return Redeemed::Paired(first);
        }
    }
    Redeemed::Paired(Pairing { url: link.urls[i].clone(), password: token })
}

/// `GET /auth/connect/<code>` on `url`: the session token, or whether the server refused it (401).
fn connect(url: &str, code: &str) -> Result<String, bool> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(5))).http_status_as_error(false).build().into();
    let mut resp = agent.get(format!("{url}/auth/connect/{code}")).call().map_err(|_| false)?;
    if resp.status().as_u16() == 401 {
        return Err(true);
    }
    if !resp.status().is_success() {
        return Err(false);
    }
    let v: Value = resp.body_mut().read_to_string().ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or(false)?;
    v["token"].as_str().map(str::to_owned).ok_or(false)
}
