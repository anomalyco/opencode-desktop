//! App-side `oc-preview://` routing and authenticated remote/artifact proxy runner (`#63002`).
//!
//! Responsibilities:
//! - Classify workspace `file://`, remote `workspace://<sid>/...`, `artifact://<id>/...`, and
//!   same-server `/api/experimental/artifacts/<id>/view/...` URLs into isolated [`PreviewSpec`]s.
//! - Reject direct `oc-preview://`, cross-session `workspace://`, outside-workspace `file://`,
//!   and (for agent requests) hidden/secret entry HTML documents.
//! - Fail closed on older engines once `Ready { preview: false }` is known (never failing during
//!   `Connecting`, never falling back to `file://`, and never sending `OpenTab("about:blank")`).
//! - Answer `EngineMsg::PreviewFetch` on a bounded worker pool (8 workers, 64 global slots,
//!   16/tab RAII permit, `catch_unwind`) using strictly the tab's bound owning session `Server`
//!   handle (`max_redirects(0)`, `Accept-Encoding: identity`, 15 s header / 30 s read / 120 s
//!   global timeouts, immediate socket shutdown via `PreviewAbort`, bounded chunk back-pressure,
//!   exact status/length/range checks, single-encoded remote paths, and generation-guarded errors).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use base64::Engine as _;
use browser_pane::policy::preview::{
    MAX_ARTIFACT_BYTES, MAX_PREVIEW_FILE_BYTES, RangeSpec, artifact_address, artifact_link,
    encode_rel_path, is_artifact_scheme, is_preview_scheme, is_valid_artifact_id,
    is_workspace_scheme, local_display_url, normalize_range_header, parse_content_range,
    parse_prefixed_path, parse_range, preview_allowed, preview_path_valid, relative_to_root,
    root_is_windows, sanitize_response_mime, workspace_prefix,
};
use browser_pane::{EngineStatus, PreviewFetchRequest, PreviewKindTag, PreviewResponder, PreviewSpec};
use opencode_kit::client::PreviewAbort;
use opencode_servers::ServerKey;

use super::PLUGIN;
use super::preview_pool::PreviewFetchPool;
use crate::App;
use crate::panels::{SessionPane, panel_key};

/// Tab-scoped binding pinning a preview tab strictly to the owning session's live server handle,
/// a monotonic generation, an atomic liveness flag, an HTTP socket abort group, and a per-tab
/// concurrent-fetch counter.
pub(crate) struct PreviewTabBinding {
    pub server_key: ServerKey,
    pub server: opencode_kit::client::Server,
    pub generation: u64,
    pub alive: Arc<AtomicBool>,
    pub abort: PreviewAbort,
    pub in_flight: Arc<AtomicUsize>,
}

struct BoundFetch {
    server: opencode_kit::client::Server,
    generation: u64,
    alive: Arc<AtomicBool>,
    abort: PreviewAbort,
    in_flight: Arc<AtomicUsize>,
}

pub enum PreviewTarget {
    NotPreview,
    Refused(&'static str),
    Preview {
        display_url: String,
        spec: PreviewSpec,
    },
}

impl App {
    pub(crate) fn home_dir_opt(&self) -> Option<&str> {
        (!self.home.is_empty()).then_some(self.home.as_str())
    }

    /// Whether the active browser engine supports isolated `oc-preview://` tabs.
    /// Returns `true` while disconnected OR still in `Connecting` (before `Ready` arrives) so
    /// tabs wait cleanly in `pending_opens`; returns `false` ONLY once `Ready { preview: false }`
    /// is known (`notes/review-rd281.md` PR 2 Blocker #4).
    pub fn browser_preview_supported(&self) -> bool {
        self.use_mock_browser()
            || !self.workspace_panes.browser.host.engine_caps_known()
            || self.workspace_panes.browser.host.engine_preview()
    }

