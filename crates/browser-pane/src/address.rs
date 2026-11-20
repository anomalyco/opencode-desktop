//! Address bar input resolution and canonical host display matching OpenCode Desktop
//! (`packages/gui-plugins/src/browser/address.ts`, `host-display.ts`).

use crate::BrowserTab;

/// Maximum host characters named as text in prompts, bars, and lists (`host-display.ts` `HOST_MAX`).
pub const HOST_MAX: usize = 64;

/// Turns user-typed address bar input into a destination URL or Google search (`policy::resolve_address`).
pub fn resolve_address(raw: &str) -> String {
    crate::policy::resolve_address(raw)
}

/// A URL without its scheme or trailing slash, as tabs and address bars show it.
pub fn bare_url(url: &str) -> String {
    url.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_owned()
}

/// Just the host of `url` (`github.com`).
pub fn host_of(url: &str) -> String {
    bare_url(url).split('/').next().unwrap_or(url).to_owned()
}

/// Canonical ASCII/punycode authority of an `http:` or `https:` URL (port kept, credentials
/// rejected), left-elided behind `"…"` when longer than `max` characters so the registrable
/// right edge is never hidden (`host-display.ts::displayHost`).
pub fn display_host(url: &str, max: usize) -> String {
    let Some(parts) = host_display(url) else {
        return String::new();
    };
    elide_host_left(&parts.host, max)
}

/// Parts of a credential-free `http:` / `https:` address for the address bar (`host-display.ts::hostDisplay`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDisplay {
    pub scheme: String,
    pub host: String,
    pub rest: String,
}

/// Splits a credential-free `http:` / `https:` URL into scheme, canonical ASCII/punycode authority,
/// and path/query/fragment (`host-display.ts::hostDisplay`).
pub fn host_display(address: &str) -> Option<HostDisplay> {
    let trimmed = address.trim();
    let (scheme, after) = if trimmed.len() >= 8 && trimmed.as_bytes()[..8].eq_ignore_ascii_case(b"https://") {
        ("https://", &trimmed[8..])
    } else if trimmed.len() >= 7 && trimmed.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        ("http://", &trimmed[7..])
    } else {
        return None;
    };
    let split = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..split];
    let rest = &after[split..];
    if authority.is_empty() || authority.contains('@') || authority.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let host = canonical_authority(authority)?;
    Some(HostDisplay {
        scheme: scheme.to_owned(),
        host,
        rest: if rest.is_empty() { "/".to_owned() } else { rest.to_owned() },
    })
}

/// Keeps the rightmost `max - 1` characters behind `"…"` when `host` exceeds `max` Unicode scalar values.
pub fn elide_host_left(host: &str, max: usize) -> String {
    let len = host.chars().count();
    if max == 0 || len <= max {
        return host.to_owned();
    }
    let keep = max.saturating_sub(1);
    let tail: String = host.chars().skip(len - keep).collect();
    format!("…{tail}")
}

/// Lowercases ASCII host labels and encodes non-ASCII IDN labels to RFC 3492 punycode (`xn--...`),
/// preserving IPv6 literals and valid numeric ports.
fn canonical_authority(authority: &str) -> Option<String> {
    let (raw_host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let ipv6 = &authority[..=end + 1];
        let tail = &rest[end + 1..];
        let port = match tail {
            "" => None,
            t => {
                let p = t.strip_prefix(':')?;
                if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                Some(p)
            }
        };
        (ipv6, port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
            (h, Some(p))
        } else {
            (authority, None)
        }
    } else {
        (authority, None)
    };
    if raw_host.is_empty() {
        return None;
    }
    let host = if raw_host.starts_with('[') {
        raw_host.to_ascii_lowercase()
    } else {
        let mut out = Vec::new();
        for label in raw_host.split(['.', '\u{3002}', '\u{FF0E}', '\u{FF61}']) {
            if label.is_ascii() {
                out.push(label.to_ascii_lowercase());
            } else {
                let lowered: String = label.chars().flat_map(char::to_lowercase).collect();
                let encoded = encode_punycode(&lowered)?;
                out.push(format!("xn--{encoded}"));
            }
        }
        out.join(".")
    };
    match port {
        Some(p) => Some(format!("{host}:{p}")),
        None => Some(host),
    }
}

/// RFC 3492 Bootstring / Punycode encoder for a single non-ASCII domain label.
fn encode_punycode(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    const INITIAL_BIAS: u32 = 72;
    const INITIAL_N: u32 = 128;

    let chars: Vec<u32> = input.chars().map(u32::from).collect();
    let mut output = String::new();
    for &cp in &chars {
        if cp < 128 {
            output.push(char::from(cp as u8));
        }
    }
    let basic = output.len() as u32;
    let mut handled = basic;
    if basic > 0 {
        output.push('-');
    }

    let mut n = INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = INITIAL_BIAS;

    let encode_digit = |d: u32| -> char {
        if d < 26 { char::from(b'a' + d as u8) } else { char::from(b'0' + (d - 26) as u8) }
    };

    let adapt = |mut d: u32, numpoints: u32, first: bool| -> u32 {
        d = if first { d / DAMP } else { d / 2 };
        d += d / numpoints;
        let mut k = 0;
        while d > ((BASE - TMIN) * TMAX) / 2 {
            d /= BASE - TMIN;
            k += BASE;
        }
        k + (((BASE - TMIN + 1) * d) / (d + SKEW))
    };

    while (handled as usize) < chars.len() {
        let m = chars.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &cp in &chars {
            if cp < n {
                delta = delta.checked_add(1)?;
            } else if cp == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias { TMIN } else if k >= bias + TMAX { TMAX } else { k - bias };
                    if q < t {
                        break;
                    }
                    output.push(encode_digit(t + (q - t) % (BASE - t)));
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(encode_digit(q));
                bias = adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(output)
}

impl BrowserTab {
    /// What its tab says: the page's title, else its bare URL, else "New tab".
    pub fn label(&self) -> String {
        if !self.title.is_empty() && self.title != "New tab" {
            return self.title.clone();
        }
        if self.url.is_empty() || self.url == "about:blank" { opencode_i18n::t!("plugin.browser.tab.new").into() } else { bare_url(&self.url) }
    }

    /// No page yet.
    pub fn is_blank(&self) -> bool {
        self.url.is_empty() || self.url == "about:blank"
    }
}
