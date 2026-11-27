//! OSC 8 terminal hyperlink display sanitization, RFC 3492 punycode conversion, registrable-domain
//! emphasis, and IDN homoglyph confirmation gating.

const MAX_DISPLAY_CHARS: usize = 240;

const PUNY_BASE: u32 = 36;
const PUNY_TMIN: u32 = 1;
const PUNY_TMAX: u32 = 26;
const PUNY_SKEW: u32 = 38;
const PUNY_DAMP: u32 = 700;
const PUNY_INITIAL_BIAS: u32 = 72;
const PUNY_INITIAL_N: u32 = 128;

/// Sanitized target presentation for the terminal OSC 8 confirmation modal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Osc8Display {
    /// Subdomain prefix before the registrable domain (including trailing `.`), empty if none.
    pub host_prefix: String,
    /// Registrable domain (or IP literal) to emphasize in the confirmation dialog, empty for non-HTTP(S).
    pub registrable: String,
    /// Optional `:port` suffix after the registrable domain.
    pub host_suffix: String,
    /// Sanitized, punycode-normalized, userinfo-free, length-capped URL text.
    pub url_display: String,
    /// True if any host label was non-ASCII or already used `xn--` punycode (IDN homoglyph risk).
    pub has_idn: bool,
    /// True if the URL authority carried `userinfo@` (which is stripped from display and refused).
    pub has_userinfo: bool,
    /// True if the input contained control, bidi override/isolate, BOM, or zero-width characters.
    pub had_unsafe_chars: bool,
}

fn is_unsafe_display_char(c: char) -> bool {
    let n = c as u32;
    n <= 0x1f
        || (0x7f..=0x9f).contains(&n)
        || matches!(
            n,
            0x061c
                | 0x200b..=0x200f
                | 0x202a..=0x202e
                | 0x2060
                | 0x2066..=0x2069
                | 0xfeff
        )
}

fn strip_unsafe(raw: &str) -> (String, bool) {
    let mut had = false;
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if is_unsafe_display_char(c) {
            had = true;
        } else {
            out.push(c);
        }
    }
    (out, had)
}

fn cap_chars(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}\u{2026}")
    } else {
        head
    }
}

fn encode_digit(d: u32) -> char {
    if d < 26 {
        (b'a' + d as u8) as char
    } else {
        (b'0' + (d - 26) as u8) as char
    }
}

fn puny_adapt(delta: u32, num_points: u32, first: bool) -> u32 {
    let mut d = if first { delta / PUNY_DAMP } else { delta / 2 };
    d += d / num_points;
    let mut k = 0;
    while d > ((PUNY_BASE - PUNY_TMIN) * PUNY_TMAX) / 2 {
        d /= PUNY_BASE - PUNY_TMIN;
        k += PUNY_BASE;
    }
    k + (((PUNY_BASE - PUNY_TMIN + 1) * d) / (d + PUNY_SKEW))
}

