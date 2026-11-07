//! The app's menus and pickers: a session's, a tab's and a project's right-click menu, the
//! session's … menu, Open in, the side panel's tab menus, Settings' selects and the Windows app
//! menu. macOS shows them as native menus (`Tx::popup_menu`), as Kit chose there. Everywhere
//! else they're drawn in the window, as Desktop draws its own (`menu-v2`), with the one popover
//! every menu is built from (`popover::MENU`), or menu-v2's standard look ([`Menu::standard`]):
//! they take the theme, the keyboard, the window's zoom and headless captures. A menu drawn on
//! every platform (the side panel's) opens with [`open_drawn`].

use rune::{Align, El, Key, KeyEvent, PopupAnchor, PopupItem, PopupMenu, Side, TextStyle, Tx, col, el, row, text};

use crate::App;
use crate::palette::Pal;
use crate::popover::{self, MENU, Opt};

mod standard;

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
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Item {
        Item { id: id.into(), title: title.into(), enabled: true, checked: false, danger: false, project: None, shortcut: None }
    }
    pub fn enabled(mut self, on: bool) -> Item { self.enabled = on; self }
    /// A check at the row's end, as a picker marks its current value.
    pub fn checked(mut self, on: bool) -> Item { self.checked = on; self }
    /// A destructive action, in Desktop's critical ink (drawn menus only).
    pub fn danger(mut self, on: bool) -> Item { self.danger = on; self }
    /// The 16 pt project avatar for folder `dir` before the title, as a session row has it.
    pub fn project(mut self, dir: impl Into<String>) -> Item { self.project = Some(dir.into()); self }
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
    /// Presses on it leave the focused field focused (`El::keeps_text_selection`).
    keeps_focus: bool,
}

impl Menu {
    /// `label` names the menu for assistive technology ("Session", "Theme").
    pub fn new(label: impl Into<String>) -> Menu {
        Menu { label: label.into(), entries: Vec::new(), selected: None, align: Align::Start, min_w: MIN_W, width: None, max_h: MAX_H, look: Look::Menu, keeps_focus: false }
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
        self.entries.push(Entry::Sub(Box::new(Sub { id: id.into(), title: title.into(), menu })));
        self
    }
    /// Beside an element, which of its edges the drawn menu lines up with (its start by default).
    pub fn align(mut self, align: Align) -> Menu { self.align = align; self }
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
    /// The submenu open beside its row.
    sub: Option<SubOpen>,
    pick: Pick,
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
    s.popup.shown = Some(Shown { menu, anchor: anchor.into(), active, sub: None, pick: Box::new(pick) });
    popover::motion::opening(&s.knobs, tx);
}

/// Closes the drawn menu, answering `picked`. Nothing when none is open.
fn close(s: &mut App, picked: Option<&str>, tx: &mut Tx) {
    let Some(shown) = s.popup.shown.take() else { return };
    popover::motion::closing(&s.knobs, tx);
    (shown.pick)(s, picked, tx);
}

/// Picks row `n`, if it's offered.
fn choose(s: &mut App, n: usize, tx: &mut Tx) {
    let Some(id) = s.popup.shown.as_ref().and_then(|m| m.menu.item_at(n)).filter(|i| i.enabled).map(|i| i.id.clone()) else { return };
    close(s, Some(&id), tx);
}

/// Picks row `m` of the open submenu, if it's offered.
fn choose_sub(s: &mut App, m: usize, tx: &mut Tx) {
    let item = s.popup.shown.as_ref().and_then(|shown| shown.menu.sub_at(shown.sub?.at)?.item_at(m));
    let Some(id) = item.filter(|i| i.enabled).map(|i| i.id.clone()) else { return };
    close(s, Some(&id), tx);
}

/// Opens row `n`'s submenu, lighting its first row when the keys opened it.
fn open_sub(s: &mut App, n: usize, keys: bool, tx: &mut Tx) {
    let Some(shown) = s.popup.shown.as_mut() else { return };
    let Some(sub) = shown.menu.sub_at(n) else { return };
    let active = keys.then(|| sub.offered().first().copied()).flatten();
    shown.active = Some(n);
    if shown.sub.is_none_or(|o| o.at != n) || keys {
        shown.sub = Some(SubOpen { at: n, active });
        popover::motion::opening(&s.knobs, tx);
    }
}

