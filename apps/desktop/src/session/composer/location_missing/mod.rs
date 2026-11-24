//! "Session location unavailable": the card that stands in for the composer while the open
//! session's folder is gone, as a removed worktree leaves it
//! (`packages/app/src/session/composer/location-missing.tsx`, `region.tsx`). The person picks
//! where the session goes on: another worktree of its project (or a new one), or any directory.
//! Nothing is written into a folder they didn't choose.
//!
//! Three parts: whether the folder is gone ([`sync`], the server's answer, asked off this thread),
//! the card's own steps ([`card`]), and what it draws ([`view`]). A question or a permission
//! stands in front of it, as it does of the composer (`session-composer-region.tsx`).
//!
//! Beyond Desktop, where a subagent's page has no composer and so never shows the card: this
//! app's subagent pages take replies, so theirs gives way to the card too.

mod card;
mod sync;
mod view;

use std::time::Duration;

use opencode_i18n::t;
use opencode_kernel::{Feature, KernelBuilder, Slot};
use opencode_kit::model::{parse_managed_worktrees, parse_worktrees};
use opencode_kit::wire_error::{WireError, WireKind};
use opencode_search::query::{Highlight, HighlightEvent};
use rune::machine::{Attempt, Effects};
use rune::{PointerEvent, TextEdit, Tx};
use serde_json::{Value, json};

use crate::jobs::Apply;
use crate::projects::ProjectsHost;
use crate::projects::remote_picker::{OnCancel, OnPick};
use crate::{App, Route};

pub(crate) use view::panel;

pub(crate) static LOCATION_MISSING: Slot<LocationMissing> = Slot::new();

/// Desktop's global storage name: every window's, all of it in the boot record, so a launch has
/// it before its first frame with no read of its own.
const GLOBAL: &str = "opencode.global.dat";
const KEY: &str = "rune.location.missing";
/// The folders kept; the oldest goes first.
const KNOWN_MAX: usize = 32;

/// The keys of what takes focus.
pub(crate) const TRIGGER: &str = "session-location-worktree";
pub(crate) const CHOOSE: &str = "session-location-choose";
pub(crate) const LIST: &str = "session-location-worktrees";
const SCROLL: &str = "session-location-worktrees-scroll";

/// What the card is for (`region.tsx`'s `missing`): a new one starts its steps over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Subject {
    pub session: String,
    pub directory: String,
    /// The session's project, once this window knows it.
    pub project: Option<String>,
}

/// The open Choose worktree menu's search text and lit row; the card's machine says whether it
/// is open.
#[derive(Default)]
pub struct Menu {
    pub search: TextEdit,
    active: Highlight,
    /// Where the pointer last moved over a row: only a moving pointer lights one.
    pointer: Option<[f32; 2]>,
    /// Whether keys opened it, so its trigger takes the focus ring back when keys close it.
    by_keys: bool,
}

#[derive(Default)]
pub struct LocationMissing {
    /// Folders their servers last said don't exist, oldest first, kept for the next launch.
    known: Vec<sync::Location>,
    /// The open session's folder and where asking about it stands; none off a session's page.
    sync: Option<sync::Check>,
    /// Whether the server's stream was up when the folder was last looked at (`connected`).
    connected: bool,
    /// The card's steps while it shows, and what it shows for.
    card: Option<(Subject, card::Card)>,
    menu: Menu,
}

/// The open session and its folder: its info's, or what its tab saved while its server hasn't
/// listed it (a launch). None for a session still starting: its worktree may not be made yet.
fn folder(s: &App) -> Option<(&str, &str, &str)> {
    let Route::Session { id, server } = &*s.route else { return None };
    if s.queue.starts.has(id) {
        return None;
    }
    let directory = match s.session(id) {
        Some(info) => info.directory.as_str(),
        None => s.tabs.iter().find(|t| t.id == *id && t.kind == crate::tabs::TabKind::Session).map(|t| t.directory.as_str())?,
    };
    (!directory.is_empty()).then_some((id.as_str(), server.as_str(), directory))
}

