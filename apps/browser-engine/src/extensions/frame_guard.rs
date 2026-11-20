//! Narrow extension sub-frame masking and geometry helpers (`review-rd274.md` Rounds 4 & 6).
//!
//! Trust boundaries enforced here:
//! - Whole-tab extension surfaces (extension main frames, popups, or missing `Browser` handles)
//!   receive unconditional refusal (`ExtSurfaceKind::WholeTab`).
//! - Regular web tabs hosting extension sub-frames (direct, closed-ShadowRoot, DNR-redirected,
//!   or nested inside cross-origin web OOPIFs) are classified as `ExtSurfaceKind::Subframes` and
//!   never fall back to heuristic `CefFrame::identifier()` vs CDP `frameId` comparisons:
//!   - True CDP `frameId`s (`devtools_frame_token`) come exclusively from `Page.getFrameTree` +
//!     `Page.frameNavigated`/`navigatedWithinDocument`/`frameDetached`.
//!   - Nested `top -> web_A -> ... -> ext` frames map up to their direct main-frame child
//!     `host_child_cdp_frame_ids` so `DOM.getFrameOwner` + `DOM.getBoxModel` cover the full outer
//!     cross-origin OOPIF box.
//!   - Internal native C++ `DOM.getDocument({ depth: -1, pierce: true })` identifies every closed
//!     `ShadowRoot`, every descendant inside it, every extension iframe/object/embed, and every
//!     `chrome-extension:` attribute node so external `DOM.*`/`Accessibility.*`/`Page.getFrameTree`
//!     replies drop them completely without leaking `backendNodeId`s, and `DOM.resolveNode`/`focus`/
//!     `getFrameOwner`/`getBoxModel`/`getContentQuads`/`scrollIntoViewIfNeeded` fail closed.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use cef::{Browser, CefString, ImplBrowser, ImplFrame};
use serde_json::Value;

use crate::extensions::manifest::is_extension_scheme;
use crate::prompts::TabPrompts;
use crate::wire::TabSnapshot;

/// Maximum top-level host child frames allowed before failing closed (never silently drops).
pub const MAX_PROTECTED_FRAMES: usize = 64;
/// Maximum frame-tree nodes walked before failing closed.
pub const MAX_FRAME_TREE_NODES: usize = 512;
/// Maximum protected DOM nodes recorded from internal pierced `DOM.getDocument` before failing closed.
pub const MAX_PROTECTED_DOM_NODES: usize = 8192;
/// Maximum DOM recursion depth before failing closed.
pub const MAX_DOM_WALK_DEPTH: usize = 128;
/// Conservative CSS pixel margin around protected frame boxes for pointer hit-testing.
pub const POINTER_PAD_CSS: f64 = 8.0;
/// Maximum CSS pixel drift allowed between pre- and post-capture boxes before failing closed.
pub const RECT_SHIFT_TOLERANCE_CSS: f64 = 1.0;

#[derive(Clone, Debug)]
pub struct SubframeSnapshot {
    pub has_pending: bool,
    pub cef_ext_count: usize,
    pub cdp_ext_frames: HashSet<String>,
}

#[derive(Clone, Debug)]
pub enum ExtSurfaceKind {
    Clean,
    WholeTab,
    Subframes(SubframeSnapshot),
}

impl ExtSurfaceKind {
    pub fn is_whole_tab(&self) -> bool {
        matches!(self, Self::WholeTab)
    }

    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Clean)
    }
}

