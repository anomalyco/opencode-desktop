//! Cookie export format parsers (Netscape `cookies.txt`, Chrome cookies API / Cookie-Editor JSON,
//! Playwright storage state), Chromium timestamps, and cookie URL helpers (`profile/cookie.ts`).

use serde::Deserialize;

use super::CookieRead;
use crate::protocol::{CookieSameSite, WireCookie};

/// Seconds between 1601-01-01 (Chromium's / Windows' epoch) and 1970-01-01.
const WINDOWS_EPOCH_OFFSET_SECS: f64 = 11_644_473_600.0;

/// Converts a Chromium timestamp (microseconds since 1601-01-01) to Unix seconds (`None` for 0).
pub fn chromium_time(micros: i64) -> Option<f64> {
    if micros == 0 {
        return None;
    }
    Some((micros as f64) / 1_000_000.0 - WINDOWS_EPOCH_OFFSET_SECS)
}

/// Returns the request URL a cookie binds to (IPv6 bracketed, leading dot stripped).
pub fn cookie_url(host: &str, path: &str, secure: bool) -> String {
    let bare = host.trim_start_matches('.');
    let bracketed = if bare.contains(':') && !bare.starts_with('[') {
        format!("[{bare}]")
    } else {
        bare.to_owned()
    };
    let scheme = if secure { "https" } else { "http" };
    let norm_path = if path.starts_with('/') { path } else { "/" };
    format!("{scheme}://{bracketed}{norm_path}")
}