/// The session whose folder the server says is gone, with that folder: the open one, unless a
/// question or permission stands in the composer's place.
fn gone(s: &App) -> Option<(&str, &str)> {
    let (id, server, directory) = folder(s)?;
    let check = s.kernel.slots.get(&LOCATION_MISSING).sync.as_ref()?;
    let here = check.location.server == server && check.location.directory == directory;
    (here && check.missing(&check.location) && !s.dock_shown()).then_some((id, directory))
}

/// Whether the card stands in the composer's place (`region.tsx`: `missing()`).
pub(crate) fn replaces_composer(s: &App) -> bool {
    gone(s).is_some()
}

fn subject(s: &App) -> Option<Subject> {
    let (id, directory) = gone(s)?;
    let project = s.session(id).map(|i| i.project.clone()).filter(|p| !p.is_empty()).or_else(|| s.project_at(directory).map(|p| p.id.clone()));
    Some(Subject { session: id.to_owned(), directory: directory.to_owned(), project })
}

/// Whether the server's event stream is up (`connection.status() === "connected"`).
fn connected(s: &App, server: &str) -> bool {
    use opencode_kit::servers::link::Link;
    s.live() && matches!(crate::reconnect::link_of(s, server), Link::Online | Link::Suspect { .. })
}

/// The page's folder or the stream changed (`location.tsx`'s effect on `ref()` and `connected()`).
fn retarget(s: &mut App, connected: Option<bool>) -> Effects<sync::Effect> {
    let now = *s.clock;
    let st = s.kernel.slots.get(&LOCATION_MISSING);
    let target = folder(s).map(|(_, server, directory)| (server, directory, connected.unwrap_or_else(|| self::connected(s, server))));
    let same = match (&st.sync, target) {
        (None, None) => true,
        (Some(y), Some((server, directory, up))) => y.location.server == server && y.location.directory == directory && st.connected == up,
        _ => false,
    };
    if same {
        return Effects::none();
    }
    let target = target.map(|(server, directory, up)| (sync::Location { server: server.to_owned(), directory: directory.to_owned() }, up));
    let st = s.kernel.slots.get_mut(&LOCATION_MISSING);
    let Some((location, up)) = target else {
        st.sync = None;
        return Effects::none();
    };
    let known_missing = st.known.contains(&location);
    st.connected = up;
    let check = st.sync.get_or_insert_with(|| sync::Check::new(location.clone(), known_missing));
    sync::step(check, sync::Event::Target { location, connected: up, known_missing, now })
}

/// Before a launch's first frame: the folders known gone, and the saved page's among them shows
/// its card at once. Its server is asked when its stream opens (`poll`).
pub(crate) fn restore(s: &mut App) {
    if !s.persists() {
        return;
    }
    let known: Vec<sync::Location> = crate::storage::json(GLOBAL, KEY)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| Some(sync::Location { server: v["server"].as_str()?.to_owned(), directory: v["directory"].as_str()?.to_owned() }))
        .collect();
    if known.is_empty() {
        return;
    }
    s.kernel.slots.get_mut(&LOCATION_MISSING).known = known;
    // Not connected yet, so nothing is asked.
    let _ = retarget(s, Some(false));
    if mount(s) == Some(true) {
        s.composer.focused = false;
    }
}

/// The stream came back (`Incoming::Reconnected`): the folder is asked about again, the card
/// staying meanwhile. Told as the stream dropping and returning, which one poll may miss.
pub(crate) fn reconnected(s: &mut App, tx: &mut Tx) {
    let _ = retarget(s, Some(false));
    let effects = retarget(s, None);
    run_sync(s, effects, tx);
}

fn wake(s: &App, at: f64, tx: &mut Tx) {
    tx.wake_after(Duration::from_secs_f64((at - *s.clock).max(0.0)));
}

enum Answer {
    Found,
    Missing,
    Failed,
}

