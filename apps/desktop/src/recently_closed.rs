//! `+` and New session's right-click: Desktop's "Recently closed" menu
//! (`recently-closed-tabs-menu.tsx`), 224 pt wide. Up to ten closed tabs that had a prompt,
//! newest first, reopen at the end of the strip; recent sessions without a tab fill the rest of
//! ten and open as a new tab. Each row leads with its project's avatar.

use rune::{PopupAnchor, Tx};

use crate::App;
use crate::popup::{self, Item, Menu};
use crate::tabs::tab_title;

/// How many entries the menu lists in all.
const LIMIT: usize = 10;

/// The menu's width (`w-56`).
const WIDTH: f32 = 224.0;

/// Opens the menu at `at`.
pub fn open(s: &mut App, at: impl Into<PopupAnchor>, tx: &mut Tx) {
    tx.instant();
    popup::open(s, menu(s), at, tx, |s: &mut App, picked, tx| {
        tx.instant();
        let Some(picked) = picked else { return };
        if let Some(id) = picked.strip_prefix("closed:") {
            s.reopen_closed(Some(id), true, tx);
        } else if let Some(id) = picked.strip_prefix("recent:") {
            s.open(id, tx);
        }
    });
}

fn menu(s: &App) -> Menu {
    let row = |key: &str, id: &str, title: &str, dir: &str| Item::new(format!("{key}:{id}"), tab_title(title)).project(dir);
    let entries: Vec<_> = s.closed.list(&s.tabs).filter(|e| e.prompted).take(LIMIT).collect();
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
        .filter(|i| i.parent.is_none() && !s.tabs.iter().any(|t| t.id == i.id) && !entries.iter().any(|e| e.tab.id == i.id))
        .collect();
    // Newest first (`compareSessionTime`).
    recent.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    let recent: Vec<Item> = recent.into_iter().take(LIMIT - closed.len()).map(|i| row("recent", &i.id, &i.title, &i.directory)).collect();
    let none = closed.is_empty() && recent.is_empty();
    let mut menu = Menu::new("Recently closed").width(WIDTH);
    if !closed.is_empty() {
        menu = menu.group("Recently closed").items(closed);
    }
    if !recent.is_empty() {
        menu = menu.group("Recent sessions").items(recent);
    }
    if none {
        menu = menu.item(Item::new("empty", "No recently closed sessions").enabled(false));
    }
    menu
}
