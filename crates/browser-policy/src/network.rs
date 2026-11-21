//! Loopback, unspecified-host, and remote tunnel proxy endpoint validation.

/// Parses an IPv4 literal permissively according to BSD `inet_aton` / WHATWG URL host rules
/// (1–4 dot-separated parts in decimal, `0x` hex, or leading-`0` octal), so non-canonical forms
/// like `127.1`, `2130706433`, `0x7f000001`, `0177.0.0.1`, or `0` are recognized cleanly.
fn parse_ipv4_permissive(hostname: &str) -> Option<std::net::Ipv4Addr> {
    if hostname.is_empty() || hostname.ends_with('.') {
        return None;
    }
    let parts: Vec<&str> = hostname.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut vals = [0u32; 4];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return None;
        }
        let val = if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            // WHATWG URL §3.5 parses empty `"0x"` as `0`.
            if hex.is_empty() {
                0
            } else {
                u32::from_str_radix(hex, 16).ok()?
            }
        } else if part.len() > 1 && part.starts_with('0') {
            if !part.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
                return None;
            }
            u32::from_str_radix(part, 8).ok()?
        } else {
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse::<u32>().ok()?
        };
        vals[i] = val;
    }
    let ip_u32 = match parts.len() {
        1 => vals[0],
        2 => {
            if vals[0] > 0xff || vals[1] > 0x00ff_ffff {
                return None;
            }
            (vals[0] << 24) | vals[1]
        }
        3 => {
            if vals[0] > 0xff || vals[1] > 0xff || vals[2] > 0xffff {
                return None;
            }
            (vals[0] << 24) | (vals[1] << 16) | vals[2]
        }
        4 => {
            if vals.iter().any(|&v| v > 0xff) {
                return None;
            }
            (vals[0] << 24) | (vals[1] << 16) | (vals[2] << 8) | vals[3]
        }
        _ => return None,
    };
    Some(std::net::Ipv4Addr::from(ip_u32))
}

fn parse_ipv6_host(hostname: &str) -> Option<std::net::Ipv6Addr> {
    let unbracketed = hostname
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(hostname);
    let no_zone = unbracketed.split('%').next().unwrap_or(unbracketed);
    no_zone.parse::<std::net::Ipv6Addr>().ok()
}

/// Whether `hostname` names a loopback address (`localhost`, `*.localhost`, `127.0.0.0/8` in any
/// canonical or permissive `inet_aton` form, `::1`, or IPv4-mapped `::ffff:127.0.0.0/104`).
pub fn loopback_host(hostname: &str) -> bool {
    let h = hostname.trim().trim_end_matches('.');
    if h.is_empty() {
        return false;
    }
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if h.len() > 10 && h.as_bytes()[h.len() - 10..].eq_ignore_ascii_case(b".localhost") {
        return true;
    }
    if let Some(v6) = parse_ipv6_host(h) {
        return v6.is_loopback()
            || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            || v6.to_ipv4().is_some_and(|v4| v4.is_loopback());
    }
    if let Some(v4) = parse_ipv4_permissive(h) {
        return v4.is_loopback();
    }
    false
}

/// Whether `hostname` targets the local host via loopback OR unspecified addresses (`0.0.0.0/8`,
/// `[::]`, `[::ffff:0.0.0.0/104]`, octal/hex/decimal equivalents) — used to block handing
/// remote-server local URLs to the user's local OS default browser.
pub fn is_local_or_loopback_host(hostname: &str) -> bool {
    if loopback_host(hostname) {
        return true;
    }
    let h = hostname.trim().trim_end_matches('.');
    if let Some(v6) = parse_ipv6_host(h) {
        return v6.is_unspecified()
            || v6.to_ipv4_mapped().is_some_and(|v4| v4.octets()[0] == 0)
            || v6.to_ipv4().is_some_and(|v4| v4.octets()[0] == 0);
    }
    if let Some(v4) = parse_ipv4_permissive(h) {
        return v4.octets()[0] == 0;
    }
    false
}

/// Whether `host` is a strict canonical loopback IP literal (`127.0.0.0/8` or `::1`, with optional
/// `[...]`) — rejects hostnames like `localhost` / `*.localhost` so configuring a proxy never
/// depends on local DNS or `/etc/hosts` resolution.
pub fn is_strict_loopback_ip(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return false;
    }
    if let Ok(v4) = trimmed.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback();
    }
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    if unbracketed.contains('%') {
        return false;
    }
    unbracketed
        .parse::<std::net::Ipv6Addr>()
        .is_ok_and(|v6| v6.is_loopback())
}

/// Validates a remote tunnel proxy endpoint: must be a strict loopback IP literal (`127.0.0.1` /
/// `::1`), non-zero port, and carry non-empty Basic credentials (`username` and `password`).
pub fn validate_proxy_endpoint(
    host: &str,
    port: u16,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(), &'static str> {
    if port == 0 || !is_strict_loopback_ip(host) {
        return Err("browser.proxy.invalid");
    }
    let Some(user) = username.filter(|u| !u.is_empty()) else {
        return Err("browser.proxy.invalid");
    };
    let Some(pass) = password.filter(|p| !p.is_empty()) else {
        return Err("browser.proxy.invalid");
    };
    if user.chars().any(|c| c == ':' || c.is_control())
        || pass.chars().any(|c| c.is_control())
    {
        return Err("browser.proxy.invalid");
    }
    Ok(())
}