/// Classifies a tab's extension surface fail-closed.
/// Note: returns `WholeTab` unconditionally if `browser` is `None` (`review-rd274.md` Round-6 §2).
pub fn classify_surface(
    browser: Option<&Browser>,
    is_popup: bool,
    snap: &Mutex<TabSnapshot>,
    prompts: &TabPrompts,
) -> ExtSurfaceKind {
    if is_popup {
        return ExtSurfaceKind::WholeTab;
    }
    if is_extension_scheme(&snap.lock().unwrap().url) {
        return ExtSurfaceKind::WholeTab;
    }
    if let Some(pending) = prompts.pending_top_url()
        && is_extension_scheme(&pending)
    {
        return ExtSurfaceKind::WholeTab;
    }
    let Some(browser) = browser else {
        return ExtSurfaceKind::WholeTab;
    };
    if let Some(frame) = browser.main_frame() {
        let live_url = CefString::from(&frame.url()).to_string();
        if is_extension_scheme(&live_url) {
            return ExtSurfaceKind::WholeTab;
        }
    }
    let mut ids = cef::CefStringList::new();
    browser.frame_identifiers(Some(&mut ids));
    let mut live_frames: HashMap<String, String> = HashMap::new();
    let mut cef_ext_count = 0usize;
    for id in ids {
        let Some(frame) = browser.frame_by_identifier(Some(&CefString::from(id.as_str()))) else {
            continue;
        };
        if frame.is_main() == 1 {
            continue;
        }
        let furl = CefString::from(&frame.url()).to_string();
        if is_extension_scheme(&furl) {
            cef_ext_count += 1;
        }
        live_frames.insert(id, furl);
    }
    let has_pending = prompts.has_pending_ext_subframes(&live_frames);
    let cdp_ext_frames = prompts.cdp_ext_frames_snapshot();
    if has_pending || cef_ext_count > 0 || !cdp_ext_frames.is_empty() {
        ExtSurfaceKind::Subframes(SubframeSnapshot {
            has_pending,
            cef_ext_count,
            cdp_ext_frames,
        })
    } else {
        ExtSurfaceKind::Clean
    }
}

/// Whether the tab's currently focused Chromium frame is an extension frame.
pub fn focused_frame_is_extension(browser: &Browser) -> bool {
    let Some(frame) = browser.focused_frame() else {
        return false;
    };
    let url = CefString::from(&frame.url()).to_string();
    is_extension_scheme(&url)
}

/// True CDP frame-tree classification built from internal `Page.getFrameTree`.
#[derive(Clone, Debug, Default)]
pub struct FrameTreeAnalysis {
    pub main_frame_id: String,
    /// Every true CDP `frameId` that is itself an extension frame (or descendant of one).
    pub ext_cdp_frame_ids: HashSet<String>,
    /// Deduplicated direct children of `main_frame_id` that are extension frames or host a nested
    /// extension descendant (e.g. `top -> cross_origin_web_iframe -> ext_iframe`).
    pub host_child_cdp_frame_ids: Vec<String>,
    /// Union of `ext_cdp_frame_ids` and all non-main ancestors hosting an extension descendant.
    pub protected_frame_ids: HashSet<String>,
    /// Frames in the tree with no extension identity and no extension descendant (includes main).
    pub clean_frame_ids: HashSet<String>,
}

