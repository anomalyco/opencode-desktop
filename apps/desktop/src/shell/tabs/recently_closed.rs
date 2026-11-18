//! `+` and New session's right-click: Desktop's "Recently closed" menu
//! (`recently-closed-tabs-menu.tsx`), 224 pt wide, under its "Search recently closed" field. Up
//! to ten closed tabs that had a prompt, newest first, reopen at the end of the strip; recent
//! sessions without a tab fill the rest of ten and open as a new tab. Each row leads with its
//! project's avatar, and the field narrows both lists by title.

use rune::{PopupAnchor, Tx};

use crate::App;
use crate::popup::{self, Item, Menu};
use crate::tabs::tab_title;

/// How many entries the menu lists in all.
const LIMIT: usize = 10;

/// The menu's width (`w-56`). Its two titles and ten rows fit without scrolling, as in
/// Desktop's two-thirds-of-the-window cap.
const WIDTH: f32 = 224.0;

/// Opens the menu at `at`, drawn on every platform for its search field, as Desktop draws it.
pub fn open(s: &mut App, at: impl Into<PopupAnchor>, tx: &mut Tx) {
    tx.instant();
    popup::open_searchable(
        s,
        opencode_i18n::t!("session.tabs.closed.search"),
        menu,
        at,
        tx,
        |s: &mut App, picked, tx| {
            tx.instant();
            let Some(picked) = picked else { return };
            if let Some(id) = picked.strip_prefix("closed:") {
                s.reopen_closed(Some(id), true, tx);
            } else if let Some(id) = picked.strip_prefix("recent:") {
                s.open(id, tx);
            }
        },
    );
}

/// The rows whose titles hold `query`, ignoring case.
fn menu(s: &App, query: &str) -> Menu {
    let query = query.trim().to_lowercase();
    let matches = |title: &str| query.is_empty() || tab_title(title).to_lowercase().contains(&query);
    let row = |key: &str, id: &str, title: &str, dir: &str| Item::new(format!("{key}:{id}"), tab_title(title)).project(dir);
    let entries: Vec<_> = s
        .closed
        .list(&s.tabs)
        .filter(|e| e.prompted)
        .filter(|e| matches(s.session(&e.tab.id).map_or(e.tab.title.as_str(), |i| i.title.as_str())))
        .take(LIMIT)
        .collect();
    let closed: Vec<Item> = entries
        .iter()
        .map(|e| {
            let info = s.session(&e.tab.id);
            let title = info.map_or(e.tab.title.as_str(), |i| i.title.as_str());
            let dir = if e.tab.directory.is_empty() { info.map_or("", |i| i.directory.as_str()) } else { e.tab.directory.as_str() };
            row("closed", &e.tab.id, title, dir)
        })
        .collect();
    let mut recent: Vec<_> = s
        .sessions
        .iter()
        .filter(|i| i.parent.is_none() && !s.tabs.iter().any(|t| t.id == i.id) && !entries.iter().any(|e| e.tab.id == i.id) && matches(&i.title))
        .collect();
    // Newest first (`compareSessionTime`).
    recent.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    let recent: Vec<Item> = recent.into_iter().take(LIMIT - closed.len()).map(|i| row("recent", &i.id, &i.title, &i.directory)).collect();
    let none = closed.is_empty() && recent.is_empty();
    let mut menu = Menu::new(opencode_i18n::t!("home.recentlyClosed")).width(WIDTH).rows_before_scroll(13.5);
    if !closed.is_empty() {
        menu = menu.group(opencode_i18n::t!("home.recentlyClosed")).items(closed);
    }
    if !recent.is_empty() {
        menu = menu.group(opencode_i18n::t!("palette.group.recentSessions")).items(recent);
    }
    if none {
        let empty = if query.is_empty() { opencode_i18n::t!("session.tabs.closed.empty") } else { opencode_i18n::t!("session.search.noResults") };
        menu = menu.item(Item::new("empty", empty).enabled(false));
    }
    menu
}