    /// Resolves `(ServerKey, Server)` strictly for `sid` (never substituting `active` for a parked
    /// session). Only falls back to `active` when `sid` is `None`.
    pub(crate) fn preview_server_for(&self, sid: Option<&str>) -> Option<(ServerKey, opencode_kit::client::Server)> {
        if let Some(id) = sid {
            let key = crate::servers::runtime::session_server_key(self, id)
                .or_else(|| crate::surfaces::browser::rpc::server_for_session(self, id))?;
            let srv = crate::servers::runtime::live_server_for_key(self, &key)?;
            return Some((key, srv));
        }
        let key = crate::servers::runtime::active(self).clone();
        crate::servers::runtime::live_server_for_key(self, &key).map(|srv| (key, srv))
    }

    /// Whether session `sid` lives on the local sidecar disk (`is_sidecar && server.is_local()`).
    pub(crate) fn session_server_is_local(&self, sid: Option<&str>) -> bool {
        if let Some((key, srv)) = self.preview_server_for(sid) {
            return key.is_sidecar() && srv.is_local();
        }
        if let Some(id) = sid
            && let Some(key) = crate::servers::runtime::session_server_key(self, id)
                .or_else(|| crate::surfaces::browser::rpc::server_for_session(self, id))
        {
            return key.is_sidecar();
        }
        crate::servers::runtime::active(self).is_sidecar()
    }

    pub(crate) fn bind_preview_tab(&mut self, tab_id: u64, sid: Option<&str>) {
        self.revoke_preview_binding(tab_id);
        if let Some((server_key, server)) = self.preview_server_for(sid) {
            self.workspace_panes.browser.next_preview_gen =
                self.workspace_panes.browser.next_preview_gen.wrapping_add(1).max(1);
            let generation = self.workspace_panes.browser.next_preview_gen;
            self.workspace_panes.browser.preview_bindings.insert(
                tab_id,
                PreviewTabBinding {
                    server_key,
                    server,
                    generation,
                    alive: Arc::new(AtomicBool::new(true)),
                    abort: PreviewAbort::new(),
                    in_flight: Arc::new(AtomicUsize::new(0)),
                },
            );
        }
    }

    pub(crate) fn revoke_preview_binding(&mut self, tab_id: u64) {
        if let Some(binding) = self.workspace_panes.browser.preview_bindings.remove(&tab_id) {
            binding.alive.store(false, Ordering::Release);
            binding.abort.abort();
        }
    }

    /// Classifies `url` for session `sid` into either a normal web navigation, a refusal code,
    /// or an isolated preview grant specification (`Local`, `Remote`, or `Artifact`).
    /// When `agent` is `true`, hidden/dotfile entry HTML documents are strictly refused.
    pub fn resolve_preview_target(&self, sid: Option<&str>, url: &str, agent: bool) -> PreviewTarget {
        let trimmed = url.trim();
        let has_local_roots = !self.browser_file_roots(sid).is_empty();
        let workspace_refusal = if has_local_roots {
            "browser.address.workspace"
        } else {
            "browser.address.web"
        };
        let allow_hidden_entry = !agent;

        if is_preview_scheme(trimmed) {
            return PreviewTarget::Refused(workspace_refusal);
        }
        if is_artifact_scheme(trimmed) {
            return self.resolve_artifact_target(trimmed);
        }
        if let Some((_, srv)) = self.preview_server_for(sid)
            && let Some(addr) = artifact_link(trimmed, &srv.base)
        {
            return self.resolve_artifact_target(&addr);
        }
        if is_workspace_scheme(trimmed) {
            let Some(sid) = sid else {
                return PreviewTarget::Refused("browser.address.web");
            };
            let prefix = workspace_prefix(sid);
            let dir = self.link_directory(sid);
            let Some(rel) = parse_prefixed_path(trimmed, &prefix, root_is_windows(&dir)) else {
                return PreviewTarget::Refused("browser.address.workspace");
            };
            return self.resolve_remote_workspace_target(sid, &rel, allow_hidden_entry);
        }
        if browser_pane::policy::is_file_scheme(trimmed) {
            let roots = self.browser_file_roots(sid);
            if !browser_pane::policy::file_url_within(trimmed, &roots) {
                return PreviewTarget::Refused(workspace_refusal);
            }
            let Some(target) = browser_pane::policy::local_file_path(trimmed) else {
                return PreviewTarget::Refused(workspace_refusal);
            };
            let Some((root, rel)) = roots.iter().find_map(|r| relative_to_root(&target, r).map(|rel| (r.clone(), rel))) else {
                return PreviewTarget::Refused(workspace_refusal);
            };
            if !preview_allowed(&root, &rel, &rel, allow_hidden_entry, self.home_dir_opt()) {
                return PreviewTarget::Refused("browser.address.workspace");
            }
            let suffix = trimmed.find(['?', '#']).map(|i| &trimmed[i..]).unwrap_or("");
            let display_url = local_display_url(&root, &rel, suffix);
            return PreviewTarget::Preview {
                display_url,
                spec: PreviewSpec::Local { root, document: rel },
            };
        }
        PreviewTarget::NotPreview
    }