fn run_sync(s: &mut App, effects: Effects<sync::Effect>, tx: &mut Tx) {
    for effect in effects {
        match effect {
            sync::Effect::WakeAt(at) => wake(s, at, tx),
            // Off this thread: the server looks at its disk, which a dead network drive can hold.
            sync::Effect::Ask { attempt, location } => s.job(move |srv| {
                let answer = match srv.get(&format!("/api/location?location%5Bdirectory%5D={}", crate::switcher::encode(&location.directory))) {
                    Ok(_) => Answer::Found,
                    Err(WireError { kind: WireKind::Status { tag: Some(tag), .. }, .. }) if tag == "LocationNotFoundError" => Answer::Missing,
                    Err(_) => Answer::Failed,
                };
                Box::new(move |s: &mut App, tx: &mut Tx| answered(s, attempt, answer, tx)) as Apply
            }),
        }
    }
}

fn answered(s: &mut App, attempt: Attempt, answer: Answer, tx: &mut Tx) {
    let now = *s.clock;
    let event = match answer {
        Answer::Found => sync::Event::Found { attempt },
        Answer::Missing => sync::Event::Missing { attempt },
        Answer::Failed => sync::Event::Failed { attempt, now },
    };
    step_sync(s, event, tx);
}

/// Feeds the folder's check `event`, runs what it asks for, and keeps what it learned.
fn step_sync(s: &mut App, event: sync::Event, tx: &mut Tx) -> bool {
    let Some(before) = s.kernel.slots.get(&LOCATION_MISSING).sync.as_ref().map(|y| y.state) else { return false };
    let st = s.kernel.slots.get_mut(&LOCATION_MISSING);
    let Some(check) = st.sync.as_mut() else { return false };
    let effects = sync::step(check, event);
    let (state, location) = (check.state, check.location.clone());
    if state == before {
        return false;
    }
    let listed = st.known.iter().position(|l| *l == location);
    let changed = match (state, listed) {
        (sync::State::Missing, None) => {
            st.known.push(location);
            let over = st.known.len().saturating_sub(KNOWN_MAX);
            st.known.drain(..over);
            true
        }
        (sync::State::Synced, Some(at)) => {
            st.known.remove(at);
            true
        }
        _ => false,
    };
    if changed && s.persists() {
        let known: Vec<Value> = s.kernel.slots.get(&LOCATION_MISSING).known.iter().map(|l| json!({ "server": l.server, "directory": l.directory })).collect();
        crate::storage::set_json(GLOBAL, KEY, &Value::Array(known));
    }
    tx.instant();
    run_sync(s, effects, tx);
    true
}

/// The card's steps start when it takes the composer's place and end when it leaves (the
/// component mounting and unmounting). Whether it came (`true`) or went, when either.
fn mount(s: &mut App) -> Option<bool> {
    let subject = subject(s);
    let st = s.kernel.slots.get(&LOCATION_MISSING);
    if st.card.as_ref().map(|(k, _)| k) == subject.as_ref() {
        return None;
    }
    let st = s.kernel.slots.get_mut(&LOCATION_MISSING);
    st.card = subject.map(|subject| (subject, card::Card::default()));
    st.menu = Menu::default();
    Some(st.card.is_some())
}

/// Each tick: the page's folder (asked about when it changes or its server's stream comes up),
/// the timers, and the card coming and going.
fn poll(s: &mut App, _now: f64, tx: &mut Tx) -> bool {
    let effects = retarget(s, None);
    let mut changed = !effects.is_empty();
    run_sync(s, effects, tx);
    let now = *s.clock;
    if s.kernel.slots.get(&LOCATION_MISSING).sync.as_ref().is_some_and(|y| y.due(now)) {
        changed |= step_sync(s, sync::Event::Tick { now }, tx);
    }
    if let Some(came) = mount(s) {
        tx.instant();
        // No composer under the card takes keys; back, it takes them as a page that opens does.
        s.composer.focused = !came && s.has_composer() && !dialog_open(s) && crate::auto_focus_composer(tx);
        changed = true;
    }
    let Some(card) = s.kernel.slots.get(&LOCATION_MISSING).card.as_ref().map(|(_, c)| *c) else { return changed };
    if card.due(now) {
        changed |= dispatch(s, card::Event::Tick { now }, tx);
    }
    if card.state == (card::State::Idle { restore_focus: true }) && !dialog_open(s) {
        changed |= dispatch(s, card::Event::DialogChanged { open: false }, tx);
    }
    changed
}

