//! Mini apps: the server's widgets outside their sessions. "All widgets" lists every widget of
//! the server (the instances the widgets plugin keeps, not their templates), each live; the
//! dashboard holds those added from the list, live and interactive, on a grid that Edit lets you
//! rearrange as a tiling window manager would ([`grid`], [`board`]).
//!
//! It opens from the sidebar, under Search (Home, Search and New session are the sidebar's
//! places that aren't a session), and fills the page as Settings does, until the window shows
//! something else. The widgets come from the shared store next to the widget glue
//! (`opencode_feature_widgets::instances`), which follows them as agents update them, and run
//! there as the timeline's do: their calls go to their own session and its location.
//!
//! The layout is kept per server on this device, in the app's state (`opencode.global.dat`,
//! `miniApps:<server>`).

mod board;
mod grid;
mod view;

use std::collections::HashSet;

use opencode_kernel::{Feature, KernelBuilder, Slot};
use rune::Tx;

use crate::session::widgets::{WidgetsHost as _, instances};
use crate::{App, Route};

pub use view::page;

pub(crate) static MINI_APPS: Slot<MiniApps> = Slot::new();

/// The app's state, where the dashboard's layout is kept per server.
const GLOBAL: &str = "opencode.global.dat";
/// What [`opencode_feature_widgets::Widgets::set_outside`] knows this view by.
const OUTSIDE: &str = "mini-apps";

/// The Mini apps page and its dashboard.
#[derive(Default)]
pub struct MiniApps {
    pub page: Page,
    pub board: board::Board,
    /// Whose layout the board holds.
    layout: Layout,
    /// The list's rows in view, by instance key: their widgets run.
    seen: HashSet<String>,
    /// The dashboard's width as last laid out, in points.
    width: f32,
    /// The store's revision the tiles were last resolved at.
    resolved: u64,
}

/// Whether the page shows, and which of its views.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Page {
    #[default]
    Closed,
    /// Open over `route`: once the window shows another page, it's closed.
    Open { route: Route, view: View },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Dashboard,
    /// All widgets.
    All,
}

/// Which server's layout the board holds.
#[derive(Default, PartialEq)]
enum Layout {
    #[default]
    Unread,
    Read(String),
}

/// The view showing, while the page shows.
/// Settings › Experimental › Mini apps: off, nothing of Mini apps shows.
pub fn enabled(s: &App) -> bool {
    s.settings.mini_apps()
}

pub fn shown(s: &App) -> Option<View> {
    if !enabled(s) {
        return None;
    }
    match &s.kernel.slots.get(&MINI_APPS).page {
        Page::Open { route, view } if *route == *s.route && !s.settings_open() => Some(*view),
        _ => None,
    }
}

/// Opens the page at `view` (the sidebar's Mini apps), or closes it when it shows already.
pub fn toggle(s: &mut App, tx: &mut Tx) {
    if shown(s).is_some() {
        close(s);
    } else {
        open(s, View::Dashboard);
    }
    tx.settle();
}

pub fn open(s: &mut App, view: View) {
    s.close_settings();
    let route = (*s.route).clone();
    s.kernel.slots.get_mut(&MINI_APPS).page = Page::Open { route, view };
    instances::want(s);
}

pub fn close(s: &mut App) {
    let m = s.kernel.slots.get_mut(&MINI_APPS);
    m.page = Page::Closed;
    if m.board.mode.editing() {
        step(s, board::Event::Done);
    }
}

/// Adds the widget instance `key` (`sessionID:messageID:callID`) to this server's dashboard, at
/// the first free spot: the list's "Add to dashboard", and a session's "Move to dashboard".
pub fn add_to_dashboard(s: &mut App, key: &str) {
    load(s);
    step(s, board::Event::Add(key.to_owned()));
    instances::want(s);
}

/// Another window changed the layout kept for this server ("Move to dashboard"): read it again
/// at the next frame, unless it's being edited here.
pub fn reload(s: &mut App) {
    let m = s.kernel.slots.get_mut(&MINI_APPS);
    if !m.board.mode.editing() {
        m.layout = Layout::Unread;
    }
}

/// Takes the widget instance `key` off this server's dashboard.
pub fn remove_from_dashboard(s: &mut App, key: &str) {
    load(s);
    step(s, board::Event::Remove(key.to_owned()));
}

