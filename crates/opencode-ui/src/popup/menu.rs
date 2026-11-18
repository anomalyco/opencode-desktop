//! Pure, host-independent menu data types ([`Menu`], [`Item`]) and native [`PopupMenu`]
//! conversion.

use opencode_kit::servers::health::Indicator;
use rune::{Align, Icon, PopupItem, PopupMenu, Side, TextStyle};
use crate::popover::{self, MENU};

use super::{GROUP, MAX_H, MIN_W};

/// One choice. Enabled, unchecked and in the text colour unless set.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) enabled: bool,
    pub(super) checked: bool,
    pub(super) danger: bool,
    /// The folder whose project avatar leads the row (drawn menus only).
    pub(super) project: Option<String>,
    pub(super) shortcut: Option<String>,
    /// The keys that do the same as keycap chips at the row's end, instead of `shortcut`'s text
    /// (drawn menus only; native ones show `shortcut`).
    pub(super) keycaps: Vec<String>,
    /// The glyph before the title (drawn menus only).
    pub(super) glyph: Option<Glyph>,
    /// A row that only tells (`SessionLocationDetail`): never lit or picked, in the base ink.
    pub(super) detail: bool,
    /// Lines added and removed after the title, in the diff's inks (drawn menus only).
    pub(super) diff: Option<(u32, u32)>,
    /// A muted "· hint" after the title (`ServerLabel`'s "This computer"; drawn menus only).
    pub(super) hint: Option<String>,
    /// A faint word at the row's end (the server switcher's "Unreachable"; drawn menus only).
    pub(super) note: Option<String>,
}

