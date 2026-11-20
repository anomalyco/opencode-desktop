//! Narrow extension sub-frame masking and true CDP frame-graph helpers for PR 1b-2
//! (`review-rd274.md` Round 4, Round 6, and `#311` first read).
//!
//! Trust boundaries enforced here:
//! - Whole-tab extension surfaces (extension main frames, popups, or missing `Browser` handles)
//!   receive unconditional refusal (`ExtSurfaceKind::WholeTab`).
//! - Regular web tabs hosting extension sub-frames (direct, closed-ShadowRoot, DNR-redirected,
//!   empty-`src`, or nested inside cross-origin web OOPIFs) use a unified true CDP frame graph
//!   combining local frames from `Page.getFrameTree` with cross-origin OOPIF targets from
//!   `Target.getTargets()` (`targetId` == `devtools_frame_token` == CDP `frameId`), filtered to
//!   ancestors of THIS tab's `main_frame_id` BEFORE applying per-tab safety caps.
//! - Internal native C++ `DOM.getDocument({ depth: -1, pierce: true })` plus `DOM.getFrameOwner`
//!   identifies every closed `ShadowRoot`, every descendant inside it, every extension or
//!   protected-host `IFRAME`/`FRAME`/`OBJECT`/`EMBED`, and every `chrome-extension:` attribute
//!   node so external read replies drop them without returning `backendNodeId`s and node-targeted
//!   calls (`DOM.resolveNode`, `DOM.focus`, `DOM.describeNode`, `DOM.getFrameOwner`,
//!   `DOM.getBoxModel`, `DOM.getContentQuads`, `DOM.scrollIntoViewIfNeeded`) fail closed even on
//!   `Clean` tabs when extensions are active.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use cef::{Browser, CefString, ImplBrowser, ImplFrame};
use serde_json::Value;

use crate::extensions::manifest::is_extension_scheme;
use crate::prompts::TabPrompts;
use crate::wire::TabSnapshot;

/// Maximum top-level host child frames on THIS tab before failing closed.
pub const MAX_PROTECTED_FRAMES: usize = 64;
/// Maximum total frames belonging to THIS tab before failing closed.
pub const MAX_FRAME_TREE_NODES: usize = 512;
/// Hard hygiene cap on raw global `Target.getTargets()` iframe entries parsed across all tabs.
const MAX_GLOBAL_TARGETS: usize = 8192;
/// Maximum protected DOM nodes recorded from internal pierced `DOM.getDocument` before failing closed.
pub const MAX_PROTECTED_DOM_NODES: usize = 8192;
/// Maximum DOM recursion depth before failing closed.
pub const MAX_DOM_WALK_DEPTH: usize = 128;

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
/// Returns `WholeTab` unconditionally if `browser` is `None` (`review-rd274.md` Round-6 §2).
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

/// True CDP frame-tree classification built from internal `Page.getFrameTree` + `Target.getTargets`.
#[derive(Clone, Debug, Default)]
pub struct FrameTreeAnalysis {
    pub main_frame_id: String,
    /// Every true CDP `frameId` on THIS tab that is itself an extension frame (or descendant of one).
    pub ext_cdp_frame_ids: HashSet<String>,
    /// Sorted, deduplicated direct children of `main_frame_id` that are extension frames or host a
    /// nested extension descendant (e.g. `top -> cross_origin_web_iframe -> ext_iframe`).
    pub host_child_cdp_frame_ids: Vec<String>,
    /// Union of `ext_cdp_frame_ids` and all non-main ancestors hosting an extension descendant.
    pub protected_frame_ids: HashSet<String>,
    /// Frames on THIS tab with no extension identity and no extension descendant (includes main).
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

    // Step 1: Collect THIS tab's local frames from `Page.getFrameTree` (capped at 512).
    let mut local_nodes: HashMap<String, (Option<String>, bool)> = HashMap::new();
    local_nodes.insert(main_id.clone(), (None, false));
    collect_local_frames(root, None, known_cdp_ext_frames, 0, &mut local_nodes)?;