/// Whether widget instance `key` is on this server's dashboard.
pub fn on_dashboard(s: &App, key: &str) -> bool {
    s.kernel.slots.get(&MINI_APPS).board.tiles.iter().any(|t| t.key == key)
}

/// Steps the board and keeps the layout when it changed for good.
fn step(s: &mut App, ev: board::Event) {
    if let Some(board::Effect::Save) = board::step(&mut s.kernel.slots.get_mut(&MINI_APPS).board, ev) {
        save(s);
    }
}

fn server(s: &App) -> String {
    crate::servers::runtime::active(s).as_str().to_owned()
}

fn storage_key(server: &str) -> String {
    format!("miniApps:{server}")
}

/// Reads the server's layout into the board, unless it holds it already.
fn load(s: &mut App) {
    let server = server(s);
    if s.kernel.slots.get(&MINI_APPS).layout == Layout::Read(server.clone()) {
        return;
    }
    let tiles: Vec<grid::Tile> = crate::storage::json(GLOBAL, &storage_key(&server)).and_then(|v| serde_json::from_value(v["tiles"].clone()).ok()).unwrap_or_default();
    let tiles = grid::compact(&tiles.into_iter().map(grid::Tile::clamped).collect::<Vec<_>>());
    let m = s.kernel.slots.get_mut(&MINI_APPS);
    m.layout = Layout::Read(server);
    m.resolved = 0;
    board::step(&mut m.board, board::Event::Load(tiles));
}

fn save(s: &App) {
    let Layout::Read(server) = &s.kernel.slots.get(&MINI_APPS).layout else { return };
    let tiles = &s.kernel.slots.get(&MINI_APPS).board.tiles;
    crate::storage::set_json(GLOBAL, &storage_key(server), &serde_json::json!({ "tiles": tiles }));
}

/// Each frame: while the page shows, its server's layout is read, its tiles follow their widgets
/// as agents update them, and the widgets it shows run; when it doesn't, they rest.
fn poll(s: &mut App) -> bool {
    let Some(view) = shown(s) else {
        return s.widgets_mut().set_outside(OUTSIDE, HashSet::new());
    };
    load(s);
    // A widget an agent updated since: its tile shows the one that replaced it, where it was.
    let revision = s.widgets().instances.revision();
    if s.kernel.slots.get(&MINI_APPS).resolved != revision && s.widgets().instances.read() {
        let store = &s.widgets().instances;
        let tiles = s.kernel.slots.get(&MINI_APPS).board.tiles.clone();
        let mut moved = false;
        let next: Vec<grid::Tile> = tiles
            .into_iter()
            .map(|t| match store.current(&t.key) {
                Some(k) if k != t.key => {
                    moved = true;
                    grid::Tile { key: k, ..t }
                }
                _ => t,
            })
            .collect();
        s.kernel.slots.get_mut(&MINI_APPS).resolved = revision;
        if moved {
            step(s, board::Event::Load(next));
            save(s);
        }
    }
    let keys: Vec<String> = match view {
        View::Dashboard => s.kernel.slots.get(&MINI_APPS).board.tiles.iter().map(|t| t.key.clone()).collect(),
        View::All => s.kernel.slots.get(&MINI_APPS).seen.iter().cloned().collect(),
    };
    let mut running = HashSet::new();
    let mut changed = false;
    for key in keys {
        let known = s.widgets().instances.resolve(&key).is_some_and(|i| s.widgets().entry(&i.state.key()).is_some());
        if let Some(id) = instances::open(s, &key) {
            changed |= !known;
            running.insert(id);
        }
    }
    changed | s.widgets_mut().set_outside(OUTSIDE, running)
}

/// Kernel feature registration for Mini apps.
pub struct MiniAppsFeature;

impl Feature<App> for MiniAppsFeature {
    fn id(&self) -> &'static str {
        "feature.mini_apps"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&MINI_APPS);
        // After the widgets' own poll: a widget opened here starts on the next frame.
        builder.schedule(crate::tick::stage::INGEST_STREAM, "mini_apps.poll", |s, _, _| poll(s)).after("widgets.poll");
    }
}
