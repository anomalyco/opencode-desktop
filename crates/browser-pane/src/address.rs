//! Address bar input resolution matching OpenCode Desktop (`packages/gui-plugins/src/browser/address.ts`).

/// Turns user-typed address bar input into a navigable URL.
///
/// - Full schemes (`http://`, `https://`, `about:`, `data:`, `file://`) stay intact.
/// - Port shorthands (`:3000`, `:5173/app`) become `http://localhost:<port>...`.
/// - Loopback hosts (`localhost`, `127.0.0.1`, `[::1]`, `*.localhost`) default to `http://`.
/// - Bare domains (`github.com/anomalyco/rune`) default to `https://`.
/// - Multi-word or plain queries resolve to a web search URL.
pub fn resolve_address(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "about:blank".into();
    }
    let lower = trimmed.to_ascii_lowercase();
    for scheme in ["http://", "https://", "about:", "data:", "file://"] {
        if lower.starts_with(scheme) {
            return trimmed.to_owned();
        }
    }
    if trimmed.starts_with(':')
        && trimmed[1..].chars().take_while(|c| *c != '/').all(|c| c.is_ascii_digit())
        && trimmed.len() > 1
    {
        return format!("http://localhost{trimmed}");
    }
    if trimmed.contains(char::is_whitespace) {
        return search_url(trimmed);
    }
    let host_port = trimmed.split(['/', '?', '#']).next().unwrap_or(trimmed);
    let host = if host_port.starts_with('[') {
        host_port.split(']').next().map_or(host_port, |h| &host_port[..=h.len()])
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    let host_lower = host.to_ascii_lowercase();
    if host_lower == "localhost"
        || host_lower == "127.0.0.1"
        || host_lower == "[::1]"
        || host_lower.ends_with(".localhost")
    {
        return format!("http://{trimmed}");
    }
    if looks_like_domain(&host_lower) || host_port.contains(':') {
        return format!("https://{trimmed}");
    }
    search_url(trimmed)
}

fn looks_like_domain(host: &str) -> bool {
    let Some((_, tld)) = host.rsplit_once('.') else {
        return false;
    };
    !tld.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && tld.chars().all(|c| c.is_ascii_alphabetic() || c.is_ascii_digit())
}

fn search_url(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len() * 2);
    for b in query.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(char::from_digit(u32::from(b >> 4), 16).unwrap().to_ascii_uppercase());
                encoded.push(char::from_digit(u32::from(b & 0xf), 16).unwrap().to_ascii_uppercase());
            }
        }
    }
    format!("https://www.google.com/search?q={encoded}")
}
