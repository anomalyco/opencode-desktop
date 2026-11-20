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
            if hex.is_empty() {
                return None;
            }
            u32::from_str_radix(hex, 16).ok()?
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
