//! Registrable-domain ("Choose sites") grouping and filtering for cookie imports (`profile/sites.ts`).
//!
//! Pure, zero-dependency implementation covering:
//! - IPv4 and IPv6 literals (with IPv6 canonicalized in brackets `[...]`).
//! - `localhost` and single-label intranet hosts.
//! - Multi-label public ccTLD suffixes (`co.uk`, `com.au`, `co.jp`, ...) and private hosting
//!   suffixes (`github.io`, `pages.dev`, `vercel.app`, `netlify.app`, ...) so tenants stay separate.
//! - RFC 3492 Punycode label encoding for Unicode domain names in cookie files (`shop.münchen.de` ->
//!   `xn--mnchen-3ya.de`).

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};

use super::{CookieHost, ProfileSite};

/// Multi-label public suffixes & private hosting suffixes (`allowPrivateDomains: true` parity).
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    // Common ccTLD 2-label public suffixes
    "ac.at", "ac.cn", "ac.id", "ac.il", "ac.in", "ac.jp", "ac.kr", "ac.nz", "ac.th", "ac.uk",
    "ac.za", "adv.br", "asn.au", "biz.id", "biz.pl", "biz.tr", "co.at", "co.id", "co.il", "co.in",
    "co.jp", "co.ke", "co.kr", "co.nz", "co.th", "co.tz", "co.uk", "co.za", "com.ar", "com.au",
    "com.bd", "com.br", "com.cn", "com.co", "com.eg", "com.es", "com.hk", "com.mx", "com.my",
    "com.ng", "com.pe", "com.ph", "com.pk", "com.pl", "com.pt", "com.ru", "com.sa", "com.sg",
    "com.tr", "com.tw", "com.ua", "com.ve", "com.vn", "ed.jp", "edu.ar", "edu.au", "edu.br",
    "edu.cn", "edu.es", "edu.hk", "edu.in", "edu.mx", "edu.my", "edu.pl", "edu.sg", "edu.tr",
    "edu.tw", "edu.ua", "gen.in", "gen.nz", "go.id", "go.jp", "go.kr", "go.th", "gob.es", "gob.mx",
    "gov.ar", "gov.au", "gov.br", "gov.cn", "gov.hk", "gov.il", "gov.in", "gov.my", "gov.ph",
    "gov.pl", "gov.sg", "gov.tr", "gov.tw", "gov.ua", "gov.uk", "gov.za", "govt.nz", "gr.jp",
    "id.au", "ind.in", "info.pl", "lg.jp", "ltd.uk", "me.uk", "mil.br", "ne.jp", "net.ar",
    "net.au", "net.br", "net.cn", "net.hk", "net.id", "net.il", "net.in", "net.mx", "net.my",
    "net.nz", "net.ph", "net.pk", "net.pl", "net.ru", "net.sg", "net.tr", "net.tw", "net.ua",
    "net.uk", "net.za", "nhs.uk", "nic.in", "nom.es", "or.at", "or.id", "or.jp", "or.kr",
    "org.ar", "org.au", "org.br", "org.cn", "org.es", "org.hk", "org.il", "org.in", "org.mx",
    "org.my", "org.nz", "org.ph", "org.pk", "org.pl", "org.ru", "org.sg", "org.tr", "org.tw",
    "org.ua", "org.uk", "org.za", "plc.uk", "res.in", "sch.uk", "web.id",
    // High-value private suffixes (tenants separated in Chromium cookie scoping)
    "amazonaws.com", "appspot.com", "azurecontainer.io", "azurestaticapps.net", "azurewebsites.net",
    "blogspot.com", "cloudfront.net", "cloudfunctions.net", "cloudflarestorage.com", "deno.dev",
    "duckdns.org", "eu.org", "firebaseapp.com", "fly.dev", "github.io", "githubusercontent.com",
    "gitlab.io", "glitch.me", "herokuapp.com", "herokudns.com", "localtest.me", "myshopify.com",
    "netlify.app", "netlify.com", "ngrok-free.app", "ngrok.io", "nip.io", "notion.site",
    "onrender.com", "pages.dev", "pythonanywhere.com", "r2.dev", "railway.app", "replit.app",
    "replit.dev", "run.app", "s3.amazonaws.com", "surge.sh", "supabase.co", "trycloudflare.com",
    "up.railway.app", "vercel.app", "web.app", "wixsite.com", "wordpress.com", "workers.dev",
    "xip.io",
];

/// Returns the site (registrable domain or IP literal) a cookie host belongs to, or `None` for an
/// invalid host (`profile/sites.ts::cookieSite`).
pub fn cookie_site(host: &str) -> Option<String> {
    let bare = host.trim_start_matches('.').trim_end_matches('.');
    if bare.is_empty() {
        return None;
    }
    // IPv6 literal (with or without brackets).
    let unbracketed = bare
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(bare);
    if let Ok(ip) = unbracketed.parse::<Ipv6Addr>() {
        return Some(format!("[{ip}]"));
    }
    if bare.contains('[') || bare.contains(']') || bare.contains(':') {
        return None;
    }
    if let Ok(ip) = bare.parse::<Ipv4Addr>() {
        return Some(ip.to_string());
    }

    // Validate & ASCII-normalize labels (punycode non-ASCII labels).
    let mut labels = Vec::new();
    for raw in bare.split('.') {
        if raw.is_empty() {
            return None;
        }
        let lower = raw.to_lowercase();
        let ascii = if lower.is_ascii() {
            if !lower
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return None;
            }
            lower
        } else {
            if lower.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return None;
            }
            punycode_label(&lower)?
        };
        labels.push(ascii);
    }

    if labels.len() <= 1 {
        return labels.into_iter().next();
    }

    // Check longest matching suffix (3-label e.g. `s3.amazonaws.com`, then 2-label e.g. `co.uk`).
    for suffix_len in [3usize, 2usize] {
        if labels.len() >= suffix_len {
            let tail = labels[labels.len() - suffix_len..].join(".");
            if MULTI_LABEL_SUFFIXES.binary_search(&tail.as_str()).is_ok()
                || MULTI_LABEL_SUFFIXES.contains(&tail.as_str())
            {
                return if labels.len() == suffix_len {
                    Some(tail)
                } else {
                    Some(format!("{}.{tail}", labels[labels.len() - suffix_len - 1]))
                };
            }
        }
    }

    // Standard single-label TLD (including `*.localhost` -> `app.localhost`).
    Some(format!(
        "{}.{}",
        labels[labels.len() - 2],
        labels[labels.len() - 1]
    ))
}