/// Whether a dialog is open over the card (`dialog.active`).
fn dialog_open(s: &App) -> bool {
    s.settings_open() || !s.kernel.overlays.is_empty()
}

/// Feeds the card `event` and runs what it asks for. True when it changed.
pub(super) fn dispatch(s: &mut App, event: card::Event, tx: &mut Tx) -> bool {
    let Some((subject, before)) = s.kernel.slots.get(&LOCATION_MISSING).card.clone() else { return false };
    let mut next = before;
    let effects = card::step(&mut next, event);
    if next == before && effects.is_empty() {
        return false;
    }
    let st = s.kernel.slots.get_mut(&LOCATION_MISSING);
    if let Some((_, card)) = st.card.as_mut() {
        *card = next;
    }
    if before.menu_open() && !next.menu_open() {
        st.menu = Menu::default();
    }
    tx.instant();
    for effect in effects {
        run_card(s, &subject, effect, tx);
    }
    true
}

fn run_card(s: &mut App, subject: &Subject, effect: card::Effect, tx: &mut Tx) {
    match effect {
        card::Effect::WakeAt(at) => wake(s, at, tx),
        card::Effect::FocusChoose => tx.focus(CHOOSE, tx.focus_visible()),
        card::Effect::ToastWorktreesFailed => crate::status_toast::fail(s, t!("session.location.worktreesFailed"), ""),
        card::Effect::ToastMoveFailed { error } => crate::status_toast::fail(s, t!("session.location.moveFailed"), error),
        card::Effect::WorktreesLoad { attempt } => load_worktrees(s, subject, attempt),
        card::Effect::PickerOpen { attempt } => {
            let server = crate::servers::runtime::active(s).clone();
            let picked: OnPick = std::rc::Rc::new(move |h: &mut dyn ProjectsHost, directory: String, tx: &mut Tx| h.session_location_picked(attempt, Some(directory), tx));
            let cancelled: OnCancel = std::rc::Rc::new(move |h: &mut dyn ProjectsHost, tx: &mut Tx| h.session_location_picked(attempt, None, tx));
            // The server's own folders for a server that isn't this computer's (`useDirectoryPicker`).
            crate::projects::remote_picker::pick_settling(s, &server, t!("session.location.choose").to_owned(), tx, picked, Some(cancelled));
        }
        card::Effect::MoveRun { attempt, directory } => run_move(s, subject, attempt, directory),
    }
}

/// `worktrees.load`: the server looks for the project's worktrees again, then lists them.
fn load_worktrees(s: &mut App, subject: &Subject, attempt: Attempt) {
    let loaded = move |listed: bool| Box::new(move |s: &mut App, tx: &mut Tx| { dispatch(s, card::Event::WorktreesLoaded { attempt, listed }, tx); }) as Apply;
    let Some(project) = subject.project.clone().filter(|_| s.live()) else { return s.later(loaded(true)) };
    let root = s.project(&project).map(|p| p.directory.clone()).unwrap_or_default();
    let refresh = s.can_write();
    s.job(move |srv| {
        if refresh {
            srv.write("POST", "/api/worktree/refresh", Some(json!({ "projectID": project }))).ok();
        }
        let listed = srv.get(&format!("/api/worktree?projectID={}", crate::switcher::encode(&project))).ok().map(|v| (parse_worktrees(&v, &root), parse_managed_worktrees(&v, &root)));
        Box::new(move |s: &mut App, tx: &mut Tx| {
            let ok = listed.is_some();
            if let Some((directories, managed)) = listed {
                s.worktrees_listed(project, directories, managed);
            }
            if dispatch(s, card::Event::WorktreesLoaded { attempt, listed: ok }, tx) {
                menu_listed(s, tx);
            }
        }) as Apply
    });
}

