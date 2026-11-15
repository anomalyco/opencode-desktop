//! ⌘[ and ⌘] go back and forward through where you've been: Desktop's titlebar history
//! (`shell/titlebar/history.ts`). Every move to another page is an entry (a session, a subagent,
//! Home, a new draft, Settings); a new move drops what was ahead. Back and forward open the
//! entry at once, with no animation. Closing a tab or deleting a session drops its entries.
//! Settings is one entry whose page follows the page shown (Desktop navigates between its pages
//! with `replace`), so going back into Settings reopens the page it was left on.

use crate::pickers::PickerHost as _;
use rune::{Key, KeyEvent, MenuItem, PhysicalKeyCode, Tx};

use crate::keymap::Bind;
use crate::settings::pages::Page;
use crate::{App, Route};

const MAX: usize = 100;

/// One entry: the page, and the Settings page over it, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub route: Route,
    pub settings: Option<Page>,
}

impl Place {
    /// The same entry, whichever Settings page it shows.
    fn same(&self, other: &Place) -> bool {
        self.route == other.route && self.settings.is_some() == other.settings.is_some()
    }
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

    /// Whether `observe` would leave the history as it is: the app is where the current entry says.
    fn holds(&self, route: &Route, settings: &Option<Page>) -> bool {
        self.expect.is_none() && self.stack.get(self.index).is_some_and(|cur| cur.route == *route && cur.settings == *settings)
    }

    /// Where the app is now. The first call starts the stack with Home behind it.
    fn observe(&mut self, route: &Route, settings: Option<Page>, opening: bool) {
        // Every poll lands here, so staying put clones nothing.
        if self.expect.is_none()
            && let Some(cur) = self.stack.get_mut(self.index)
            && cur.route == *route
            && cur.settings.is_some() == settings.is_some()
        {
            cur.settings = settings;
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
            let home = Place { route: Route::Home, settings: None };
            if now != home {
                self.stack.push(home);
            }
            self.stack.push(now);
            self.index = self.stack.len() - 1;
            return;
        }
        if self.stack[self.index].same(&now) {
            self.stack[self.index].settings = now.settings;
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
            if !kept.last().is_some_and(|k| k.same(&place)) {
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
    let Key::Command(c) = ev.key else { return None };
    // Desktop's mod+[ has no Shift, and ⇧⌘[ is the side panel's previous tab: the keybind's
    // match lets Shift through on punctuation, so Back must say no to it itself.
    if ev.mods.shift {
        return None;
    }
    let key = match (c.is_ascii(), physical) {
        (false, Some(PhysicalKeyCode::BracketLeft)) => '[',
        (false, Some(PhysicalKeyCode::BracketRight)) => ']',
        _ => c,
    };
    let ev = KeyEvent { key: Key::Command(key), mods: ev.mods };
    [(Bind::GoBack, false), (Bind::GoForward, true)].into_iter().find(|(b, _)| b.matches(&ev)).map(|(_, forward)| forward)
}

impl App {
    /// Notes where the app is, so a move made since the last look joins the history.
    pub fn sync_history(&mut self) {
        let page = self.settings.open.then(|| self.settings_pages.page.clone());
        // Every poll comes here: the history is written only when the app moved.
        if !self.nav.holds(&self.route, &page) {
            self.nav.observe(&*self.route, page, self.opening.is_some());
        }
    }

    /// Opens the entry before (or after) this one. False when there's nowhere to go.
    pub fn navigate(&mut self, forward: bool, tx: &mut Tx) -> bool {
        self.sync_history();
        let Some(to) = self.nav.step(forward) else { return false };
        let keep_home = tx.focused() == Some("home");
        tx.instant();
        self.cancel_open(tx);
        self.close_picker(false);
        match to.route {
            Route::Session { id, server } => {
                crate::servers::runtime::switch(self, &server);
                self.open(&id, tx);
            }
            route => *self.route = route,
        }
        match (to.settings, self.settings.open) {
            (Some(page), _) => {
                tx.scroll_to_instant("settings", 0.0, 0.0);
                self.open_settings_at(page);
            }
            (None, true) => self.close_settings(),
            (None, false) => {}
        }
        session_view::jump::settle(self, tx);
        // The persistent titlebar trigger didn't leave when the route changed.
        if keep_home {
            self.composer.focused = false;
            tx.focus("home", tx.focus_visible());
        }
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
                Route::Session { id, server } if server == crate::servers::runtime::active(self) && gone(self, id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.nav.forget(|p| matches!(&p.route, Route::Session { id, .. } if ids.contains(id)));
    }
}

/// The Go menu's Back and Forward (`keys_first`, so focused fields, modals and `on_key` see
/// ⌘[ and ⌘] before the menu item).
pub fn menu_items() -> Vec<MenuItem<App>> {
    use crate::menubar::keyed;
    vec![
        keyed(MenuItem::new(opencode_i18n::t!("desktop.menu.back"), |s: &mut App, tx: &mut Tx| {
            s.navigate(false, tx);
        }).id("Go/Back").menu("Go"), Bind::GoBack)
        .keys_first(),
        keyed(MenuItem::new(opencode_i18n::t!("desktop.menu.forward"), |s: &mut App, tx: &mut Tx| {
            s.navigate(true, tx);
        }).id("Go/Forward").menu("Go"), Bind::GoForward)
        .keys_first(),
    ]
}