/// The drawn menu's keys:   Home End light a row, a letter lights the next one it starts,
/// → or Enter opens a submenu and ← closes it, Enter or Space picks, Esc and Tab dismiss. It
/// takes every key while it's open.
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(shown) = s.popup.shown.as_mut() else { return false };
    let in_sub = shown.sub.is_some_and(|o| o.active.is_some());
    match &ev.key {
        Key::Escape | Key::Tab => {
            close(s, None, tx);
            return true;
        }
        Key::Left if in_sub => {
            shown.sub = None;
            tx.instant();
            return true;
        }
        Key::Right if !in_sub => {
            if let Some(n) = shown.active.filter(|&n| shown.menu.sub_at(n).is_some()) {
                open_sub(s, n, true, tx);
                tx.instant();
            }
            return true;
        }
        Key::Enter | Key::Text(_) if is_space_or_enter(&ev.key) => {
            match (in_sub, shown.sub, shown.active) {
                (true, Some(SubOpen { active: Some(m), .. }), _) => choose_sub(s, m, tx),
                (false, _, Some(n)) if shown.menu.sub_at(n).is_some() => {
                    open_sub(s, n, true, tx);
                    tx.instant();
                }
                (false, _, Some(n)) => choose(s, n, tx),
                _ => {}
            }
            return true;
        }
        _ => {}
    }
    let (menu, active) = match shown.sub.filter(|_| in_sub) {
        Some(o) => (shown.menu.sub_at(o.at).unwrap_or(&shown.menu), o.active),
        None => (&shown.menu, shown.active),
    };
    let Some(next) = step(menu, active, &ev.key) else { return true };
    let id = menu.item_at(next).map(|i| i.id.clone());
    match shown.sub.as_mut().filter(|_| in_sub) {
        Some(o) => o.active = Some(next),
        None => {
            shown.active = Some(next);
            // The keys leave a submenu the pointer opened as they move off its row.
            if shown.sub.is_some_and(|o| o.at != next) { shown.sub = None; }
        }
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
        PopupAnchor::Key(key) => (key.clone(), None, shown.menu.align, Side::Bottom),
        &PopupAnchor::Point(x, y) => ("popup-point".to_owned(), Some(el().key("popup-point").absolute(x, y).size(0.0, 0.0)), Align::Start, Side::Bottom),
    };
    let at_point = point.is_some();
    let gap = if at_point { GAP } else { 4.0 };
    let mut layer = vec![scrim];
    layer.extend(point);
    if shown.menu.look == Look::Standard {
        layer.push(popover::place(&s.knobs, standard::root(p, &shown.menu, shown.active, shown.sub), &anchor, side, align, gap));
        let sub = shown.sub.and_then(|o| Some((o, shown.menu.entries.get(o.at)?)));
        if let Some((o, Entry::Sub(sub))) = sub {
            layer.push(popover::place(&s.knobs, standard::sub(p, &sub.menu, o.active, shown.menu.keeps_focus), &row_key(&sub.id), Side::Right, Align::Start, 0.0));
        }
        return layer;
    }
    // A right-click menu is at least Desktop's context-menu width; one from a control hugs its rows.
    let menu = drawn(s, p, &shown.menu, shown.active);
    let menu = if at_point { menu.min_w(CONTEXT_MIN_W.max(shown.menu.min_w)) } else { menu };
    layer.push(popover::place(&s.knobs, menu, &anchor, side, align, gap));
    layer
}

/// `menu` as a drawn surface, row `active` lit. Its rows light and pick the open menu's, so a
/// copy drawn while none is open (the storybook's) stays still.
pub fn surface(p: &Pal, menu: &Menu, active: Option<usize>) -> El {
    build(None, p, menu, active)
}

/// [`surface`] with the app's project avatars.
fn drawn(s: &App, p: &Pal, menu: &Menu, active: Option<usize>) -> El {
    build(Some(s), p, menu, active)
}

fn build(s: Option<&App>, p: &Pal, menu: &Menu, active: Option<usize>) -> El {
    let mut list = popover::List::new("popup-rows", MENU);
    let role = if menu.selected.is_some() { "menuitemradio" } else { "menuitem" };
    for (n, entry) in menu.entries.iter().enumerate() {
        match entry {
            Entry::Item(i) => {
                let o = Opt::new(row_key(&i.id), i.title.clone())
                    .role(role)
                    .active(active == Some(n))
                    .checked(i.checked)
                    .danger(i.danger)
                    .disabled(!i.enabled);
                let o = match &i.shortcut {
                    Some(keys) => o.trailing(popover::hint(p, keys)),
                    None => o,
                };
                let o = match (s, &i.project) {
                    (Some(s), Some(dir)) => {
                        let project = s.project_at(dir);
                        o.lead(crate::icons::badge(s, p, project, &crate::view::project_or_dir(project, dir)))
                    }
                    _ => o,
                };
                list.row(popover::option::<App>(p, MENU, o, move |s| hover(s, n), move |s, tx| choose(s, n, tx)));
            }
            Entry::Separator => {
                let line = col().key(format!("popup-sep-{n}")).w_full().py(2.0).child(popover::divider(p, "line"));
                list.other(line, 4.5);
            }
            Entry::Group(title) => {
                let label = row().key(format!("popup-group-{n}")).w_full().h(GROUP_H).pl(MENU.pad_l).pr(MENU.pad_r).center_y()
                    .child(text(title.clone()).font(GROUP).color(p.faint).shrink().truncate_fade());
                list.other(label, GROUP_H);
            }
            // Only the standard look opens submenus beside their rows.
            Entry::Sub(_) => {}
        }
    }
    let surface = popover::surface::<App>(p, MENU, "popup-menu", menu.label.clone()).min_w(menu.min_w);
    let surface = match menu.width {
        Some(w) => surface.w(w).max_w(w),
        None => surface,
    };
    surface.child(list.build("popup-scroll", menu.max_h))
}

/// The pointer over row `n`: it lights, and opens its submenu or closes the open one.
fn hover(s: &mut App, n: usize) {
    let Some(shown) = &mut s.popup.shown else { return };
    shown.active = Some(n);
    let opens = shown.menu.sub_at(n).is_some();
    match shown.sub {
        Some(o) if o.at == n => {}
        _ if opens => shown.sub = Some(SubOpen { at: n, active: None }),
        _ => shown.sub = None,
    }
}

/// The pointer over row `m` of the open submenu.
fn hover_sub(s: &mut App, m: usize) {
    if let Some(o) = s.popup.shown.as_mut().and_then(|m| m.sub.as_mut()) {
        o.active = Some(m);
    }
}