pub fn analyze_frame_tree(
    val: &Value,
    targets_val: &Value,
    known_cdp_ext_frames: &HashSet<String>,
) -> Result<FrameTreeAnalysis, &'static str> {
    let Some(root) = val.get("frameTree") else {
        return Err("Missing frameTree in Page.getFrameTree reply.");
    };
    let Some(main_frame) = root.get("frame") else {
        return Err("Missing root frame in Page.getFrameTree reply.");
    };
    let main_id = main_frame
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if main_id.is_empty() {
        return Err("Empty main frameId in Page.getFrameTree reply.");
    }
    let main_url = main_frame.get("url").and_then(Value::as_str).unwrap_or("");
    let main_origin = main_frame
        .get("securityOrigin")
        .and_then(Value::as_str)
        .unwrap_or("");
    if is_extension_scheme(main_url) || is_extension_scheme(main_origin) {
        return Err("Agent browser tools cannot read or act on extension pages.");
    }

    // Build unified multi-process frame graph: `fid -> (parent_fid, is_self_ext)` combining
    // local frames from `Page.getFrameTree` AND cross-origin OOPIF targets from
    // `Target.getTargets()` (whose `targetId` IS Chromium's `devtools_frame_token` = CDP `frameId`).
    let mut nodes: HashMap<String, (Option<String>, bool)> = HashMap::new();
    nodes.insert(main_id.clone(), (None, false));
    collect_local_frames(root, None, known_cdp_ext_frames, 0, &mut nodes)?;

    if let Some(infos) = targets_val.get("targetInfos").and_then(Value::as_array) {
        for info in infos {
            if info.get("type").and_then(Value::as_str) != Some("iframe") {
                continue;
            }
            let fid = info.get("targetId").and_then(Value::as_str).unwrap_or("");
            let parent = info
                .get("parentFrameId")
                .or_else(|| info.get("parentId"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if fid.is_empty() || parent.is_empty() {
                continue;
            }
            if nodes.len() >= MAX_FRAME_TREE_NODES && !nodes.contains_key(fid) {
                return Err("Frame tree exceeded safety limit.");
            }
            let url = info.get("url").and_then(Value::as_str).unwrap_or("");
            let is_ext = is_extension_scheme(url) || known_cdp_ext_frames.contains(fid);
            nodes
                .entry(fid.to_owned())
                .and_modify(|(p, e)| {
                    if p.is_none() {
                        *p = Some(parent.to_owned());
                    }
                    *e |= is_ext;
                })
                .or_insert((Some(parent.to_owned()), is_ext));
        }
    }

    // Filter to frames whose ancestor chain reaches `main_id` (excludes targets from other tabs),
    // propagating extension taint down to descendants and up to direct children of `main_id`.
    let mut out = FrameTreeAnalysis {
        main_frame_id: main_id.clone(),
        ..Default::default()
    };
    out.clean_frame_ids.insert(main_id.clone());

    let mut in_tab_paths: HashMap<String, Vec<String>> = HashMap::new();
    for fid in nodes.keys() {
        if fid == &main_id {
            continue;
        }
        let mut chain = Vec::new();
        let mut cur = fid.as_str();
        let mut reached_main = false;
        for _ in 0..64 {
            let Some((parent, _)) = nodes.get(cur) else {
                break;
            };
            chain.push(cur.to_owned());
            let Some(p) = parent.as_deref() else {
                break;
            };
            if p == main_id {
                reached_main = true;
                break;
            }
            cur = p;
        }
        if reached_main {
            in_tab_paths.insert(fid.clone(), chain);
        }
    }

    let mut host_child_set: HashSet<String> = HashSet::new();
    for (fid, chain) in &in_tab_paths {
        // `chain` runs from `fid` up to `direct_child` of `main_id` (last element).
        let is_self_or_ancestor_ext = chain
            .iter()
            .any(|id| nodes.get(id).is_some_and(|(_, ext)| *ext));
        if is_self_or_ancestor_ext {
            out.ext_cdp_frame_ids.insert(fid.clone());
            for anc in chain {
                out.protected_frame_ids.insert(anc.clone());
            }
            if let Some(direct_child) = chain.last()
                && host_child_set.insert(direct_child.clone())
            {
                if out.host_child_cdp_frame_ids.len() >= MAX_PROTECTED_FRAMES {
                    return Err("Too many extension sub-frames to safely mask.");
                }
                out.host_child_cdp_frame_ids.push(direct_child.clone());
            }
        }
    }
    out.host_child_cdp_frame_ids.sort();
    for fid in in_tab_paths.keys() {
        if !out.protected_frame_ids.contains(fid) {
            out.clean_frame_ids.insert(fid.clone());
        }
    }
    Ok(out)
}

fn collect_local_frames(
    node: &Value,
    parent_id: Option<&str>,
    known_cdp_ext_frames: &HashSet<String>,
    depth: usize,
    nodes: &mut HashMap<String, (Option<String>, bool)>,
) -> Result<(), &'static str> {
    if depth > 64 || nodes.len() >= MAX_FRAME_TREE_NODES {
        return Err("Frame tree exceeded safety limit.");
    }
    let Some(frame) = node.get("frame") else {
        return Err("Malformed frame node.");
    };
    let fid = frame.get("id").and_then(Value::as_str).unwrap_or("");
    if fid.is_empty() {
        return Err("Frame missing CDP frameId.");
    }
    let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
    let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("");
    let is_ext = is_extension_scheme(url)
        || is_extension_scheme(origin)
        || known_cdp_ext_frames.contains(fid);
    if parent_id.is_some() {
        nodes.insert(fid.to_owned(), (parent_id.map(str::to_owned), is_ext));
    }
    if let Some(children) = node.get("childFrames").and_then(Value::as_array) {
        for child in children {
            collect_local_frames(child, Some(fid), known_cdp_ext_frames, depth + 1, nodes)?;
        }
    }
    Ok(())
}

