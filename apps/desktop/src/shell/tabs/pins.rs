//! Pinned sessions and pin groups for Consolidated navigation (`packages/app/src/shell/tabs/pins.ts`,
//! `pinned-area.tsx`, #64668).
//!
//! Groups exist only among pins (Arc's folders): grouping a session pins it, and unpinning removes
//! it from its group (pruning empty groups). Persisted globally in `opencode.global.dat` under
//! `"navigation.pins"` in Desktop's exact JSON shape.

use std::collections::{BTreeMap, BTreeSet};

use opencode_i18n::t;
use opencode_kernel::Slot;
use opencode_kit::servers::ServerKey;
use rune::Tx;
use serde_json::{Value, json};

use crate::App;
use crate::popup::{Item, Menu};
use crate::session_menu::Line;
use crate::tabs::Tab;

pub static PINS: Slot<PinsState> = Slot::new();

const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "navigation.pins";

/// Group color palette shared with tab groups (`TAB_GROUP_COLORS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinColor {
    Blue,
    Green,
    Orange,
    Purple,
    Pink,
    Cyan,
    Gray,
}

pub const AUTO_COLORS: [PinColor; 6] = [
    PinColor::Blue,
    PinColor::Green,
    PinColor::Orange,
    PinColor::Purple,
    PinColor::Cyan,
    PinColor::Pink,
];

const ALL_COLORS: [PinColor; 7] = [
    PinColor::Blue,
    PinColor::Green,
    PinColor::Orange,
    PinColor::Purple,
    PinColor::Pink,
    PinColor::Cyan,
    PinColor::Gray,
];

