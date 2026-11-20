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

use std::collections::{HashMap, HashSet, VecDeque};
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
/// Maximum clean DOM nodes stored in the per-epoch cache before falling back to uncached mode.
const MAX_CLEAN_CACHE_NODES: usize = 65_536;
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

/// Native C++-authoritative set of protected and clean DOM nodes extracted from an internal
/// `DOM.getDocument({ depth: -1, pierce: true })` pass plus `DOM.getFrameOwner`.
#[derive(Clone, Debug, Default)]
pub struct ProtectedDomNodes {
    pub backend_ids: HashSet<i64>,
    pub node_ids: HashSet<i64>,
    pub clean_backend_ids: HashSet<i64>,
    pub clean_node_ids: HashSet<i64>,
    pub backend_to_clean_node: HashMap<i64, i64>,
    pub clean_cache_complete: bool,
}

pub fn collect_protected_dom_nodes(
    doc: &Value,
    tree: &FrameTreeAnalysis,
) -> Result<ProtectedDomNodes, &'static str> {
    let Some(root) = doc.get("root") else {
        return Err("Missing root node in internal DOM.getDocument reply.");
    };
    let mut out = ProtectedDomNodes {
        clean_cache_complete: true,
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
    let bid = obj.get("backendNodeId").and_then(Value::as_i64);
    let nid = obj.get("nodeId").and_then(Value::as_i64).filter(|&n| n > 0);
    if subtree_protected || attr_has_ext {
        if let Some(bid) = bid {
            if out.backend_ids.len() >= MAX_PROTECTED_DOM_NODES && !out.backend_ids.contains(&bid) {
                return Err("Protected DOM node count exceeded safety limit.");
            }
            out.backend_ids.insert(bid);
            out.clean_backend_ids.remove(&bid);
            out.backend_to_clean_node.remove(&bid);
        }
        if let Some(nid) = nid {
            out.node_ids.insert(nid);
            out.clean_node_ids.remove(&nid);
        }
    } else if out.clean_cache_complete {
        if let Some(bid) = bid
            && !out.backend_ids.contains(&bid)
        {
            if out.clean_backend_ids.len() < MAX_CLEAN_CACHE_NODES {
                out.clean_backend_ids.insert(bid);
                if let Some(nid) = nid
                    && !out.node_ids.contains(&nid)
                {
                    out.backend_to_clean_node.insert(bid, nid);
                }
            } else {
                out.clean_cache_complete = false;
            }
        }
        if let Some(nid) = nid
            && !out.node_ids.contains(&nid)
        {
            if out.clean_node_ids.len() < MAX_CLEAN_CACHE_NODES {
                out.clean_node_ids.insert(nid);
            } else {
                out.clean_cache_complete = false;
            }
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
    if let Some(pseudos) = obj.get("pseudoElements").and_then(Value::as_array) {
        for p in pseudos {
            walk_dom_for_protected(p, tree, subtree_protected, depth + 1, out)?;
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

/// Strips extension and closed-ShadowRoot AX nodes (cascading through `childIds` and `parentId`
/// so descendant `InlineTextBox` and pseudo-element nodes without `backendDOMNodeId` never
/// survive; `review-rd274.md` #314 Finding 1), prunes surviving `childIds` and `relatedNodes`, and
/// redacts extension URLs.
pub fn strip_ax_tree(val: &mut Value, ext_ids: &HashSet<String>, protected_bids: &HashSet<i64>) {
    let Some(Value::Array(nodes)) = val.get_mut("nodes") else {
        redact_ext_strings(val);
        return;
    };
    let mut seed_indices: Vec<usize> = Vec::new();
    for (idx, n) in nodes.iter().enumerate() {
        if ax_node_directly_protected(n, ext_ids, protected_bids) {
            seed_indices.push(idx);
        }
    }
    if !seed_indices.is_empty() {
        let mut children_by_parent: HashMap<String, Vec<String>> = HashMap::new();
        for n in nodes.iter() {
            let Some(nid_str) = n.get("nodeId").and_then(Value::as_str) else {
                continue;
            };
            if let Some(cids) = n.get("childIds").and_then(Value::as_array) {
                let entry = children_by_parent.entry(nid_str.to_owned()).or_default();
                for cid in cids {
                    if let Some(s) = cid.as_str() {
                        entry.push(s.to_owned());
                    }
                }
            }
            if let Some(pid) = n.get("parentId").and_then(Value::as_str) {
                children_by_parent
                    .entry(pid.to_owned())
                    .or_default()
                    .push(nid_str.to_owned());
            }
        }
        let mut dropped: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        let mut direct_drop_flags = vec![false; nodes.len()];
        for idx in seed_indices {
            direct_drop_flags[idx] = true;
            if let Some(nid_str) = nodes[idx].get("nodeId").and_then(Value::as_str)
                && dropped.insert(nid_str.to_owned())
            {
                queue.push_back(nid_str.to_owned());
            }
        }
        while let Some(cur) = queue.pop_front() {
            if let Some(kids) = children_by_parent.get(&cur) {
                for kid in kids {
                    if dropped.insert(kid.clone()) {
                        queue.push_back(kid.clone());
                    }
                }
            }
        }
        let mut idx = 0usize;
        nodes.retain(|n| {
            let direct = direct_drop_flags[idx];
            idx += 1;
            if direct {
                return false;
            }
            if n.get("nodeId")
                .and_then(Value::as_str)
                .is_some_and(|nid| dropped.contains(nid))
            {
                return false;
            }
            if n.get("parentId")
                .and_then(Value::as_str)
                .is_some_and(|pid| dropped.contains(pid))
            {
                return false;
            }
            true
        });
        if !dropped.is_empty() {
            for n in nodes.iter_mut() {
                if let Some(Value::Array(cids)) = n.get_mut("childIds") {
                    cids.retain(|id| id.as_str().is_none_or(|s| !dropped.contains(s)));
                }
            }
        }
    }
    if !protected_bids.is_empty() {
        for n in nodes.iter_mut() {
            if let Some(Value::Array(props)) = n.get_mut("properties") {
                for prop in props.iter_mut() {
                    if let Some(Value::Array(rel)) = prop.pointer_mut("/value/relatedNodes") {
                        rel.retain(|rn| {
                            rn.get("backendDOMNodeId")
                                .and_then(Value::as_i64)
                                .is_none_or(|bid| !protected_bids.contains(&bid))
                                && !value_contains_ext_url(rn)
                        });
                    }
                }
            }
        }
    }
    redact_ext_strings(val);
}

fn ax_node_directly_protected(
    n: &Value,
    ext_ids: &HashSet<String>,
    protected_bids: &HashSet<i64>,
) -> bool {
    let fid_ext = n
        .get("frameId")
        .and_then(Value::as_str)
        .is_some_and(|fid| ext_ids.contains(fid));
    let bid_ext = n
        .get("backendDOMNodeId")
        .and_then(Value::as_i64)
        .is_some_and(|bid| protected_bids.contains(&bid));
    fid_ext || bid_ext || value_contains_ext_url(n)
}

/// Strips closed ShadowRoots, `pseudoElements`, and protected extension
/// `IFRAME`/`FRAME`/`OBJECT`/`EMBED` nodes from `DOM.getDocument` / `DOM.describeNode` replies
/// without leaking `backendNodeId`. Returns `Ok(false)` if the top-level node itself is protected
/// and `Err(...)` fail-closed if DOM recursion exceeds `MAX_DOM_WALK_DEPTH`.
pub fn strip_dom_value(
    val: &mut Value,
    protected_fids: &HashSet<String>,
    protected_bids: &HashSet<i64>,
) -> Result<bool, &'static str> {
    if let Some(root) = val.get_mut("root") {
        if node_is_dropped(root, protected_fids, protected_bids) {
            return Ok(false);
        }
        strip_dom_node(root, protected_fids, protected_bids, 0)?;
    }
    if let Some(node) = val.get_mut("node") {
        if node_is_dropped(node, protected_fids, protected_bids) {
            return Ok(false);
        }
        strip_dom_node(node, protected_fids, protected_bids, 0)?;
    }
    redact_ext_strings(val);
    Ok(true)
}

/// Clones an already-stripped `DOM.getDocument` reply down to `requested_depth` (`-1` = full)
/// while omitting `shadowRoots` and `contentDocument` (matching external `pierce: false`).
/// For shallow depths (`depth: 0` / `depth: 1`), only visits the top levels without cloning the
/// entire tree.
pub fn clone_dom_to_depth(val: &Value, requested_depth: i64) -> Result<Value, &'static str> {
    let Some(root) = val.get("root") else {
        return Err("Missing root node in stripped DOM document.");
    };
    let pruned = clone_dom_node_depth(root, requested_depth, 0)?;
    Ok(serde_json::json!({ "root": pruned }))
}

fn clone_dom_node_depth(
    node: &Value,
    requested_depth: i64,
    depth: usize,
) -> Result<Value, &'static str> {
    if depth > MAX_DOM_WALK_DEPTH {
        return Err("DOM depth exceeded safety limit.");
    }
    let Some(obj) = node.as_object() else {
        return Ok(node.clone());
    };
    let at_limit = requested_depth >= 0 && (depth as i64) >= requested_depth;
    let mut out = serde_json::Map::with_capacity(obj.len());
    for (k, v) in obj {
        match k.as_str() {
            "shadowRoots" | "contentDocument" => continue,
            "children" => {
                if at_limit {
                    continue;
                }
                if let Some(arr) = v.as_array() {
                    let mut kids = Vec::with_capacity(arr.len());
                    for c in arr {
                        kids.push(clone_dom_node_depth(c, requested_depth, depth + 1)?);
                    }
                    out.insert(k.clone(), Value::Array(kids));
                }
            }
            "templateContent" => {
                if !at_limit {
                    out.insert(k.clone(), clone_dom_node_depth(v, requested_depth, depth + 1)?);
                }
            }
            _ => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(Value::Object(out))
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
) -> Result<(), &'static str> {
    if depth > MAX_DOM_WALK_DEPTH {
        return Err("DOM depth exceeded safety limit.");
    }
    let Some(obj) = node.as_object_mut() else {
        return Ok(());
    };
    if let Some(Value::Array(shadows)) = obj.get_mut("shadowRoots") {
        shadows.retain(|s| !node_is_dropped(s, protected_fids, protected_bids));
        for s in shadows.iter_mut() {
            strip_dom_node(s, protected_fids, protected_bids, depth + 1)?;
        }
        if shadows.is_empty() {
            obj.remove("shadowRoots");
        }
    }
    if let Some(Value::Array(children)) = obj.get_mut("children") {
        children.retain(|c| !node_is_dropped(c, protected_fids, protected_bids));
        for c in children.iter_mut() {
            strip_dom_node(c, protected_fids, protected_bids, depth + 1)?;
        }
        let len = children.len();
        if obj.contains_key("childNodeCount") {
            obj.insert("childNodeCount".into(), Value::from(len));
        }
    }
    if let Some(Value::Array(pseudos)) = obj.get_mut("pseudoElements") {
        pseudos.retain(|p| !node_is_dropped(p, protected_fids, protected_bids));
        for p in pseudos.iter_mut() {
            strip_dom_node(p, protected_fids, protected_bids, depth + 1)?;
        }
        if pseudos.is_empty() {
            obj.remove("pseudoElements");
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
        strip_dom_node(cd, protected_fids, protected_bids, depth + 1)?;
    }
    if let Some(tc) = obj.get_mut("templateContent") {
        strip_dom_node(tc, protected_fids, protected_bids, depth + 1)?;
    }
    Ok(())
}

/// Redacts all extension-scheme URLs (literal, bare `extension://`, and percent/entity/unicode
/// encoded forms) anywhere inside a JSON value (`review-rd274.md` #314 Finding 4).
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
    "blob:extension://",
    "filesystem:chrome-extension://",
    "filesystem:moz-extension://",
    "filesystem:safari-web-extension://",
    "filesystem:ms-browser-extension://",
    "filesystem:extension://",
    "chrome-extension://",
    "moz-extension://",
    "safari-web-extension://",
    "ms-browser-extension://",
    "extension://",
];

const ENCODED_EXT_PREFIXES: &[&str] = &[
    "blob%3achrome-extension%3a%2f%2f",
    "blob%3amoz-extension%3a%2f%2f",
    "blob%3asafari-web-extension%3a%2f%2f",
    "blob%3ams-browser-extension%3a%2f%2f",
    "blob%3aextension%3a%2f%2f",
    "blob:chrome-extension%3a%2f%2f",
    "blob:extension%3a%2f%2f",
    "chrome-extension%3a%2f%2f",
    "moz-extension%3a%2f%2f",
    "safari-web-extension%3a%2f%2f",
    "ms-browser-extension%3a%2f%2f",
    "extension%3a%2f%2f",
];

fn value_contains_ext_url(val: &Value) -> bool {
    match val {
        Value::String(s) => str_contains_ext_url(s),
        Value::Array(a) => a.iter().any(value_contains_ext_url),
        Value::Object(m) => m.values().any(value_contains_ext_url),
        _ => false,
    }
}

fn may_contain_ext_scheme(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 12 {
        return false;
    }
    if !bytes.iter().any(|&b| matches!(b, b':' | b'%' | b'&' | b'\\')) {
        return false;
    }
    if bytes.contains(&b'%')
        || s.contains("&#")
        || s.contains("&colon;")
        || s.contains("&sol;")
        || s.contains("\\u")
        || s.contains("\\U")
    {
        return true;
    }
    bytes.windows(9).any(|w| w.eq_ignore_ascii_case(b"extension"))
}

fn decode_percent_once(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(hi), Some(lo)) = (hex_nibble(b[i + 1]), hex_nibble(b[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn normalize_encoded_schemes(s: &str) -> String {
    let mut cur = s.to_ascii_lowercase();
    if cur.contains('%') {
        cur = decode_percent_once(&cur).to_ascii_lowercase();
        if cur.contains('%') {
            cur = decode_percent_once(&cur).to_ascii_lowercase();
        }
    }
    if cur.contains("&#") || cur.contains("&colon;") || cur.contains("&sol;") {
        cur = cur
            .replace("&#58;", ":")
            .replace("&#x3a;", ":")
            .replace("&colon;", ":")
            .replace("&#47;", "/")
            .replace("&#x2f;", "/")
            .replace("&sol;", "/")
            .replace("&#45;", "-")
            .replace("&#x2d;", "-");
    }
    if cur.contains("\\u") {
        cur = cur
            .replace("\\u003a", ":")
            .replace("\\u002f", "/")
            .replace("\\u002d", "-");
    }
    cur
}

pub fn str_contains_ext_url(s: &str) -> bool {
    if !may_contain_ext_scheme(s) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if EXT_SCHEMES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    if lower.contains('%')
        || lower.contains("&#")
        || lower.contains("&colon;")
        || lower.contains("&sol;")
        || lower.contains("\\u")
    {
        let norm = normalize_encoded_schemes(&lower);
        return EXT_SCHEMES.iter().any(|p| norm.contains(p));
    }
    false
}

fn replace_prefixes_in_str(s: &str, prefixes: &[&str]) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    if !prefixes.iter().any(|p| lower.contains(p)) {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    let mut pos = 0usize;
    while pos < s.len() {
        let mut best: Option<usize> = None;
        for scheme in prefixes {
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

fn redact_ext_urls_in_str(s: &str) -> Option<String> {
    if !str_contains_ext_url(s) {
        return None;
    }
    let mut cur = replace_prefixes_in_str(s, EXT_SCHEMES).unwrap_or_else(|| s.to_owned());
    if let Some(enc_cleaned) = replace_prefixes_in_str(&cur, ENCODED_EXT_PREFIXES) {
        cur = enc_cleaned;
    }
    // Fail closed if an obfuscated double-encoded / entity / unicode-escaped extension URL
    // still survives targeted prefix replacement.
    if str_contains_ext_url(&cur) {
        return Some("[redacted-extension-url]".to_owned());
    }
    Some(cur)
}

/// Evaluates whether a raw (unredacted) CDP event is attributed to a browser extension so
/// `TabCdpObserver::on_dev_tools_event` drops it BEFORE running `redact_ext_strings`
/// (`review-rd274.md` #314 Finding 3).
pub fn is_extension_cdp_event(
    method: &str,
    params: &Value,
    cdp_ext_frames: &HashSet<String>,
    prompts: &TabPrompts,
) -> bool {
    match method {
        "Runtime.consoleAPICalled" => {
            params
                .get("executionContextId")
                .and_then(Value::as_i64)
                .is_some_and(|id| prompts.is_ext_exec_context(id))
                || stack_is_extension(params.get("stackTrace"))
        }
        "Log.entryAdded" => {
            let Some(entry) = params.get("entry") else {
                return false;
            };
            entry
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(url_is_extension)
                || stack_is_extension(entry.get("stackTrace"))
        }
        "Runtime.exceptionThrown" => {
            let Some(details) = params.get("exceptionDetails") else {
                return false;
            };
            details
                .get("executionContextId")
                .and_then(Value::as_i64)
                .is_some_and(|id| prompts.is_ext_exec_context(id))
                || details
                    .get("url")
                    .and_then(Value::as_str)
                    .is_some_and(url_is_extension)
                || stack_is_extension(details.get("stackTrace"))
        }
        "Runtime.executionContextCreated" => {
            let Some(ctx) = params.get("context") else {
                return false;
            };
            let origin_ext = ctx
                .get("origin")
                .and_then(Value::as_str)
                .is_some_and(url_is_extension);
            let frame_ext = ctx
                .pointer("/auxData/frameId")
                .and_then(Value::as_str)
                .is_some_and(|fid| cdp_ext_frames.contains(fid));
            if origin_ext || frame_ext {
                if let Some(id) = ctx.get("id").and_then(Value::as_i64) {
                    prompts.mark_ext_exec_context(id);
                }
                return true;
            }
            false
        }
        "Runtime.executionContextDestroyed" => {
            if let Some(id) = params.get("executionContextId").and_then(Value::as_i64)
                && prompts.is_ext_exec_context(id)
            {
                prompts.remove_ext_exec_context(id);
                return true;
            }
            false
        }
        "Runtime.executionContextsCleared" => {
            prompts.clear_ext_exec_contexts();
            false
        }
        "Network.requestWillBeSent" => {
            let req_id = params.get("requestId").and_then(Value::as_str).unwrap_or("");
            let url_ext = params
                .pointer("/request/url")
                .and_then(Value::as_str)
                .is_some_and(url_is_extension)
                || params
                    .get("documentURL")
                    .and_then(Value::as_str)
                    .is_some_and(url_is_extension)
                || params
                    .pointer("/initiator/url")
                    .and_then(Value::as_str)
                    .is_some_and(url_is_extension)
                || params
                    .pointer("/redirectResponse/url")
                    .and_then(Value::as_str)
                    .is_some_and(url_is_extension)
                || params
                    .get("frameId")
                    .and_then(Value::as_str)
                    .is_some_and(|fid| cdp_ext_frames.contains(fid))
                || stack_is_extension(params.pointer("/initiator/stack"));
            if url_ext {
                prompts.mark_ext_network_request(req_id);
                return true;
            }
            false
        }
        "Network.requestWillBeSentExtraInfo"
        | "Network.responseReceivedExtraInfo"
        | "Network.loadingFinished"
        | "Network.loadingFailed" => params
            .get("requestId")
            .and_then(Value::as_str)
            .is_some_and(|id| prompts.is_ext_network_request(id)),
        "Network.responseReceived" => {
            let req_id = params.get("requestId").and_then(Value::as_str).unwrap_or("");
            let resp_ext = params
                .pointer("/response/url")
                .and_then(Value::as_str)
                .is_some_and(url_is_extension)
                || params
                    .get("frameId")
                    .and_then(Value::as_str)
                    .is_some_and(|fid| cdp_ext_frames.contains(fid));
            if resp_ext {
                prompts.mark_ext_network_request(req_id);
                return true;
            }
            prompts.is_ext_network_request(req_id)
        }
        _ => false,
    }
}

fn url_is_extension(url: &str) -> bool {
    is_extension_scheme(url) || str_contains_ext_url(url)
}

fn stack_is_extension(stack: Option<&Value>) -> bool {
    let mut cur = stack;
    let mut depth = 0usize;
    while let Some(s) = cur {
        if depth > 64 {
            return true;
        }
        if let Some(frames) = s.get("callFrames").and_then(Value::as_array)
            && frames
                .iter()
                .any(|f| f.get("url").and_then(Value::as_str).is_some_and(url_is_extension))
        {
            return true;
        }
        cur = s.get("parent");
        depth += 1;
    }
    false
}

/// Strictly validated single DOM target identifier (`review-rd274.md` #314 Finding 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomTarget {
    None,
    NodeId(i64),
    BackendNodeId(i64),
    ObjectId(String),
}

/// Validates that at most one of `nodeId`, `backendNodeId`, or `objectId` is specified (and
/// exactly one when `require_one` is `true`) and rejects non-integer/float IDs such as `12.0`
/// so Chromium's `InspectorDOMAgent::AssertNode` precedence (`nodeId` over `backendNodeId`) can
/// never bypass `NodeGuardOp` or `ReadFilterOp`.
pub fn parse_single_dom_target(
    params: &Value,
    require_one: bool,
) -> Result<DomTarget, &'static str> {
    if params.is_null() {
        return if require_one {
            Err("DevTools DOM call requires a valid nodeId, backendNodeId, or objectId.")
        } else {
            Ok(DomTarget::None)
        };
    }
    let Some(obj) = params.as_object() else {
        return Err("Invalid DevTools DOM call parameters.");
    };
    let mut found = DomTarget::None;
    let mut count = 0u8;

    if let Some(v) = obj.get("nodeId")
        && !v.is_null()
    {
        let Some(nid) = v.as_i64().filter(|&n| n > 0) else {
            return Err("DevTools DOM 'nodeId' must be a positive integer.");
        };
        found = DomTarget::NodeId(nid);
        count += 1;
    }
    if let Some(v) = obj.get("backendNodeId")
        && !v.is_null()
    {
        let Some(bid) = v.as_i64().filter(|&n| n > 0) else {
            return Err("DevTools DOM 'backendNodeId' must be a positive integer.");
        };
        found = DomTarget::BackendNodeId(bid);
        count += 1;
    }
    if let Some(v) = obj.get("objectId")
        && !v.is_null()
    {
        let Some(oid) = v.as_str().filter(|s| !s.is_empty()) else {
            return Err("DevTools DOM 'objectId' must be a non-empty string.");
        };
        found = DomTarget::ObjectId(oid.to_owned());
        count += 1;
    }
    if count > 1 {
        return Err(
            "DevTools DOM calls must specify at most one of nodeId, backendNodeId, or objectId.",
        );
    }
    if count == 0 && require_one {
        return Err("DevTools DOM call requires a valid nodeId, backendNodeId, or objectId.");
    }
    Ok(found)
}

/// Validates that `DOM.querySelector*` passes strictly a single positive integer `nodeId`
/// (rejecting floats like `12.0` and any extra `backendNodeId`/`objectId` keys).
pub fn parse_required_node_id(params: &Value) -> Result<i64, &'static str> {
    match parse_single_dom_target(params, true)? {
        DomTarget::NodeId(nid) => Ok(nid),
        _ => Err("DevTools DOM query call requires a single positive integer nodeId."),
    }
}

/// Validates `DOM.getDocument`'s optional integer `depth` parameter (defaults to `1`, `-1` = full).
pub fn parse_get_document_depth(params: &Value) -> Result<i64, &'static str> {
    let Some(v) = params.get("depth") else {
        return Ok(1);
    };
    if v.is_null() {
        return Ok(1);
    }
    let Some(d) = v.as_i64().filter(|&d| d >= -1) else {
        return Err("DevTools 'DOM.getDocument' depth must be an integer >= -1.");
    };
    Ok(d)
}

/// Strips explicit `null` target keys before forwarding validated DOM parameters to Chromium.
pub fn sanitize_dom_params(params: &Value) -> Value {
    let Some(obj) = params.as_object() else {
        return params.clone();
    };
    let mut out = obj.clone();
    for key in ["nodeId", "backendNodeId", "objectId"] {
        if out.get(key).is_some_and(Value::is_null) {
            out.remove(key);
        }
    }
    Value::Object(out)
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
/// active so `Accessibility.getFullAXTree` / `DOM.*` cannot dump or target a closed ShadowRoot
/// created by an extension content script before any extension iframe mounts
/// (`review-rd274.md` #311 Finding 1 & #314 Finding 1).
pub fn is_clean_ext_guarded_method(method: &str) -> bool {
    is_node_guard_method(method)
        || matches!(
            read_filter_for_method(method),
            Some(
                ReadFilterKind::AxTree
                    | ReadFilterKind::Dom
                    | ReadFilterKind::QuerySelector
                    | ReadFilterKind::QuerySelectorAll
            )
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
