//! Address bar input resolution matching OpenCode Desktop (`packages/gui-plugins/src/browser/address.ts`).

use crate::BrowserTab;

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
