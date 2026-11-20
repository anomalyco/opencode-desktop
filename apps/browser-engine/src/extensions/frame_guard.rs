//! Narrow extension sub-frame protection for web tabs (`review-rd274.md` Round-3 §2).
//!
//! Policy:
//! - Extension main frames and popups receive whole-tab refusal (`tab_is_extension_main_surface`).
//! - Regular web tabs that embed `chrome-extension://` sub-frames (such as Bitwarden's
//!   closed-ShadowRoot `overlay/menu-button.html` / `overlay/menu-list.html` or uBlock Origin
//!   Lite's DNR-redirected `noop.html`) remain readable and drivable on the web page itself, while
//!   every extension sub-frame (committed OR pending pre-commit) is:
//!   1. Stripped from `Page.getFrameTree`, `Accessibility.*`, and `DOM.*` responses.
//!   2. Refused on `Input.dispatchMouseEvent` / `dispatchTouchEvent` / `emulateTouchFromMouseEvent`
//!      whenever any coordinate lands inside a matched extension `<iframe>`'s viewport box (or while
//!      an extension sub-frame is still loading pre-commit).
//!   3. Refused on `Input.dispatchKeyEvent` / `insertText` when focus is inside an extension frame
//!      or an extension sub-frame is still loading pre-commit.
//!   4. Fail-closed refused on `Page.captureScreenshot` until the tracked screenshot-masking
//!      follow-up lands (which still gates shipping).

use std::collections::HashSet;

use cef::{Browser, CefString, ImplBrowser, ImplFrame};
use serde_json::Value;

use crate::extensions::manifest::is_extension_scheme;
use crate::prompts::TabPrompts;

/// Maximum extension `<iframe>` nodes queried for box models in a single hit-test pass.
pub const MAX_HIT_TEST_IFRAMES: usize = 64;

/// Snapshot of committed and pending (pre-commit) extension sub-frames on a web tab.
#[derive(Clone, Debug, Default)]
pub struct ExtSubframeState {
    pub has_pending: bool,
    pub ext_frame_ids: HashSet<String>,
    pub non_ext_subframe_urls: HashSet<String>,
}

impl ExtSubframeState {
    pub fn any_ext(&self) -> bool {
        self.has_pending || !self.ext_frame_ids.is_empty()
    }

    pub fn matches_frame_id(&self, frame_id: &str) -> bool {
        if frame_id.is_empty() {
            return false;
        }
        let norm = normalize_frame_id(frame_id);
        self.ext_frame_ids.contains(&norm)
    }
}

/// Which CDP reply filter to apply in `TabCdpObserver` before returning `EngineMsg::CdpResponse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    FrameTree,
    AxTree,
    Dom,
}

/// Completed internal CDP step emitted by `TabCdpObserver` for `EngineManager::poll_hit_tests`.
pub enum HitTestReply {
    Doc { job_id: u64, ok: bool, result: Value },
    Box { job_id: u64, ok: bool, result: Value },
}

/// Inspects all live child frames (via CEF `browser.frame_identifiers()`) plus any in-flight
/// pre-commit extension sub-frames tracked on `prompts`.
pub fn inspect_subframes(browser: &Browser, prompts: &TabPrompts) -> ExtSubframeState {
    let mut state = ExtSubframeState::default();
    let mut ids = cef::CefStringList::new();
    browser.frame_identifiers(Some(&mut ids));
    let mut live_frames: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for id in ids {
        let Some(frame) = browser.frame_by_identifier(Some(&CefString::from(id.as_str()))) else {
            continue;
        };
        if frame.is_main() == 1 {
            continue;
        }
        let url = CefString::from(&frame.url()).to_string();
        if is_extension_scheme(&url) {
            state.ext_frame_ids.insert(normalize_frame_id(&id));
        } else if !url.is_empty() {
            state.non_ext_subframe_urls.insert(url.clone());
        }
        live_frames.insert(id, url);
    }
    // Keep only genuinely in-flight pre-commit sub-frames: prune entries whose pre-swap
    // `RenderFrameHost` was already destroyed during an OOPIF swap OR whose frame URL has already
    // committed as an extension scheme (now tracked in `state.ext_frame_ids`).
    let pending = prompts.retain_pending_ext_subframes(|id| {
        live_frames
            .get(id)
            .is_some_and(|url| !is_extension_scheme(url))
    });
    if !pending.is_empty() {
        state.has_pending = true;
        for id in pending {
            state.ext_frame_ids.insert(normalize_frame_id(&id));
        }
    }
    state
}