fn encode_punycode_label(label: &str) -> Option<String> {
    let cps: Vec<u32> = label.chars().flat_map(|c| c.to_lowercase()).map(|c| c as u32).collect();
    let mut out = String::from("xn--");
    let mut basic: u32 = 0;
    for &cp in &cps {
        if cp < 128 {
            out.push(cp as u8 as char);
            basic += 1;
        }
    }
    let mut handled = basic;
    if basic > 0 {
        out.push('-');
    }
    let mut n = PUNY_INITIAL_N;
    let mut delta: u32 = 0;
    let mut bias = PUNY_INITIAL_BIAS;
    while (handled as usize) < cps.len() {
        let m = cps.iter().copied().filter(|&cp| cp >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(handled + 1)?)?;
        n = m;
        for &cp in &cps {
            if cp < n {
                delta = delta.checked_add(1)?;
            } else if cp == n {
                let mut q = delta;
                let mut k = PUNY_BASE;
                loop {
                    let t = if k <= bias {
                        PUNY_TMIN
                    } else if k >= bias + PUNY_TMAX {
                        PUNY_TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    out.push(encode_digit(t + ((q - t) % (PUNY_BASE - t))));
                    q = (q - t) / (PUNY_BASE - t);
                    k += PUNY_BASE;
                }
                out.push(encode_digit(q));
                bias = puny_adapt(delta, handled + 1, handled == basic);
                delta = 0;
                handled += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(out)
}

fn to_ascii_host(raw_host: &str) -> (String, bool) {
    if raw_host.starts_with('[') && raw_host.ends_with(']') {
        return (raw_host.to_ascii_lowercase(), false);
    }
    let normalized = raw_host.replace(['\u{3002}', '\u{ff0e}', '\u{ff61}'], ".");
    let mut has_idn = normalized != raw_host;
    let mut labels = Vec::new();
    for label in normalized.split('.') {
        if label.is_ascii() {
            let lower = label.to_ascii_lowercase();
            if lower.starts_with("xn--") {
                has_idn = true;
            }
            labels.push(lower);
        } else {
            has_idn = true;
            let encoded = encode_punycode_label(label).unwrap_or_else(|| "xn--invalid".to_owned());
            labels.push(encoded);
        }
    }
    (labels.join("."), has_idn)
}

fn split_registrable(host: &str) -> (String, String) {
    let trimmed = host.trim_end_matches('.');
    if trimmed.is_empty()
        || trimmed.starts_with('[')
        || trimmed.parse::<std::net::Ipv4Addr>().is_ok()
    {
        return (String::new(), host.to_owned());
    }
    let labels: Vec<&str> = trimmed.split('.').collect();
    if labels.len() <= 2 {
        return (String::new(), host.to_owned());
    }
    const MULTI_PART_TLDS: &[(&str, &str)] = &[
        ("co", "uk"),
        ("org", "uk"),
        ("gov", "uk"),
        ("ac", "uk"),
        ("com", "au"),
        ("net", "au"),
        ("org", "au"),
        ("co", "jp"),
        ("ne", "jp"),
        ("or", "jp"),
        ("com", "br"),
        ("co", "nz"),
        ("co", "kr"),
        ("com", "cn"),
        ("com", "tw"),
        ("com", "sg"),
        ("com", "hk"),
        ("co", "in"),
        ("co", "za"),
        ("com", "mx"),
    ];
    let second = labels[labels.len() - 2];
    let last = labels[labels.len() - 1];
    let take = if labels.len() >= 3 && MULTI_PART_TLDS.contains(&(second, last)) { 3 } else { 2 };
    let split_at = labels.len() - take;
    let prefix = if split_at == 0 {
        String::new()
    } else {
        format!("{}.", labels[..split_at].join("."))
    };
    let trailing_dot = if host.ends_with('.') { "." } else { "" };
    let registrable = format!("{}{trailing_dot}", labels[split_at..].join("."));
    (prefix, registrable)
}

/// Sanitizes an OSC 8 target URL for display in the confirmation dialog: strips bidi/control/
/// zero-width characters, drops any `userinfo@`, converts non-ASCII hosts to RFC 3492 punycode,
/// identifies the registrable domain, and caps the displayed URL length.
pub fn format_osc8_target(raw_url: &str) -> Osc8Display {
    let (clean, had_unsafe_chars) = strip_unsafe(raw_url.trim());
    let web_scheme = clean
        .get(..7)
        .filter(|s| s.eq_ignore_ascii_case("http://"))
        .map(|_| ("http", &clean[7..]))
        .or_else(|| {
            clean
                .get(..8)
                .filter(|s| s.eq_ignore_ascii_case("https://"))
                .map(|_| ("https", &clean[8..]))
        });
    let Some((scheme, rest)) = web_scheme else {
        let has_idn = !clean.is_ascii();
        return Osc8Display {
            host_prefix: String::new(),
            registrable: String::new(),
            host_suffix: String::new(),
            url_display: cap_chars(&clean, MAX_DISPLAY_CHARS),
            has_idn,
            has_userinfo: false,
            had_unsafe_chars,
        };
    };
    let split_pos = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let raw_authority = &rest[..split_pos];
    let tail = &rest[split_pos..];
    let (has_userinfo, host_port) = match raw_authority.rsplit_once('@') {
        Some((_, hp)) => (true, hp),
        None => (false, raw_authority),
    };
    let (raw_host, port_suffix) = if let Some(after_open) = host_port.strip_prefix('[') {
        match after_open.find(']') {
            Some(end) => (&host_port[..=end + 1], &host_port[end + 2..]),
            None => (host_port, ""),
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                (h, &host_port[h.len()..])
            }
            _ => (host_port, ""),
        }
    };
    let (ascii_host, has_idn) = to_ascii_host(raw_host);
    let (host_prefix, registrable) = split_registrable(&ascii_host);
    let rebuilt = format!("{scheme}://{ascii_host}{port_suffix}{tail}");
    Osc8Display {
        host_prefix,
        registrable,
        host_suffix: port_suffix.to_owned(),
        url_display: cap_chars(&rebuilt, MAX_DISPLAY_CHARS),
        has_idn,
        has_userinfo,
        had_unsafe_chars,
    }
}

/// Whether clicking an OSC 8 hyperlink with visible text `visible` and target `url` MUST open the
/// confirmation modal (even when `visible == url` for IDN homoglyphs, punycode, bidi/controls, or
/// non-web schemes).
pub fn osc8_needs_confirm(visible: &str, url: &str) -> bool {
    let trimmed_url = url.trim();
    let is_web = trimmed_url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        || trimmed_url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
    let (_, vis_unsafe) = strip_unsafe(visible);
    let display = format_osc8_target(trimmed_url);
    !is_web
        || visible.trim() != trimmed_url
        || vis_unsafe
        || display.had_unsafe_chars
        || display.has_idn
        || display.has_userinfo
}