    fn resolve_artifact_target(&self, address: &str) -> PreviewTarget {
        let Some(item) = artifact_address(address) else {
            return PreviewTarget::Refused("browser.address.web");
        };
        let decoded = if item.path.is_empty() {
            String::new()
        } else {
            let Some(d) = parse_prefixed_path(&item.path, "", true) else {
                return PreviewTarget::Refused("browser.address.web");
            };
            d
        };
        let display_url = format!("artifact://{}/{}", item.id, item.path);
        PreviewTarget::Preview {
            display_url,
            spec: PreviewSpec::Artifact { artifact_id: item.id, path: decoded },
        }
    }

    pub(crate) fn resolve_workspace_file_target(
        &self,
        sid: &str,
        rel_path: &str,
        agent: bool,
    ) -> PreviewTarget {
        let norm = rel_path.replace('\\', "/");
        let rel = norm.trim_start_matches('/');
        let allow_hidden_entry = !agent;
        if self.session_server_is_local(Some(sid)) {
            let roots = self.browser_file_roots(Some(sid));
            let Some(root) = roots.first().cloned() else {
                return PreviewTarget::Refused("browser.address.workspace");
            };
            if !preview_path_valid(rel, cfg!(windows))
                || !preview_allowed(&root, rel, rel, allow_hidden_entry, self.home_dir_opt())
            {
                return PreviewTarget::Refused("browser.address.workspace");
            }
            let display_url = local_display_url(&root, rel, "");
            return PreviewTarget::Preview {
                display_url,
                spec: PreviewSpec::Local { root, document: rel.to_owned() },
            };
        }
        self.resolve_remote_workspace_target(sid, rel, allow_hidden_entry)
    }

    fn resolve_remote_workspace_target(
        &self,
        sid: &str,
        rel: &str,
        allow_hidden_entry: bool,
    ) -> PreviewTarget {
        let dir = self.link_directory(sid);
        let is_win = root_is_windows(&dir);
        if dir.is_empty()
            || !preview_path_valid(rel, is_win)
            || !preview_allowed(&dir, rel, rel, allow_hidden_entry, None)
        {
            return PreviewTarget::Refused("browser.address.workspace");
        }
        let prefix = workspace_prefix(sid);
        let display_url = format!("{prefix}{}", encode_rel_path(rel));
        PreviewTarget::Preview {
            display_url,
            spec: PreviewSpec::Remote {
                root: dir,
                session_prefix: prefix,
                document: rel.to_owned(),
            },
        }
    }

