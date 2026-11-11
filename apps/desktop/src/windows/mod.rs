//! The app's windows, all in this process as Desktop's are in its main process
//! (`packages/desktop/src/main/windows`): each has its own tabs, page and layout, and they share
//! the server connection's event stream (`opencode_kit::client::EventHub`) and one copy of the
//! session data it brings (`peer`), and the plugins' app halves (`plugin_host`).
//!
//! New Window (Ctrl+Shift+N, the ≡ menu) opens one with Rune's `open_window`; Close Window
//! (Ctrl+Shift+W) closes the one in front (`close`). Each keeps its tabs, closed tabs, panes, page and
//! drafts under its own id (`store`), and every window open at quit opens again where it was.
//! A tab dragged off the strip opens in a window of its own under the pointer (`tear`).

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, Sender};

use opencode_kit::client::{Incoming, Request, Write};
use rune::{AppHandle, WindowHandle, WindowPlacement};

use crate::App;

pub mod close;
mod life;
mod open;
pub mod peer;
pub mod store;
pub mod tear;
pub(crate) mod tear_stories;

pub use open::{launch, new_window};

/// One window's identity and what it tracks of itself.
#[derive(Default)]
pub struct Window {
    /// Its persistent id (`opencode.window.<id>.dat`); empty for headless runs and tests, which
    /// keep nothing.
    pub id: String,
    /// The one window of a profile that never saved a window list: Desktop's tabs come into it
    /// (`crate::seed`).
    pub fresh: bool,
    /// Its frame while neither maximized nor full screen, what restoring it returns to.
    normal: Option<WindowPlacement>,
    /// When `normal` was last read (`App::clock`).
    read_at: f64,
    /// The frame last saved.
    saved: Option<store::Frame>,
    /// The tab strip's rect in window points, where a tab dropped from another window lands.
    pub strip: Option<[f32; 4]>,
    /// A tab being dragged out of this window's strip into a window of its own.
    pub tear: tear::Tear,
    /// Tabs the window opened with (a torn-out tab), and their drafts.
    pub seed: Option<open::Seed>,
    /// How it starts its life: open, or a torn-out tab's window following the pointer.
    pub life: life::Life,
    /// The last window, hidden by Close Window on macOS while the app keeps running.
    pub hidden: bool,
}

impl Window {
    /// A window with its own rows.
    pub fn new(id: String, fresh: bool) -> Window {
        Window { id, fresh, ..Window::default() }
    }

    /// A window that keeps nothing: headless runs and tests.
    pub fn unsaved() -> Window {
        Window::default()
    }
}

/// A hot build's window: the profile's first saved window (a new one when none was saved), so
/// it reads and writes the rows the app's first window keeps (`opencode.window.<id>.dat`).
#[cfg(feature = "hot")]
pub fn hot() -> Window {
    let saved = store::saved_ids();
    let mut ids = saved.clone().unwrap_or_default();
    if ids.is_empty() {
        ids.push(store::new_id());
        store::save_ids(&ids);
    }
    Window::new(ids.swap_remove(0), saved.is_none())
}

/// An open window: its handles, where its strip is, and its life.
struct Open {
    id: String,
    handle: WindowHandle,
    app: AppHandle<App>,
    strip: Option<[f32; 4]>,
    life: life::Life,
    /// When it last came to the front (`FOCUSES`): of windows under one point, the latest is on top.
    focused: u64,
}

thread_local! {
    /// Every open window, in the order they opened (the UI thread's, which all windows share).
    static OPEN: RefCell<Vec<Open>> = const { RefCell::new(Vec::new()) };
    /// Counts windows coming to the front.
    static FOCUSES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static QUIT: RefCell<QuitSave> = const { RefCell::new(QuitSave::Idle) };
}

