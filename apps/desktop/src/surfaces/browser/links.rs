//! Policy-first link routing into the embedded browser pane (`#63002`, `ours.md` §4):
//! chat Markdown links, rendered file links, terminal plain URLs, confirmed OSC 8 hyperlinks,
//! and app-scheme links all pass through `browser_policy` here. The policy — never the link's
//! source — decides what opens in the pane, what opens in the OS browser, what shows a guarded
//! review banner or copies to the clipboard, and what is refused.

use browser_policy::{AppSchemeAction, AppSchemeSource, classify_app_link, destination_origin, is_file_scheme, is_http_with_credentials};
use rune::{PointerButton, Tx};

use crate::App;

/// Where a clicked web or app-scheme link originated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSource {
    Chat,
    FileViewer,
    Terminal,
}

impl App {
    /// Routes a web or app-scheme URL clicked in a session through `browser_policy` and
    /// `Settings > Browser` (`Open links in OpenCode`).
    ///
    /// - `http:` / `https:` without credentials: opens in the embedded browser pane when
    ///   `browser_links_enabled()` is on, a session is active, the window is wide, the browser
    ///   is supported, and neither Shift nor middle-click forced the system browser; otherwise
    ///   opens in the OS default browser (live runs only).
    /// - `http:` / `https:` with credentials (`user:pass@host`): refused (never opened).
    /// - `file:`: never handed to the OS browser (`ours.md` §4).
    /// - App-scheme links (`mailto:`, `slack:`, `obsidian:`, `vscode:`, etc.): validated by
    ///   `browser_policy::classify_app_link` (RFC 6068 allow-listed `mailto:` and the 18
    ///   conversation app schemes open an in-pane 500 ms guarded review banner bound to a
    ///   single-use host token, showing all `to`/`cc`/`bcc` recipients for `mailto:`, or copy if
    ///   unavailable; non-allow-listed `mailto:` keys, conversation editor schemes, and non-open
    ///   `obsidian:` copy only; denied schemes like `javascript:`, `data:`, `ms-msdt:`, `opencode*`
    ///   are silently refused).
    pub fn route_web_or_app_link(
        &mut self,
        sid: Option<&str>,
        href: &str,
        _source: LinkSource,
        shift_override: Option<bool>,
        tx: &mut Tx,
    ) {
        let trimmed = href.trim();
        if trimmed.is_empty() || is_file_scheme(trimmed) {
            return;
        }
        let is_http = trimmed.get(..7).is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || trimmed.get(..8).is_some_and(|s| s.eq_ignore_ascii_case("https://"));
        if is_http {
            if is_http_with_credentials(trimmed) {
                crate::status_toast::fail(
                    self,
                    opencode_i18n::t!("plugin.browser.refused.credentials").to_owned(),
                    String::new(),
                );
                return;
            }
            if destination_origin(trimmed).is_none() {
                return;
            }
            let shift = shift_override.unwrap_or_else(|| tx.mods().shift);
            let middle = tx.button() == PointerButton::Middle;
            let open_in_pane = !shift
                && !middle
                && sid.is_some()
                && !self.narrow()
                && self.browser_links_enabled()
                && !self.browser_unsupported_for_links();
            if open_in_pane {
                self.open_new_browser_tab_for_session(sid, trimmed, None, true);
            } else if self.live() {
                if self.external_url_allowed_for_session(sid, trimmed) {
                    crate::open_in::open_external(tx, trimmed);
                } else {
                    crate::status_toast::fail(
                        self,
                        opencode_i18n::t!("rune.browser.menu.pageUnavailable").to_owned(),
                        String::new(),
                    );
                }
            }
            return;
        }

        // Non-HTTP(S) scheme: validate the ORIGINAL string without trimming away unsafe characters.
        // Both conversation `mailto:` (showing every to/cc/bcc recipient before opening) and the
        // 23 allow-listed app schemes route through the 500 ms guarded review bar with a single-
        // use host-minted token — never one-click launching OS app handlers.
        match classify_app_link(href, AppSchemeSource::Conversation) {
            Some(AppSchemeAction::Mail | AppSchemeAction::Open) => {
                if let Some(sid) = sid
                    && !self.narrow()
                    && !self.browser_unsupported_for_links()
                    && self.queue_browser_app_link_review(sid, href, AppSchemeSource::Conversation)
                {
                    return;
                }
                tx.copy(href.to_owned());
                crate::status_toast::say(self, opencode_i18n::t!("common.copied").to_owned(), String::new());
            }
            Some(AppSchemeAction::Copy) => {
                tx.copy(href.to_owned());
                crate::status_toast::say(self, opencode_i18n::t!("common.copied").to_owned(), String::new());
            }
            None => {}
        }
    }

    fn browser_unsupported_for_links(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            true
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.browser_unsupported()
        }
    }

    pub(crate) fn queue_browser_app_link_review(
        &mut self,
        sid: &str,
        href: &str,
        source: AppSchemeSource,
    ) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (sid, href, source);
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Never fall back to another session's focused browser tab (`review-rd-tools.md` §2).
            let existing_in_session = self.active_browser_tab_for_session(sid).or_else(|| {
                self.workspace_panes
                    .browser
                    .session_tabs
                    .get(sid)?
                    .iter()
                    .rev()
                    .copied()
                    .find(|&id| self.workspace_panes.browser.host.tab(id).is_some())
            });
            let tab_id = if let Some(existing) = existing_in_session {
                let key = crate::panels::panel_key(super::PLUGIN, &existing.to_string());
                self.open_side_tab_for_session(sid, &key, None, false, false);
                existing
            } else {
                self.open_new_browser_tab_for_session(Some(sid), "about:blank", None, true)
            };
            self.queue_browser_app_link_review_for_tab(tab_id, href, source)
        }
    }

    pub(crate) fn queue_browser_app_link_review_for_tab(
        &mut self,
        tab_id: u64,
        href: &str,
        source: AppSchemeSource,
    ) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (tab_id, href, source);
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.workspace_panes.browser.host.tab(tab_id).is_none()
                || classify_app_link(href, source).is_none()
            {
                return false;
            }
            let seq = self.workspace_panes.browser.app_review_seq.get().wrapping_add(1);
            self.workspace_panes.browser.app_review_seq.set(seq);
            let mut pending = self.workspace_panes.browser.pending_app_links.borrow_mut();
            if pending.len() >= 64 {
                pending.pop_front();
            }
            pending.push_back(super::AppLinkReview {
                token: format!("rev-{seq}"),
                tab_id,
                url: href.to_owned(),
                source,
                shown_at: rune::time::Instant::now(),
            });
            drop(pending);
            crate::plugin_host::bump();
            true
        }
    }
}