    /// Opens a workspace HTML/file preview tab (local or remote) in its own isolated context.
    /// Never falls back to `file://` on older engines lacking `Ready.preview`.
    pub fn open_workspace_preview_tab(
        &mut self,
        sid: &str,
        rel_path: &str,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
    ) -> u64 {
        if !self.browser_preview_supported() {
            return self.open_refused_browser_tab(
                Some(sid),
                "browser.preview.unsupported",
                target_pane,
                steal_focus,
            );
        }
        match self.resolve_workspace_file_target(sid, rel_path, false) {
            PreviewTarget::Preview { display_url, spec } => {
                self.open_resolved_preview_tab(Some(sid), &display_url, spec, target_pane, steal_focus)
            }
            PreviewTarget::Refused(code) => {
                self.open_refused_browser_tab(Some(sid), code, target_pane, steal_focus)
            }
            PreviewTarget::NotPreview => {
                self.open_refused_browser_tab(Some(sid), "browser.address.workspace", target_pane, steal_focus)
            }
        }
    }

    /// Opens an artifact preview tab if `href` names an artifact on the owning session's server or `artifact://`.
    pub fn try_open_artifact_link(&mut self, sid: &str, href: &str) -> bool {
        if !self.browser_plugin_ready() {
            return false;
        }
        match self.resolve_preview_target(Some(sid), href, false) {
            PreviewTarget::Preview {
                display_url,
                spec: spec @ PreviewSpec::Artifact { .. },
            } => {
                if !self.browser_preview_supported() {
                    self.open_refused_browser_tab(Some(sid), "browser.preview.unsupported", None, true);
                } else {
                    self.open_resolved_preview_tab(Some(sid), &display_url, spec, None, true);
                }
                true
            }
            _ => false,
        }
    }

    pub(crate) fn open_resolved_preview_tab(
        &mut self,
        sid: Option<&str>,
        display_url: &str,
        spec: PreviewSpec,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
    ) -> u64 {
        if !self.browser_preview_supported() {
            return self.open_refused_browser_tab(
                sid,
                "browser.preview.unsupported",
                target_pane,
                steal_focus,
            );
        }
        let id = if self.use_mock_browser() {
            let tag = spec.tag();
            let title = preview_title(display_url);
            let id = self
                .workspace_panes
                .browser
                .host
                .insert_mock_tab(display_url, &title, EngineStatus::Ready);
            self.workspace_panes.browser.host.set_mock_history(id, false, false);
            self.workspace_panes.browser.host.set_mock_preview(id, tag, spec);
            id
        } else {
            self.workspace_panes
                .browser
                .host
                .open_preview_tab(display_url, spec)
        };
        self.bind_preview_tab(id, sid);
        self.attach_browser_tab_to_session(sid, id, target_pane, steal_focus);
        id
    }

    pub(crate) fn reopen_resolved_preview_tab(
        &mut self,
        id: u64,
        sid: Option<&str>,
        display_url: &str,
        spec: PreviewSpec,
    ) {
        if !self.browser_preview_supported() {
            self.revoke_preview_binding(id);
            self.workspace_panes
                .browser
                .host
                .set_mock_refusal(id, "browser.preview.unsupported");
            return;
        }
        self.bind_preview_tab(id, sid);
        if self.use_mock_browser() {
            let tag = spec.tag();
            let title = preview_title(display_url);
            self.workspace_panes.browser.host.set_mock_page(id, display_url, &title);
            self.workspace_panes.browser.host.set_mock_preview(id, tag, spec);
        } else {
            self.workspace_panes
                .browser
                .host
                .reopen_tab_as_preview(id, display_url, spec);
        }
    }

    /// Opens a locally refused tab WITHOUT sending any `HostMsg::OpenTab` to the engine.
    pub(crate) fn open_refused_browser_tab(
        &mut self,
        sid: Option<&str>,
        code: &str,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
    ) -> u64 {
        let id = if self.use_mock_browser() {
            let id = self
                .workspace_panes
                .browser
                .host
                .insert_mock_tab("about:blank", "", EngineStatus::Ready);
            self.workspace_panes.browser.host.set_mock_history(id, false, false);
            self.workspace_panes.browser.host.set_mock_refusal(id, code);
            id
        } else {
            let ctx = self.browser_context_for_session(sid);
            self.workspace_panes.browser.host.open_refused_tab(code, ctx)
        };
        self.attach_browser_tab_to_session(sid, id, target_pane, steal_focus);
        id
    }

