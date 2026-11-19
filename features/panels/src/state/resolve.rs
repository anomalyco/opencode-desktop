//! Cached side-region projection from panel providers and stored per-session tabs.

use std::rc::Rc;

use crate::host::PanelsHost;
use crate::tabs::{SessionTabs, panel_key, session_tab_pane};
use crate::width::side_wide;
use crate::{PanelSessionCtx, SessionPane};

use super::{ResolvedEntry, ResolvedRegion, all_panels};

/// The routed session's side region: its tabs from every panel's provider, in strip order.
///
/// Asking every provider (each plugin's `list`) costs a view about 100 us, and views and
/// handlers ask several times a frame: the last answer stands while the plugins' surfaces and
/// the `Tracked` state it read are as they were, as a region's would. A region that asks gets
/// those reads either way.
pub fn resolve_side_region(host: &dyn PanelsHost) -> Option<Rc<ResolvedRegion>> {
    let plugins = host.plugin_surfaces_version();
    {
        let cache = host.side().resolved.borrow();
        if let Some((version, read, region)) = &*cache
            && *version == plugins
            && !read.changed()
        {
            read.read();
            return region.clone();
        }
    }
    let (region, read) = rune::tracked_reads(|| resolve_side_region_now(host).map(Rc::new));
    *host.side().resolved.borrow_mut() = Some((plugins, read, region.clone()));
    host.side().resolved_gen.set(host.side().resolved_gen.get().wrapping_add(1));
    region
}

fn resolve_side_region_now(host: &dyn PanelsHost) -> Option<ResolvedRegion> {
    let session_id = host.active_session_id()?.to_owned();
    let directory = host.session_directory(&session_id).unwrap_or_default().to_owned();
    let project_id = host.session_project_id(&session_id);
    let empty_tabs = SessionTabs::default();
    let stored = host
        .side()
        .sessions
        .get(&session_id)
        .map_or(&empty_tabs, |s| &s.tab_state.tabs);

    let providers = all_panels(host);
    let mut entries = Vec::new();
    let mut launchers = Vec::new();

    for p in providers {
        let prefix = format!("{}:", p.plugin);
        let open_ids: Vec<String> = stored
            .all
            .iter()
            .filter_map(|k| k.strip_prefix(&prefix).map(str::to_owned))
            .collect();
        let ctx = PanelSessionCtx {
            plugin: p.plugin,
            session_id: &session_id,
            directory: &directory,
            project_id: &project_id,
            open_ids: &open_ids,
        };
        launchers.extend(p.launcher_entries(host, &ctx).into_iter().map(|(desc, launch)| (p.plugin, desc, launch)));
        for tab in p.list(host, &ctx) {
            let key = panel_key(p.plugin, &tab.id);
            let pane = if tab.pinned {
                SessionPane::Side
            } else if stored.all.contains(&key) {
                session_tab_pane(stored, &key)
            } else if let Some(sib) = stored.all.iter().rev().find(|k| k.starts_with(&prefix)) {
                session_tab_pane(stored, sib)
            } else {
                p.default_pane
            };
            entries.push(ResolvedEntry { key, plugin: p.plugin, tab, provider: p, pane });
        }
    }
    launchers.sort_by_key(|(_, d, _)| d.order);

    // Strip order: pinned first, then stored `first` tabs, then regular stored tabs.
    let mut strip: Vec<ResolvedEntry> = entries.iter().filter(|e| e.tab.pinned).cloned().collect();
    let mut first_stored = Vec::new();
    let mut rest_stored = Vec::new();
    for key in &stored.all {
        if let Some(entry) = entries.iter().find(|e| e.key == *key && !e.tab.pinned) {
            if entry.tab.first {
                first_stored.push(entry.clone());
            } else {
                rest_stored.push(entry.clone());
            }
        }
    }
    strip.extend(first_stored);
    strip.extend(rest_stored);

    let side_strip: Vec<&ResolvedEntry> = strip.iter().filter(|e| e.pane != SessionPane::Bottom).collect();
    let active = if let Some(act) = stored.active.as_deref()
        && strip.iter().any(|e| e.key == act)
    {
        Some(act.to_owned())
    } else {
        let eligible: Vec<&&ResolvedEntry> =
            side_strip.iter().filter(|e| e.pane == SessionPane::Side && e.tab.fallback).collect();
        eligible
            .iter()
            .find(|e| !e.tab.pinned && !e.tab.first)
            .or_else(|| eligible.iter().find(|e| !e.tab.pinned && e.tab.first))
            .or_else(|| eligible.iter().find(|e| e.tab.pinned))
            .map(|e| e.key.clone())
            .or_else(|| side_strip.first().map(|e| e.key.clone()))
    };

    let active_pane = active
        .as_deref()
        .and_then(|k| strip.iter().find(|e| e.key == k))
        .map_or(SessionPane::Side, |e| e.pane);
    let wide = side_wide(host);

    Some(ResolvedRegion {
        session_id,
        directory,
        project_id,
        entries,
        strip,
        active,
        active_pane,
        wide,
        launchers,
    })
}