/// `move.run`: a new worktree of the project first when no folder was picked, then the session
/// moves (`POST /api/session/:id/move`), and its info is read again so the card gives way even
/// when the server's event lags.
fn run_move(s: &mut App, subject: &Subject, attempt: Attempt, directory: Option<String>) {
    let session = subject.session.clone();
    // The missing worktree can't resolve its own project, so a new one is made from the project's
    // saved checkout.
    let checkout = subject.project.as_ref().and_then(|id| s.project(id)).map(|p| (p.id.clone(), p.directory.clone()));
    if !s.live() {
        return;
    }
    s.job(move |srv| {
        let made = directory.is_none();
        let moved = match (directory, checkout.as_ref()) {
            (Some(directory), _) => Ok(directory),
            (None, Some((project, from))) => srv.create_worktree(project, from, None),
            // The project hasn't loaded yet, so there is no checkout to make the worktree from.
            (None, None) => Err(WireError::new("POST", "/api/worktree", WireKind::Other(t!("common.requestFailed").to_owned()))),
        }
        .and_then(|directory| srv.move_session(&session, &directory));
        Box::new(move |s: &mut App, tx: &mut Tx| {
            if made && let Some((project, _)) = &checkout {
                s.worktree_updated(project);
            }
            let event = match moved {
                Ok(()) => {
                    s.fetch_info(&session);
                    card::Event::MoveSucceeded { attempt, now: *s.clock }
                }
                Err(e) => card::Event::MoveFailed { attempt, error: failure(e), dialog_open: dialog_open(s) },
            };
            dispatch(s, event, tx);
        }) as Apply
    });
}

/// The server's own words when it gave them, else what the client knows.
fn failure(e: WireError) -> String {
    match &e.kind {
        WireKind::Status { message: Some(m), .. } | WireKind::Other(m) => m.clone(),
        _ => e.to_string(),
    }
}

/// `picker.settled`: the folder picked, or none.
pub(crate) fn picker_settled(s: &mut App, attempt: Attempt, directory: Option<String>, tx: &mut Tx) {
    let dialog_open = dialog_open(s);
    dispatch(s, card::Event::PickerSettled { attempt, directory, dialog_open }, tx);
}

/// The project's other worktrees the menu can list: those OpenCode made (the inventory's entries
/// with a `strategy`), never the missing folder itself.
pub(super) fn others(s: &App, subject: &Subject) -> Vec<String> {
    let Some(project) = &subject.project else { return Vec::new() };
    let gone = opencode_servers::home::same_directory_key(&subject.directory);
    s.managed_worktrees(project).into_iter().filter(|d| opencode_servers::home::same_directory_key(d) != gone).collect()
}

/// `selectLocationMissingView`'s `visibleWorktrees`: the first ten whose folder's name holds the
/// query.
pub(super) fn visible(others: &[String], query: &str) -> Vec<String> {
    let query = query.trim().to_lowercase();
    others.iter().filter(|d| query.is_empty() || name(d).to_lowercase().contains(&query)).take(10).cloned().collect()
}

/// A worktree's row reads its folder's name (`getFilename`).
pub(super) fn name(directory: &str) -> &str {
    opencode_kit::path::basename(directory)
}

/// The menu has a search field from ten other worktrees (`searchable`).
pub(super) fn searchable(others: &[String]) -> bool {
    others.len() >= 10
}

/// The menu's rows by key: New worktree (the empty key), then the worktrees on screen.
fn entries(s: &App) -> Vec<String> {
    let st = s.kernel.slots.get(&LOCATION_MISSING);
    let Some((subject, _)) = &st.card else { return Vec::new() };
    std::iter::once(String::new()).chain(visible(&others(s, subject), &st.menu.search.text)).collect()
}

/// The lit row: the one chosen while it's listed, else New worktree.
pub(super) fn active(s: &App) -> String {
    let entries = entries(s);
    s.kernel.slots.get(&LOCATION_MISSING).menu.active.resolve(entries.iter().map(String::as_str)).unwrap_or_default()
}

