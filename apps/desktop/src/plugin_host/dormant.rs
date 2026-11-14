//! Built-ins that start at their first use. A built-in this launch's first screen doesn't draw,
//! with a record of what it contributed when it last started, stays `Dormant` after the first
//! frame: its commands, keybinds, menu rows, panels' launchers, settings search rows and MCP tools
//! are listed from the record, and no VM runs for it. Its first use starts it in that same turn
//! and the use goes on against what it registers live ([`wake`]):
//!
//! - **At once:** one of its commands, keybinds, slash commands, menu or app-menu rows, code block
//!   actions or MCP tools runs; a link is opened (link handlers answer `match`); a session's
//!   screen it draws on is routed to ([`wake_for_route`]); the palette, the slash list or a menu
//!   whose rows it answers for opens ([`wake_kind`]).
//! - **Ahead of a use:** a session row is hovered or highlighted ([`near_session`]): the ones a
//!   session draws start one per turn, none while input waits.
//! - **When idle:** the window has had no input for [`crate::idle::AFTER`] and none waits; one per
//!   turn. Background duties (the updater's check and pill, Keep screen active) and anything a surface on screen
//!   asked of a dormant plugin ([`want`]) start this way.
//!
//! Nothing is missed meanwhile: the server events it listened for ([`Record::events`]) are kept
//! while it's dormant ([`buffer_event`]) and handed to it, in order, as it starts; everything else
//! a plugin mirrors (sessions, the route, stores) it reads as it starts, as a plugin started after
//! the first frame always has.
//!
//! The records live in the cache folder (`plugin-contributions.json`), each keyed by the plugin's
//! files and the interface language: a built-in whose record doesn't match (a new build of it,
//! another language, a first launch) starts after the first frame as before, and records itself.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use opencode_plugin_spec as spec;
use rune::Tx;
use serde::{Deserialize, Serialize};

use super::plugin::{Contrib, Contribs};
use super::{Life, LifeEvent};
use crate::App;

/// Server events kept for one dormant plugin; the oldest go first past it.
const KEPT_EVENTS: usize = 256;

/// One recorded contribution: its key (`<plugin>.<id>`), its spec and the names of the functions it
/// handed over (`run`, `enabled`, `when`, ...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Entry {
    key: String,
    spec: serde_json::Value,
    fns: Vec<String>,
}

/// What a plugin contributed when it last started, and what it was keyed by.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Its files' hash (`sources::files_hash`).
    hash: String,
    /// The interface language its strings were made in.
    locale: String,
    /// Its window half's contributions by kind (`commands`, `panels`, ...).
    window: BTreeMap<String, Vec<Entry>>,
    /// Its app half's: app menu rows and MCP tools.
    app: BTreeMap<String, Vec<Entry>>,
    /// The server event prefixes its window half listened for (`Server:OnEvent`).
    pub events: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    version: u32,
    plugins: BTreeMap<String, Record>,
}

const VERSION: u32 = 1;

/// Why a dormant plugin starts: its first use, the next turn after it was asked for, or idle time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Use,
    Near,
    Idle,
}

/// When a dormant plugin starts if nothing uses it first: at its first use only; in idle time (a
/// background duty, or a surface asked something only its VM answers); or at the next turn (a
/// session it draws is near, or a surface shows its part now). It only moves later in the list.
///
/// ```text
/// FirstUse ─ want ▶ Idle ─ want_now / near ▶ Next
/// FirstUse ─────────── want_now / near ───▶ Next
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Wait {
    #[default]
    FirstUse,
    Idle,
    Next,
}

/// A dormant plugin's side of the host: its record, the server events kept for it, and when it
/// starts unless used first.
#[derive(Default)]
pub struct Asleep {
    pub record: Record,
    events: VecDeque<(String, serde_json::Value)>,
    wait: Wait,
}

/// The window's dormant plugins and the records file as this window read it.
#[derive(Default)]
pub struct Dormant {
    pub asleep: BTreeMap<String, Asleep>,
    /// Every record read or made this launch; written back when one changed.
    records: BTreeMap<String, Record>,
    read: bool,
    dirty: bool,
    /// Events kept for a plugin that started, handed to it at the next poll.
    pub catch_up: BTreeMap<String, Vec<(String, serde_json::Value)>>,
}