/// A row's icon, the same when it's the same icon.
#[derive(Clone, Copy, Debug)]
pub(super) enum Glyph {
    /// 14 pt, in the title's ink.
    Ink(&'static Icon),
    /// 14 pt, in `icon-weak` (`text-icon-weak`).
    Weak(&'static Icon),
    /// An app's 16 pt mark in its own colours (`AppIcon`), by its id, in the appearance's variant.
    App(&'static str),
    /// 16 pt in a 16 pt slot, in `icon-muted` (the server switcher's Manage servers).
    Slot(&'static Icon),
    /// A server's health mark (`ServerHealthIndicator`) in a 16 pt slot.
    Status(Indicator),
}

impl Glyph {
    /// A one-colour glyph's icon; None for an app's mark or status indicator.
    pub(super) fn mono(self) -> Option<&'static Icon> {
        match self {
            Glyph::Ink(g) | Glyph::Weak(g) | Glyph::Slot(g) => Some(g),
            Glyph::App(_) | Glyph::Status(_) => None,
        }
    }
}

impl PartialEq for Glyph {
    fn eq(&self, other: &Glyph) -> bool {
        match (self, other) {
            (Glyph::Ink(a), Glyph::Ink(b)) | (Glyph::Weak(a), Glyph::Weak(b)) | (Glyph::Slot(a), Glyph::Slot(b)) => std::ptr::eq(*a, *b),
            (Glyph::App(a), Glyph::App(b)) => a == b,
            (Glyph::Status(a), Glyph::Status(b)) => a == b,
            _ => false,
        }
    }
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Item {
        Item {
            id: id.into(),
            title: title.into(),
            enabled: true,
            checked: false,
            danger: false,
            project: None,
            shortcut: None,
            keycaps: Vec::new(),
            glyph: None,
            detail: false,
            diff: None,
            hint: None,
            note: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// A 16 pt icon in a 16 pt slot, in the muted icon ink (`settings-server-switcher-icon`).
    pub fn slot_icon(mut self, glyph: &'static Icon) -> Item {
        self.glyph = Some(Glyph::Slot(glyph));
        self
    }

    /// A server's health mark in a 16 pt slot before the title.
    pub fn status(mut self, indicator: Indicator) -> Item {
        self.glyph = Some(Glyph::Status(indicator));
        self
    }

    /// A muted "· hint" after the title, as a server's "This computer".
    pub fn hint(mut self, hint: impl Into<String>) -> Item {
        self.hint = Some(hint.into());
        self
    }

    /// A faint word at the row's end, as a server's "Unreachable".
    pub fn note(mut self, note: impl Into<String>) -> Item {
        self.note = Some(note.into());
        self
    }

    pub fn enabled(mut self, on: bool) -> Item {
        self.enabled = on;
        self
    }

    /// A check at the row's end, as a picker marks its current value.
    pub fn checked(mut self, on: bool) -> Item {
        self.checked = on;
        self
    }

    /// A destructive action, in Desktop's critical ink (drawn menus only).
    pub fn danger(mut self, on: bool) -> Item {
        self.danger = on;
        self
    }

    /// The 16 pt project avatar for folder `dir` before the title, as a session row has it.
    pub fn project(mut self, dir: impl Into<String>) -> Item {
        self.project = Some(dir.into());
        self
    }

    /// A 14 pt icon before the title, as menu-v2's items lead with one (`menu.tsx`).
    pub fn icon(mut self, glyph: &'static Icon) -> Item {
        self.glyph = Some(Glyph::Ink(glyph));
        self
    }

    /// A 14 pt icon in Desktop's weak icon ink, as Open in's Copy path has.
    pub fn weak_icon(mut self, glyph: &'static Icon) -> Item {
        self.glyph = Some(Glyph::Weak(glyph));
        self
    }

    /// App `id`'s 16 pt mark (`AppIcon`) before the title, as Open in's apps have.
    pub fn app_icon(mut self, id: &'static str) -> Item {
        self.glyph = Some(Glyph::App(id));
        self
    }

    /// A row that only tells, as the location card's server and its branch-less line do.
    pub fn detail(mut self) -> Item {
        self.detail = true;
        self.enabled = false;
        self
    }

    /// A change count's lines after the title, as Desktop's `DiffChanges` reads them.
    pub fn diff(mut self, added: u32, removed: u32) -> Item {
        self.diff = Some((added, removed));
        self
    }

    /// The keys that do the same as Desktop's neutral `Keybind` chips (`CTRL` `＋`), and as the native
    /// menu's text.
    pub fn keycaps(mut self, caps: Vec<String>) -> Item {
        self.shortcut = (!caps.is_empty()).then(|| caps.join("+"));
        self.keycaps = caps;
        self
    }

    /// The keys that do the same, at the row's end ("Ctrl+Shift+N"); none when empty.
    pub fn shortcut(mut self, keys: impl Into<String>) -> Item {
        let keys = keys.into();
        self.shortcut = (!keys.is_empty()).then_some(keys);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Item(Item),
    Separator,
    /// A group's title (`Menu.GroupLabel`).
    Group(String),
    /// A row that opens another menu beside it.
    Sub(Box<Sub>),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Sub {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) menu: Menu,
    /// The glyph before the title (drawn menus only).
    pub(super) glyph: Option<Glyph>,
}

/// How a drawn menu looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Look {
    /// `menu-v2`'s own: [`crate::popover::MENU`].
    #[default]
    Menu,
    /// `menu-v2`'s `appearance="standard"`, which Desktop's app menu takes.
    Standard,
}

/// A menu's rows, where it opens beside an element, and how narrow it may be.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub(super) label: String,
    pub(super) entries: Vec<Entry>,
    /// A picker's current value: checked, lit as the menu opens, and over the anchor natively.
    pub(super) selected: Option<String>,
    pub(super) align: Align,
    pub(super) min_w: f32,
    /// A fixed width, as a menu with `w-56` has.
    pub(super) width: Option<f32>,
    /// How tall it gets before its rows scroll.
    pub(super) max_h: f32,
    pub(super) look: Look,
    /// Which side of an element it opens on, flipping when it doesn't fit.
    pub(super) side: Side,
    /// Remembers the field focused as it opens, for its Edit items (`windows-menu.tsx`).
    pub(super) remembers_focus: bool,
    /// Its distance from the element it opens beside (Kobalte's `gutter`), when not the default.
    pub(super) gap: Option<f32>,
    /// Its rows' start padding, when not the look's (`.settings-server-switcher-menu`'s 8).
    pub(super) item_inset: Option<f32>,
    /// The location card's rows (`session-location-card`): 2 apart, 8 in on both sides, and the
    /// card 4 to the start of its trigger (`shift={-4}`).
    pub(super) card: bool,
    /// Home's row uses a 1 pt dropdown trigger at the pointer, not ContextMenu's virtual point.
    pub(super) pointer_trigger: bool,
}

impl Menu {
    /// `label` names the menu for assistive technology ("Session", "Theme").
    pub fn new(label: impl Into<String>) -> Menu {
        Menu {
            label: label.into(),
            entries: Vec::new(),
            selected: None,
            align: Align::Start,
            min_w: MIN_W,
            width: None,
            max_h: MAX_H,
            look: Look::Menu,
            side: Side::Bottom,
            remembers_focus: false,
            gap: None,
            item_inset: None,
            card: false,
            pointer_trigger: false,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A picker: one checked-when-current row per `(id, title)`, lined up with the end of the
    /// select that opens it.
    pub fn picker<I: Into<String>, T: Into<String>>(label: impl Into<String>, options: impl IntoIterator<Item = (I, T)>, selected: &str) -> Menu {
        Menu::picker_some(label, options.into_iter().map(|(id, title)| (id, title, true)), selected)
    }

    /// [`Menu::picker`] whose rows can be disabled (`(id, title, enabled)`).
    pub fn picker_some<I: Into<String>, T: Into<String>>(label: impl Into<String>, options: impl IntoIterator<Item = (I, T, bool)>, selected: &str) -> Menu {
        let items = options.into_iter().map(|(id, title, enabled)| {
            let id = id.into();
            let on = id == selected;
            Item::new(id, title).checked(on).enabled(enabled)
        });
        Menu { selected: Some(selected.to_owned()), min_w: 180.0, align: Align::End, ..Menu::new(label) }.items(items)
    }

    pub fn item(mut self, item: Item) -> Menu {
        self.entries.push(Entry::Item(item));
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = Item>) -> Menu {
        self.entries.extend(items.into_iter().map(Entry::Item));
        self
    }

    pub fn separator(mut self) -> Menu {
        self.entries.push(Entry::Separator);
        self
    }

    /// A group's title above the rows that follow.
    pub fn group(mut self, title: impl Into<String>) -> Menu {
        self.entries.push(Entry::Group(title.into()));
        self
    }

    /// Draws the menu this wide, whatever its rows.
    pub fn width(mut self, w: f32) -> Menu {
        self.width = Some(w);
        self
    }

    /// Lets the menu grow to `rows` rows before they scroll.
    pub fn rows_before_scroll(mut self, rows: f32) -> Menu {
        self.max_h = 2.0 * MENU.inset + rows * MENU.row_h;
        self
    }

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
    pub fn above(mut self) -> Menu {
        self.side = Side::Top;
        self
    }

    /// At least `w` wide, as menu-v2's content is 160 pt (`menu.css:12`).
    pub fn min_width(mut self, w: f32) -> Menu {
        self.min_w = w;
        self
    }

    /// As wide as its widest row (`menu-v2`'s `w-max`), its entries added, and at least its
    /// [`Menu::min_width`].
    pub fn hug(mut self) -> Menu {
        self.width = Some(hug_width(&self).max(self.min_w));
        self
    }

    /// Beside an element, which of its edges the drawn menu lines up with (its start by default).
    pub fn align(mut self, align: Align) -> Menu {
        self.align = align;
        self
    }

    /// Opens `gap` from its element rather than 4 (`gutter={6}`).
    pub fn gap(mut self, gap: f32) -> Menu {
        self.gap = Some(gap);
        self
    }

    /// Its rows start `pad` in rather than the look's 12.
    pub fn item_inset(mut self, pad: f32) -> Menu {
        self.item_inset = Some(pad);
        self
    }

    /// The location card's look (`session-location-card`).
    pub fn card(mut self) -> Menu {
        self.card = true;
        self
    }

    pub fn pointer_trigger(mut self) -> Menu {
        self.pointer_trigger = true;
        self
    }

    /// Drawn in menu-v2's standard look, at least `min_w` wide, as Desktop's app menu is.
    pub fn standard(mut self, min_w: f32) -> Menu {
        self.look = Look::Standard;
        self.min_w = min_w;
        self
    }

    /// Remembers the field focused as it opens ([`super::Popup::remembered`]), so its Edit items
    /// reach it, as Desktop's app menu puts focus back before it runs them (`windows-menu.tsx:50-70`).
    pub fn remembers_focus(mut self) -> Menu {
        self.remembers_focus = true;
        self
    }

    /// Converts this menu into a native [`PopupMenu`] for `Tx::popup_menu`.
    pub fn native(&self) -> PopupMenu {
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

    pub(super) fn item_at(&self, n: usize) -> Option<&Item> {
        match self.entries.get(n)? {
            Entry::Item(i) => Some(i),
            _ => None,
        }
    }

    pub(super) fn sub_at(&self, n: usize) -> Option<&Menu> {
        match self.entries.get(n)? {
            Entry::Sub(sub) => Some(&sub.menu),
            _ => None,
        }
    }

    /// Row `n`'s title, for a letter that lights it: an item's or a submenu's.
    pub(super) fn title_at(&self, n: usize) -> Option<&str> {
        match self.entries.get(n)? {
            Entry::Item(i) => Some(&i.title),
            Entry::Sub(sub) => Some(&sub.title),
            _ => None,
        }
    }

    /// The enabled rows' indices, in order: items and submenus.
    pub(super) fn offered(&self) -> Vec<usize> {
        (0..self.entries.len()).filter(|&n| self.item_at(n).is_some_and(|i| i.enabled) || self.sub_at(n).is_some()).collect()
    }
}

pub(super) fn row_key(id: &str) -> String {
    format!("popup-item-{id}")
}

/// The key of row `n` of `menu`: an item's or a submenu's.
pub(super) fn key_at(menu: &Menu, n: usize) -> Option<String> {
    match menu.entries.get(n)? {
        Entry::Item(i) => Some(row_key(&i.id)),
        Entry::Sub(sub) => Some(row_key(&sub.id)),
        _ => None,
    }
}

/// The widest row of `menu`, its glyph, label and shortcut with the row's padding and the
/// surface's inset.
pub(super) fn hug_width(menu: &Menu) -> f32 {
    let label = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
    let w = |s: &str, st| rune::text::measure(s, &[], st, None).0;
    let rows = menu.entries.iter().map(|e| match e {
        Entry::Item(i) => {
            let words = |s: &Option<String>, prefix: &str, gap: f32| s.as_deref().map_or(0.0, |t| gap + w(&format!("{prefix}{t}"), label));
            let check = if i.checked || matches!(i.glyph, Some(Glyph::Status(_))) { MENU.gap + 16.0 } else { 0.0 };
            i.glyph.map_or(0.0, |_| 16.0 + MENU.gap)
                + w(&i.title, label)
                + i.shortcut.as_deref().map_or(0.0, |k| MENU.gap + w(k, popover::HINT))
                + words(&i.hint, "· ", 4.0)
                + words(&i.note, "", MENU.gap)
                + check
        }
        Entry::Group(t) => w(t, GROUP),
        Entry::Sub(sub) => sub.glyph.map_or(0.0, |_| 16.0 + MENU.gap) + w(&sub.title, label) + MENU.gap + 16.0,
        Entry::Separator => 0.0,
    });
    rows.fold(0.0, f32::max) + menu.item_inset.unwrap_or(MENU.pad_l) + MENU.pad_r + 2.0 * MENU.inset + 2.0
}