/// Choose worktree: opens its menu, or closes it. Focus goes to the search field, or to the list
/// without one.
pub(super) fn toggle_menu(s: &mut App, tx: &mut Tx) {
    let Some((subject, card)) = s.kernel.slots.get(&LOCATION_MISSING).card.clone() else { return };
    if card.menu_open() {
        return close_menu(s, true, tx);
    }
    let (by_keys, now) = (tx.focus_visible(), *s.clock);
    if !dispatch(s, card::Event::MenuToggled { open: true, now }, tx) {
        return;
    }
    let field = searchable(&others(s, &subject));
    let menu = &mut s.kernel.slots.get_mut(&LOCATION_MISSING).menu;
    *menu = Menu { by_keys, ..Menu::default() };
    menu.search.focused = field;
    if !field {
        tx.focus(LIST, false);
    }
}

/// Worktrees can load after the menu opens, so the field takes focus from the list when it
/// comes.
fn menu_listed(s: &mut App, tx: &mut Tx) {
    let Some((subject, card)) = s.kernel.slots.get(&LOCATION_MISSING).card.clone() else { return };
    if card.menu_open() && tx.focused() == Some(LIST) && searchable(&others(s, &subject)) {
        s.kernel.slots.get_mut(&LOCATION_MISSING).menu.search.focused = true;
    }
}

/// Closes the menu. Keys hand focus back to its trigger, ringed if keys opened it; a press
/// outside leaves focus where it landed (`createOverlayFocus`).
pub(super) fn close_menu(s: &mut App, to_trigger: bool, tx: &mut Tx) {
    let (by_keys, now) = (s.kernel.slots.get(&LOCATION_MISSING).menu.by_keys, *s.clock);
    if dispatch(s, card::Event::MenuToggled { open: false, now }, tx) && to_trigger {
        tx.focus(TRIGGER, by_keys);
    }
}

/// A row: the session moves to that worktree, or to a new one for the empty key.
pub(super) fn choose(s: &mut App, key: &str, tx: &mut Tx) {
    dispatch(s, card::Event::MoveWorktree { directory: (!key.is_empty()).then(|| key.to_owned()) }, tx);
}

pub(super) fn choose_directory(s: &mut App, tx: &mut Tx) {
    dispatch(s, card::Event::ChooseRequested, tx);
}

/// The search text changed: a query lights its first match, so Enter after typing picks one; an
/// empty one falls back to New worktree.
pub(super) fn searched(s: &mut App, tx: &mut Tx) {
    tx.instant();
    let first = entries(s).into_iter().nth(1).filter(|_| !s.kernel.slots.get(&LOCATION_MISSING).menu.search.text.trim().is_empty());
    let menu = &mut s.kernel.slots.get_mut(&LOCATION_MISSING).menu;
    match first {
        Some(key) => menu.active.set(key),
        None => menu.active.clear(),
    }
}

/// The pointer moved on a row: it lights it, and focus stays in the search field (or on the
/// list). Only a moving pointer does, so rows the keys scroll under a still one keep the
/// highlight where the keys put it (opencode#66092).
pub(super) fn pointed(s: &mut App, key: &str, ev: PointerEvent, tx: &mut Tx) {
    let at = [ev.window_x, ev.window_y];
    let menu = &s.kernel.slots.get(&LOCATION_MISSING).menu;
    if menu.pointer == Some(at) || menu.active.pointed(key) {
        return tx.live_only();
    }
    let menu = &mut s.kernel.slots.get_mut(&LOCATION_MISSING).menu;
    menu.pointer = Some(at);
    menu.active.step(HighlightEvent::Pointer(key.to_owned()));
    tx.instant();
}

