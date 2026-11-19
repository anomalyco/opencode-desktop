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
        self.settings.is_some() && other.settings.is_some() || (self.route == other.route && self.settings == other.settings)
    }
}

#[derive(Default)]
pub struct History {
    stack: Vec<Place>,
    cursor: Cursor,
}

/// `shell/titlebar/history-machine.ts` at e85035a36c. While traversing, repeated
/// Back/Forward steps from the target; a failed navigation stays at the source.
enum Cursor { Idle(usize), Traversing { source: usize, target: usize } }

impl Default for Cursor { fn default() -> Self { Self::Idle(0) } }

enum Event { Location(Place), Back(Option<Place>), Forward }

impl History {
    pub fn places(&self) -> Vec<Place> {
        self.stack.clone()
    }

    pub fn index(&self) -> usize {
        match self.cursor { Cursor::Idle(i) => i, Cursor::Traversing { target, .. } => target }
    }

    /// Whether `observe` would leave the history as it is: the app is where the current entry says.
    fn holds(&self, route: &Route, settings: &Option<Page>) -> bool {
        matches!(self.cursor, Cursor::Idle(i) if self.stack.get(i).is_some_and(|cur| cur.route == *route && cur.settings == *settings))
    }

    /// Where the app is now. The first call starts the stack with Home behind it.
    fn observe(&mut self, now: Place) {
        if let Cursor::Traversing { source, target } = self.cursor {
            self.cursor = Cursor::Idle(if self.stack[target].same(&now) { target } else { source });
        }
        if self.stack.is_empty() {
            let home = Place { route: Route::Home, settings: None };
            if now != home {
                self.stack.push(home);
            }
            self.stack.push(now);
            self.cursor = Cursor::Idle(self.stack.len() - 1);
            return;
        }
        let index = self.index();
        if self.stack[index].same(&now) {
            self.stack[index] = now;
            return;
        }
        self.stack.truncate(index + 1);
        self.stack.push(now);
        if self.stack.len() > MAX {
            self.stack.remove(0);
        }
        self.cursor = Cursor::Idle(self.stack.len() - 1);
    }

    fn step(&mut self, event: Event) -> Option<Place> {
        let mut source = match self.cursor { Cursor::Idle(i) | Cursor::Traversing { source: i, .. } => i };
        let target = match event {
            Event::Location(now) => { self.observe(now); return None; }
            Event::Forward => self.index() + 1,
            Event::Back(None) => self.index().checked_sub(1)?,
            Event::Back(Some(to)) => match self.index().checked_sub(1) {
                Some(i) => { self.stack[i] = to; i }
                None => { self.stack.insert(0, to); source += 1; 0 }
            },
        };
        let to = self.stack.get(target)?.clone();
        self.cursor = Cursor::Traversing { source, target };
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
            if i <= self.index() {
                index = kept.len() - 1;
            }
        }
        self.stack = kept;
        self.cursor = Cursor::Idle(index);
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
        let page = self.settings.open.then(|| self.kernel.slots.get(&crate::settings::SETTINGS_PAGES).page.clone());
        // Every poll comes here: the history is written only when the app moved.
        if !self.nav.holds(&self.route, &page) && (self.opening.is_none() || matches!(self.nav.cursor, Cursor::Idle(_))) {
            let now = Place { route: (*self.route).clone(), settings: page };
            self.nav.step(Event::Location(now));
        }
    }

    /// Opens the entry before (or after) this one. False when there's nowhere to go.
    pub fn navigate(&mut self, forward: bool, tx: &mut Tx) -> bool {
        self.sync_history();
        let dest = (self.settings.open && self.project_navigation()).then(|| Place { route: self.restore_project_tab().map_or(Route::Home, |tab| tab.route()), settings: None });
        let Some(to) = self.nav.step(if forward { Event::Forward } else { Event::Back(dest) }) else { return false };
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