/// Native C++-authoritative set of protected DOM nodes extracted from an internal
/// `DOM.getDocument({ depth: -1, pierce: true })` pass plus `DOM.getFrameOwner`.
#[derive(Clone, Debug, Default)]
pub struct ProtectedDomNodes {
    pub backend_ids: HashSet<i64>,
    pub shadow_host_backend_ids: HashSet<i64>,
    pub node_ids: HashSet<i64>,
    pub root_node_id: Option<i64>,
}

pub fn collect_protected_dom_nodes(
    doc: &Value,
    tree: &FrameTreeAnalysis,
) -> Result<ProtectedDomNodes, &'static str> {
    let Some(root) = doc.get("root") else {
        return Err("Missing root node in internal DOM.getDocument reply.");
    };
    let mut out = ProtectedDomNodes {
        root_node_id: root.get("nodeId").and_then(Value::as_i64),
        ..Default::default()
    };
    walk_dom_for_protected(root, tree, false, 0, &mut out)?;
    Ok(out)
}

fn walk_dom_for_protected(
    node: &Value,
    tree: &FrameTreeAnalysis,
    in_protected: bool,
    depth: usize,
    out: &mut ProtectedDomNodes,
) -> Result<bool, &'static str> {
    if depth > MAX_DOM_WALK_DEPTH {
        return Err("DOM depth exceeded safety limit.");
    }
    let Some(obj) = node.as_object() else {
        return Ok(false);
    };
    let is_closed_shadow = obj
        .get("shadowRootType")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("closed"));
    let is_frame_elem = is_frame_owner_tag(obj);
    let fid = obj.get("frameId").and_then(Value::as_str).unwrap_or("");
    let src_ext = attr_value(obj.get("attributes"), "src").is_some_and(is_extension_scheme)
        || attr_value(obj.get("attributes"), "data").is_some_and(is_extension_scheme);
    let frame_protected = is_frame_elem
        && (src_ext || (!fid.is_empty() && tree.protected_frame_ids.contains(fid)));
    let attr_has_ext = attrs_contain_extension_url(obj.get("attributes"));

    let subtree_protected = in_protected || is_closed_shadow || frame_protected;
    let mut has_protected = subtree_protected || attr_has_ext;
    if has_protected {
        if let Some(bid) = obj.get("backendNodeId").and_then(Value::as_i64) {
            if out.backend_ids.len() >= MAX_PROTECTED_DOM_NODES && !out.backend_ids.contains(&bid) {
                return Err("Protected DOM node count exceeded safety limit.");
            }
            out.backend_ids.insert(bid);
        }
        if let Some(nid) = obj.get("nodeId").and_then(Value::as_i64)
            && nid > 0
        {
            out.node_ids.insert(nid);
        }
    }

    if let Some(shadows) = obj.get("shadowRoots").and_then(Value::as_array) {
        let mut shadow_has_protected = false;
        for shadow in shadows {
            if walk_dom_for_protected(shadow, tree, subtree_protected, depth + 1, out)? {
                shadow_has_protected = true;
            }
        }
        if shadow_has_protected {
            has_protected = true;
            if let Some(bid) = obj.get("backendNodeId").and_then(Value::as_i64) {
                out.shadow_host_backend_ids.insert(bid);
            }
        }
    }
    if let Some(children) = obj.get("children").and_then(Value::as_array) {
        for c in children {
            if walk_dom_for_protected(c, tree, subtree_protected, depth + 1, out)? {
                has_protected = true;
            }
        }
    }
    if let Some(cd) = obj.get("contentDocument")
        && walk_dom_for_protected(cd, tree, subtree_protected, depth + 1, out)?
    {
        has_protected = true;
    }
    if let Some(tc) = obj.get("templateContent")
        && walk_dom_for_protected(tc, tree, subtree_protected, depth + 1, out)?
    {
        has_protected = true;
    }
    Ok(has_protected)
}