impl Dormant {
    /// The record of built-in `id` for `hash`, in the current language.
    pub fn record(&mut self, id: &str, hash: &str) -> Option<Record> {
        if !self.read {
            self.read = true;
            self.records = read_file();
        }
        let locale = opencode_i18n::locale().id();
        self.records.get(id).filter(|r| r.hash == hash && r.locale == locale).cloned()
    }
}

/// The records as written last: none when the file is missing, unreadable or of another version.
fn read_file() -> BTreeMap<String, Record> {
    let Some(path) = path() else { return BTreeMap::new() };
    let Ok(bytes) = std::fs::read(&path) else { return BTreeMap::new() };
    match serde_json::from_slice::<File>(&bytes) {
        Ok(f) if f.version == VERSION => f.plugins,
        _ => BTreeMap::new(),
    }
}

fn path() -> Option<std::path::PathBuf> {
    Some(crate::dirs::cache()?.join("plugin-contributions.json"))
}

/// Notes what plugin `i` contributed as it started, for its next launch's record.
pub fn note_started(app: &mut App, i: usize) {
    let p = &app.plugins.list[i];
    let Some(hash) = super::sources::files_hash(&p.manifest.id, &p.origin, p.tier) else { return };
    let id = p.manifest.id.clone();
    let app_half = super::app_half::each(|a| (*a.half.id == *id).then(|| record_kinds(&a.half.contribs.borrow(), &["menubar_items", "mcp_tools"]))).into_iter().next().unwrap_or_default();
    let record = Record {
        hash,
        locale: opencode_i18n::locale().id().to_owned(),
        window: record_kinds(&p.half.contribs.borrow(), &[]),
        app: app_half,
        events: p.half.events.borrow().iter().map(|(_, prefix, _)| prefix.clone()).collect(),
    };
    let d = &mut app.plugins.dormant;
    if !d.read {
        d.read = true;
        d.records = read_file();
    }
    if d.records.get(&id) != Some(&record) {
        d.records.insert(id, record);
        d.dirty = true;
    }
}

/// Writes the records back off the app's thread, when one changed.
pub fn save(app: &mut App) {
    // Read before it's taken: a write to the host rebuilds every region showing it.
    if !app.plugins.dormant.dirty {
        return;
    }
    let d = &mut app.plugins.dormant;
    d.dirty = false;
    let (Some(path), Ok(json)) = (path(), serde_json::to_vec(&File { version: VERSION, plugins: d.records.clone() })) else { return };
    super::tasks::run(move || {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    });
}

/// Puts plugin `i` to sleep with `record`: its contributions come from it until it starts.
pub fn doze(app: &mut App, i: usize, record: Record, background: bool) {
    let p = &app.plugins.list[i];
    let id = p.manifest.id.clone();
    super::sources::intern_icons(&id, &p.origin.files());
    *p.half.contribs.borrow_mut() = contribs_of(&record.window);
    p.half.epoch.set(p.half.epoch.get() + 1);
    let wait = if background { Wait::Idle } else { Wait::FirstUse };
    app.plugins.dormant.asleep.insert(id, Asleep { record, wait, ..Asleep::default() });
    super::drive(app, i, LifeEvent::Doze);
    let id = app.plugins.list[i].manifest.id.clone();
    let app_half = contribs_of(&app.plugins.dormant.asleep[&id].record.app);
    APP_HALVES.with_borrow_mut(|a| a.push((id, app_half)));
}

/// One window's dormant app half `id` is gone (it started, was turned off, or its window closed).
fn release_app_half(id: &str) {
    APP_HALVES.with_borrow_mut(|a| {
        if let Some(at) = a.iter().position(|(owner, _)| owner == id) {
            a.remove(at);
        }
    });
}

/// The window is closing: its dormant plugins' app halves are no longer listed for it.
pub fn close(host: &super::PluginHost) {
    for id in host.dormant.asleep.keys() {
        release_app_half(id);
    }
}

