//! The app's menus and pickers: a session's, a tab's and a project's right-click menu, the
//! session's … menu, Open in, the side panel's tab menus, Settings' selects and the Windows app
//! menu. macOS shows them as native menus (`Tx::popup_menu`), as Kit chose there. Everywhere
//! else they're drawn in the window, as Desktop draws its own (`menu-v2`), with the one popover
//! every menu is built from (`popover::MENU`), or menu-v2's standard look ([`Menu::standard`]):
//! they take the theme, the keyboard, the window's zoom and headless captures. A menu drawn on
//! every platform (the side panel's) opens with [`open_drawn`].

use rune::{Align, El, Icon, Key, KeyEvent, PopupAnchor, PopupItem, PopupMenu, Side, TextEdit, TextStyle, Tx, col, el, icon, row, text};

use crate::App;
use crate::palette::Pal;
use crate::popover::{self, MENU, Opt};

mod search;
mod standard;

use search::{SEARCH_H, search_field};

/// Native menus where Kit chose them.
const NATIVE: bool = cfg!(target_os = "macos");

/// The tallest a drawn menu gets before its rows scroll: eleven rows and most of the twelfth.
const MAX_H: f32 = 2.0 * MENU.inset + 11.5 * MENU.row_h;

/// A drawn menu's narrowest, as Desktop's: a dropdown's, and a right-click menu's.
const MIN_W: f32 = 96.0;
const CONTEXT_MIN_W: f32 = 160.0;

/// From a point (a right-click) or an element to the menu's corner.
const GAP: f32 = 2.0;

/// A group's title (`menu-v2-group-label`): a 28 pt row, 11/530, faint, on the labels' edge.
const GROUP_H: f32 = 28.0;
const GROUP: TextStyle = TextStyle::new(11.0, 530.0).lh(14.0).tracking(0.05);

/// One choice. Enabled, unchecked and in the text colour unless set.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    id: String,
    title: String,
    enabled: bool,
    checked: bool,
    danger: bool,
    /// The folder whose project avatar leads the row (drawn menus only).
    project: Option<String>,
    shortcut: Option<String>,
    /// The glyph before the title (drawn menus only).
    glyph: Option<Glyph>,
    /// A row that only tells (`SessionLocationDetail`): never lit or picked, in the base ink.
    detail: bool,
    /// Lines added and removed after the title, in the diff's inks (drawn menus only).
    diff: Option<(u32, u32)>,
}