/// Whether the tab's currently focused frame is an extension frame.
pub fn focused_frame_is_extension(browser: &Browser) -> bool {
    let Some(frame) = browser.focused_frame() else {
        return false;
    };
    let url = CefString::from(&frame.url()).to_string();
    is_extension_scheme(&url)
}

/// Classifies whether a CDP method needs reply stripping on a tab with extension sub-frames.
pub fn reply_filter_for_method(method: &str) -> Option<FilterKind> {
    match method {
        "Page.getFrameTree" => Some(FilterKind::FrameTree),
        "Accessibility.getFullAXTree"
        | "Accessibility.getPartialAXTree"
        | "Accessibility.getRootAXNode"
        | "Accessibility.getChildAXNodes"
        | "Accessibility.queryAXTree" => Some(FilterKind::AxTree),
        "DOM.getDocument"
        | "DOM.describeNode"
        | "DOM.requestChildNodes"
        | "DOM.getOuterHTML" => Some(FilterKind::Dom),
        _ => None,
    }
}

/// Whether `method` dispatches pointer/touch coordinates that must be hit-tested against extension
/// sub-frame bounding boxes before reaching Chromium.
pub fn is_pointer_input_method(method: &str) -> bool {
    matches!(
        method,
        "Input.dispatchMouseEvent"
            | "Input.dispatchTouchEvent"
            | "Input.emulateTouchFromMouseEvent"
    )
}

/// Whether `method` dispatches keyboard/text input that must check `focused_frame()`.
pub fn is_keyboard_input_method(method: &str) -> bool {
    matches!(method, "Input.dispatchKeyEvent" | "Input.insertText")
}