/// The quit's save: Rune runs `on_quit` in every open window, one after another; each queues
/// what it keeps, and the last lands all of it in one transaction (a Windows session may end
/// the process once the hooks returned).
#[derive(Debug, Default, PartialEq, Eq)]
enum QuitSave {
    #[default]
    Idle,
    /// The windows whose `on_quit` hasn't run yet.
    Saving { left: Vec<String> },
}

enum QuitEvent {
    /// Window `id`'s `on_quit` queued its rows; `open` are the windows open now.
    Saved { id: String, open: Vec<String> },
}

#[derive(Debug, PartialEq, Eq)]
enum QuitEffect {
    None,
    /// Every window queued: land it all.
    Flush,
}

impl QuitSave {
    fn step(self, event: QuitEvent) -> (QuitSave, QuitEffect) {
        let QuitEvent::Saved { id, open } = event;
        let left: Vec<String> = match self {
            QuitSave::Idle => open.into_iter().filter(|w| *w != id).collect(),
            QuitSave::Saving { left } => left.into_iter().filter(|w| *w != id && open.contains(w)).collect(),
        };
        if left.is_empty() {
            return (QuitSave::Idle, QuitEffect::Flush);
        }
        (QuitSave::Saving { left }, QuitEffect::None)
    }
}

/// Window `id` queued what it keeps for the quit; the last window to do so lands every window's
/// rows in one transaction and waits for it.
pub fn quit_saved(id: &str) {
    let open = OPEN.with(|o| o.borrow().iter().map(|w| w.id.clone()).collect());
    let effect = QUIT.with(|q| {
        let state = std::mem::take(&mut *q.borrow_mut());
        let (next, effect) = state.step(QuitEvent::Saved { id: id.to_owned(), open });
        *q.borrow_mut() = next;
        effect
    });
    if effect == QuitEffect::Flush {
        crate::storage::flush(opencode_storage::FlushReason::Quit, true);
    }
}

/// Window `id` is on screen: other windows can reach it from now on.
pub fn register(id: &str, handle: WindowHandle, app: AppHandle<App>, life: life::Life) {
    OPEN.with(|o| {
        let mut o = o.borrow_mut();
        o.retain(|w| w.id != id);
        o.push(Open { id: id.to_owned(), handle, app, strip: None, life, focused: 0 });
    });
    focused(id);
}

/// Window `id` came to the front.
pub fn focused(id: &str) {
    let n = FOCUSES.with(|f| {
        f.set(f.get() + 1);
        f.get()
    });
    OPEN.with(|o| o.borrow_mut().iter_mut().filter(|w| w.id == id).for_each(|w| w.focused = n));
}

/// Steps window `id`'s life (`life::Life`); the list saved for the next launch follows.
/// Returns what the window does itself (keeping its frame).
pub fn life(id: &str, event: life::LifeEvent) -> life::LifeEffect {
    let effect = OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let w = o.iter_mut().find(|w| w.id == id)?;
        let (next, effect) = w.life.step(event);
        w.life = next;
        o.retain(|w| w.life != life::Life::Closed);
        Some(effect)
    }).unwrap_or(life::LifeEffect::None);
    match effect {
        life::LifeEffect::SaveIds => store::save_ids(&saved_ids()),
        life::LifeEffect::Forget => {
            store::save_ids(&saved_ids());
            store::forget(id);
        }
        life::LifeEffect::None | life::LifeEffect::Keep => {}
    }
    effect
}

/// The windows that open again next launch: the open ones, in the order they opened.
fn saved_ids() -> Vec<String> {
    OPEN.with(|o| o.borrow().iter().filter(|w| w.life.saved()).map(|w| w.id.clone()).collect())
}

/// Window `id`'s native handle, while it's open.
pub fn handle(id: &str) -> Option<WindowHandle> {
    OPEN.with(|o| o.borrow().iter().find(|w| w.id == id).map(|w| w.handle.clone()))
}