    // Step 2: Index global `Target.getTargets()` `iframe` targets WITHOUT counting other tabs
    // against this tab's `MAX_FRAME_TREE_NODES` cap (`review-rd274.md` #311 Finding 4a).
    let mut global_iframes: HashMap<String, (String, bool)> = HashMap::new();
    if let Some(infos) = targets_val.get("targetInfos").and_then(Value::as_array) {
        if infos.len() > MAX_GLOBAL_TARGETS {
            return Err("Global DevTools target list exceeded safety limit.");
        }
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
            let url = info.get("url").and_then(Value::as_str).unwrap_or("");
            let is_ext = is_extension_scheme(url) || known_cdp_ext_frames.contains(fid);
            global_iframes
                .entry(fid.to_owned())
                .and_modify(|(_, e)| *e |= is_ext)
                .or_insert((parent.to_owned(), is_ext));
        }
    }

    // Step 3: Resolve ancestor chains strictly for frames reaching THIS tab's `main_id`, enforcing
    // `MAX_FRAME_TREE_NODES` only on frames belonging to THIS tab.
    let lookup = |id: &str| -> Option<(Option<&str>, bool)> {
        if let Some((p, e)) = local_nodes.get(id) {
            let extra_ext = global_iframes.get(id).is_some_and(|(_, ge)| *ge);
            return Some((p.as_deref(), *e || extra_ext));
        }
        global_iframes
            .get(id)
            .map(|(p, e)| (Some(p.as_str()), *e))
    };

    let mut in_tab_paths: HashMap<String, Vec<String>> = HashMap::new();
    let candidate_ids: Vec<&str> = local_nodes
        .keys()
        .chain(global_iframes.keys())
        .map(String::as_str)
        .collect();
    for fid in candidate_ids {
        if fid == main_id || in_tab_paths.contains_key(fid) {
            continue;
        }
        let mut chain = Vec::new();
        let mut cur = fid;
        let mut reached_main = false;
        for _ in 0..64 {
            let Some((parent, _)) = lookup(cur) else {
                break;
            };
            chain.push(cur.to_owned());
            let Some(p) = parent else {
                break;
            };
            if p == main_id {
                reached_main = true;
                break;
            }
            cur = p;
        }
        if reached_main {
            if in_tab_paths.len() + 1 >= MAX_FRAME_TREE_NODES {
                return Err("Frame tree exceeded safety limit.");
            }
            in_tab_paths.insert(fid.to_owned(), chain);
        }
    }

    let mut out = FrameTreeAnalysis {
        main_frame_id: main_id.clone(),
        ..Default::default()
    };
    out.clean_frame_ids.insert(main_id);

    let mut host_child_set: HashSet<String> = HashSet::new();
    for (fid, chain) in &in_tab_paths {
        let is_self_or_ancestor_ext = chain
            .iter()
            .any(|id| lookup(id).is_some_and(|(_, ext)| ext));
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
    pub node_ids: HashSet<i64>,
}

pub fn collect_protected_dom_nodes(
    doc: &Value,
    tree: &FrameTreeAnalysis,
) -> Result<ProtectedDomNodes, &'static str> {
    let Some(root) = doc.get("root") else {
        return Err("Missing root node in internal DOM.getDocument reply.");
    };
    let mut out = ProtectedDomNodes::default();
    walk_dom_for_protected(root, tree, false, 0, &mut out)?;
    Ok(out)
}

fn walk_dom_for_protected(
    node: &Value,
    tree: &FrameTreeAnalysis,
    in_protected: bool,
    depth: usize,
    out: &mut ProtectedDomNodes,
) -> Result<(), &'static str> {
    if depth > MAX_DOM_WALK_DEPTH {
        return Err("DOM depth exceeded safety limit.");
    }
    let Some(obj) = node.as_object() else {
        return Ok(());
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
    if subtree_protected || attr_has_ext {
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
        for shadow in shadows {
            walk_dom_for_protected(shadow, tree, subtree_protected, depth + 1, out)?;
        }
    }
    if let Some(children) = obj.get("children").and_then(Value::as_array) {
        for c in children {
            walk_dom_for_protected(c, tree, subtree_protected, depth + 1, out)?;
        }
    }
    if let Some(cd) = obj.get("contentDocument") {
        walk_dom_for_protected(cd, tree, subtree_protected, depth + 1, out)?;
    }
    if let Some(tc) = obj.get("templateContent") {
        walk_dom_for_protected(tc, tree, subtree_protected, depth + 1, out)?;
    }
    Ok(())
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

/// Methods unconditionally refused on `ExtSurfaceKind::Subframes` in PR 1b-2 (`Input.*` stays
/// refused under Option A; `Page.captureScreenshot` moves to PR 1b-4).
pub fn is_refused_on_subframes(method: &str) -> bool {
    matches!(
        method,
        "Page.captureScreenshot"
            | "Input.dispatchMouseEvent"
            | "Input.dispatchTouchEvent"
            | "Input.emulateTouchFromMouseEvent"
            | "Input.dispatchKeyEvent"
            | "Input.insertText"
    )
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

/// Methods that MUST route through `TabMaskQueue` even on a `Clean` tab whenever extensions are
/// active so `backendNodeId` brute-forcing cannot dump a closed ShadowRoot created by an extension
/// content script before any extension iframe mounts (`review-rd274.md` #311 Finding 1).
pub fn is_clean_ext_guarded_method(method: &str) -> bool {
    is_node_guard_method(method)
        || matches!(
            method,
            "DOM.getDocument"
                | "DOM.describeNode"
                | "DOM.querySelector"
                | "DOM.querySelectorAll"
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
