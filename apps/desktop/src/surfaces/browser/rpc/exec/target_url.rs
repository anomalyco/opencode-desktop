//! WHATWG URL canonicalization for server permission matching (`browser.navigate` targets).

use browser_pane::policy::with_scheme;

fn parse_whatwg_ipv4_part(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        if hex.is_empty() {
            return Some(0);
        }
        return u64::from_str_radix(hex, 16).ok();
    }
    if part.len() > 1 && part.starts_with('0') {
        if !part.bytes().all(|b| matches!(b, b'0'..=b'7')) {
            return None;
        }
        return u64::from_str_radix(part, 8).ok();
    }
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

fn canonicalize_whatwg_ipv4(host: &str) -> Option<String> {
    if host.is_empty() || host.starts_with('[') {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut nums = Vec::with_capacity(parts.len());
    for p in &parts {
        nums.push(parse_whatwg_ipv4_part(p)?);
    }
    let last = *nums.last()?;
    let n = nums.len() as u32;
    let max_last = 1u64 << (8 * (5 - n));
    if last >= max_last {
        return None;
    }
    let mut val = last;
    for (i, &part) in nums[..nums.len() - 1].iter().enumerate() {
        if part >= 256 {
            return None;
        }
        val += part << (8 * (3 - i));
    }
    Some(std::net::Ipv4Addr::from(val as u32).to_string())
}

fn canonicalize_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    if lower.starts_with('[') && lower.ends_with(']') {
        let inner = &lower[1..lower.len() - 1];
        if let Ok(ip) = inner.parse::<std::net::Ipv6Addr>() {
            return format!("[{ip}]");
        }
        return lower;
    }
    let trimmed = lower.trim_end_matches('.');
    let base = if trimmed.is_empty() { lower.as_str() } else { trimmed };
    canonicalize_whatwg_ipv4(base).unwrap_or_else(|| base.to_owned())
}

fn is_single_dot_seg(seg: &str) -> bool {
    seg == "." || seg.eq_ignore_ascii_case("%2e")
}

fn is_double_dot_seg(seg: &str) -> bool {
    seg == ".."
        || seg.eq_ignore_ascii_case(".%2e")
        || seg.eq_ignore_ascii_case("%2e.")
        || seg.eq_ignore_ascii_case("%2e%2e")
}

/// Canonicalizes a URL for server permission matching per WHATWG URL rules: strips ASCII tabs/
/// newlines, adds default `https://` scheme, treats `\` as `/` in authority & path, lowercases
/// scheme & host, strips trailing DNS dots, canonicalizes alternate IPv4 (decimal/octal/hex/
/// shorthand) & IPv6 literals, strips default `:80`/`:443` ports, and normalizes `.`/`..`/
/// `%2e`/`%2e%2e` path segments.
pub(super) fn canonical_target_url(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let with_s = with_scheme(&cleaned);
    let Some((scheme_raw, rest)) = with_s.split_once("://") else {
        return with_s;
    };
    let scheme = scheme_raw.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return with_s;
    }
    let (before_frag, frag) = match rest.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    let (before_q, query) = match before_frag.split_once('?') {
        Some((a, b)) => (a, Some(b)),
        None => (before_frag, None),
    };
    let (auth_raw, path_raw) = match before_q.find(['/', '\\']) {
        Some(idx) => (&before_q[..idx], &before_q[idx..]),
        None => (before_q, ""),
    };
    let (userinfo, hostport) = match auth_raw.rsplit_once('@') {
        Some((u, hp)) => (Some(u), hp),
        None => (None, auth_raw),
    };
    let (host_raw, port_raw) = if hostport.starts_with('[') {
        match hostport.find(']') {
            Some(end) => {
                let tail = &hostport[end + 1..];
                (&hostport[..=end], tail.strip_prefix(':'))
            }
            None => (hostport, None),
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let host = canonicalize_host(host_raw);
    let default_port: u16 = if scheme == "http" { 80 } else { 443 };
    let port_norm: Option<String> = match port_raw {
        Some(p) if !p.is_empty() => match p.parse::<u16>() {
            Ok(n) if n == default_port => None,
            Ok(n) => Some(n.to_string()),
            Err(_) => Some(p.to_owned()),
        },
        _ => None,
    };
    let path_slashed = path_raw.replace('\\', "/");
    let mut segs: Vec<&str> = Vec::new();
    let mut ends_with_slash = path_slashed.ends_with('/') && path_slashed.len() > 1;
    for seg in path_slashed.split('/') {
        if seg.is_empty() {
            continue;
        }
        if is_single_dot_seg(seg) {
            ends_with_slash = true;
        } else if is_double_dot_seg(seg) {
            segs.pop();
            ends_with_slash = true;
        } else {
            segs.push(seg);
            ends_with_slash = false;
        }
    }
    let mut out = format!("{scheme}://");
    if let Some(u) = userinfo {
        out.push_str(u);
        out.push('@');
    }
    out.push_str(&host);
    if let Some(p) = port_norm {
        out.push(':');
        out.push_str(&p);
    }
    if !segs.is_empty() || path_slashed.starts_with('/') {
        out.push('/');
        out.push_str(&segs.join("/"));
        if ends_with_slash && !segs.is_empty() {
            out.push('/');
        }
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    out
}