    pub(crate) fn attach_browser_tab_to_session(
        &mut self,
        sid: Option<&str>,
        id: u64,
        target_pane: Option<SessionPane>,
        steal_focus: bool,
    ) {
        if let Some(sid) = sid {
            self.workspace_panes
                .browser
                .session_tabs
                .entry(sid.to_owned())
                .or_default()
                .push(id);
            let key = panel_key(PLUGIN, &id.to_string());
            if steal_focus {
                self.open_side_tab_for_session(sid, &key, target_pane, false, false);
            } else {
                self.keeping_keyboard(|s| {
                    s.open_side_tab_for_session(sid, &key, target_pane, false, false)
                });
            }
        }
    }

    /// Normalizes a URL for agent tools (`Browser:NormalizeUrl`), allowing local workspace `file://`,
    /// same-session `workspace://<sid>/...`, and `artifact://<id>/...` when supported and non-hidden.
    pub fn normalize_browser_url_for_session(
        &self,
        sid: Option<&str>,
        raw: &str,
        agent: bool,
    ) -> Result<String, String> {
        let candidate = browser_pane::with_scheme(raw);
        let live_no_preview = self.workspace_panes.browser.host.engine_caps_known()
            && !self.workspace_panes.browser.host.engine_preview();
        match self.resolve_preview_target(sid, &candidate, agent) {
            PreviewTarget::Preview { display_url, .. } => {
                if live_no_preview {
                    Err("Previews require an updated browser engine.".to_owned())
                } else {
                    Ok(display_url)
                }
            }
            PreviewTarget::Refused(code) => {
                let has_roots = !self.browser_file_roots(sid).is_empty();
                let why = match code {
                    "browser.address.credentials" => browser_pane::policy::Refusal::Credentials,
                    "browser.address.workspace" => browser_pane::policy::Refusal::Workspace,
                    _ => browser_pane::policy::Refusal::Web,
                };
                Err(why.agent_message(has_roots).to_owned())
            }
            PreviewTarget::NotPreview => browser_pane::normalize_url(&candidate, &[]),
        }
    }