/// Strips extension frames from an external `Page.getFrameTree` reply and redacts extension URLs.
pub fn strip_frame_tree(val: &mut Value, ext_ids: &HashSet<String>) {
    if let Some(root) = val.get_mut("frameTree") {
        strip_frame_tree_node(root, ext_ids);
    }
    redact_ext_strings(val);
}

fn strip_frame_tree_node(node: &mut Value, ext_ids: &HashSet<String>) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    if let Some(Value::Array(children)) = obj.get_mut("childFrames") {
        children.retain(|c| {
            let Some(frame) = c.get("frame") else {
                return false;
            };
            let fid = frame.get("id").and_then(Value::as_str).unwrap_or("");
            let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
            let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("");
            !ext_ids.contains(fid)
                && !is_extension_scheme(url)
                && !is_extension_scheme(origin)
        });
        for c in children.iter_mut() {
            strip_frame_tree_node(c, ext_ids);
        }
    }
}

/// Strips extension AX nodes (by true CDP `frameId`, `protected_bids`, and URL/text) and prunes
/// their IDs from surviving parents' `childIds`.
pub fn strip_ax_tree(val: &mut Value, ext_ids: &HashSet<String>, protected_bids: &HashSet<i64>) {
    let Some(Value::Array(nodes)) = val.get_mut("nodes") else {
        redact_ext_strings(val);
        return;
    };
    let mut dropped: HashSet<String> = HashSet::new();
    for n in nodes.iter() {
        let fid_ext = n
            .get("frameId")
            .and_then(Value::as_str)
            .is_some_and(|fid| ext_ids.contains(fid));
        let bid_ext = n
            .get("backendDOMNodeId")
            .and_then(Value::as_i64)
            .is_some_and(|bid| protected_bids.contains(&bid));
        if (fid_ext || bid_ext || value_contains_ext_url(n))
            && let Some(nid) = n.get("nodeId").and_then(Value::as_str)
        {
            dropped.insert(nid.to_owned());
        }
    }
    if !dropped.is_empty() {
        nodes.retain(|n| {
            n.get("nodeId")
                .and_then(Value::as_str)
                .is_none_or(|nid| !dropped.contains(nid))
        });
        for n in nodes.iter_mut() {
            if let Some(Value::Array(cids)) = n.get_mut("childIds") {
                cids.retain(|id| id.as_str().is_none_or(|s| !dropped.contains(s)));
            }
        }
    }
    redact_ext_strings(val);
}

/// Strips closed ShadowRoots and protected extension `IFRAME`/`FRAME`/`OBJECT`/`EMBED` nodes
/// completely from `DOM.getDocument` / `DOM.describeNode` replies without leaking `backendNodeId`.
/// Returns `false` if the top-level node itself is protected (caller MUST convert to refusal).
pub fn strip_dom_value(
    val: &mut Value,
    protected_fids: &HashSet<String>,
    protected_bids: &HashSet<i64>,
) -> bool {
    if let Some(root) = val.get_mut("root") {
        if node_is_dropped(root, protected_fids, protected_bids) {
            return false;
        }
        strip_dom_node(root, protected_fids, protected_bids, 0);
    }
    if let Some(node) = val.get_mut("node") {
        if node_is_dropped(node, protected_fids, protected_bids) {
            return false;
        }
        strip_dom_node(node, protected_fids, protected_bids, 0);
    }
    redact_ext_strings(val);
    true
}

