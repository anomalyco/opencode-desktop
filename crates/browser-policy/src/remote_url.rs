//! Remote-server HTTP(S) destination checks for media/resources that bypass `TunnelProxy`
//! (such as OS native video playback on click).
//!
//! Refuses credentials, percent-encoded/non-ASCII host obfuscations, single-label hosts,
//! local/private/metadata DNS suffixes (`.localhost`, `.local`, `.internal`, `.arpa`, etc.),
//! alternate IPv4 encodings (`127.1`, `2130706433`, `0x7f000001`, `0177.0.0.1`), and every
//! non-globally-routable IPv4/IPv6 range (loopback, RFC 1918 private, CGNAT `100.64.0.0/10`,
//! link-local & cloud IMDS `169.254.0.0/16`, ULA `fc00::/7` including `fd00:ec2::254`,
//! IPv4-mapped/compatible/NAT64/6to4 embeddings, multicast, and reserved ranges).

use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use crate::destination_origin;

/// Returns `true` only if `input` is a credential-free `http://` or `https://` URL whose host
/// is a syntactically valid, globally routable public IP address or multi-label public DNS name.
pub fn is_public_http_url(input: &str) -> bool {
    let Some(origin) = destination_origin(input) else {
        return false;
    };
    let Some((_, authority)) = origin.split_once("://") else {
        return false;
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((ipv6_str, tail)) = rest.split_once(']') else {
            return false;
        };
        if !valid_optional_port(tail) || ipv6_str.contains('%') {
            return false;
        }
        let Ok(ip) = Ipv6Addr::from_str(ipv6_str) else {
            return false;
        };
        return is_public_ipv6(ip);
    }
    let (host, tail) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    if let Some(p) = tail
        && p.parse::<u16>().is_ok_and(|n| n == 0)
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
    if labels.is_empty() {
        return false;
    }
    // WHATWG URL IPv4 rule: if the final label parses as a decimal/hex/octal number, the entire
    // host MUST be parsed as an IPv4 address (never treated as a public DNS name).
    if let Some(last) = labels.last()
        && parse_ipv4_part(last).is_some()
    {
        let Some(ip) = parse_whatwg_ipv4(&labels) else {
            return false;
        };
        return is_public_ipv4(ip);
    }
    // Public DNS hostnames require at least two valid ASCII LDH labels.
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
    if lower == "metadata.goog" {
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

/// Extracts a sanitized, bounded host (with optional port) from a valid credential-free
/// `http(s)` URL for display on a click-to-load control.
pub fn sanitized_http_host(input: &str) -> Option<String> {
    let origin = destination_origin(input)?;
    let (_, authority) = origin.split_once("://")?;
    let clean: String = authority
        .chars()
        .filter(|&c| {
            let n = c as u32;
            !c.is_control()
                && !matches!(n, 0x7f..=0x9f | 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069 | 0xfeff)
        })
        .take(64)
        .collect();
    (!clean.is_empty()).then_some(clean)
}

fn valid_optional_port(tail: &str) -> bool {
    if tail.is_empty() {
        return true;
    }
    let Some(port_str) = tail.strip_prefix(':') else {
        return false;
    };
    port_str.parse::<u16>().is_ok_and(|p| p > 0)
}

fn parse_ipv4_part(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return u64::from_str_radix(hex, 16).ok();
    }
    if s.len() > 1 && s.starts_with('0') {
        if !s.bytes().all(|b| matches!(b, b'0'..=b'7')) {
            return None;
        }
        return u64::from_str_radix(s, 8).ok();
    }
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok()
}

fn parse_whatwg_ipv4(labels: &[&str]) -> Option<Ipv4Addr> {
    if labels.is_empty() || labels.len() > 4 {
        return None;
    }
    let mut parts = Vec::with_capacity(labels.len());
    for label in labels {
        parts.push(parse_ipv4_part(label)?);
    }
    let last_idx = parts.len() - 1;
    for &p in &parts[..last_idx] {
        if p > 255 {
            return None;
        }
    }
    let last_shift = (4 - last_idx) as u32 * 8;
    let max_last = if last_shift == 32 { u32::MAX as u64 } else { (1u64 << last_shift) - 1 };
    if parts[last_idx] > max_last {
        return None;
    }
    let mut addr = parts[last_idx] as u32;
    for (i, &p) in parts[..last_idx].iter().enumerate() {
        addr |= (p as u32) << ((3 - i) * 8);
    }
    Some(Ipv4Addr::from(addr))
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    if a == 0 // 0.0.0.0/8 unspecified
        || a == 10 // 10.0.0.0/8 RFC 1918
        || (a == 100 && (b & 0xc0) == 64) // 100.64.0.0/10 CGNAT / Tailscale
        || a == 127 // 127.0.0.0/8 loopback
        || (a == 169 && b == 254) // 169.254.0.0/16 link-local & cloud IMDS
        || (a == 172 && (b & 0xf0) == 16) // 172.16.0.0/12 RFC 1918
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // IETF protocol / TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // 6to4 relay
        || (a == 192 && b == 168) // 192.168.0.0/16 RFC 1918
        || (a == 198 && (b & 0xfe) == 18) // 198.18.0.0/15 benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
        || a >= 224 // multicast, reserved, broadcast
    {
        return false;
    }
    true
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() {
        return false;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }
    let seg = ip.segments();
    // IPv4-compatible `::a.b.c.d`
    if seg[..6] == [0; 6] {
        let v4 = Ipv4Addr::from(((seg[6] as u32) << 16) | (seg[7] as u32));
        return is_public_ipv4(v4);
    }
    // NAT64 well-known `64:ff9b::/96` and local-use `64:ff9b:1::/48`
    if seg[0] == 0x0064 && seg[1] == 0xff9b {
        if seg[2] == 0x0001 {
            return false;
        }
        if seg[2..6] == [0; 4] {
            let v4 = Ipv4Addr::from(((seg[6] as u32) << 16) | (seg[7] as u32));
            return is_public_ipv4(v4);
        }
        return false;
    }
    // 6to4 `2002::/16`: inspect embedded IPv4
    if seg[0] == 0x2002 {
        let v4 = Ipv4Addr::from(((seg[1] as u32) << 16) | (seg[2] as u32));
        return is_public_ipv4(v4);
    }
    // Teredo, documentation, benchmarking, ORCHID
    if seg[0] == 0x2001
        && (seg[1] == 0x0000
            || seg[1] == 0x0db8
            || (seg[1] == 0x0002 && seg[2] == 0x0000)
            || (seg[1] & 0xfff0) == 0x0010
            || (seg[1] & 0xfff0) == 0x0020)
    {
        return false;
    }
    // Only global unicast `2000::/3` (automatically excludes ULA `fc00::/7` incl. AWS IPv6 IMDS
    // `fd00:ec2::254`, link-local `fe80::/10`, multicast `ff00::/8`, discard `100::/64`).
    (seg[0] & 0xe000) == 0x2000
}