thread_local! {
    /// The app halves of dormant plugins, as their records list them: their app menu rows and MCP
    /// tools, which the app lists without a window (`commands::menubar_items`).
    static APP_HALVES: std::cell::RefCell<Vec<(String, Contribs)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Runs `f` on each dormant app half's recorded contributions, once per plugin, but for those whose
/// app half runs (another window started it): the running one lists them.
pub fn each_app_half<R>(mut f: impl FnMut(&str, &Contribs) -> Option<R>) -> Vec<R> {
    let running: Vec<String> = super::app_half::each(|a| Some(a.half.id.to_string()));
    APP_HALVES.with_borrow(|a| {
        let mut seen: Vec<&str> = Vec::new();
        let mut out = Vec::new();
        for (id, c) in a {
            if running.contains(id) || seen.contains(&id.as_str()) {
                continue;
            }
            seen.push(id);
            out.extend(f(id, c));
        }
        out
    })
}

/// Starts plugin `id` now, for `why`, if it's dormant; true when it was and runs now.
pub fn wake(app: &mut App, id: &str, why: Why) -> bool {
    let Some(i) = app.plugins.find(id).filter(|&i| matches!(app.plugins.list[i].life, Life::Dormant)) else { return false };
    let started = rune::time::Instant::now();
    let asleep = app.plugins.dormant.asleep.remove(id).unwrap_or_default();
    release_app_half(id);
    *app.plugins.list[i].half.contribs.borrow_mut() = Contribs::default();
    super::drive(app, i, LifeEvent::Load);
    // The events kept for it, but for those still waiting for this poll: it hears those with the rest.
    let pending = app.plugins.events.borrow().clone();
    let kept: Vec<_> = asleep.events.into_iter().filter(|e| !pending.contains(e)).collect();
    let caught = kept.len();
    if !kept.is_empty() {
        app.plugins.dormant.catch_up.entry(id.to_owned()).or_default().extend(kept);
    }
    let running = app.plugins.list[i].running();
    let when = match why { Why::Use => "first use", Why::Near => "next turn (a session near, or a surface showing it)", Why::Idle => "idle time" };
    eprintln!("plugins: started {id} at its {when} in {} us; {caught} server events kept while it slept go to it", started.elapsed().as_micros());
    rune::marks::mark(Box::leak(format!("plugin.{id}.woke").into_boxed_str()));
    super::bump();
    running
}

/// Plugin `id` was turned off while dormant: its recorded contributions go with it.
pub fn forget(app: &mut App, id: &str) {
    if app.plugins.dormant.asleep.remove(id).is_some()
        && let Some(half) = app.plugins.half(id)
    {
        *half.contribs.borrow_mut() = Contribs::default();
        release_app_half(id);
    }
}

/// Wakes, now, each dormant plugin whose recorded contributions `owns` picks; true when one woke.
pub fn wake_owners(app: &mut App, owns: impl Fn(&Contribs) -> bool) -> bool {
    let ids: Vec<String> = app.plugins.dormant.asleep.keys().filter(|id| app.plugins.half(id).is_some_and(|h| owns(&h.contribs.borrow()))).cloned().collect();
    let app_ids: Vec<String> = each_app_half(|id, c| owns(c).then(|| id.to_owned()));
    let mut any = false;
    for id in ids.into_iter().chain(app_ids) {
        any |= wake(app, &id, Why::Use);
    }
    any
}

/// The palette, the slash list or a menu is opening: the dormant plugins whose rows in it answer
/// a function (`enabled`, `hidden`, `when`) start, so the rows it lists are the ones they say.
pub fn wake_kind(app: &mut App, kind: &str, fns: &[&str]) -> bool {
    let ids: Vec<String> = app.plugins.dormant.asleep.iter().filter(|(_, a)| a.record.window.get(kind).is_some_and(|es| es.iter().any(|e| e.fns.iter().any(|f| fns.contains(&f.as_str()))))).map(|(id, _)| id.clone()).collect();
    let mut any = false;
    for id in ids {
        any |= wake(app, &id, Why::Use);
    }
    any
}

/// Whether dormant plugin `id`'s recorded contribution `key` of `kind` handed over function `name`.
pub fn recorded_fn(app: &App, id: &str, kind: &str, key: &str, name: &str) -> bool {
    app.plugins.dormant.asleep.get(id).and_then(|a| a.record.window.get(kind)).is_some_and(|es| es.iter().any(|e| e.key == key && e.fns.iter().any(|f| f == name)))
}

/// The side region's New tab view or its `+` menu is opening: the dormant plugins with entries
/// there (panels, `session.panel` rows) start, so the entries are the ones they offer.
pub fn wake_launchers(app: &mut App) -> bool {
    wake_owners(app, |c| !c.panels.is_empty() || c.menu_items.iter().any(|m| m.spec.menu == spec::MenuName::SessionPanel))
}

/// A surface on screen asked dormant plugin `id` something only its VM answers: it starts in idle
/// time. Called from views, so it only marks.
pub fn want(app: &App, id: &str) {
    if wants(app, id, Wait::Idle) {
        ASKED.with_borrow_mut(|w| push_once(w, id, Wait::Idle));
    }
}

/// [`want`] for a surface that shows dormant plugin `id`'s part now (a slot, a code block, a file
/// view): it starts at the next turn, so the surface draws without it for one frame at most.
pub fn want_now(app: &App, id: &str) {
    if wants(app, id, Wait::Next) {
        ASKED.with_borrow_mut(|w| push_once(w, id, Wait::Next));
    }
}

/// Whether asking dormant plugin `id` to start at `to` moves it.
fn wants(app: &App, id: &str, to: Wait) -> bool {
    // The debug plugin's slot is its bar, which draws nothing while it's hidden.
    if id == "debug" && !super::first_screen::debug_bar_shown() {
        return false;
    }
    app.plugins.dormant.asleep.get(id).is_some_and(|a| a.wait < to)
}

fn push_once(list: &mut Vec<(String, Wait)>, id: &str, to: Wait) {
    match list.iter_mut().find(|(x, _)| x == id) {
        Some((_, w)) => *w = (*w).max(to),
        None => list.push((id.to_owned(), to)),
    }
}

thread_local! {
    /// What views asked of dormant plugins since the last poll, which carries it out.
    static ASKED: std::cell::RefCell<Vec<(String, Wait)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The dormant plugins a session's screen draws, as `first_screen` decides for a launch: its
/// header slots (the debug bar's only while shown), its composer pills, the plugins with a tab in
/// the session, and the file viewers when it has a Files tab. A code block a plugin draws asks for
/// it as it shows ([`want_now`]); a link handler answers when a link is opened.
fn for_session(app: &App, session: &str) -> Vec<String> {
    let with_tabs: Vec<&str> = app.side.sessions.get(session).map(|side| side.tab_state.tabs.all.iter().filter_map(|k| crate::panels::tabs::split_panel_key(k).map(|(plugin, _)| plugin)).collect()).unwrap_or_default();
    let files = with_tabs.contains(&"file");
    let has = |a: &Asleep, kind: &str| a.record.window.get(kind).is_some_and(|es| !es.is_empty());
    app.plugins
        .dormant
        .asleep
        .iter()
        .filter(|(id, a)| {
            let slot = has(a, "slots") && (id.as_str() != "debug" || super::first_screen::debug_bar_shown());
            slot || has(a, "composer_pills") || with_tabs.contains(&id.as_str()) || (files && has(a, "file_viewers"))
        })
        .map(|(id, _)| id.clone())
        .collect()
}

/// The window routes to session `session`: what its screen draws starts before its frame.
pub fn wake_for_route(app: &mut App, session: &str) {
    for id in for_session(app, session) {
        wake(app, &id, Why::Use);
    }
}

/// A session row was hovered or highlighted: what a session's screen draws starts ahead of the
/// click, one per turn ([`poll`]).
pub fn near_session(app: &App, session: &str) {
    for id in for_session(app, session) {
        ASKED.with_borrow_mut(|n| push_once(n, &id, Wait::Next));
    }
}

/// Keeps server event `ty` for each dormant plugin that listened for it.
pub fn buffer_event(app: &mut App, ty: &str, data: &serde_json::Value) {
    for a in app.plugins.dormant.asleep.values_mut().filter(|a| a.record.events.iter().any(|p| ty.starts_with(p.as_str()))) {
        if a.events.len() >= KEPT_EVENTS {
            a.events.pop_front();
        }
        a.events.push_back((ty.to_owned(), data.clone()));
    }
}

/// Whether a dormant plugin listened for server event `ty`.
pub fn listens(app: &App, ty: &str) -> bool {
    app.plugins.dormant.asleep.values().any(|a| a.record.events.iter().any(|p| ty.starts_with(p.as_str())))
}

/// Each turn: one dormant plugin starts, none while input waits; a near one at once, others only
/// once the window has had no input for [`crate::idle::AFTER`]. Asks to be woken when idle time comes.
pub fn poll(app: &mut App, tx: &mut Tx) -> bool {
    for (id, to) in ASKED.with_borrow_mut(std::mem::take) {
        if let Some(a) = app.plugins.dormant.asleep.get_mut(&id) {
            a.wait = a.wait.max(to);
        }
    }
    if tx.input_waiting() {
        return false;
    }
    let due = |wait: Wait| app.plugins.dormant.asleep.iter().find(|(_, a)| a.wait == wait).map(|(id, _)| id.clone());
    if let Some(id) = due(Wait::Next) {
        wake(app, &id, Why::Near);
        if app.plugins.dormant.asleep.values().any(|a| a.wait > Wait::FirstUse) {
            tx.wake_after(Duration::ZERO);
        }
        return true;
    }
    let Some(id) = due(Wait::Idle) else { return false };
    let quiet = crate::idle::quiet_for();
    if quiet < crate::idle::AFTER {
        tx.wake_after(crate::idle::AFTER - quiet);
        return false;
    }
    wake(app, &id, Why::Idle);
    if app.plugins.dormant.asleep.values().any(|a| a.wait > Wait::FirstUse) {
        tx.wake_after(Duration::ZERO);
    }
    true
}

/// The language changed: records in the old one name things in it, so every dormant plugin starts
/// in idle time and records itself again.
pub fn locale_changed(app: &mut App) {
    for a in app.plugins.dormant.asleep.values_mut() {
        a.wait = a.wait.max(Wait::Idle);
    }
}

// ---- Records and contributions ----------------------------------------------------------------

macro_rules! kinds {
    ($m:ident) => {
        $m! {
            commands: CommandSpec,
            panels: PanelSpec,
            settings: SettingSpec,
            settings_pages: SettingsPageSpec,
            menu_items: MenuItemSpec,
            menubar_items: MenubarItemSpec,
            link_handlers: LinkHandlerSpec,
            titlebar_items: TitlebarItemSpec,
            composer_pills: ComposerPillSpec,
            code_blocks: CodeBlockSpec,
            code_block_actions: CodeBlockActionSpec,
            slots: SlotSpec,
            themes: ThemeSpec,
            file_viewers: FileViewerSpec,
            server_sources: ServerSourceSpec,
            dialogs: DialogSpec,
            popovers: PopoverSpec,
            mcp_tools: McpToolSpec,
        }
    };
}

/// `contribs`' kinds as record entries (`only`: just those kinds; empty: all).
fn record_kinds(contribs: &Contribs, only: &[&str]) -> BTreeMap<String, Vec<Entry>> {
    fn entries<S: Serialize>(list: &[Contrib<S>]) -> Vec<Entry> {
        list.iter().filter_map(|c| Some(Entry { key: c.key.to_owned(), spec: serde_json::to_value(&c.spec).ok()?, fns: c.fns.keys().map(|k| (*k).to_owned()).collect() })).collect()
    }
    let mut out = BTreeMap::new();
    macro_rules! put {
        ($($field:ident: $ty:ident,)*) => {
            $(
                if (only.is_empty() || only.contains(&stringify!($field))) && !contribs.$field.is_empty() {
                    out.insert(stringify!($field).to_owned(), entries(&contribs.$field));
                }
            )*
        };
    }
    kinds!(put);
    out
}

/// Contributions as a record lists them: their specs and keys, no functions.
fn contribs_of(kinds: &BTreeMap<String, Vec<Entry>>) -> Contribs {
    fn list<S: serde::de::DeserializeOwned>(kinds: &BTreeMap<String, Vec<Entry>>, kind: &str) -> Vec<Contrib<S>> {
        kinds.get(kind).into_iter().flatten().filter_map(|e| Some(Contrib { spec: serde_json::from_value(e.spec.clone()).ok()?, fns: Default::default(), key: super::icons::intern(&e.key) })).collect()
    }
    let mut c = Contribs::default();
    macro_rules! take {
        ($($field:ident: $ty:ident,)*) => {
            $( c.$field = list::<spec::$ty>(kinds, stringify!($field)); )*
        };
    }
    kinds!(take);
    c
}
