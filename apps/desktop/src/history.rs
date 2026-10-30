//! ⌘[ and ⌘] go back and forward through where you've been: Desktop's titlebar history
//! (`shell/titlebar/history.ts`). Every move to another page is an entry (a session, a subagent,
//! Home, a new draft, Settings); a new move drops what was ahead. Back and forward open the
//! entry at once, with no animation. Closing a tab or deleting a session drops its entries.

use rune::{Key, KeyEvent, MenuItem, Mods, PhysicalKeyCode, Tx};

use crate::{App, Route};

const MAX: usize = 100;

/// One entry: the page, and whether Settings is over it.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub route: Route,
    pub settings: bool,
}

#[derive(Default)]
pub struct History {
    stack: Vec<Place>,
    index: usize,
    /// The entry back or forward is opening, until the page lands on it (a session may wait for its page).
    expect: Option<Place>,
}

impl History {
    pub fn places(&self) -> Vec<Place> {
        self.stack.clone()
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Where the app is now. The first call starts the stack with Home behind it.
    fn observe(&mut self, route: &Route, settings: bool, opening: bool) {
        if self.expect.is_none()
            && let Some(cur) = self.stack.get(self.index)
            && cur.route == *route
            && cur.settings == settings
        {
            return;
        }
        let now = Place { route: route.clone(), settings };
        if let Some(want) = &self.expect {
            if *want == now {
                self.expect = None;
                return;
            }
            if opening {
                return;
            }
            self.expect = None;
        }
        if self.stack.is_empty() {
            let home = Place { route: Route::Home, settings: false };
            if now != home {
                self.stack.push(home);
            }
            self.stack.push(now);
            self.index = self.stack.len() - 1;
            return;
        }
        if self.stack[self.index] == now {
            return;
        }
        self.stack.truncate(self.index + 1);
        self.stack.push(now);
        if self.stack.len() > MAX {
            self.stack.remove(0);
        }
        self.index = self.stack.len() - 1;
    }

    fn step(&mut self, forward: bool) -> Option<Place> {
        let index = if forward { self.index + 1 } else { self.index.checked_sub(1)? };
        let to = self.stack.get(index)?.clone();
        self.index = index;
        self.expect = Some(to.clone());
        Some(to)
    }

    /// Drops the entries `gone` picks, and the repeats that leaves side by side.
    pub fn forget(&mut self, gone: impl Fn(&Place) -> bool) {
        let mut kept: Vec<Place> = Vec::new();
        let mut index = 0;
        for (i, place) in std::mem::take(&mut self.stack).into_iter().enumerate() {
            if gone(&place) {
                continue;
            }
            if kept.last() != Some(&place) {
                kept.push(place);
            }
            if i <= self.index {
                index = kept.len() - 1;
            }
        }
        self.stack = kept;
        self.index = index;
        self.expect = None;
    }
}

/// `forward` for ⌘], `false` for ⌘[. Desktop reads `{` and `}` as the brackets, and a layout
/// that types no ASCII there falls back to the physical key, so it works on any layout.
pub fn bracket(ev: &KeyEvent, physical: Option<PhysicalKeyCode>) -> Option<bool> {
    let Mods { cmd, ctrl, alt, shift, .. } = ev.mods;
    if !cmd || ctrl || alt || shift {
        return None;
    }
    let Key::Command(c) = ev.key else { return None };
    match c {
        '[' => Some(false),
        ']' => Some(true),
        _ if !c.is_ascii() => match physical? {
            PhysicalKeyCode::BracketLeft => Some(false),
            PhysicalKeyCode::BracketRight => Some(true),
            _ => None,
        },
        _ => None,
    }
}

impl App {
    /// Notes where the app is, so a move made since the last look joins the history.
    pub fn sync_history(&mut self) {
        self.nav.observe(&self.route, self.settings.open, self.opening.is_some());
    }

    /// Opens the entry before (or after) this one. False when there's nowhere to go.
    pub fn navigate(&mut self, forward: bool, tx: &mut Tx) -> bool {
        self.sync_history();
        let Some(to) = self.nav.step(forward) else { return false };
        tx.instant();
        self.cancel_open(tx);
        self.close_picker(false);
        match to.route {
            Route::Session(id) => self.open(&id, tx),
            route => self.route = route,
        }
        match (to.settings, self.settings.open) {
            (true, false) => self.open_settings(),
            (false, true) => self.close_settings(),
            _ => {}
        }
        tx.settle();
        true
    }

    /// A tab is closed or sessions are deleted: their pages leave the history.
    pub fn forget_history(&mut self, gone: impl Fn(&App, &str) -> bool) {
        self.sync_history();
        let ids: Vec<String> = self
            .nav
            .stack
            .iter()
            .filter_map(|p| match &p.route {
                Route::Session(id) if gone(self, id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.nav.forget(|p| matches!(&p.route, Route::Session(id) if ids.contains(id)));
    }
}

/// The Go menu's Back and Forward. Like the other menus they carry no key equivalent: Rune
/// draws those on the wrong keys and the menu would then take them, so `on_key` handles ⌘[ and ⌘].
pub fn menu_items() -> Vec<MenuItem<App>> {
    vec![
        MenuItem::new("Back", |s: &mut App, tx: &mut Tx| {
            s.navigate(false, tx);
        }).id("Go/Back").menu("Go"),
        MenuItem::new("Forward", |s: &mut App, tx: &mut Tx| {
            s.navigate(true, tx);
        }).id("Go/Forward").menu("Go"),
    ]
}

pub fn install(app: rune::App<App>) -> rune::App<App> {
    menu_items().into_iter().fold(app, |a, i| a.menu_item(i))
}