/// A row's icon, the same when it's the same icon.
#[derive(Clone, Copy, Debug)]
enum Glyph {
    /// 14 pt, in the title's ink.
    Ink(&'static Icon),
    /// 14 pt, in `icon-weak` (`text-icon-weak`).
    Weak(&'static Icon),
    /// An app's 16 pt mark in its own colours (`AppIcon`), by its id, in the appearance's variant.
    App(&'static str),
}

impl Glyph {
    /// A one-colour glyph's icon; None for an app's mark.
    fn mono(self) -> Option<&'static Icon> {
        match self {
            Glyph::Ink(g) | Glyph::Weak(g) => Some(g),
            Glyph::App(_) => None,
        }
    }
}

impl PartialEq for Glyph {
    fn eq(&self, other: &Glyph) -> bool {
        match (self, other) {
            (Glyph::Ink(a), Glyph::Ink(b)) | (Glyph::Weak(a), Glyph::Weak(b)) => std::ptr::eq(*a, *b),
            (Glyph::App(a), Glyph::App(b)) => a == b,
            _ => false,
        }
    }
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Item {
        Item { id: id.into(), title: title.into(), enabled: true, checked: false, danger: false, project: None, shortcut: None, glyph: None, detail: false, diff: None }
    }
    pub fn enabled(mut self, on: bool) -> Item { self.enabled = on; self }
    /// A check at the row's end, as a picker marks its current value.
    pub fn checked(mut self, on: bool) -> Item { self.checked = on; self }
    /// A destructive action, in Desktop's critical ink (drawn menus only).
    pub fn danger(mut self, on: bool) -> Item { self.danger = on; self }
    /// The 16 pt project avatar for folder `dir` before the title, as a session row has it.
    pub fn project(mut self, dir: impl Into<String>) -> Item { self.project = Some(dir.into()); self }
    /// A 14 pt icon before the title, as menu-v2's items lead with one (`menu.tsx`).
    pub fn icon(mut self, glyph: &'static Icon) -> Item { self.glyph = Some(Glyph::Ink(glyph)); self }
    /// A 14 pt icon in Desktop's weak icon ink, as Open in's Copy path has.
    pub fn weak_icon(mut self, glyph: &'static Icon) -> Item { self.glyph = Some(Glyph::Weak(glyph)); self }
    /// App `id`'s 16 pt mark (`AppIcon`) before the title, as Open in's apps have.
    pub fn app_icon(mut self, id: &'static str) -> Item { self.glyph = Some(Glyph::App(id)); self }
    /// A row that only tells, as the location card's server and its branch-less line do.
    pub fn detail(mut self) -> Item { self.detail = true; self.enabled = false; self }
    /// A change count's lines after the title, as Desktop's `DiffChanges` reads them.
    pub fn diff(mut self, added: u32, removed: u32) -> Item { self.diff = Some((added, removed)); self }
    /// The keys that do the same, at the row's end ("Ctrl+Shift+N"); none when empty.
    pub fn shortcut(mut self, keys: impl Into<String>) -> Item {
        let keys = keys.into();
        self.shortcut = (!keys.is_empty()).then_some(keys);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Item(Item),
    Separator,
    /// A group's title (`Menu.GroupLabel`).
    Group(String),
    /// A row that opens another menu beside it.
    Sub(Box<Sub>),
}

#[derive(Clone, Debug, PartialEq)]
struct Sub {
    id: String,
    title: String,
    menu: Menu,
    /// The glyph before the title (drawn menus only).
    glyph: Option<Glyph>,
}

/// How a drawn menu looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Look {
    /// `menu-v2`'s own: [`popover::MENU`].
    #[default]
    Menu,
    /// `menu-v2`'s `appearance="standard"`, which Desktop's app menu takes.
    Standard,
}

/// A search field over a drawn menu's rows, which `rows` rebuilds from what's typed.
#[derive(Clone, Debug, PartialEq)]
struct Search {
    placeholder: String,
    rows: fn(&App, &str) -> Menu,
}

/// A menu's rows, where it opens beside an element, and how narrow it may be.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    label: String,
    entries: Vec<Entry>,
    /// A picker's current value: checked, lit as the menu opens, and over the anchor natively.
    selected: Option<String>,
    align: Align,
    min_w: f32,
    /// A fixed width, as a menu with `w-56` has.
    width: Option<f32>,
    /// How tall it gets before its rows scroll.
    max_h: f32,
    look: Look,
    /// Which side of an element it opens on, flipping when it doesn't fit.
    side: Side,
    /// Presses on it leave the focused field focused (`El::keeps_text_selection`).
    keeps_focus: bool,
    search: Option<Search>,
    /// Its distance from the element it opens beside (Kobalte's `gutter`), when not the default.
    gap: Option<f32>,
}

impl Menu {
    /// `label` names the menu for assistive technology ("Session", "Theme").
    pub fn new(label: impl Into<String>) -> Menu {
        Menu { label: label.into(), entries: Vec::new(), selected: None, align: Align::Start, min_w: MIN_W, width: None, max_h: MAX_H, look: Look::Menu, side: Side::Bottom, keeps_focus: false, search: None, gap: None }
    }

    /// A search field over the rows, focused as the menu opens (drawn menus): `rows` builds the
    /// menu for what's typed, and the arrows and Enter move through what it lists.
    pub fn search(mut self, placeholder: impl Into<String>, rows: fn(&App, &str) -> Menu) -> Menu {
        self.search = Some(Search { placeholder: placeholder.into(), rows });
        self
    }

    /// A picker: one checked-when-current row per `(id, title)`, lined up with the end of the
    /// select that opens it.
    pub fn picker<I: Into<String>, T: Into<String>>(label: impl Into<String>, options: impl IntoIterator<Item = (I, T)>, selected: &str) -> Menu {
        let items = options.into_iter().map(|(id, title)| {
            let id = id.into();
            let on = id == selected;
            Item::new(id, title).checked(on)
        });
        Menu { selected: Some(selected.to_owned()), min_w: 180.0, align: Align::End, ..Menu::new(label) }.items(items)
    }

    pub fn item(mut self, item: Item) -> Menu { self.entries.push(Entry::Item(item)); self }
    pub fn items(mut self, items: impl IntoIterator<Item = Item>) -> Menu {
        self.entries.extend(items.into_iter().map(Entry::Item));
        self
    }
    pub fn separator(mut self) -> Menu { self.entries.push(Entry::Separator); self }
    /// A group's title above the rows that follow.
    pub fn group(mut self, title: impl Into<String>) -> Menu { self.entries.push(Entry::Group(title.into())); self }
    /// Draws the menu this wide, whatever its rows.
    pub fn width(mut self, w: f32) -> Menu { self.width = Some(w); self }
    /// Lets the menu grow to `rows` rows before they scroll.
    pub fn rows_before_scroll(mut self, rows: f32) -> Menu { self.max_h = 2.0 * MENU.inset + rows * MENU.row_h; self }
    /// A row titled `title` that opens `menu` beside it; its items answer the same `pick`.
    pub fn submenu(mut self, id: impl Into<String>, title: impl Into<String>, menu: Menu) -> Menu {
        self.entries.push(Entry::Sub(Box::new(Sub { id: id.into(), title: title.into(), menu, glyph: None })));
        self
    }
    /// [`Menu::submenu`] led by `glyph`, as the location card's branch and folder rows are.
    pub fn submenu_icon(mut self, id: impl Into<String>, title: impl Into<String>, glyph: &'static Icon, menu: Menu) -> Menu {
        self.entries.push(Entry::Sub(Box::new(Sub { id: id.into(), title: title.into(), menu, glyph: Some(Glyph::Ink(glyph)) })));
        self
    }
    /// Opens above the element, or below it when there's no room (`placement="top-*"`).
    pub fn above(mut self) -> Menu { self.side = Side::Top; self }
    /// At least `w` wide, as menu-v2's content is 160 pt (`menu.css:12`).
    pub fn min_width(mut self, w: f32) -> Menu { self.min_w = w; self }
    /// As wide as its widest row (`menu-v2`'s `w-max`), its entries added.
    pub fn hug(mut self) -> Menu { self.width = Some(hug_width(&self)); self }
    /// Beside an element, which of its edges the drawn menu lines up with (its start by default).
    pub fn align(mut self, align: Align) -> Menu { self.align = align; self }
    /// Opens `gap` from its element rather than 4 (`gutter={6}`).
    pub fn gap(mut self, gap: f32) -> Menu { self.gap = Some(gap); self }
    /// Drawn in menu-v2's standard look, at least `min_w` wide, as Desktop's app menu is.
    pub fn standard(mut self, min_w: f32) -> Menu {
        self.look = Look::Standard;
        self.min_w = min_w;
        self
    }
    /// Presses on the menu leave the focused field focused, so its Edit items reach it, as
    /// Desktop's app menu puts focus back before it runs them (`windows-menu.tsx`).
    pub fn keeps_focus(mut self) -> Menu { self.keeps_focus = true; self }

    fn native(&self) -> PopupMenu {
        let menu = self.entries.iter().fold(PopupMenu::new(), |m, e| match e {
            Entry::Item(i) => m.item(PopupItem::new(i.id.clone(), i.title.clone()).enabled(i.enabled).checked(i.checked)),
            Entry::Separator => m.separator(),
            Entry::Group(title) => m.item(PopupItem::new(format!("group-{title}"), title.clone()).enabled(false)),
            Entry::Sub(sub) => m.submenu(sub.title.clone(), sub.menu.native()),
        });
        match &self.selected {
            Some(id) => menu.selected(id.clone()),
            None => menu,
        }
    }

    fn item_at(&self, n: usize) -> Option<&Item> {
        match self.entries.get(n)? {
            Entry::Item(i) => Some(i),
            _ => None,
        }
    }

    fn sub_at(&self, n: usize) -> Option<&Menu> {
        match self.entries.get(n)? {
            Entry::Sub(sub) => Some(&sub.menu),
            _ => None,
        }
    }

    /// Row `n`'s title, for a letter that lights it: an item's or a submenu's.
    fn title_at(&self, n: usize) -> Option<&str> {
        match self.entries.get(n)? {
            Entry::Item(i) => Some(&i.title),
            Entry::Sub(sub) => Some(&sub.title),
            _ => None,
        }
    }

    /// The enabled rows' indices, in order: items and submenus.
    fn offered(&self) -> Vec<usize> {
        (0..self.entries.len()).filter(|&n| self.item_at(n).is_some_and(|i| i.enabled) || self.sub_at(n).is_some()).collect()
    }
}

type Pick = Box<dyn FnOnce(&mut App, Option<&str>, &mut Tx)>;

/// The drawn menu showing now.
struct Shown {
    menu: Menu,
    anchor: PopupAnchor,
    /// The lit row, by pointer or keys.
    active: Option<usize>,
    /// The submenus open beside their rows, outermost first: one opened from the root's row,
    /// then one from a row of that, and so on.
    subs: Vec<SubOpen>,
    pick: Pick,
}

impl Shown {
    /// The menu at `level`: the root (0), or the submenu open that deep.
    fn menu_at(&self, level: usize) -> Option<&Menu> {
        self.subs.get(..level)?.iter().try_fold(&self.menu, |m, o| m.sub_at(o.at))
    }

    /// The lit row of the menu at `level`.
    fn active_at(&self, level: usize) -> Option<usize> {
        if level == 0 { self.active } else { self.subs.get(level - 1)?.active }
    }

    fn set_active(&mut self, level: usize, n: usize) {
        if level == 0 {
            self.active = Some(n);
        } else if let Some(o) = self.subs.get_mut(level - 1) {
            o.active = Some(n);
        }
    }

    /// The menu the keys move in: the deepest open submenu with a lit row, else the root.
    fn key_level(&self) -> usize {
        self.subs.iter().take_while(|o| o.active.is_some()).count()
    }
}

/// A submenu open beside row `at`, row `active` of it lit. The keys move in it once a row of
/// it is lit, as the pointer leaves them in the menu that opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SubOpen {
    at: usize,
    active: Option<usize>,
}

/// The drawn menu, while one is open. Its callback runs once: with the picked id, or `None`
/// when it's dismissed.
#[derive(Default)]
pub struct Popup {
    shown: Option<Shown>,
    /// A searchable menu's field, and the query its rows were last lit for.
    query: TextEdit,
    lit_for: String,
}

impl Popup {
    pub fn is_open(&self) -> bool { self.shown.is_some() }

    /// Whether the menu showing now is anchored at element `key`.
    pub fn open_at(&self, key: &str) -> bool {
        self.shown.as_ref().is_some_and(|m| matches!(&m.anchor, PopupAnchor::Key(k) if k == key))
    }
}

/// Opens `menu` at `anchor`, an element's key or a point in window points. `pick` runs once it
/// closes, with the picked item's id or `None`.
pub fn open(s: &mut App, menu: Menu, anchor: impl Into<PopupAnchor>, tx: &mut Tx, pick: impl FnOnce(&mut App, Option<&str>, &mut Tx) + 'static) {
    if NATIVE {
        tx.popup_menu(menu.native(), anchor, pick);
        return;
    }
    open_drawn(s, menu, anchor, tx, pick);
}

/// [`open`], drawn on every platform.
pub fn open_drawn(s: &mut App, menu: Menu, anchor: impl Into<PopupAnchor>, tx: &mut Tx, pick: impl FnOnce(&mut App, Option<&str>, &mut Tx) + 'static) {
    close(s, None, tx);
    let active = menu.selected.as_deref().and_then(|id| menu.entries.iter().position(|e| matches!(e, Entry::Item(i) if i.id == id)));
    if let Some(id) = menu.selected.clone() {
        tx.scroll_into_view(row_key(&id));
    }
    // The search field takes the keys while it's open, as Desktop's does once it opens.
    s.popup.query = TextEdit::new("");
    s.popup.query.focused = menu.search.is_some();
    s.popup.lit_for.clear();
    if menu.search.is_some() {
        s.composer.focused = false;
    }
    s.popup.shown = Some(Shown { menu, anchor: anchor.into(), active, subs: Vec::new(), pick: Box::new(pick) });
    refresh(s);
    popover::motion::opening(&s.knobs, tx);
}

/// Closes the drawn menu as a press outside it would. Nothing when none is open.
pub fn dismiss(s: &mut App, tx: &mut Tx) {
    close(s, None, tx);
}

/// A searchable menu's rows for what's typed now; when the query changed since its rows were
/// lit, the first one it lists lights.
fn refresh(s: &mut App) {
    let Some(search) = s.popup.shown.as_ref().and_then(|m| m.menu.search.clone()) else { return };
    let query = s.popup.query.text.clone();
    let menu = Menu { search: Some(search.clone()), ..(search.rows)(s, &query) };
    let Some(shown) = s.popup.shown.as_mut() else { return };
    let changed = s.popup.lit_for != query || shown.menu.entries != menu.entries;
    shown.menu = menu;
    if changed {
        shown.active = shown.menu.offered().first().copied();
        s.popup.lit_for = query;
    }
}

/// Closes the drawn menu, answering `picked`. Nothing when none is open.
fn close(s: &mut App, picked: Option<&str>, tx: &mut Tx) {
    let Some(shown) = s.popup.shown.take() else { return };
    s.popup.query.focused = false;
    popover::motion::closing(&s.knobs, tx);
    (shown.pick)(s, picked, tx);
}

/// Picks row `n`, if it's offered.
fn choose(s: &mut App, n: usize, tx: &mut Tx) {
    refresh(s);
    let Some(id) = s.popup.shown.as_ref().and_then(|m| m.menu.item_at(n)).filter(|i| i.enabled).map(|i| i.id.clone()) else { return };
    close(s, Some(&id), tx);
}

/// Picks row `m` of the first open submenu, if it's offered.
fn choose_sub(s: &mut App, m: usize, tx: &mut Tx) {
    choose_at(s, 1, m, tx);
}

/// Picks row `m` of the menu `level` deep, if it's offered.
fn choose_at(s: &mut App, level: usize, m: usize, tx: &mut Tx) {
    if level == 0 {
        return choose(s, m, tx);
    }
    let item = s.popup.shown.as_ref().and_then(|shown| shown.menu_at(level)?.item_at(m));
    let Some(id) = item.filter(|i| i.enabled).map(|i| i.id.clone()) else { return };
    close(s, Some(&id), tx);
}

/// Opens the root's row `n`'s submenu, lighting its first row when the keys opened it.
fn open_sub(s: &mut App, n: usize, keys: bool, tx: &mut Tx) {
    open_sub_at(s, 0, n, keys, tx);
}

/// Opens row `n`'s submenu of the menu `level` deep, closing any open from its other rows.
fn open_sub_at(s: &mut App, level: usize, n: usize, keys: bool, tx: &mut Tx) {
    let Some(shown) = s.popup.shown.as_mut() else { return };
    let Some(sub) = shown.menu_at(level).and_then(|m| m.sub_at(n)) else { return };
    let active = keys.then(|| sub.offered().first().copied()).flatten();
    shown.set_active(level, n);
    if shown.subs.get(level).is_none_or(|o| o.at != n) || keys {
        shown.subs.truncate(level);
        shown.subs.push(SubOpen { at: n, active });
        popover::motion::opening(&s.knobs, tx);
    }
}

/// The drawn menu's keys:   Home End light a row, a letter lights the next one it starts,
/// → or Enter opens a submenu and ← closes it, Enter or Space picks, Esc and Tab dismiss. It
/// takes every key while it's open.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.popup.shown.as_ref().is_some_and(|m| m.menu.search.is_some()) {
        refresh(s);
        // Typing and moving in the search field are the field's; the rest move in the menu.
        let field = !matches!(ev.key, Key::Escape | Key::Tab | Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Enter);
        if field {
            return false;
        }
    }
    let Some(shown) = s.popup.shown.as_mut() else { return false };
    // The keys move in the deepest open menu with a lit row.
    let level = shown.key_level();
    let lit = shown.active_at(level);
    let opens = |shown: &Shown, n: usize| shown.menu_at(level).is_some_and(|m| m.sub_at(n).is_some());
    match &ev.key {
        Key::Escape | Key::Tab => {
            close(s, None, tx);
            return true;
        }
        Key::Left if level > 0 => {
            shown.subs.truncate(level - 1);
            tx.instant();
            return true;
        }
        Key::Right => {
            if let Some(n) = lit.filter(|&n| opens(shown, n)) {
                open_sub_at(s, level, n, true, tx);
                tx.instant();
            }
            return true;
        }
        Key::Enter | Key::Text(_) if is_space_or_enter(&ev.key) => {
            match lit {
                Some(n) if opens(shown, n) => {
                    open_sub_at(s, level, n, true, tx);
                    tx.instant();
                }
                Some(n) => choose_at(s, level, n, tx),
                None => {}
            }
            return true;
        }
        _ => {}
    }
    let Some(menu) = shown.menu_at(level) else { return true };
    let Some(next) = step(menu, lit, &ev.key) else { return true };
    let id = menu.item_at(next).map(|i| i.id.clone());
    shown.set_active(level, next);
    // The keys leave a submenu the pointer opened as they move off its row.
    if shown.subs.get(level).is_some_and(|o| o.at != next) {
        shown.subs.truncate(level);
    }
    if let Some(id) = id {
        tx.scroll_into_view(row_key(&id));
    }
    tx.instant();
    true
}

/// The row `key` lights in `menu`, from row `active`: arrows step and wrap, Home and End go to
/// the ends, a letter goes to the next row it starts. None for any other key.
fn step(menu: &Menu, active: Option<usize>, key: &Key) -> Option<usize> {
    let offered = menu.offered();
    let at = active.and_then(|a| offered.iter().position(|&n| n == a));
    let last = offered.len().saturating_sub(1);
    let i = match key {
        Key::Down => at.map_or(0, |i| if i >= last { 0 } else { i + 1 }),
        Key::Up => at.map_or(last, |i| if i == 0 { last } else { i - 1 }),
        Key::Home | Key::PageUp => 0,
        Key::End | Key::PageDown => last,
        Key::Text(t) => {
            let c = t.chars().next()?.to_lowercase().to_string();
            let starts = |&&n: &&usize| menu.title_at(n).is_some_and(|title| title.to_lowercase().starts_with(&c));
            let after = at.map_or(0, |i| i + 1);
            let n = offered.iter().skip(after).find(starts).or_else(|| offered.iter().find(starts))?;
            offered.iter().position(|o| o == n)?
        }
        _ => return None,
    };
    offered.get(i).copied()
}

fn is_space_or_enter(key: &Key) -> bool {
    matches!(key, Key::Enter) || matches!(key, Key::Text(t) if t == " ")
}

fn row_key(id: &str) -> String { format!("popup-item-{id}") }

/// The drawn menu and the scrim under it that dismisses it, for the window's top layer.
pub fn view(s: &App, p: &Pal) -> Vec<El> {
    let Some(shown) = &s.popup.shown else { return vec![] };
    let scrim = popover::scrim::<App>("popup-scrim", |s, tx| close(s, None, tx))
        .on_secondary_press(|s: &mut App, _: rune::PointerPress, tx: &mut Tx| close(s, None, tx));
    let (anchor, point, align, side) = match &shown.anchor {
        PopupAnchor::Key(key) => (key.clone(), None, shown.menu.align, shown.menu.side),
        &PopupAnchor::Point(x, y) => ("popup-point".to_owned(), Some(el().key("popup-point").absolute(x, y).size(0.0, 0.0)), Align::Start, Side::Bottom),
    };
    let at_point = point.is_some();
    let gap = if at_point { GAP } else { shown.menu.gap.unwrap_or(4.0) };
    let mut layer = vec![scrim];
    layer.extend(point);
    if shown.menu.look == Look::Standard {
        let first = shown.subs.first().copied();
        layer.push(popover::place(&s.knobs, standard::root(p, &shown.menu, shown.active, first), &anchor, side, align, gap));
        let sub = first.and_then(|o| Some((o, shown.menu.entries.get(o.at)?)));
        if let Some((o, Entry::Sub(sub))) = sub {
            layer.push(popover::place(&s.knobs, standard::sub(p, &sub.menu, o.active, shown.menu.keeps_focus), &row_key(&sub.id), Side::Right, Align::Start, 0.0));
        }
        return layer;
    }
    // A searchable menu lists what matches the field as it is now.
    let fresh = shown.menu.search.as_ref().map(|search| {
        let query = &s.popup.query.text;
        let menu = Menu { search: Some(search.clone()), ..(search.rows)(s, query) };
        let active = if s.popup.lit_for == *query { shown.active } else { menu.offered().first().copied() };
        (menu, active)
    });
    let (listed, active) = match &fresh {
        Some((menu, active)) => (menu, *active),
        None => (&shown.menu, shown.active),
    };
    // A right-click menu is at least Desktop's context-menu width; one from a control hugs its rows.
    let menu = drawn(s, p, listed, active, shown.subs.first().map(|o| o.at));
    let menu = if at_point { menu.min_w(CONTEXT_MIN_W.max(shown.menu.min_w)) } else { menu };
    layer.push(popover::place(&s.knobs, menu, &anchor, side, align, gap));
    // Each open submenu beside its row, 4 off it (`Menu.Sub gutter={4}`) unless it says.
    let mut parent = listed;
    for (k, o) in shown.subs.iter().enumerate() {
        let Some(Entry::Sub(sub)) = parent.entries.get(o.at) else { break };
        let open = shown.subs.get(k + 1).map(|o| o.at);
        let gap = sub.menu.gap.unwrap_or(4.0);
        layer.push(popover::place(&s.knobs, build(Some(s), p, &sub.menu, o.active, open, k + 1), &row_key(&sub.id), Side::Right, Align::Start, gap));
        parent = &sub.menu;
    }
    layer
}

/// `menu` as a drawn surface, row `active` lit. Its rows light and pick the open menu's, so a
/// copy drawn while none is open (the storybook's) stays still.
pub fn surface(p: &Pal, menu: &Menu, active: Option<usize>) -> El {
    build(None, p, menu, active, None, 0)
}

/// [`surface`] with the app's project avatars, and the row whose submenu is open.
fn drawn(s: &App, p: &Pal, menu: &Menu, active: Option<usize>, open: Option<usize>) -> El {
    build(Some(s), p, menu, active, open, 0)
}

/// A menu's surface: the root's (`level` 0), or the submenu open `level` deep, whose rows light,
/// pick and open their own submenus in it.
fn build(s: Option<&App>, p: &Pal, menu: &Menu, active: Option<usize>, open: Option<usize>, level: usize) -> El {
    let nested = level > 0;
    // The first submenu keeps its keys; deeper ones number theirs.
    let name = |root: &str, sub: &str| match level {
        0 => root.to_owned(),
        1 => sub.to_owned(),
        _ => format!("{sub}-{level}"),
    };
    let mut list = popover::List::new(&name("popup-rows", "popup-sub-rows"), MENU);
    if let (Some(s), Some(search)) = (s, &menu.search) {
        list.other(search_field(s, p, &search.placeholder), SEARCH_H);
    }
    let role = if menu.selected.is_some() { "menuitemradio" } else { "menuitem" };
    for (n, entry) in menu.entries.iter().enumerate() {
        match entry {
            Entry::Item(i) if i.detail => list.other(detail_row(p, &row_key(&i.id), i), MENU.row_h),
            Entry::Item(i) => {
                let o = Opt::new(row_key(&i.id), i.title.clone())
                    .role(role)
                    .active(active == Some(n))
                    .checked(i.checked)
                    .danger(i.danger)
                    .disabled(!i.enabled);
                let ink = if !i.enabled { p.faint } else if i.danger { p.danger } else { p.text };
                let o = match i.glyph {
                    Some(Glyph::Ink(g)) => o.lead(icon(g, 14.0).color(ink)),
                    Some(Glyph::Weak(g)) => o.lead(icon(g, 14.0).color(session_view::palette::token(p.dark, "icon-weak-base").unwrap_or(p.icon_faint))),
                    Some(Glyph::App(id)) => match rune_theme_opencode::app_icons::named(id, p.dark) {
                        Some(g) => o.lead(icon(g, 16.0)),
                        None => o.lead(el().size(16.0, 16.0)),
                    },
                    None => o,
                };
                let o = match &i.shortcut {
                    Some(keys) => o.trailing(popover::hint(p, keys)),
                    None => o,
                };
                let o = match i.diff {
                    Some((a, d)) => o.after(diff_note(p, a, d)),
                    None => o,
                };
                let o = match (s, &i.project) {
                    (Some(s), Some(dir)) => {
                        let project = s.project_at(dir);
                        o.lead(crate::icons::badge(s, p, project, &crate::view::project_or_dir(project, dir)))
                    }
                    _ => o,
                };
                let row = if nested {
                    popover::option::<App>(p, MENU, o, move |s| hover_at(s, level, n), move |s, tx| choose_at(s, level, n, tx))
                } else {
                    popover::option::<App>(p, MENU, o, move |s| hover(s, n), move |s, tx| choose(s, n, tx))
                };
                list.row(row);
            }
            Entry::Separator => {
                // A 1 pt rule across the whole surface, 2 above and below (`menu.css:163-169`):
                // stretched past the inset on both sides, as `width: calc(100% + 4px)` is.
                let line = col().key(format!("popup-sep-{n}")).py(2.0).mx(-MENU.inset)
                    .child(el().key("line").h(1.0).w_full().fill(p.border_muted));
                list.other(line, 5.0);
            }
            Entry::Group(title) => {
                let label = row().key(format!("popup-group-{n}")).w_full().h(GROUP_H).pl(MENU.pad_l).pr(MENU.pad_r).center_y()
                    .child(text(title.clone()).font(GROUP).color(p.faint).shrink().truncate_fade());
                list.other(label, GROUP_H);
            }
            // A row that opens its menu beside it (`Menu.SubTrigger`), lit while that menu shows,
            // its chevron in the muted ink, at any depth.
            Entry::Sub(sub) => {
                let o = Opt::new(row_key(&sub.id), sub.title.clone())
                    .role("menuitem")
                    .active(active == Some(n) || open == Some(n))
                    .trailing(icon(&CHEVRON, 16.0).key("chevron").color(p.icon_muted));
                let o = match sub.glyph.and_then(Glyph::mono) {
                    Some(g) => o.lead(icon(g, 16.0).color(p.icon_muted)),
                    None => o,
                };
                list.row(popover::option::<App>(p, MENU, o, move |s| hover_at(s, level, n), move |s, tx| open_sub_at(s, level, n, false, tx)));
            }
        }
    }
    let surface = popover::surface::<App>(p, MENU, &name("popup-menu", "popup-submenu"), menu.label.clone()).min_w(menu.min_w);
    // A submenu is as wide as its widest row (`w-max`), its rows laid out apart from it.
    let width = menu.width.or_else(|| nested.then(|| hug_width(menu).max(menu.min_w)));
    let surface = match width {
        Some(w) => surface.w(w).max_w(w),
        None => surface,
    };
    surface.child(list.build(&name("popup-scroll", "popup-sub-scroll"), menu.max_h))
}

/// The widest row of `menu`, its glyph, label and shortcut with the row's padding and the
/// surface's inset.
fn hug_width(menu: &Menu) -> f32 {
    let label = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    let w = |s: &str, st| rune::text::measure(s, &[], st, None).0;
    let rows = menu.entries.iter().map(|e| match e {
        Entry::Item(i) => {
            i.glyph.map_or(0.0, |_| 16.0 + MENU.gap) + w(&i.title, label) + i.shortcut.as_deref().map_or(0.0, |k| MENU.gap + w(k, popover::HINT))
        }
        Entry::Group(t) => w(t, GROUP),
        Entry::Sub(sub) => sub.glyph.map_or(0.0, |_| 16.0 + MENU.gap) + w(&sub.title, label) + MENU.gap + 16.0,
        Entry::Separator => 0.0,
    });
    rows.fold(0.0, f32::max) + MENU.pad_l + MENU.pad_r + 2.0 * MENU.inset + 2.0
}

/// `menu-v2-item-chevron`: a submenu row's end.
static CHEVRON: Icon = Icon { name: "menu-v2-chevron", view: 16.0, body: r#"<path d="M6 4L10 8L6 12V4Z" fill="currentColor"/>"# };

/// `· +added -removed`: the separator muted, the counts in the diff's inks.
fn diff_note(p: &Pal, added: u32, removed: u32) -> El {
    let st = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    row().gap(8.0).center_y()
        .child(text("·").font(st).color(p.muted))
        .child(row().gap(4.0).child(text(format!("+{added}")).font(st).color(p.diff_add)).child(text(format!("-{removed}")).font(st).color(p.diff_del)))
}

/// A row that only tells (`SessionLocationDetail`): its 16 pt glyph in the muted ink and its
/// line, 13/440 in the base ink, 28 tall.
fn detail_row(p: &Pal, key: &str, i: &Item) -> El {
    row()
        .key(key.to_owned())
        .w_full()
        .h(MENU.row_h)
        .pl(MENU.pad_l)
        .pr(MENU.pad_r)
        .gap(MENU.gap)
        .center_y()
        .children(i.glyph.and_then(Glyph::mono).map(|g| icon(g, 16.0).color(p.icon_muted)))
        .child(text(i.title.clone()).font(TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04)).color(p.text).truncate_fade().shrink())
}

/// The pointer over the root's row `n`: it lights, and opens its submenu or closes the open one.
fn hover(s: &mut App, n: usize) {
    refresh(s);
    hover_at(s, 0, n);
}

/// The pointer over row `m` of the first open submenu.
fn hover_sub(s: &mut App, m: usize) {
    hover_at(s, 1, m);
}

/// The pointer over row `n` of the menu `level` deep: it lights, and opens its submenu or
/// closes the ones open from its other rows.
fn hover_at(s: &mut App, level: usize, n: usize) {
    let Some(shown) = &mut s.popup.shown else { return };
    if level > shown.subs.len() {
        return;
    }
    shown.set_active(level, n);
    let opens = shown.menu_at(level).is_some_and(|m| m.sub_at(n).is_some());
    match shown.subs.get(level) {
        Some(o) if o.at == n => {}
        _ => {
            shown.subs.truncate(level);
            if opens {
                shown.subs.push(SubOpen { at: n, active: None });
            }
        }
    }
}
