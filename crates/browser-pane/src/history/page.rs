//! Canonical page keys, bounds, and URL normalization for browser history
//! (`packages/gui-plugins/src/browser/history/page.ts`).

/// Maximum pages kept in `history.sqlite` and imported in one pass.
pub const MAX_PAGES: usize = 50_000;
/// Longer URLs are skipped from history & autocomplete.
pub const MAX_URL_LENGTH: usize = 2_048;
/// Stored titles are trimmed and capped at 200 UTF-8 characters.
pub const MAX_TITLE_LENGTH: usize = 200;

/// One page visit row as stored in `history.sqlite` or read from another browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageVisit {
    pub url: String,
    pub title: String,
    /// Total visits (at least 1).
    pub visits: i64,
    /// Visits initiated from an address bar.
    pub typed: i64,
    /// Last visit timestamp in Unix milliseconds.
    pub visited: i64,
}

/// One ranked omnibox search result returned from [`crate::history::HistoryStore::search`].
#[derive(Clone, Debug, PartialEq)]
pub struct FoundPage {
    pub url: String,
    pub title: String,
    pub score: f64,
}

fn is_forbidden_control_or_bidi(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Canonical `(key, host)` for an `http(s)` web URL without credentials and within [`MAX_URL_LENGTH`].
///
/// Key = lowercased authority without `www.` + pathname without trailing `/` + query + fragment,
/// so `http`, `https`, `www.`, and trailing slash collapse onto one row.
/// Returns `None` for non-web schemes (`file:`, `workspace:`, `artifact:`, `oc-preview:`, `about:`,
/// `data:`, `chrome-extension:`, etc.), malformed hosts, C0/C1/bidi controls, or URLs carrying `user:pass@`.
pub fn web_page(url: &str) -> Option<(String, String)> {
    if url.len() > MAX_URL_LENGTH || url.is_empty() {
        return None;
    }
    let rest = if let Some(r) = url.get(..7).filter(|s| s.eq_ignore_ascii_case("http://")).map(|_| &url[7..]) {
        r
    } else if let Some(r) = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..]) {
        r
    } else {
        return None;
    };
    if rest.chars().any(|c| c == ' ' || is_forbidden_control_or_bidi(c)) {
        return None;
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host_lower, port_suffix) = split_authority(authority)?;
    let canon_host = host_lower.strip_prefix("www.").unwrap_or(&host_lower);
    let tail = &rest[auth_end..];
    let (path_part, qf_part) = match tail.find(['?', '#']) {
        Some(idx) => (&tail[..idx], &tail[idx..]),
        None => (tail, ""),
    };
    let clean_path = path_part.trim_end_matches('/');
    let key = format!("{canon_host}{port_suffix}{clean_path}{qf_part}");
    Some((key, host_lower))
}

/// Canonical key of `web_page`; `None` for URLs history never keeps.
pub fn page_key(url: &str) -> Option<String> {
    web_page(url).map(|(k, _)| k)
}

/// Strips C0/C1 controls and Unicode bidi overrides, trims, and bounds a page title to
/// [`MAX_TITLE_LENGTH`] Unicode scalar values.
pub fn page_title(title: &str) -> String {
    title
        .trim()
        .chars()
        .filter(|&c| !is_forbidden_control_or_bidi(c))
        .take(MAX_TITLE_LENGTH)
        .collect()
}

/// Strips `http://` / `https://` and a leading `www.` (ASCII case-insensitive).
pub fn bare(url: &str) -> &str {
    let no_scheme = if url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
        &url[8..]
    } else if url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) {
        &url[7..]
    } else {
        url
    };
    if no_scheme.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("www.")) {
        &no_scheme[4..]
    } else {
        no_scheme
    }
}

/// Extracts `scheme://authority` origin from an `http(s)` URL.
pub(crate) fn origin_of(url: &str) -> Option<String> {
    let scheme_len = if url.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://")) {
        8
    } else if url.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://")) {
        7
    } else {
        return None;
    };
    let scheme = url[..scheme_len].to_ascii_lowercase();
    let rest = &url[scheme_len..];
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(format!("{scheme}{}", authority.to_ascii_lowercase()))
}

fn split_authority(authority: &str) -> Option<(String, String)> {
    if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host = authority[..=close].to_ascii_lowercase();
        let after = &authority[close + 1..];
        let port = if let Some(p) = after.strip_prefix(':') {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            format!(":{p}")
        } else if after.is_empty() {
            String::new()
        } else {
            return None;
        };
        return Some((host, port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            Some((h.to_ascii_lowercase(), format!(":{p}")))
        }
        Some(_) => None,
        None => Some((authority.to_ascii_lowercase(), String::new())),
    }
}
