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
    ///   `browser_policy::classify_app_link` (safe `mailto:` opens the mail client; `mailto:` with
    ///   attachment params, editor schemes, and non-open/search `obsidian:` copy only; allowed
    ///   app schemes open an in-pane 500 ms guarded review banner or copy if unavailable; denied
    ///   schemes like `javascript:`, `data:`, `ms-msdt:`, `opencode*` are silently refused).
    pub fn route_web_or_app_link(
        &mut self,
        sid: Option<&str>,
        href: &str,
        source: LinkSource,
        shift_override: Option<bool>,
        tx: &mut Tx,
    ) {
        self.route_web_or_app_link_inner(sid, href, source, shift_override, true, tx);
    }

    /// Routes a web/app link initiated by a third-party plugin (`Links:Open`): refuses all
    /// `artifact://` and same-server `/api/experimental/artifacts/...` links so third-party
    /// plugins can never mint artifact preview grants (`notes/review-rd281.md` PR 2 Blocker #1).
    pub fn route_plugin_web_or_app_link(
        &mut self,
        sid: Option<&str>,
        href: &str,
        source: LinkSource,
        tx: &mut Tx,
    ) {
        self.route_web_or_app_link_inner(sid, href, source, None, false, tx);
    }

    fn route_web_or_app_link_inner(
        &mut self,
        sid: Option<&str>,
        href: &str,
        _source: LinkSource,
        shift_override: Option<bool>,
        allow_artifacts: bool,
        tx: &mut Tx,
    ) {
        let trimmed = href.trim();
        if trimmed.is_empty()
            || is_file_scheme(trimmed)
            || browser_policy::preview::is_preview_scheme(trimmed)
            || browser_policy::preview::is_workspace_scheme(trimmed)
        {
            return;
        }
        if !allow_artifacts {
            if browser_policy::preview::is_artifact_scheme(trimmed) {
                return;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if let Some((_, srv)) = self.preview_server_for(sid)
                && browser_policy::preview::artifact_link(trimmed, &srv.base).is_some()
            {
                return;
            }
        }
        // Artifact links (`artifact://<id>/<path>` or same-server `/api/experimental/artifacts/<id>/view/...`)
        // open directly in an isolated preview tab when a session is active (built-in/user only).
        if allow_artifacts
            && let Some(sid) = sid
            && self.try_open_artifact_link(sid, trimmed)
        {
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
                crate::open_in::open_external(tx, trimmed);
            }
            return;
        }

        // Non-HTTP(S) scheme: validate the ORIGINAL string without trimming away unsafe characters.
        match classify_app_link(href, AppSchemeSource::Conversation) {
            Some(AppSchemeAction::Mail) => {
                if self.live() {
                    crate::open_in::open_external(tx, href);
                }
            }
            Some(AppSchemeAction::Open) => {
                if let Some(sid) = sid
                    && !self.narrow()
                    && !self.browser_unsupported_for_links()
                    && self.queue_browser_app_link_review(sid, href)
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

    fn queue_browser_app_link_review(&mut self, sid: &str, href: &str) -> bool {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (sid, href);
            false
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tab_id = if let Some(existing) = self.default_browser_tab_for_session(sid) {
                let key = crate::panels::panel_key(super::PLUGIN, &existing.to_string());
                self.open_side_tab_for_session(sid, &key, None, false, false);
                existing
            } else {
                self.open_new_browser_tab_for_session(Some(sid), "about:blank", None, true)
            };
            self.workspace_panes.browser.pending_app_links.borrow_mut().push_back((tab_id, href.to_owned()));
            crate::plugin_host::bump();
            true
        }
    }
}
