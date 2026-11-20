//! Strict bounded HTTP/1.1 request-head parser, anti-smuggling validator, `CONNECT` authority
//! parser, and forwarded-header sanitizer for [`super::tunnel_proxy::TunnelProxy`].

use std::io::{BufReader, Read};
use std::net::TcpStream;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 128;

pub(super) struct ParsedHead {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl ParsedHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn has_header(&self, name: &str) -> bool {
        self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(name))
    }
}

pub(super) struct ProxyUrl {
    pub scheme: String,
    pub host: String,
    pub host_header: String,
    pub port: u16,
    pub path_and_query: String,
}

/// Reads an HTTP/1.x request head strictly bounded to `MAX_HEADER_BYTES` (never calls unbounded
/// `read_line`), enforcing exact CRLF (`\r\n`) endings, RFC 9110 token syntax, and rejecting
/// bare `\r`/`\n`, control bytes, and obs-fold.
pub(super) fn read_http_head(reader: &mut BufReader<TcpStream>) -> Result<ParsedHead, &'static str> {
    let mut raw = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if raw.len() >= MAX_HEADER_BYTES {
            return Err("431 Request Header Fields Too Large");
        }
        match reader.read(&mut byte) {
            Ok(0) => return Err("400 Bad Request"),
            Ok(_) => raw.push(byte[0]),
            Err(_) => return Err("400 Bad Request"),
        }
        if raw.ends_with(b"\r\n\r\n") {
            break;
        }
        if byte[0] == b'\n' && (raw.len() < 2 || raw[raw.len() - 2] != b'\r') {
            return Err("400 Bad Request");
        }
    }
    let head = std::str::from_utf8(&raw[..raw.len() - 4]).map_err(|_| "400 Bad Request")?;
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or("400 Bad Request")?;
    if first.contains('\r') || first.contains('\n') || has_ctl(first, false) {
        return Err("400 Bad Request");
    }
    let mut parts = first.split(' ');
    let method = parts.next().filter(|m| is_http_token(m)).ok_or("400 Bad Request")?.to_owned();
    let target = parts.next().filter(|t| !t.is_empty()).ok_or("400 Bad Request")?.to_owned();
    let version = parts.next().ok_or("400 Bad Request")?;
    if parts.next().is_some() || !matches!(version, "HTTP/1.1" | "HTTP/1.0") {
        return Err("400 Bad Request");
    }

    let mut headers = Vec::new();
    for line in lines {
        if headers.len() >= MAX_HEADERS {
            return Err("431 Request Header Fields Too Large");
        }
        if line.is_empty()
            || line.starts_with([' ', '\t'])
            || line.contains('\r')
            || line.contains('\n')
        {
            return Err("400 Bad Request");
        }
        let (k, v) = line.split_once(':').ok_or("400 Bad Request")?;
        if !is_http_token(k) || has_ctl(v, true) {
            return Err("400 Bad Request");
        }
        headers.push((k.to_owned(), v.trim().to_owned()));
    }
    Ok(ParsedHead { method, target, headers })
}

pub(super) fn parse_content_length(req: &ParsedHead, is_connect: bool) -> Result<u64, &'static str> {
    let mut found: Option<u64> = None;
    for (k, v) in &req.headers {
        if !k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        if is_connect || v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
            return Err("400 Bad Request");
        }
        let n = v.parse::<u64>().map_err(|_| "400 Bad Request")?;
        if let Some(prev) = found
            && prev != n
        {
            return Err("400 Bad Request");
        }
        found = Some(n);
    }
    Ok(found.unwrap_or(0))
}

/// Parses a strict `CONNECT` authority (`host:port` or `[ipv6]:port`, explicit port `1..=65535`).
pub(super) fn parse_connect_authority(target: &str) -> Option<ProxyUrl> {
    if target.is_empty()
        || target.contains(['/', '?', '#', '@'])
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (host, port) = if let Some(after_open) = target.strip_prefix('[') {
        let close = after_open.find(']')?;
        let host = &after_open[..close];
        let port_str = after_open[close + 1..].strip_prefix(':')?;
        if !port_str.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        (host.to_owned(), port_str.parse::<u16>().ok()?)
    } else {
        let (h, p) = target.rsplit_once(':')?;
        if h.is_empty() || h.contains(':') || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        (h.to_owned(), p.parse::<u16>().ok()?)
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(ProxyUrl {
        scheme: "https".into(),
        host,
        host_header: target.to_owned(),
        port,
        path_and_query: String::new(),
    })
}

pub(super) fn parse_proxy_url(raw: &str) -> Option<ProxyUrl> {
    let (scheme_raw, rest) = raw.split_once("://")?;
    let scheme = scheme_raw.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" | "ws" => 80u16,
        "https" | "wss" => 443u16,
        _ => return None,
    };
    let rest_no_frag = rest.split('#').next().unwrap_or(rest);
    let (authority, path_and_query) = match rest_no_frag.find(['/', '?']) {
        Some(idx) => {
            let tail = &rest_no_frag[idx..];
            let pq = if tail.starts_with('?') { format!("/{tail}") } else { tail.to_owned() };
            (&rest_no_frag[..idx], pq)
        }
        None => (rest_no_frag, "/".to_owned()),
    };
    if authority.is_empty()
        || authority.contains('@')
        || authority.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let (host, port) = if let Some(after_open) = authority.strip_prefix('[') {
        let close = after_open.find(']')?;
        let host = &after_open[..close];
        let tail = &after_open[close + 1..];
        let port = if tail.is_empty() {
            default_port
        } else {
            tail.strip_prefix(':')?.parse::<u16>().ok()?
        };
        (host.to_owned(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        let port = p.parse::<u16>().ok()?;
        (h.to_owned(), port)
    } else {
        (authority.to_owned(), default_port)
    };
    if host.is_empty() || port == 0 {
        return None;
    }
    Some(ProxyUrl {
        scheme,
        host,
        host_header: authority.to_owned(),
        port,
        path_and_query,
    })
}

pub(super) fn build_forwarded_head(
    req: &ParsedHead,
    url: &ProxyUrl,
    is_ws: bool,
    content_length: u64,
) -> String {
    let mut hop_names: Vec<String> = req
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(',').map(|s| s.trim().to_ascii_lowercase()))
        .filter(|s| !s.is_empty())
        .collect();
    hop_names.extend(
        [
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "proxy-connection",
            "transfer-encoding",
            "content-length",
            "te",
            "trailer",
            "upgrade",
            "expect",
            "host",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut out = format!("{} {} HTTP/1.1\r\nHost: {}\r\n", req.method, url.path_and_query, url.host_header);
    if is_ws {
        out.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\n");
    } else {
        out.push_str("Connection: close\r\n");
        if content_length > 0 {
            out.push_str(&format!("Content-Length: {content_length}\r\n"));
        }
    }
    for (k, v) in &req.headers {
        let lower = k.to_ascii_lowercase();
        if hop_names.iter().any(|h| h == &lower) {
            continue;
        }
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

fn is_http_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
}

fn has_ctl(s: &str, allow_htab: bool) -> bool {
    s.bytes().any(|b| b == 0x7f || (b < 0x20 && !(allow_htab && b == b'\t')))
}