fn node_is_dropped(
    node: &Value,
    protected_fids: &HashSet<String>,
    protected_bids: &HashSet<i64>,
) -> bool {
    let Some(obj) = node.as_object() else {
        return false;
    };
    if obj
        .get("shadowRootType")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("closed"))
    {
        return true;
    }
    if obj
        .get("backendNodeId")
        .and_then(Value::as_i64)
        .is_some_and(|bid| protected_bids.contains(&bid))
    {
        return true;
    }
    if is_frame_owner_tag(obj) {
        let fid = obj.get("frameId").and_then(Value::as_str).unwrap_or("");
        let src_ext = attr_value(obj.get("attributes"), "src").is_some_and(is_extension_scheme)
            || attr_value(obj.get("attributes"), "data").is_some_and(is_extension_scheme);
        if src_ext || (!fid.is_empty() && protected_fids.contains(fid)) {
            return true;
        }
    }
    false
}

fn strip_dom_node(
    node: &mut Value,
    protected_fids: &HashSet<String>,
    protected_bids: &HashSet<i64>,
    depth: usize,
) {
    if depth > MAX_DOM_WALK_DEPTH {
        return;
    }
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    if let Some(Value::Array(shadows)) = obj.get_mut("shadowRoots") {
        shadows.retain(|s| !node_is_dropped(s, protected_fids, protected_bids));
        for s in shadows.iter_mut() {
            strip_dom_node(s, protected_fids, protected_bids, depth + 1);
        }
        if shadows.is_empty() {
            obj.remove("shadowRoots");
        }
    }
    if let Some(Value::Array(children)) = obj.get_mut("children") {
        children.retain(|c| !node_is_dropped(c, protected_fids, protected_bids));
        for c in children.iter_mut() {
            strip_dom_node(c, protected_fids, protected_bids, depth + 1);
        }
        let len = children.len();
        if obj.contains_key("childNodeCount") {
            obj.insert("childNodeCount".into(), Value::from(len));
        }
    }
    if let Some(Value::Array(dist)) = obj.get_mut("distributedNodes") {
        dist.retain(|n| !node_is_dropped(n, protected_fids, protected_bids));
    }
    if obj
        .get("contentDocument")
        .is_some_and(|cd| node_is_dropped(cd, protected_fids, protected_bids))
    {
        obj.remove("contentDocument");
    } else if let Some(cd) = obj.get_mut("contentDocument") {
        strip_dom_node(cd, protected_fids, protected_bids, depth + 1);
    }
    if let Some(tc) = obj.get_mut("templateContent") {
        strip_dom_node(tc, protected_fids, protected_bids, depth + 1);
    }
}

/// Redacts all extension-scheme URLs anywhere inside a JSON value (attributes, text, URLs).
pub fn redact_ext_strings(val: &mut Value) {
    match val {
        Value::String(s) => {
            if let Some(cleaned) = redact_ext_urls_in_str(s) {
                *s = cleaned;
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_ext_strings(v);
            }
        }
        Value::Object(map) => {
            for v in map.values_mut() {
                redact_ext_strings(v);
            }
        }
        _ => {}
    }
}

const EXT_SCHEMES: &[&str] = &[
    "blob:chrome-extension://",
    "blob:moz-extension://",
    "blob:safari-web-extension://",
    "blob:ms-browser-extension://",
    "chrome-extension://",
    "moz-extension://",
    "safari-web-extension://",
    "ms-browser-extension://",
];

fn value_contains_ext_url(val: &Value) -> bool {
    match val {
        Value::String(s) => str_contains_ext_url(s),
        Value::Array(a) => a.iter().any(value_contains_ext_url),
        Value::Object(m) => m.values().any(value_contains_ext_url),
        _ => false,
    }
}

