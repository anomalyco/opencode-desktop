//! Which built-in plugins draw on this launch's first screen, decided from the boot state before
//! any of them starts: those start before the first frame, so nothing they show arrives a frame
//! late, and the rest start after it (`super::boot::first_frame`). Each plugin's contributions come from
//! its source at build time (`sources::contributions`).
//!
//! - **Every route:** a server source whose servers were listed at the last run (the sidebar and
//!   Home show their rows; the plugin's state for them would change after the frame otherwise),
//!   and the debug plugin while its bar (a `window.bottom` slot) is shown.
//! - **Not before the frame on any route:** the updater's titlebar pill shows only once the
//!   updater's worker, started after the first frame, has an update ready, so it's hidden in the
//!   first frame and the second either way.
//! - **A session:** the slots in its header (the debug bar's only while it's shown, above), the
//!   composer's pills; with a cached transcript on screen, the code blocks and link handlers its
//!   messages draw with; and every
//!   plugin with a tab in the session's open side or bottom pane (the tab strip shows each one's
//!   title), plus the file viewers when the Files plugin has one.
//! - **Home and a draft:** nothing else.

use opencode_storage::Lookup;

use crate::App;
use crate::panels::SessionPane;
use crate::route::Route;

use super::sources;

/// The built-in plugins `app`'s first screen shows.
pub fn plugins(app: &App) -> Vec<&'static str> {
    let has = |id: &str, kinds: &[&str]| sources::contributions(id).iter().any(|k| kinds.contains(k));
    let mut out: Vec<&'static str> = sources::builtin_ids().filter(|id| has(id, &["ServerSource"]) && crate::plugin_host::server_sources::remembers(id)).collect();
    if debug_bar_shown() {
        out.push("debug");
    }
    if let Some(open) = &app.plugins.dialog
        && let Some(id) = sources::builtin_ids().find(|id| *id == &*open.plugin)
    {
        out.push(id);
    }
    if app.settings_open() {
        out.extend(sources::builtin_ids().filter(|id| has(id, &["SettingsPage", "Setting"])));
    }
    let Route::Session { id: session, .. } = &*app.route else { return out };
    let transcript = app.launch.cold_transcript.is_some();
    // The debug plugin's one slot is its bar (`window.bottom`), counted above: hidden, it draws nothing.
    out.extend(sources::builtin_ids().filter(|id| (*id != "debug" && has(id, &["Slot", "ComposerPill"])) || (transcript && has(id, &["CodeBlock", "LinkHandler"]))));
    let side_state = app.kernel.slots.get(&crate::panels::SIDE);
    let side = side_state.sessions.get(session);
    let side_open = if app.settings.shared_side_panel {
        side_state.shared_open.unwrap_or(false)
    } else {
        side.is_some_and(|st| st.visibility.open())
    };
    let with_tabs: Vec<&str> = side
        .map(|side| {
            let tabs = &side.tab_state.tabs;
            let shown = |key: &String| match tabs.panes.get(key).copied().unwrap_or(SessionPane::Side) {
                SessionPane::Bottom => side.bottom_open,
                _ => side_open,
            };
            tabs.all.iter().filter(|k| shown(k)).filter_map(|k| crate::panels::tabs::split_panel_key(k).map(|(plugin, _)| plugin)).collect()
        })
        .unwrap_or_default();
    let files = with_tabs.contains(&"file");
    let has_side_tab = side.is_some_and(|side| {
        side.tab_state.tabs.all.iter().any(|k| side.tab_state.tabs.panes.get(k).copied().unwrap_or(SessionPane::Side) != SessionPane::Bottom)
    });
    let empty_side = side_open && !has_side_tab;
    if !with_tabs.is_empty() || empty_side {
        out.extend(sources::builtin_ids().filter(|id| with_tabs.contains(id) || (files && has(id, &["FileViewer"])) || (empty_side && has(id, &["Panel", "MenuItem"]))));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether the debug bar is up at launch: the debug plugin's `bar` value says so, or it isn't in
/// memory to say otherwise.
pub(super) fn debug_bar_shown() -> bool {
    let ns = opencode_storage::plugin_namespace("debug");
    let value = match crate::storage::plugin_value(&ns, "bar") {
        Lookup::Pending => crate::storage::boot_packet().map_or(Lookup::Pending, |boot| boot.plugin_value(&ns, "bar")),
        ready => ready,
    };
    match value {
        Lookup::Pending => true,
        Lookup::Ready(v) => v.and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok()).and_then(|v| v.get("visible")?.as_bool()).unwrap_or(false),
    }
}
