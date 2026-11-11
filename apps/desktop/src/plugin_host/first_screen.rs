//! Which built-in plugins draw on this launch's first screen, decided from the boot state before
//! any of them starts: those start before the first frame, so nothing they show arrives a frame
//! late, and the rest start after it (`super::first_frame`). Each plugin's contributions come from
//! its source at build time (`sources::contributions`).
//!
//! - **Every route:** a server source whose servers were listed at the last run (the sidebar and
//!   Home show their rows; the plugin's state for them would change after the frame otherwise).
//! - **A session:** the slots in its header and debug bar, the composer's pills; with a cached
//!   transcript on screen, the code blocks and link handlers its messages draw with; and every
//!   plugin with a tab in the session's open side or bottom pane (the tab strip shows each one's
//!   title), plus the file viewers when the Files plugin has one.
//! - **Home and a draft:** nothing else.

use crate::App;
use crate::panels::SessionPane;
use crate::route::Route;

use super::sources;

/// The built-in plugins `app`'s first screen shows.
pub fn plugins(app: &App) -> Vec<&'static str> {
    let has = |id: &str, kinds: &[&str]| sources::contributions(id).iter().any(|k| kinds.contains(k));
    let mut out: Vec<&'static str> = sources::builtin_ids().filter(|id| has(id, &["ServerSource"]) && super::server_sources::remembers(id)).collect();
    let Route::Session { id: session, .. } = &app.route else { return out };
    let transcript = app.launch.cold_transcript.is_some();
    out.extend(sources::builtin_ids().filter(|id| has(id, &["Slot", "ComposerPill"]) || (transcript && has(id, &["CodeBlock", "LinkHandler"]))));
    if let Some(side) = app.side.sessions.get(session) {
        let tabs = &side.tab_state.tabs;
        let shown = |key: &String| match tabs.panes.get(key).copied().unwrap_or(SessionPane::Side) {
            SessionPane::Bottom => side.bottom_open,
            _ => side.open,
        };
        let with_tabs: Vec<&str> = tabs.all.iter().filter(|k| shown(k)).filter_map(|k| crate::panels::tabs::split_panel_key(k).map(|(plugin, _)| plugin)).collect();
        let files = with_tabs.contains(&"file");
        out.extend(sources::builtin_ids().filter(|id| with_tabs.contains(id) || (files && has(id, &["FileViewer"]))));
    }
    out.sort_unstable();
    out.dedup();
    out
}
