//! The tab a restored subagent's page was saved in. A subagent's page shows its root session's
//! side panel (`App::panel_root`), and which session that root is comes from the server's session
//! list, which a window's first frame doesn't have: without this the page would draw the
//! subagent's own (empty) side panel first and its tab's once the server answered.
//!
//! The window says it itself: while a page draws a tab's side panel it saves that tab and the
//! page's session together (`tabs.recent`: `key` and `routeSessionId`, in one value), so the
//! first frame takes the root from there until the page's family is loaded up to its root.
//! Nothing else names it. `tabs.recent.key` alone is also the tab Home returns to, which says
//! nothing of the page, so a window saved without `routeSessionId` (by an older build, or before
//! the page's parents were listed) draws the page's own side panel until the family is loaded:
//! no strip is drawn, written or joined on a guess.
//!
//! OpenCode Desktop keeps the routed child on the tab itself (`routeSessionId`,
//! `packages/app/src/shell/tabs/tabs.tsx`), so its tab names the root before any server answers.

use rune::Tracked;

use crate::{App, Route};

/// What the window's saved state says of the tab its page was in.
#[derive(Debug, Default)]
pub enum PageTab {
    /// Nothing: no page restored yet, a tab's own page, or a page the saved state names no tab for.
    #[default]
    Unsaid,
    /// It names `session` as the session routed in its recent tab; the page and the tabs aren't
    /// back yet.
    Said { session: String },
    /// The restored page is `session`, in the tab of session `root`.
    Held { session: String, root: String },
}

/// The saved state is read (`RestoreHost::set_saved_routed_child`): what it says.
pub(crate) fn said(s: &mut App, session: Option<String>) {
    s.launch.page_tab = session.map_or(PageTab::Unsaid, |session| PageTab::Said { session });
}

/// After the window's saved page and tabs are back, before the first frame: holds the tab the
/// restored page belongs to, when the saved state said that page was routed in it.
pub(crate) fn note(s: &mut App) {
    let PageTab::Said { session } = std::mem::take(&mut s.launch.page_tab) else { return };
    let Route::Session { server, id } = &*s.route else { return };
    if *id != session || s.tabs.iter().any(|t| !t.is_draft() && t.server == *server && t.id == *id) {
        return;
    }
    let Some(root) = s.gestures.home_return_tab.as_deref().and_then(|uid| s.tab(uid)).filter(|t| !t.is_draft() && t.server == *server).map(|t| t.id.clone()) else { return };
    s.launch.page_tab = PageTab::Held { session, root };
}

/// Each poll: what is held is for the restored page only, and goes when the route leaves it.
pub(crate) fn left(s: &mut App) {
    let PageTab::Held { session, .. } = &Tracked::untracked(&s.launch).page_tab else { return };
    if !s.route.is_session(session) {
        s.launch.page_tab = PageTab::Unsaid;
    }
}

/// The root of session `sid`'s family (`App::root_of`); for the restored page while its family
/// isn't loaded up to its root, the session of the tab the saved state said it was routed in.
pub(crate) fn root_of<'a>(s: &'a App, sid: &'a str) -> &'a str {
    match &Tracked::untracked(&s.launch).page_tab {
        PageTab::Held { session, root } if session == sid && s.missing_ancestor(sid).is_some() => root.as_str(),
        _ => s.root_of(sid),
    }
}

/// The tab whose side panel the page draws and the page's session, when that tab isn't the
/// page's own (`RestoreHost::routed_child`): what the next launch may take the page's tab from.
/// Known from the sessions the server listed, or held from a save that knew.
pub(crate) fn routed_child(s: &App) -> Option<(String, String)> {
    let Route::Session { server, id } = &*s.route else { return None };
    let root = s.panel_root(id);
    if root == id {
        return None;
    }
    s.tabs.iter().find(|t| !t.is_draft() && t.server == *server && t.id == root).map(|t| (t.uid(), id.clone()))
}