/// A cookie host a browser would store: a plain hostname (letters, digits, `-`, `_`, dot-separated
/// labels of at most 63, at most 253 in all) or an IPv4 / IPv6 literal (bracketed or not), with at
/// most one leading dot. Nothing that would change the meaning of the URL a cookie is set through
/// (`user@host`, `?`, `#`, `%`, a port, a stray `:`, controls, spaces) passes.
pub fn valid_cookie_host(host: &str) -> bool {
    let bare = host.strip_prefix('.').unwrap_or(host);
    if bare.is_empty() || bare.len() > 253 {
        return false;
    }
    if let Some(inner) = bare.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return inner.parse::<std::net::Ipv6Addr>().is_ok();
    }
    if bare.contains(':') {
        return bare.parse::<std::net::Ipv6Addr>().is_ok();
    }
    bare.split('.').all(|label| {
        !label.is_empty() && label.len() <= 63 && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Filters out cookies no browser would send again (expired, nameless + empty, or a host that is
/// not `valid_cookie_host`) before sending to the engine, returning `(valid, skipped_count)`.
pub fn prepare_for_import(cookies: &[WireCookie], now_secs: f64) -> (Vec<WireCookie>, u32) {
    let mut valid = Vec::with_capacity(cookies.len());
    let mut skipped = 0u32;
    for c in cookies {
        let bare = c.host.trim_start_matches('.');
        if !valid_cookie_host(&c.host) || (c.name.is_empty() && c.value.is_empty()) {
            skipped = skipped.saturating_add(1);
            continue;
        }
        if let Some(exp) = c.expires
            && (!exp.is_finite() || exp <= now_secs)
        {
            skipped = skipped.saturating_add(1);
            continue;
        }
        let host = if c.name.starts_with("__Host-") {
            bare.to_owned()
        } else {
            c.host.clone()
        };
        let path = if c.path.starts_with('/') {
            c.path.clone()
        } else {
            "/".to_owned()
        };
        valid.push(WireCookie {
            host,
            name: c.name.clone(),
            value: c.value.clone(),
            path,
            secure: c.secure,
            http_only: c.http_only,
            expires: c.expires,
            same_site: c.same_site,
        });
    }
    (valid, skipped)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonCookie {
    domain: String,
    name: String,
    value: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    secure: Option<bool>,
    #[serde(default)]
    http_only: Option<bool>,
    #[serde(default)]
    host_only: Option<bool>,
    #[serde(default)]
    session: Option<bool>,
    #[serde(default)]
    expiration_date: Option<f64>,
    #[serde(default)]
    expires: Option<f64>,
    #[serde(default)]
    same_site: Option<String>,
}

/// Playwright's storage state: `{ "cookies": [...] }`.
#[derive(Deserialize)]
struct WrappedCookies {
    cookies: Vec<JsonCookie>,
}

/// Parses an exported cookie file: Chrome/Cookie-Editor/EditThisCookie JSON array, Playwright
/// storage state `{ cookies: [...] }`, or Netscape `cookies.txt` (including `#HttpOnly_`).
/// Returns `None` if the text matches none of those formats.
pub fn parse_cookie_file(text: &str) -> Option<CookieRead> {
    let trimmed = text.trim();
    // The first character says which JSON shape it is, so the text is parsed once, straight into
    // cookies (an untagged enum would first buffer the whole document as a generic tree).
    let json = match trimmed.as_bytes().first() {
        Some(b'[') => serde_json::from_str::<Vec<JsonCookie>>(trimmed).ok(),
        Some(b'{') => serde_json::from_str::<WrappedCookies>(trimmed).ok().map(|w| w.cookies),
        _ => None,
    };
    if let Some(list) = json {
        let cookies = list.into_iter().map(from_json).collect();
        return Some(CookieRead {
            cookies,
            protected: 0,
            failed: 0,
            skipped: 0,
        });
    }

    let mut cookies = Vec::new();
    let mut failed = 0u32;
    for line in text.lines() {
        match netscape_row(line) {
            NetscapeLine::Skip => {}
            NetscapeLine::Cookie(c) => cookies.push(c),
            NetscapeLine::Invalid => failed = failed.saturating_add(1),
        }
    }
    if cookies.is_empty() {
        return None;
    }
    Some(CookieRead {
        cookies,
        protected: 0,
        failed,
        skipped: 0,
    })
}

fn from_json(c: JsonCookie) -> WireCookie {
    let bare = c.domain.trim_start_matches('.');
    let host = match c.host_only {
        Some(true) => bare.to_owned(),
        Some(false) => format!(".{bare}"),
        None => c.domain,
    };
    let seconds = c
        .expiration_date
        .or_else(|| c.expires.filter(|&v| v.is_finite() && v > 0.0));
    let expires = if c.session == Some(true) { None } else { seconds };
    WireCookie {
        host,
        name: c.name,
        value: c.value,
        path: c.path.unwrap_or_else(|| "/".to_owned()),
        secure: c.secure.unwrap_or(false),
        http_only: c.http_only.unwrap_or(false),
        expires,
        same_site: parse_same_site(c.same_site.as_deref()),
    }
}

fn parse_same_site(value: Option<&str>) -> CookieSameSite {
    let Some(s) = value else {
        return CookieSameSite::Unspecified;
    };
    if s.eq_ignore_ascii_case("lax") {
        CookieSameSite::Lax
    } else if s.eq_ignore_ascii_case("strict") {
        CookieSameSite::Strict
    } else if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("no_restriction") {
        CookieSameSite::NoRestriction
    } else {
        CookieSameSite::Unspecified
    }
}

enum NetscapeLine {
    Skip,
    Cookie(WireCookie),
    Invalid,
}

fn netscape_row(line: &str) -> NetscapeLine {
    let (http_only, body) = match line.strip_prefix("#HttpOnly_") {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    if body.trim().is_empty() || body.starts_with('#') {
        return NetscapeLine::Skip;
    }
    let fields: Vec<&str> = body.split('\t').collect();
    if fields.len() < 7 {
        return NetscapeLine::Invalid;
    }
    let domain = fields[0];
    let subdomains = fields[1];
    let path = fields[2];
    let secure = fields[3];
    let expiry = fields[4];
    let name = fields[5];
    let value = fields[6..].join("\t");
    let bare = domain.trim_start_matches('.');
    let host = if subdomains == "TRUE" {
        format!(".{bare}")
    } else {
        bare.to_owned()
    };
    let expires = expiry
        .parse::<f64>()
        .ok()
        .filter(|&v| v.is_finite() && v > 0.0);
    NetscapeLine::Cookie(WireCookie {
        host,
        name: name.to_owned(),
        value,
        path: if path.is_empty() { "/".to_owned() } else { path.to_owned() },
        secure: secure == "TRUE",
        http_only,
        expires,
        same_site: CookieSameSite::Unspecified,
    })
}
