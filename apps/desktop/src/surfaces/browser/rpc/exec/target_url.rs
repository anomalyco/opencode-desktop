//! WHATWG URL parsing & canonicalization backed by the standard `url` crate (with IDNA UTS #46)
//! so server authorization (`compute_target`), local URL policy (`browser_policy::refusal`),
//! and Chromium (`open_new_agent_browser_tab_for_session` / `navigate_browser_tab`) all operate
//! on the exact same `Url::as_str()` string.

use browser_pane::policy::{extension_origin, extension_page_refusal, with_scheme};
use url::{Host, Url};

/// Parses and canonicalizes an agent navigation URL (`tabs.open` / `navigate`).
/// Returns the exact WHATWG-canonical URL passed to both server policy and Chromium, failing
/// closed on invalid URLs, embedded credentials, multiple trailing host dots, or extension origins.
pub(super) fn parse_agent_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if extension_origin(trimmed) {
        return Err(extension_page_refusal(trimmed));
    }
    let with_s = with_scheme(trimmed);
    let mut parsed = Url::parse(&with_s).map_err(|e| format!("Invalid URL '{trimmed}': {e}"))?;
    if matches!(parsed.scheme(), "http" | "https") {
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Embedded credentials in URLs are not allowed.".to_owned());
        }
        if let Some(Host::Domain(dom)) = parsed.host()
            && let Some(stripped) = dom.strip_suffix('.')
        {
            if stripped.is_empty() || stripped.ends_with('.') {
                return Err(format!("Invalid URL host in '{trimmed}'."));
            }
            let normalized = stripped.to_owned();
            parsed
                .set_host(Some(&normalized))
                .map_err(|e| format!("Invalid URL host in '{trimmed}': {e}"))?;
        }
    }
    let out = parsed.as_str().to_owned();
    if extension_origin(&out) {
        return Err(extension_page_refusal(&out));
    }
    Ok(out)
}