impl PinColor {
    pub fn name(self) -> &'static str {
        match self {
            PinColor::Blue => "blue",
            PinColor::Green => "green",
            PinColor::Orange => "orange",
            PinColor::Purple => "purple",
            PinColor::Pink => "pink",
            PinColor::Cyan => "cyan",
            PinColor::Gray => "gray",
        }
    }

    pub fn parse(raw: Option<&str>) -> Option<PinColor> {
        ALL_COLORS.into_iter().find(|c| Some(c.name()) == raw)
    }

    pub fn label(self) -> &'static str {
        match self {
            PinColor::Blue => t!("tabs.groups.color.blue"),
            PinColor::Green => t!("tabs.groups.color.green"),
            PinColor::Orange => t!("tabs.groups.color.orange"),
            PinColor::Purple => t!("tabs.groups.color.purple"),
            PinColor::Pink => t!("tabs.groups.color.pink"),
            PinColor::Cyan => t!("tabs.groups.color.cyan"),
            PinColor::Gray => t!("tabs.groups.color.gray"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PinRef {
    pub server: ServerKey,
    pub session_id: String,
}

impl PinRef {
    pub fn new(server: ServerKey, session_id: impl Into<String>) -> Self {
        Self { server, session_id: session_id.into() }
    }

    pub fn key(&self) -> String {
        format!("{}\0{}", self.server.as_str(), self.session_id)
    }

    pub fn uid(&self) -> String {
        crate::tabs::session_uid(&self.server, &self.session_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinGroup {
    pub id: String,
    pub name: String,
    pub color: PinColor,
    pub sessions: Vec<PinRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PinItem {
    Session(PinRef),
    Group(PinGroup),
}

/// Persisted `navigation.pins` document (`PinsStateSchema`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PinsDoc {
    pub items: Vec<PinItem>,
    pub collapsed: BTreeMap<String, bool>,
}

impl PinsDoc {
    pub fn to_json(&self) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|item| match item {
                PinItem::Session(r) => json!({
                    "type": "session",
                    "server": r.server.as_str(),
                    "sessionId": r.session_id,
                }),
                PinItem::Group(g) => json!({
                    "type": "group",
                    "id": g.id,
                    "name": g.name,
                    "color": g.color.name(),
                    "sessions": g.sessions.iter().map(|r| json!({
                        "server": r.server.as_str(),
                        "sessionId": r.session_id,
                    })).collect::<Vec<_>>(),
                }),
            })
            .collect();
        json!({
            "items": items,
            "collapsed": self.collapsed,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        if !v.is_object() {
            return Self::default();
        }
        let items = v["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry["type"].as_str()? {
                "session" => Some(PinItem::Session(PinRef::new(
                    ServerKey::new(entry["server"].as_str()?),
                    entry["sessionId"].as_str()?,
                ))),
                "group" => {
                    let sessions: Vec<PinRef> = entry["sessions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|s| Some(PinRef::new(ServerKey::new(s["server"].as_str()?), s["sessionId"].as_str()?)))
                        .collect();
                    if sessions.is_empty() {
                        return None;
                    }
                    Some(PinItem::Group(PinGroup {
                        id: entry["id"].as_str()?.to_owned(),
                        name: entry["name"].as_str()?.to_owned(),
                        color: PinColor::parse(entry["color"].as_str())?,
                        sessions,
                    }))
                }
                _ => None,
            })
            .collect();
        let collapsed = v["collapsed"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, b)| Some((k.clone(), b.as_bool()?)))
            .collect();
        let mut doc = Self { items, collapsed };
        doc.prune_collapsed();
        doc
    }

    pub fn is_pinned(&self, r: &PinRef) -> bool {
        self.items.iter().any(|item| match item {
            PinItem::Session(s) => s == r,
            PinItem::Group(g) => g.sessions.contains(r),
        })
    }

    pub fn group_of(&self, r: &PinRef) -> Option<&PinGroup> {
        self.items.iter().find_map(|item| match item {
            PinItem::Group(g) if g.sessions.contains(r) => Some(g),
            _ => None,
        })
    }

    pub fn groups(&self) -> impl Iterator<Item = &PinGroup> {
        self.items.iter().filter_map(|item| match item {
            PinItem::Group(g) => Some(g),
            PinItem::Session(_) => None,
        })
    }

    pub fn pin(&mut self, r: PinRef) -> bool {
        if self.is_pinned(&r) {
            return false;
        }
        self.items.push(PinItem::Session(r));
        true
    }

    pub fn unpin(&mut self, r: &PinRef) -> bool {
        if !self.is_pinned(r) {
            return false;
        }
        let mut next = Vec::with_capacity(self.items.len());
        for item in self.items.drain(..) {
            match item {
                PinItem::Session(s) if &s == r => {}
                PinItem::Session(s) => next.push(PinItem::Session(s)),
                PinItem::Group(mut g) => {
                    g.sessions.retain(|s| s != r);
                    if !g.sessions.is_empty() {
                        next.push(PinItem::Group(g));
                    }
                }
            }
        }
        self.items = next;
        self.prune_collapsed();
        true
    }

    /// Starts a new group holding `r`, placed where `r` was pinned or at the end.
    pub fn create_group(&mut self, r: PinRef, id: String, name: String, color: PinColor) -> bool {
        let created = PinItem::Group(PinGroup { id, name, color, sessions: vec![r.clone()] });
        if let Some(pos) = self.items.iter().position(|item| matches!(item, PinItem::Session(s) if s == &r)) {
            self.items[pos] = created;
            self.prune_collapsed();
            return true;
        }
        self.unpin(&r);
        self.items.push(created);
        self.prune_collapsed();
        true
    }

    /// Moves `r` into `group_id`, pinning it first if needed.
    pub fn move_to_group(&mut self, r: PinRef, group_id: &str) -> bool {
        if !self.groups().any(|g| g.id == group_id) {
            return false;
        }
        if self.group_of(&r).is_some_and(|g| g.id == group_id) {
            return false;
        }
        self.unpin(&r);
        for item in &mut self.items {
            if let PinItem::Group(g) = item
                && g.id == group_id
            {
                g.sessions.push(r);
                self.prune_collapsed();
                return true;
            }
        }
        false
    }

    /// Takes `r` out of its group and keeps it pinned right after the group.
    pub fn remove_from_group(&mut self, r: &PinRef) -> bool {
        let Some(pos) = self.items.iter().position(|item| matches!(item, PinItem::Group(g) if g.sessions.contains(r))) else {
            return false;
        };
        let loose = PinItem::Session(r.clone());
        if let PinItem::Group(g) = &mut self.items[pos] {
            g.sessions.retain(|s| s != r);
            if g.sessions.is_empty() {
                self.items[pos] = loose;
            } else {
                self.items.insert(pos + 1, loose);
            }
        }
        self.prune_collapsed();
        true
    }

    /// Dissolves `group_id`; its sessions stay pinned in place.
    pub fn ungroup(&mut self, group_id: &str) -> bool {
        let Some(pos) = self.items.iter().position(|item| matches!(item, PinItem::Group(g) if g.id == group_id)) else {
            return false;
        };
        let PinItem::Group(g) = self.items.remove(pos) else { return false };
        for (offset, r) in g.sessions.into_iter().enumerate() {
            self.items.insert(pos + offset, PinItem::Session(r));
        }
        self.prune_collapsed();
        true
    }

    /// Removes `group_id` and unpins all of its sessions.
    pub fn unpin_group(&mut self, group_id: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|item| !matches!(item, PinItem::Group(g) if g.id == group_id));
        if self.items.len() == before {
            return false;
        }
        self.prune_collapsed();
        true
    }

    pub fn rename_group(&mut self, group_id: &str, name: String) -> bool {
        for item in &mut self.items {
            if let PinItem::Group(g) = item
                && g.id == group_id
                && g.name != name
            {
                g.name = name;
                return true;
            }
        }
        false
    }

    pub fn recolor_group(&mut self, group_id: &str, color: PinColor) -> bool {
        for item in &mut self.items {
            if let PinItem::Group(g) = item
                && g.id == group_id
                && g.color != color
            {
                g.color = color;
                return true;
            }
        }
        false
    }

    /// Accordion toggle (`toggleGroup`): expanding one group collapses the rest; clicking an open
    /// group collapses all.
    pub fn toggle_group(&mut self, group_id: &str) -> bool {
        let was_open = !self.collapsed.get(group_id).copied().unwrap_or(false);
        let mut next = BTreeMap::new();
        for g in self.groups() {
            next.insert(g.id.clone(), was_open || g.id != group_id);
        }
        if self.collapsed == next {
            return false;
        }
        self.collapsed = next;
        true
    }

    pub fn next_color(&self) -> PinColor {
        let existing: Vec<PinColor> = self.groups().map(|g| g.color).collect();
        AUTO_COLORS
            .into_iter()
            .find(|c| !existing.contains(c))
            .unwrap_or(AUTO_COLORS[existing.len() % AUTO_COLORS.len()])
    }

    pub fn next_name(&self) -> String {
        let taken: BTreeSet<String> = self.groups().map(|g| g.name.trim().to_lowercase()).collect();
        let count = self.groups().count();
        for n in 1..=count + 1 {
            let candidate = t!("tabs.groups.unnamed", number = n);
            if !taken.contains(&candidate.trim().to_lowercase()) {
                return candidate;
            }
        }
        t!("tabs.groups.unnamed", number = count + 1)
    }

    fn prune_collapsed(&mut self) {
        let ids: BTreeSet<String> = self.groups().map(|g| g.id.clone()).collect();
        self.collapsed.retain(|id, _| ids.contains(id));
    }
}

#[derive(Default)]
pub struct PinsState {
    pub doc: PinsDoc,
}

pub fn state(s: &App) -> &PinsState {
    s.kernel.slots.get(&PINS)
}

fn state_mut(s: &mut App) -> &mut PinsState {
    s.kernel.slots.get_mut(&PINS)
}

pub fn restore(s: &mut App) {
    let doc = crate::storage::json(GLOBAL, KEY).map(|v| PinsDoc::from_json(&v)).unwrap_or_default();
    state_mut(s).doc = doc;
}

fn save(s: &App) {
    if s.persists() {
        crate::storage::set_json(GLOBAL, KEY, &state(s).doc.to_json());
    }
}

pub fn update(s: &mut App, f: impl FnOnce(&mut PinsDoc) -> bool) {
    if f(&mut state_mut(s).doc) {
        save(s);
    }
}

pub fn is_current(s: &App, r: &PinRef) -> bool {
    if s.settings_open() {
        return false;
    }
    match &*s.route {
        crate::Route::Session { id, server } => *server == r.server && s.root_of(id) == r.session_id,
        _ => false,
    }
}

pub fn holds(s: &App, tab: &Tab) -> bool {
    if tab.is_draft() || !s.settings.navigation_prototype() {
        return false;
    }
    let r = PinRef::new(tab.server.clone(), tab.id.clone());
    state(s).doc.is_pinned(&r)
}

pub fn holds_session(s: &App, server: &ServerKey, id: &str) -> bool {
    if !s.settings.navigation_prototype() {
        return false;
    }
    state(s).doc.is_pinned(&PinRef::new(server.clone(), id))
}

/// Sessions shown for a group: all when open, or only the active session while collapsed.
pub fn visible_in_group<'a>(s: &App, group: &'a PinGroup) -> Vec<&'a PinRef> {
    let collapsed = state(s).doc.collapsed.get(&group.id).copied().unwrap_or(false);
    if collapsed {
        group.sessions.iter().filter(|r| is_current(s, r)).collect()
    } else {
        group.sessions.iter().collect()
    }
}

/// Visible pinned sessions in display order (used for ⌘1–9 numbering and keyboard tab stepping).
pub fn visible_refs(s: &App) -> Vec<PinRef> {
    let doc = &state(s).doc;
    let mut out = Vec::new();
    for item in &doc.items {
        match item {
            PinItem::Session(r) => out.push(r.clone()),
            PinItem::Group(g) => out.extend(visible_in_group(s, g).into_iter().cloned()),
        }
    }
    out
}

pub fn visible_uids(s: &App) -> Vec<String> {
    visible_refs(s).into_iter().map(|r| r.uid()).collect()
}

pub fn open_pin(s: &mut App, r: &PinRef, tx: &mut Tx) {
    crate::servers::runtime::switch(s, &r.server);
    s.open(&r.session_id, tx);
}

pub fn open_by_uid(s: &mut App, uid: &str, tx: &mut Tx) -> bool {
    let found = visible_refs(s).into_iter().find(|r| r.uid() == uid);
    if let Some(r) = found {
        open_pin(s, &r, tx);
        return true;
    }
    false
}

pub fn new_group(s: &mut App, r: PinRef) {
    let id = crate::windows::store::new_id();
    update(s, |d| {
        let name = d.next_name();
        let color = d.next_color();
        d.create_group(r, id, name, color)
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HiddenMark {
    Attention,
    Unread,
}

/// What a collapsed pin group's hidden sessions need (`hiddenMark`, `pinned-area.tsx`).
pub fn hidden_mark(s: &App, group: &PinGroup) -> Option<HiddenMark> {
    if !state(s).doc.collapsed.get(&group.id).copied().unwrap_or(false) {
        return None;
    }
    let mut unread = false;
    for r in &group.sessions {
        if is_current(s, r) {
            continue;
        }
        let muted = crate::session_menu::preferences::muted(&r.server, &r.session_id);
        let status = status_of(s, r);
        match status {
            Some(crate::TabStatus::Attention) => return Some(HiddenMark::Attention),
            Some(crate::TabStatus::Unread | crate::TabStatus::Failed) if !muted => unread = true,
            _ => {}
        }
    }
    unread.then_some(HiddenMark::Unread)
}

pub fn status_of(s: &App, r: &PinRef) -> Option<crate::TabStatus> {
    if let Some(tab) = s.tabs.iter().find(|t| !t.is_draft() && t.is(&r.server, &r.session_id)) {
        return s.tab_status_of(tab);
    }
    if r.server == *crate::servers::runtime::active(s) {
        return s.tab_status(&r.session_id);
    }
    None
}

/// Pin, Unpin, and Add to group submenu rows (`PinMenuItems`, `pinned-area.tsx`).
pub fn menu_lines(s: &App, server: &ServerKey, session_id: &str) -> Vec<Line> {
    let r = PinRef::new(server.clone(), session_id);
    let doc = &state(s).doc;
    let pinned = doc.is_pinned(&r);
    let current_group = doc.group_of(&r).map(|g| g.id.as_str());
    let pin_item = Item::new(
        if pinned { "pin:unpin" } else { "pin:pin" },
        if pinned { t!("sidebar.recent.unpin") } else { t!("sidebar.recent.pin") },
    );
    let mut sub = Menu::new(t!("tabs.groups.addToGroup"))
        .min_width(164.0)
        .item(Item::new("pin:group-new", t!("tabs.groups.new")).icon(&rune_theme_opencode::icons::PLUS));
    let groups: Vec<&PinGroup> = doc.groups().collect();
    if !groups.is_empty() {
        sub = sub.separator();
        for g in groups {
            sub = sub.item(Item::new(format!("pin:group-in:{}", g.id), g.name.clone()).checked(current_group == Some(g.id.as_str())));
        }
    }
    if current_group.is_some() {
        sub = sub.separator().item(Item::new("pin:group-out", t!("tabs.groups.removeFromGroup")));
    }
    vec![
        Line::Extra(pin_item),
        Line::Sub {
            id: "pin:group-add".into(),
            title: t!("tabs.groups.addToGroup").into(),
            menu: sub,
        },
    ]
}

pub fn run_menu(s: &mut App, server: &ServerKey, session_id: &str, id: &str) -> bool {
    let Some(rest) = id.strip_prefix("pin:") else { return false };
    let r = PinRef::new(server.clone(), session_id);
    match rest {
        "pin" => update(s, |d| d.pin(r)),
        "unpin" => update(s, |d| d.unpin(&r)),
        "group-new" => new_group(s, r),
        "group-out" => update(s, |d| d.remove_from_group(&r)),
        _ => match rest.strip_prefix("group-in:") {
            Some(gid) => {
                let already = state(s).doc.group_of(&r).is_some_and(|g| g.id == gid);
                if already {
                    update(s, |d| d.remove_from_group(&r));
                } else {
                    update(s, |d| d.move_to_group(r, gid));
                }
            }
            None => return false,
        },
    }
    true
}