fn str_contains_ext_url(s: &str) -> bool {
    if !s.contains("://") {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    EXT_SCHEMES.iter().any(|p| lower.contains(p))
}

fn redact_ext_urls_in_str(s: &str) -> Option<String> {
    if !str_contains_ext_url(s) {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut pos = 0usize;
    while pos < s.len() {
        let mut best: Option<usize> = None;
        for scheme in EXT_SCHEMES {
            if let Some(rel) = lower[pos..].find(scheme) {
                let abs = pos + rel;
                best = Some(best.map_or(abs, |cur| cur.min(abs)));
            }
        }
        let Some(start) = best else {
            out.push_str(&s[pos..]);
            break;
        };
        out.push_str(&s[pos..start]);
        out.push_str("[redacted-extension-url]");
        let mut end = start;
        for (i, ch) in s[start..].char_indices() {
            if ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>' | ')' | '`' | ';') {
                break;
            }
            end = start + i + ch.len_utf8();
        }
        pos = end.max(start + 1);
    }
    Some(out)
}

fn is_frame_owner_tag(obj: &serde_json::Map<String, Value>) -> bool {
    obj.get("nodeName")
        .and_then(Value::as_str)
        .is_some_and(|n| {
            n.eq_ignore_ascii_case("IFRAME")
                || n.eq_ignore_ascii_case("FRAME")
                || n.eq_ignore_ascii_case("OBJECT")
                || n.eq_ignore_ascii_case("EMBED")
        })
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

fn attrs_contain_extension_url(attrs: Option<&Value>) -> bool {
    let Some(arr) = attrs.and_then(Value::as_array) else {
        return false;
    };
    let mut i = 1;
    while i < arr.len() {
        if arr[i].as_str().is_some_and(str_contains_ext_url) {
            return true;
        }
        i += 2;
    }
    false
}

/// Axis-aligned CSS viewport bounding box extracted from `DOM.getBoxModel`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl CssRect {
    pub fn contains_padded(self, x: f64, y: f64, pad: f64) -> bool {
        x >= self.x0 - pad && x <= self.x1 + pad && y >= self.y0 - pad && y <= self.y1 + pad
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewportMetrics {
    pub page_x: f64,
    pub page_y: f64,
    pub client_width: f64,
    pub client_height: f64,
    /// Exact device-pixel-per-CSS-pixel factor (`visualViewport.clientWidth / cssVisualViewport.clientWidth`),
    /// immune to scrollbar width/height differences between `img_w/h` and `cssVisualViewport`.
    pub scale_x: f64,
    pub scale_y: f64,
}

pub fn extract_viewport_metrics(val: &Value) -> Option<ViewportMetrics> {
    let css_vp = val
        .get("cssVisualViewport")
        .or_else(|| val.get("visualViewport"))?;
    let page_x = css_vp.get("pageX").and_then(Value::as_f64).unwrap_or(0.0);
    let page_y = css_vp.get("pageY").and_then(Value::as_f64).unwrap_or(0.0);
    let client_width = css_vp.get("clientWidth").and_then(Value::as_f64)?;
    let client_height = css_vp.get("clientHeight").and_then(Value::as_f64)?;
    if !page_x.is_finite()
        || !page_y.is_finite()
        || !client_width.is_finite()
        || !client_height.is_finite()
        || client_width <= 0.0
        || client_height <= 0.0
    {
        return None;
    }
    let dev_vp = val.get("visualViewport");
    let dev_w = dev_vp
        .and_then(|v| v.get("clientWidth"))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(client_width);
    let dev_h = dev_vp
        .and_then(|v| v.get("clientHeight"))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(client_height);
    Some(ViewportMetrics {
        page_x,
        page_y,
        client_width,
        client_height,
        scale_x: dev_w / client_width,
        scale_y: dev_h / client_height,
    })
}

pub fn extract_box_rect(val: &Value) -> Option<CssRect> {
    let border = val.get("model")?.get("border")?.as_array()?;
    if border.len() < 8 {
        return None;
    }
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for i in 0..4 {
        let x = border[i * 2].as_f64()?;
        let y = border[i * 2 + 1].as_f64()?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x1 > x0 && y1 > y0).then_some(CssRect { x0, y0, x1, y1 })
}

/// True ONLY when Blink returned the exact no-layout-object error (`display: none` / unrendered).
pub fn is_no_layout_box_error(val: &Value) -> bool {
    let msg = val
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| val.get("error").and_then(|e| e.get("message")).and_then(Value::as_str))
        .unwrap_or("");
    msg.contains("Could not compute box model")
}

pub fn is_no_node_at_location_error(val: &Value) -> bool {
    let msg = val
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| val.get("error").and_then(|e| e.get("message")).and_then(Value::as_str))
        .unwrap_or("");
    msg.contains("No node found at given location")
}

pub fn point_hits_any_rect(points: &[(f64, f64)], rects: &[CssRect], pad: f64) -> bool {
    points
        .iter()
        .any(|&(x, y)| rects.iter().any(|r| r.contains_padded(x, y, pad)))
}

pub fn rects_shifted(before: &[CssRect], after: &[CssRect], tol: f64) -> bool {
    if before.len() != after.len() {
        return true;
    }
    before.iter().zip(after.iter()).any(|(a, b)| {
        (a.x0 - b.x0).abs() > tol
            || (a.y0 - b.y0).abs() > tol
            || (a.x1 - b.x1).abs() > tol
            || (a.y1 - b.y1).abs() > tol
    })
}

pub fn metrics_shifted(a: ViewportMetrics, b: ViewportMetrics, tol: f64) -> bool {
    (a.page_x - b.page_x).abs() > tol
        || (a.page_y - b.page_y).abs() > tol
        || (a.client_width - b.client_width).abs() > tol
        || (a.client_height - b.client_height).abs() > tol
}

pub fn extract_input_points(method: &str, params: &Value) -> Option<Vec<(f64, f64)>> {
    match method {
        "Input.dispatchMouseEvent" | "Input.emulateTouchFromMouseEvent" => {
            let x = params.get("x").and_then(Value::as_f64)?;
            let y = params.get("y").and_then(Value::as_f64)?;
            (x.is_finite() && y.is_finite()).then(|| vec![(x, y)])
        }
        "Input.dispatchTouchEvent" => {
            let arr = params.get("touchPoints")?.as_array()?;
            let mut pts = Vec::with_capacity(arr.len());
            for p in arr {
                let x = p.get("x").and_then(Value::as_f64)?;
                let y = p.get("y").and_then(Value::as_f64)?;
                if !x.is_finite() || !y.is_finite() {
                    return None;
                }
                pts.push((x, y));
            }
            (!pts.is_empty()).then_some(pts)
        }
        _ => None,
    }
}

pub fn is_pointer_input_method(method: &str) -> bool {
    matches!(
        method,
        "Input.dispatchMouseEvent"
            | "Input.dispatchTouchEvent"
            | "Input.emulateTouchFromMouseEvent"
    )
}

pub fn is_pointer_press_event(method: &str, params: &Value) -> bool {
    if method == "Input.dispatchTouchEvent" {
        return params
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| matches!(t, "touchStart" | "touchEnd"));
    }
    params
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| matches!(t, "mousePressed" | "mouseReleased"))
}

pub fn is_keyboard_input_method(method: &str) -> bool {
    matches!(method, "Input.dispatchKeyEvent" | "Input.insertText")
}

pub fn is_node_guard_method(method: &str) -> bool {
    matches!(
        method,
        "DOM.resolveNode"
            | "DOM.focus"
            | "DOM.getBoxModel"
            | "DOM.getContentQuads"
            | "DOM.scrollIntoViewIfNeeded"
            | "DOM.getFrameOwner"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFilterKind {
    FrameTree,
    AxTree,
    Dom,
    QuerySelector,
    QuerySelectorAll,
}

pub fn read_filter_for_method(method: &str) -> Option<ReadFilterKind> {
    match method {
        "Page.getFrameTree" => Some(ReadFilterKind::FrameTree),
        "Accessibility.getFullAXTree"
        | "Accessibility.getPartialAXTree"
        | "Accessibility.getRootAXNode"
        | "Accessibility.getChildAXNodes"
        | "Accessibility.queryAXTree" => Some(ReadFilterKind::AxTree),
        "DOM.getDocument" | "DOM.describeNode" => Some(ReadFilterKind::Dom),
        "DOM.querySelector" => Some(ReadFilterKind::QuerySelector),
        "DOM.querySelectorAll" => Some(ReadFilterKind::QuerySelectorAll),
        _ => None,
    }
}