    /// Drains `EngineMsg::PreviewFetch` items and schedules authenticated streaming readers on the
    /// bounded `PreviewFetchPool` using strictly the tab's bound owning session `Server`.
    pub(crate) fn pump_preview_fetches(&mut self) -> bool {
        let mut changed = false;
        let errs: Vec<(u64, u64, String)> = self
            .workspace_panes
            .browser
            .preview_errors
            .as_ref()
            .map(|(_, rx)| std::iter::from_fn(|| rx.try_recv().ok()).collect())
            .unwrap_or_default();
        for (tab_id, generation, code) in errs {
            let gen_ok = self
                .workspace_panes
                .browser
                .preview_bindings
                .get(&tab_id)
                .is_some_and(|b| b.generation == generation);
            if gen_ok {
                self.workspace_panes.browser.host.set_tab_load_error(tab_id, Some(code));
                changed = true;
            }
        }
        // Revoke bindings (and immediately close sockets) for closed tabs or signed-out servers.
        let dead_tabs: Vec<u64> = self
            .workspace_panes
            .browser
            .preview_bindings
            .iter()
            .filter_map(|(&tab_id, b)| {
                let live_tab = self.workspace_panes.browser.host.tab(tab_id).is_some();
                let live_srv = crate::servers::runtime::live_server_for_key(self, &b.server_key).is_some();
                (!(live_tab && live_srv)).then_some(tab_id)
            })
            .collect();
        for tab_id in dead_tabs {
            self.revoke_preview_binding(tab_id);
        }
        if !self.workspace_panes.browser.host.has_preview_fetches() {
            return changed;
        }
        let err_tx = self
            .workspace_panes
            .browser
            .preview_errors
            .get_or_insert_with(std::sync::mpsc::channel)
            .0
            .clone();
        let fetches = self.workspace_panes.browser.host.drain_preview_fetches();
        let responder = self.workspace_panes.browser.host.preview_responder();
        let waker = (*self.waker).clone();
        for req in fetches {
            let spec = self
                .workspace_panes
                .browser
                .host
                .tab(req.tab_id)
                .and_then(|t| t.preview_spec.clone());
            let bound = self
                .workspace_panes
                .browser
                .preview_bindings
                .get(&req.tab_id)
                .filter(|b| b.alive.load(Ordering::Acquire))
                .map(|b| BoundFetch {
                    server: b.server.clone(),
                    generation: b.generation,
                    alive: Arc::clone(&b.alive),
                    abort: b.abort.clone(),
                    in_flight: Arc::clone(&b.in_flight),
                });
            dispatch_preview_fetch(req, spec, bound, responder.clone(), err_tx.clone(), waker.clone());
        }
        true
    }
}

fn dispatch_preview_fetch(
    req: PreviewFetchRequest,
    spec: Option<PreviewSpec>,
    bound: Option<BoundFetch>,
    responder: PreviewResponder,
    err_tx: std::sync::mpsc::Sender<(u64, u64, String)>,
    waker: Option<rune::Waker>,
) {
    let method_ok = req.method.eq_ignore_ascii_case("GET") || req.method.eq_ignore_ascii_case("HEAD");
    if !method_ok {
        responder.send_head(req.req_id, false, 405, "application/octet-stream".into(), None, None);
        return;
    }
    let Some(bound) = bound else {
        responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
        return;
    };
    let (route, max_bytes, prefer_server_mime, mime_hint, is_artifact_entry, is_artifact) =
        match (req.kind, spec) {
            (PreviewKindTag::Remote, Some(PreviewSpec::Remote { root, document, .. })) => {
                let is_win = root_is_windows(&root);
                // `PreviewFetch.path` arrives per-segment percent-encoded from the engine (#281);
                // decode ONCE to validate and re-encode ONCE so paths are never double-encoded.
                let Some(decoded_rel) = parse_prefixed_path(&req.path, "", is_win) else {
                    responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                    return;
                };
                if !preview_path_valid(&decoded_rel, is_win)
                    || !preview_allowed(&root, &document, &decoded_rel, decoded_rel == document, None)
                {
                    responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                    return;
                }
                let encoded = encode_rel_path(&decoded_rel);
                let query = opencode_kit::integration::location_query(Some(&root));
                (
                    format!("/api/fs/read/{encoded}{query}"),
                    MAX_PREVIEW_FILE_BYTES,
                    false,
                    decoded_rel,
                    false,
                    false,
                )
            }
            (PreviewKindTag::Artifact, Some(PreviewSpec::Artifact { artifact_id, path: entry_path })) => {
                if !is_valid_artifact_id(&artifact_id) {
                    responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                    return;
                }
                let expected_prefix = format!("/api/experimental/artifacts/{artifact_id}/view/");
                let Some(tail) = req.path.strip_prefix(&expected_prefix) else {
                    responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                    return;
                };
                if tail.contains(['?', '#', '\\']) {
                    responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                    return;
                }
                let rebuilt_tail = if tail.is_empty() {
                    String::new()
                } else {
                    let Some(decoded_tail) = parse_prefixed_path(tail, "", true) else {
                        responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                        return;
                    };
                    encode_rel_path(&decoded_tail)
                };
                let encoded_entry = if entry_path.is_empty() {
                    String::new()
                } else {
                    encode_rel_path(&entry_path)
                };
                (
                    format!("{expected_prefix}{rebuilt_tail}"),
                    MAX_ARTIFACT_BYTES,
                    true,
                    rebuilt_tail.clone(),
                    rebuilt_tail == encoded_entry,
                    true,
                )
            }
            _ => {
                responder.send_head(req.req_id, false, 403, "application/octet-stream".into(), None, None);
                return;
            }
        };

    let range = if is_artifact {
        None
    } else {
        normalize_range_header(req.range.as_deref())
    };
    let method = if req.method.eq_ignore_ascii_case("HEAD") { "HEAD" } else { "GET" };
    let req_id = req.req_id;
    let tab_id = req.tab_id;
    let responder_panic = responder.clone();
    let responder_full = responder.clone();
    let in_flight = Arc::clone(&bound.in_flight);
    let spawned = PreviewFetchPool::global().try_spawn(
        &in_flight,
        move || {
            responder_panic.send_head(req_id, false, 500, "application/octet-stream".into(), None, None);
        },
        move || {
            if !bound.alive.load(Ordering::Acquire) || bound.abort.is_aborted() {
                responder.send_head(req_id, false, 403, "application/octet-stream".into(), None, None);
                return;
            }
            let mut head_sent = false;
            let mut accepted = false;
            let mut is_416 = false;
            let ok = bound.server.preview_stream(
                method,
                &route,
                range.as_deref(),
                max_bytes,
                &bound.abort,
                |head| {
                    head_sent = true;
                    if !bound.alive.load(Ordering::Acquire) || bound.abort.is_aborted() {
                        responder.send_head(req_id, false, 403, "application/octet-stream".into(), None, None);
                        return false;
                    }
                    if is_artifact_entry && head.status == 404 {
                        let _ = err_tx.send((tab_id, bound.generation, "browser.artifact.missing".into()));
                        if let Some(w) = &waker {
                            w();
                        }
                    }
                    let over_cap = head.content_length.is_some_and(|n| n > max_bytes);
                    let valid_status = if is_artifact {
                        head.status == 200
                    } else if range.is_some() {
                        match head.status {
                            200 => true,
                            416 => true,
                            206 => parse_content_range(head.content_range.as_deref())
                                .is_some_and(|(start, end, total)| {
                                    total <= max_bytes
                                        && matches!(
                                            parse_range(range.as_deref(), total),
                                            RangeSpec::Partial { start: ws, end: we, length: wl, .. }
                                                if start == ws
                                                    && end == we
                                                    && head.content_length == Some(wl)
                                        )
                                }),
                            _ => false,
                        }
                    } else {
                        head.status == 200
                    };
                    let head_ok = valid_status && !over_cap;
                    is_416 = head_ok && head.status == 416;
                    let mime = sanitize_response_mime(&head.content_type, &mime_hint, prefer_server_mime);
                    let sent = responder.send_head(
                        req_id,
                        head_ok,
                        head.status,
                        mime,
                        head.content_length,
                        head.content_range.clone(),
                    );
                    accepted = sent && head_ok && !is_416;
                    accepted
                },
                |chunk| {
                    if !bound.alive.load(Ordering::Acquire) || bound.abort.is_aborted() {
                        return false;
                    }
                    let b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
                    responder.send_chunk(req_id, b64, &bound.alive)
                },
            );
            if !head_sent {
                responder.send_head(req_id, false, 502, "application/octet-stream".into(), None, None);
            } else if is_416 {
                responder.send_end(req_id, true);
            } else if accepted {
                responder.send_end(req_id, ok && bound.alive.load(Ordering::Acquire) && !bound.abort.is_aborted());
            }
        },
    );
    if !spawned {
        responder_full.send_head(req_id, false, 429, "application/octet-stream".into(), None, None);
    }
}

fn preview_title(display_url: &str) -> String {
    let base = display_url.split(['?', '#']).next().unwrap_or(display_url);
    base.rsplit('/')
        .find(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| display_url.to_owned())
}