/// Groups cookie hosts into registrable sites with importable cookie counts (expired cookies and
/// sites > 255 chars excluded), ordered most-recently-used first then ASCII alphabetical.
pub fn group_sites(cookies: &[CookieHost], now_secs: f64) -> Vec<ProfileSite> {
    let mut sites: HashMap<String, (u32, f64)> = HashMap::new();
    let mut host_cache: HashMap<&str, Option<String>> = HashMap::new();

    for c in cookies {
        if let Some(exp) = c.expires
            && (!exp.is_finite() || exp <= now_secs)
        {
            continue;
        }
        let site = host_cache
            .entry(c.host.as_str())
            .or_insert_with(|| cookie_site(&c.host));
        let Some(site_name) = site.as_deref() else {
            continue;
        };
        if site_name.len() > 255 {
            continue;
        }
        let used = if c.used.is_finite() && c.used > 0.0 {
            c.used
        } else {
            0.0
        };
        let entry = sites.entry(site_name.to_owned()).or_insert((0, 0.0));
        entry.0 = entry.0.saturating_add(1);
        if used > entry.1 {
            entry.1 = used;
        }
    }

    let mut out: Vec<(String, u32, f64)> = sites
        .into_iter()
        .map(|(site, (count, used))| (site, count, used))
        .collect();
    out.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    out.into_iter()
        .map(|(site, cookies, _)| ProfileSite { site, cookies })
        .collect()
}

/// Builds a cached predicate checking whether a cookie host belongs to one of `chosen_sites`.
pub fn site_filter(chosen_sites: &[String]) -> impl FnMut(&str) -> bool + use<> {
    let chosen: HashSet<String> = chosen_sites.iter().cloned().collect();
    let mut cache: HashMap<String, bool> = HashMap::new();
    move |host: &str| {
        if let Some(&kept) = cache.get(host) {
            return kept;
        }
        let kept = cookie_site(host).is_some_and(|s| chosen.contains(&s));
        cache.insert(host.to_owned(), kept);
        kept
    }
}

/// Minimal RFC 3492 Punycode encoder for a single lowercase non-ASCII DNS label (`xn--...`).
fn punycode_label(input: &str) -> Option<String> {
    const BASE: u32 = 36;
    const TMIN: u32 = 1;
    const TMAX: u32 = 26;
    const SKEW: u32 = 38;
    const DAMP: u32 = 700;
    const INITIAL_BIAS: u32 = 72;
    const INITIAL_N: u32 = 128;

    let chars: Vec<u32> = input.chars().map(|c| c as u32).collect();
    let mut output = String::from("xn--");
    let mut basic = 0u32;
    for &cp in &chars {
        if cp < 0x80 {
            output.push(char::from_u32(cp)?);
            basic += 1;
        }
    }
    let mut h = basic;
    if basic > 0 {
        output.push('-');
    }

    let mut n = INITIAL_N;
    let mut delta = 0u32;
    let mut bias = INITIAL_BIAS;
    let total = u32::try_from(chars.len()).ok()?;

    while h < total {
        let m = chars.iter().copied().filter(|&c| c >= n).min()?;
        delta = delta.checked_add((m - n).checked_mul(h + 1)?)?;
        n = m;
        for &cp in &chars {
            if cp < n {
                delta = delta.checked_add(1)?;
            } else if cp == n {
                let mut q = delta;
                let mut k = BASE;
                loop {
                    let t = if k <= bias {
                        TMIN
                    } else if k >= bias + TMAX {
                        TMAX
                    } else {
                        k - bias
                    };
                    if q < t {
                        break;
                    }
                    let digit = t + (q - t) % (BASE - t);
                    output.push(encode_digit(digit)?);
                    q = (q - t) / (BASE - t);
                    k += BASE;
                }
                output.push(encode_digit(q)?);
                bias = adapt(delta, h + 1, h == basic, BASE, TMIN, TMAX, SKEW, DAMP);
                delta = 0;
                h += 1;
            }
        }
        delta = delta.checked_add(1)?;
        n = n.checked_add(1)?;
    }
    Some(output)
}

fn encode_digit(d: u32) -> Option<char> {
    match d {
        0..=25 => Some((b'a' + d as u8) as char),
        26..=35 => Some((b'0' + (d - 26) as u8) as char),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn adapt(
    mut delta: u32,
    num_points: u32,
    first_time: bool,
    base: u32,
    tmin: u32,
    tmax: u32,
    skew: u32,
    damp: u32,
) -> u32 {
    delta = if first_time { delta / damp } else { delta / 2 };
    delta += delta / num_points;
    let mut k = 0;
    while delta > ((base - tmin) * tmax) / 2 {
        delta /= base - tmin;
        k += base;
    }
    k + ((base - tmin + 1) * delta) / (delta + skew)
}