/// A window's own reads and writes on `server`. The event stream is its server's, which every
/// window on it shares (`peer`).
pub fn connect(server: &opencode_kit::client::Server, wake: rune::Waker) -> (Sender<Request>, Sender<Write>, Receiver<Incoming>) {
    opencode_kit::client::connect_workers(server.clone(), wake)
}


impl App {
    /// Notes the strip's rect for drops from other windows: laid out in the view's points, kept
    /// in the window's (times the zoom), as other windows find it.
    pub(crate) fn strip_laid_out(&mut self, rect: [f32; 4]) {
        let rect = rect.map(|v| v * self.zoom);
        self.window.strip = Some(rect);
        let id = self.window.id.clone();
        OPEN.with(|o| o.borrow_mut().iter_mut().filter(|w| w.id == id).for_each(|w| w.strip = Some(rect)));
    }

    /// `poll`: twice a second at most, reads where the window is and saves it when it moved
    /// (written behind, coalesced), so a restart, an update or a crash reopens it there too.
    pub(crate) fn poll_window(&mut self) {
        self.poll_tear();
        if self.window.strip.is_some() && self.tab_layout() == crate::layout_state::TabLayout::Inbox {
            // The inbox has no strip to drop a tab on.
            self.window.strip = None;
            let id = self.window.id.clone();
            OPEN.with(|o| o.borrow_mut().iter_mut().filter(|w| w.id == id).for_each(|w| w.strip = None));
        }
        if self.window.id.is_empty() || !self.persists() || (self.clock - self.window.read_at).abs() < 0.5 {
            return;
        }
        self.window.read_at = self.clock;
        let Some(frame) = self.window_frame() else { return };
        if self.window.saved != Some(frame) {
            self.window.saved = Some(frame);
            store::save_frame(&self.window.id, frame);
        }
    }

    /// Where the window is, with the frame restoring returns to while it's maximized.
    fn window_frame(&mut self) -> Option<store::Frame> {
        let p = handle(&self.window.id)?.placement()?;
        if p.minimized {
            return None;
        }
        if !p.maximized && !p.full_screen {
            self.window.normal = Some(p);
        }
        let normal = self.window.normal.unwrap_or(p);
        Some(store::Frame {
            x: Some(normal.position.0),
            y: Some(normal.position.1),
            width: normal.content_size.0,
            height: normal.content_size.1,
            maximized: p.maximized,
            full_screen: p.full_screen,
            display: None,
        })
    }

    /// The window is closing. Unless it's the last one open or the app is quitting
    /// (`rune::quitting`: macOS Quit, the end of a Windows session), it's forgotten, files and
    /// all (Desktop's `registry.closed`); otherwise it opens again next launch where it was.
    pub(crate) fn window_closed(&mut self) {
        let id = self.window.id.clone();
        if id.is_empty() {
            return;
        }
        let frame = self.window_frame();
        if !self.persists() {
            OPEN.with(|o| o.borrow_mut().retain(|w| w.id != id));
            return;
        }
        let last = OPEN.with(|o| !o.borrow().iter().any(|w| w.id != id && w.life.saved()));
        let effect = life(&id, life::LifeEvent::Closing { keep: rune::quitting() || last });
        life(&id, life::LifeEvent::Closed);
        if effect != life::LifeEffect::Keep {
            return;
        }
        if let Some(frame) = frame {
            store::save_frame(&id, frame);
        }
    }

    /// The app starts to quit (`App::on_quit`, in every open window): the open windows' ids,
    /// this window's frame and everything it keeps go out now, so a quit, an update's restart or
    /// a Windows logoff opens every window again where it was.
    pub(crate) fn window_quitting(&mut self) {
        if self.window.id.is_empty() || !self.persists() {
            return;
        }
        store::save_ids(&saved_ids());
        if let Some(frame) = self.window_frame() {
            self.window.saved = Some(frame);
            store::save_frame(&self.window.id, frame);
        }
    }
}