/// The menu's keys, from its search field or its list (`onListKeyDown`).
pub(super) fn menu_key(s: &mut App, ev: &rune::KeyEvent, tx: &mut Tx) -> bool {
    use rune::Key;
    let Some((subject, card)) = s.kernel.slots.get(&LOCATION_MISSING).card.clone() else { return false };
    if !card.menu_open() {
        return false;
    }
    let plain = !ev.mods.shift && !ev.mods.ctrl && !ev.mods.alt && !ev.mods.cmd;
    let entries = entries(s);
    let lit = active(s);
    let at = entries.iter().position(|k| *k == lit).unwrap_or(0);
    let light = |s: &mut App, tx: &mut Tx, i: usize| {
        tx.instant();
        tx.scroll_into_view_in(SCROLL, view::row_key(&entries[i]));
        s.kernel.slots.get_mut(&LOCATION_MISSING).menu.active.set(entries[i].clone());
    };
    let field = searchable(&others(s, &subject));
    match &ev.key {
        Key::Escape => close_menu(s, true, tx),
        // The list is one stop: Tab closes it and focus goes on from its trigger.
        Key::Tab => {
            close_menu(s, false, tx);
            tx.focus(if ev.mods.shift { TRIGGER } else { CHOOSE }, true);
        }
        Key::Down if plain => light(s, tx, (at + 1) % entries.len()),
        Key::Up if plain => light(s, tx, (at + entries.len() - 1) % entries.len()),
        Key::Enter => choose(s, &entries[at], tx),
        // Without a search field the list itself has the keys.
        Key::Home if !field => light(s, tx, 0),
        Key::End if !field => light(s, tx, entries.len() - 1),
        Key::Text(t) if !field && t == " " => choose(s, &entries[at], tx),
        _ => return false,
    }
    true
}

/// Kernel feature registration for the card.
pub struct LocationMissingFeature;

impl Feature<App> for LocationMissingFeature {
    fn id(&self) -> &'static str {
        "feature.location_missing"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&LOCATION_MISSING);
        builder.schedule(crate::tick::stage::DERIVE_COMPOSER, "location_missing.poll", poll);
        builder.register_overlay(opencode_kernel::OverlayRegistration::<App>::new("overlay.location_worktrees").with_render(crate::overlays::layer::SURFACE_POPOVER, |s, cx| {
            let p = crate::palette::pal(s.dark(cx));
            view::menu(s, p).into_iter().flatten().collect()
        }));
    }
}

/// A scene's session with its folder gone, in a Git project with `worktrees` others.
fn scene(s: &mut App, worktrees: usize) -> Option<Subject> {
    crate::scenes::open_scene(s);
    let Route::Session { id, server } = (*s.route).clone() else { return None };
    let info = s.session(&id)?.clone();
    let (root, name) = s.project(&info.project).map(|p| (p.directory.clone(), p.name.clone())).unwrap_or_default();
    let directory = format!("{}/.worktrees/removed-worktree", root.trim_end_matches('/'));
    if let Some(x) = s.sessions.iter_mut().find(|x| x.id == id) {
        x.directory = directory.clone();
    }
    // The project is a Git checkout, so Choose worktree shows.
    let _ = opencode_kit::model::parse_project(&json!({ "id": info.project, "canonical": root, "name": name, "vcs": "git" }));
    let made: Vec<String> = (1..=worktrees).map(|n| format!("{}/.worktrees/feature-{n:02}", root.trim_end_matches('/'))).collect();
    s.worktrees_listed(info.project.clone(), made.clone(), made);
    let location = sync::Location { server: server.as_str().to_owned(), directory };
    let st = s.kernel.slots.get_mut(&LOCATION_MISSING);
    st.connected = false;
    st.sync = Some(sync::Check::new(location, true));
    let _ = mount(s);
    s.composer.focused = false;
    s.kernel.slots.get(&LOCATION_MISSING).card.as_ref().map(|(subject, _)| subject.clone())
}

fn scene_card(s: &mut App, worktrees: usize, state: card::State) {
    if scene(s, worktrees).is_some()
        && let Some((_, card)) = s.kernel.slots.get_mut(&LOCATION_MISSING).card.as_mut()
    {
        card.state = state;
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("location-missing", |s| { scene(s, 3); }),
    ("location-missing-menu", |s| scene_card(s, 3, card::State::MenuReady)),
    ("location-missing-search", |s| scene_card(s, 12, card::State::MenuReady)),
    ("location-missing-moving", |s| scene_card(s, 3, card::State::Moving { attempt: Attempt(1), source: card::Source::Directory })),
]);