/// Whether an IPv4 address is globally routable (excludes unspecified, loopback, RFC 1918 private,
/// CGNAT/Tailscale `100.64.0.0/10`, link-local & cloud IMDS `169.254.0.0/16`, documentation,
/// benchmarking, multicast, and reserved ranges).
pub fn is_public_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || (a == 100 && (b & 0xc0) == 64)
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (b & 0xf0) == 16)
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 192 && b == 168)
        || (a == 198 && (b & 0xfe) == 18)
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

/// Whether an IPv6 address is globally routable (excludes unspecified, loopback, ULA `fc00::/7`
/// incl. AWS IMDS `fd00:ec2::254`, link-local `fe80::/10`, multicast, documentation, Teredo, and
/// non-public embedded IPv4 in mapped/compatible/NAT64/6to4 ranges).
pub fn is_public_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() {
        return false;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    let seg = ip.segments();
    if seg[..6] == [0; 6] {
        let v4 = std::net::Ipv4Addr::from(((seg[6] as u32) << 16) | (seg[7] as u32));
        return is_public_ipv4(v4);
    }
    if seg[0] == 0x0064 && seg[1] == 0xff9b {
        if seg[2] == 0x0001 {
            return false;
        }
        if seg[2..6] == [0; 4] {
            let v4 = std::net::Ipv4Addr::from(((seg[6] as u32) << 16) | (seg[7] as u32));
            return is_public_ipv4(v4);
        }
        return false;
    }
    if seg[0] == 0x2002 {
        let v4 = std::net::Ipv4Addr::from(((seg[1] as u32) << 16) | (seg[2] as u32));
        return is_public_ipv4(v4);
    }
    if seg[0] == 0x2001
        && (seg[1] == 0x0000
            || seg[1] == 0x0db8
            || (seg[1] == 0x0002 && seg[2] == 0x0000)
            || (seg[1] & 0xfff0) == 0x0010
            || (seg[1] & 0xfff0) == 0x0020)
    {
        return false;
    }
    (seg[0] & 0xe000) == 0x2000
}

/// Whether `ip` is globally routable (neither loopback, private LAN, link-local/IMDS, nor ULA).
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_public_ipv4(v4),
        std::net::IpAddr::V6(v6) => is_public_ipv6(v6),
    }
}

fn looks_like_ipv4_number(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        // Include empty `"0x"` because WHATWG URL §3.5 parses `"0x"` as `0` (e.g. `127.0x`).
        return hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    s.bytes().all(|b| b.is_ascii_digit())
}

/// Returns `true` only if `input` is a credential-free `http://` or `https://` URL whose host is
/// a globally routable public IP literal or multi-label public DNS name (refusing credentials,
/// loopback, RFC 1918 LAN, CGNAT, link-local/IMDS, ULA, `.localhost`/`.local`/`.internal`/`.arpa`/
/// `metadata.goog`, alternate IPv4 encodings, and invalid/zero ports).
pub fn is_public_http_url(input: &str) -> bool {
    let Some(origin) = crate::destination_origin(input) else {
        return false;
    };
    let Some((_, authority)) = origin.split_once("://") else {
        return false;
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((ipv6_str, tail)) = rest.split_once(']') else {
            return false;
        };
        if ipv6_str.contains('%') {
            return false;
        }
        if !tail.is_empty()
            && !tail.strip_prefix(':').and_then(|p| p.parse::<u16>().ok()).is_some_and(|n| n > 0)
        {
            return false;
        }
        return ipv6_str.parse::<std::net::Ipv6Addr>().is_ok_and(is_public_ipv6);
    }
    let (host, tail) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = tail
        && !p.parse::<u16>().is_ok_and(|n| n > 0)
    {
        return false;
    }
    if host.is_empty()
        || host.len() > 253
        || host.contains('%')
        || !host.is_ascii()
        || host.starts_with('.')
        || host.contains("..")
    {
        return false;
    }
    let trimmed = host.strip_suffix('.').unwrap_or(host);
    if trimmed.is_empty() || trimmed.ends_with('.') {
        return false;
    }
    let labels: Vec<&str> = trimmed.split('.').collect();
    if let Some(last) = labels.last()
        && looks_like_ipv4_number(last)
    {
        return parse_ipv4_permissive(trimmed).is_some_and(is_public_ipv4);
    }
    if labels.len() < 2 {
        return false;
    }
    for label in &labels {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower == "metadata.goog" || lower.ends_with(".metadata.goog") {
        return false;
    }
    let tld = labels.last().copied().unwrap_or("").to_ascii_lowercase();
    !matches!(
        tld.as_str(),
        "localhost"
            | "local"
            | "localdomain"
            | "internal"
            | "arpa"
            | "lan"
            | "home"
            | "corp"
            | "intranet"
            | "private"
            | "localnet"
    )
}