/// Extracts all CSS viewport `(x, y)` points targeted by a pointer/touch CDP call.
pub fn extract_input_points(method: &str, params: &Value) -> Vec<(f64, f64)> {
    match method {
        "Input.dispatchMouseEvent" | "Input.emulateTouchFromMouseEvent" => {
            match (params.get("x").and_then(Value::as_f64), params.get("y").and_then(Value::as_f64)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => vec![(x, y)],
                _ => Vec::new(),
            }
        }
        "Input.dispatchTouchEvent" => params
            .get("touchPoints")
            .and_then(Value::as_array)
            .map(|pts| {
                pts.iter()
                    .filter_map(|p| {
                        let x = p.get("x").and_then(Value::as_f64)?;
                        let y = p.get("y").and_then(Value::as_f64)?;
                        (x.is_finite() && y.is_finite()).then_some((x, y))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Collects candidate extension `<iframe>` `backendNodeId`s from a pierced `DOM.getDocument`
/// response (walks normal children AND closed/open `shadowRoots[]`).
/// Matches any `IFRAME` whose:
/// 1. `src` attribute has an extension scheme (e.g. Bitwarden closed-ShadowRoot overlays), OR
/// 2. `frameId` matches a known extension sub-frame ID, OR
/// 3. has no inline `contentDocument` (cross-origin OOPIF) AND its `src` is not in the tab's
///    committed non-extension sub-frame URLs (catches uBlock DNR `noop.html` redirects where the
///    DOM `src` attribute still shows the original ad URL).
pub fn find_ext_iframe_backend_ids(doc_result: &Value, state: &ExtSubframeState) -> Vec<i64> {
    let mut out = Vec::new();
    if let Some(root) = doc_result.get("root") {
        walk_dom_for_ext_iframes(root, state, &mut out);
    }
    out
}

fn walk_dom_for_ext_iframes(node: &Value, state: &ExtSubframeState, out: &mut Vec<i64>) {
    if out.len() >= MAX_HIT_TEST_IFRAMES {
        return;
    }
    let Some(obj) = node.as_object() else {
        return;
    };
    let is_iframe = obj
        .get("nodeName")
        .and_then(Value::as_str)
        .is_some_and(|n| n.eq_ignore_ascii_case("IFRAME"));
    if is_iframe {
        let src = attr_value(obj.get("attributes"), "src").unwrap_or("");
        let fid = obj.get("frameId").and_then(Value::as_str).unwrap_or("");
        let has_content_doc = obj.get("contentDocument").is_some_and(|v| !v.is_null());
        let is_ext_src = is_extension_scheme(src);
        let is_ext_fid = state.matches_frame_id(fid);
        let is_unmatched_oopif =
            !has_content_doc && !src.is_empty() && !state.non_ext_subframe_urls.contains(src);
        if (is_ext_src || is_ext_fid || is_unmatched_oopif)
            && let Some(bid) = obj.get("backendNodeId").and_then(Value::as_i64)
            && !out.contains(&bid)
        {
            out.push(bid);
        }
    }
    if let Some(children) = obj.get("children").and_then(Value::as_array) {
        for c in children {
            walk_dom_for_ext_iframes(c, state, out);
        }
    }
    if let Some(shadows) = obj.get("shadowRoots").and_then(Value::as_array) {
        for s in shadows {
            walk_dom_for_ext_iframes(s, state, out);
        }
    }
    if let Some(cd) = obj.get("contentDocument") {
        walk_dom_for_ext_iframes(cd, state, out);
    }
}

/// Parses an axis-aligned viewport bounding box `(min_x, min_y, max_x, max_y)` (expanded by 1 px
/// for sub-pixel rounding safety) from a `DOM.getBoxModel` result.
pub fn parse_box_model_rect(res: &Value) -> Option<(f64, f64, f64, f64)> {
    let quad = res
        .pointer("/model/border")
        .or_else(|| res.pointer("/model/content"))
        .and_then(Value::as_array)?;
    if quad.len() < 8 {
        return None;
    }
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for pair in quad.as_chunks::<2>().0 {
        let x = pair[0].as_f64()?;
        let y = pair[1].as_f64()?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some((min_x - 1.0, min_y - 1.0, max_x + 1.0, max_y + 1.0))
}

/// Whether `DOM.getBoxModel` failed solely because the element is unrendered (`display: none` /
/// zero box), rather than an unexpected CDP error.
pub fn is_unrendered_box_error(res: &Value) -> bool {
    res.get("message")
        .and_then(Value::as_str)
        .is_some_and(|m| m.contains("Could not compute box model") || m.contains("No node with given id"))
}

pub fn point_in_rect((x, y): (f64, f64), (x1, y1, x2, y2): (f64, f64, f64, f64)) -> bool {
    x >= x1 && x <= x2 && y >= y1 && y <= y2
}

/// Filters a CDP reply in-place to strip extension sub-frames and their tokens/URLs.
pub fn filter_cdp_reply(kind: FilterKind, val: &mut Value, ext_frame_ids: &HashSet<String>) {
    match kind {
        FilterKind::FrameTree => {
            if let Some(ft) = val.get_mut("frameTree") {
                strip_frame_tree_node(ft, ext_frame_ids);
            }
        }
        FilterKind::AxTree => strip_ax_tree(val, ext_frame_ids),
        FilterKind::Dom => strip_dom_value(val, ext_frame_ids),
    }
}

fn strip_frame_tree_node(node: &mut Value, ext_frame_ids: &HashSet<String>) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    if let Some(children) = obj.get_mut("childFrames").and_then(Value::as_array_mut) {
        children.retain_mut(|child| {
            let is_ext = child.get("frame").is_some_and(|f| {
                let url = f.get("url").and_then(Value::as_str).unwrap_or("");
                let origin = f.get("securityOrigin").and_then(Value::as_str).unwrap_or("");
                let id = f.get("id").and_then(Value::as_str).unwrap_or("");
                is_extension_scheme(url)
                    || is_extension_scheme(origin)
                    || (!id.is_empty() && ext_frame_ids.contains(&normalize_frame_id(id)))
            });
            if is_ext {
                return false;
            }
            strip_frame_tree_node(child, ext_frame_ids);
            true
        });
    }
}

fn strip_ax_tree(val: &mut Value, ext_frame_ids: &HashSet<String>) {
    let Some(nodes) = val.get_mut("nodes").and_then(Value::as_array_mut) else {
        return;
    };
    let mut removed: HashSet<String> = HashSet::new();
    nodes.retain(|n| {
        let fid = n.get("frameId").and_then(Value::as_str).unwrap_or("");
        let fid_ext = !fid.is_empty() && ext_frame_ids.contains(&normalize_frame_id(fid));
        let prop_ext = n
            .get("properties")
            .and_then(Value::as_array)
            .is_some_and(|props| {
                props.iter().any(|p| {
                    p.pointer("/value/value")
                        .and_then(Value::as_str)
                        .is_some_and(is_extension_scheme)
                })
            });
        if fid_ext || prop_ext {
            if let Some(nid) = n.get("nodeId").and_then(Value::as_str) {
                removed.insert(nid.to_owned());
            }
            return false;
        }
        true
    });
    if !removed.is_empty() {
        for n in nodes.iter_mut() {
            if let Some(cids) = n.get_mut("childIds").and_then(Value::as_array_mut) {
                cids.retain(|cid| !cid.as_str().is_some_and(|s| removed.contains(s)));
            }
        }
    }
}

fn strip_dom_value(val: &mut Value, ext_frame_ids: &HashSet<String>) {
    if let Some(obj) = val.as_object_mut() {
        // Redact outerHTML strings if they embed `chrome-extension://` URLs/tokens.
        if let Some(Value::String(html)) = obj.get_mut("outerHTML")
            && html.to_ascii_lowercase().contains("chrome-extension:")
        {
            *html = redact_ext_urls_in_text(html);
        }
        let is_iframe = obj
            .get("nodeName")
            .and_then(Value::as_str)
            .is_some_and(|n| n.eq_ignore_ascii_case("IFRAME"));
        if is_iframe {
            let ext_src = attr_value(obj.get("attributes"), "src").is_some_and(is_extension_scheme);
            let ext_fid = obj
                .get("frameId")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && ext_frame_ids.contains(&normalize_frame_id(id)));
            if ext_src || ext_fid {
                obj.remove("contentDocument");
                obj.remove("frameId");
                if let Some(attrs) = obj.get_mut("attributes").and_then(Value::as_array_mut) {
                    let mut i = 0;
                    while i + 1 < attrs.len() {
                        if attrs[i].as_str().is_some_and(|k| k.eq_ignore_ascii_case("src")) {
                            attrs[i + 1] = Value::String("about:blank".into());
                        }
                        i += 2;
                    }
                }
                return;
            }
        }
        for key in ["root", "node", "contentDocument"] {
            if let Some(child) = obj.get_mut(key) {
                strip_dom_value(child, ext_frame_ids);
            }
        }
        for key in ["children", "shadowRoots", "nodes"] {
            if let Some(arr) = obj.get_mut(key).and_then(Value::as_array_mut) {
                for item in arr.iter_mut() {
                    strip_dom_value(item, ext_frame_ids);
                }
            }
        }
    }
}

fn attr_value<'a>(attrs: Option<&'a Value>, name: &str) -> Option<&'a str> {
    let arr = attrs?.as_array()?;
    let mut i = 0;
    while i + 1 < arr.len() {
        if arr[i].as_str().is_some_and(|k| k.eq_ignore_ascii_case(name)) {
            return arr[i + 1].as_str();
        }
        i += 2;
    }
    None
}

fn redact_ext_urls_in_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let lower = s.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find("chrome-extension:") {
        let start = pos + rel;
        out.push_str(&s[pos..start]);
        out.push_str("about:blank");
        let rest = &s[start..];
        let end_rel = rest
            .find(['"', '\'', ' ', '>', '<', '\n', '\r', '\t'])
            .unwrap_or(rest.len());
        pos = start + end_rel;
    }
    out.push_str(&s[pos..]);
    out
}

pub fn normalize_frame_id(id: &str) -> String {
    id.trim().replace('-', "").to_ascii_uppercase()
}
