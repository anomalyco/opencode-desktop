//! The tab a restored subagent's page was saved in. A subagent's page shows its root session's
//! side panel (`App::panel_root`), and which session that root is comes from the server's session
//! list, which a window's first frame doesn't have: without this the page would draw the
//! subagent's own (empty) side panel first and its tab's once the server answered. The window's
//! saved state names the tab the page was in (`tabs.recent`, written by every build), so the first
//! frame takes the root from there until the page's family is loaded up to its root.
//!
//! OpenCode Desktop keeps the routed child on the tab itself (`routeSessionId`,
//! `packages/app/src/shell/tabs/tabs.tsx`), so its tab names the root before any server answers.

use rune::Tracked;

use crate::{App, Route};

/// The restored page's session and the session of the tab it was saved in.
#[derive(Debug)]
pub struct PageTab {
    session: String,
    root: String,
}

/// After the window's saved page and tabs are back, before the first frame: notes the tab a
/// restored page belongs to when the page isn't a tab's own session.
pub(crate) fn note(s: &mut App) {
    let Route::Session { server, id } = &*s.route else { return };
    if s.tabs.iter().any(|t| !t.is_draft() && t.server == *server && t.id == *id) {
        return;
    }
    let Some(root) = s.gestures.home_return_tab.as_deref().and_then(|uid| s.tab(uid)).filter(|t| !t.is_draft() && t.server == *server).map(|t| t.id.clone()) else { return };
    let child = id.clone();
    s.launch.page_tab = Some(PageTab { session: child.clone(), root });
    // A strip of its own from a build before its page showed its root's joins the root's now,
    // so the first frame has every tab it will have.
    crate::panels::adopt_child_strip(s, &child);
}

/// Each poll: the note is for the restored page only, and goes when the route leaves it.
pub(crate) fn left(s: &mut App) {
    let Some(noted) = &Tracked::untracked(&s.launch).page_tab else { return };
    if !s.route.is_session(&noted.session) {
        s.launch.page_tab = None;
    }
}

/// The root of session `sid`'s family (`App::root_of`); for the restored page while its family
/// isn't loaded up to its root, the session of the tab it was saved in.
pub(crate) fn root_of<'a>(s: &'a App, sid: &'a str) -> &'a str {
    match &Tracked::untracked(&s.launch).page_tab {
        Some(noted) if noted.session == sid && s.missing_ancestor(sid).is_some() => noted.root.as_str(),
        _ => s.root_of(sid),
    }
}
