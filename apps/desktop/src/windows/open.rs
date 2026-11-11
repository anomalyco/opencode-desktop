//! Opening windows: every window saved at the last quit at launch (Desktop's `restore`), and a
//! new one in the running app (`window.new`, a torn-out tab).

use std::collections::HashMap;

use session_view::composer::mode::Mode;

use rune::{AppSpec, Display, WindowHandle, WindowSpec};

use super::peer;
use super::store::{self, Frame};
use super::{Window, register};
use crate::tabs::Tab;
use crate::{App, Route};

/// Desktop's window size when none was saved.
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);

/// What a window opened from another starts with: the tabs it moves, each with its server, and
/// their drafts. It shows them at once from their servers' shared copies (`servers::runtime`).
pub struct Seed {
    pub tabs: Vec<Tab>,
    pub route: Route,
    pub drafts: HashMap<String, String>,
    /// The moving drafts' attachments, quotes and pills.
    parked: HashMap<String, crate::restore::Parked>,
    /// The moving drafts in shell mode, which they keep.
    shell: Vec<String>,
    default_dir: String,
}

impl Seed {
    /// `tabs` (with their drafts) moving out of `s` into the new window, `route` showing, in
    /// `s`'s session data.
    pub fn from(s: &App, tabs: Vec<Tab>, route: Route) -> Seed {
        Seed {
            drafts: tabs.iter().filter_map(|t| Some((t.id.clone(), s.draft_text(&t.id)?))).collect(),
            parked: tabs.iter().filter_map(|t| Some((t.id.clone(), s.draft_parked(&t.id)?))).collect(),
            shell: tabs.iter().filter(|t| s.draft_mode(&t.id) == Mode::Shell).map(|t| t.id.clone()).collect(),
            tabs,
            route,
            default_dir: s.default_dir.clone(),
        }
    }
}

impl App {
    /// The window's first state: its saved tabs and page, then what its opener gave it.
    pub(crate) fn new_in(mut w: Window) -> App {
        let seed = w.seed.take();
        let mut app = App::new_window(w);
        if let Some(seed) = seed {
            app.plant(seed);
        }
        app
    }

    fn plant(&mut self, seed: Seed) {
        self.default_dir = seed.default_dir;
        self.tabs = seed.tabs;
        self.restore.drafts.extend(seed.drafts);
        self.restore.parked.extend(seed.parked);
        for id in seed.shell {
            self.add.park(id, Mode::Shell);
        }
        self.route = seed.route;
        // Before the window's first turn: the page's server's copy comes in for this, and goes back.
        peer::in_turn(self, |s| {
            s.set_media_fetch();
            s.show_draft();
            s.save_tabs();
        });
    }

    /// Draft `id`'s mode, the open one's from the composer.
    pub(crate) fn draft_mode(&self, id: &str) -> Mode {
        if self.restore.shown.as_deref() == Some(id) {
            return self.add.mode;
        }
        self.add.parked().find(|(k, _)| *k == id).map_or(Mode::Normal, |(_, m)| m)
    }

    /// The composer words kept for tab `id`, the open one's from the composer.
    pub(crate) fn draft_text(&self, id: &str) -> Option<String> {
        let text = if self.restore.shown.as_deref() == Some(id) { Some(self.composer.text.clone()) } else { self.restore.drafts.get(id).cloned() };
        text.filter(|t| !t.trim().is_empty())
    }
}

/// Opens every window saved at the last quit, or one for a new profile, each where it was. The
/// list and the frames come from the boot packet, taken once the event loop is up (so the
/// storage's read runs under it); a list that didn't change isn't written again.
pub fn launch(diagnostics: crate::diagnostics::Diagnostics) -> usize {
    let opened = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = opened.clone();
    rune::run_windows(move |displays: &[Display]| {
        let saved = store::saved_ids();
        let mut ids = saved.clone().unwrap_or_default();
        if ids.is_empty() {
            ids.push(store::new_id());
        }
        if saved.as_ref() != Some(&ids) {
            store::save_ids(&ids);
        }
        count.set(ids.len());
        let fresh = saved.is_none();
        ids.iter()
            .enumerate()
            .map(|(i, id)| {
                let frame = store::load_frame(id).and_then(|f| fit(f, displays));
                window(Window::new(id.clone(), i == 0 && fresh), frame, &diagnostics).boxed()
            })
            .collect::<Vec<_>>()
    });
    opened.get()
}

/// New Window: another window of this app, on the same server, showing Home (Desktop's
/// `window.new`, a fresh window id with nothing saved).
pub fn new_window(s: &App) -> Option<WindowHandle> {
    open(s, Window::new(store::new_id(), false), None, false)
}

/// Opens `w` beside the open ones: at `frame` (else where the system puts a new window), and
/// `inactive`, without taking focus from the window the user works in.
pub(super) fn open(s: &App, w: Window, frame: Option<Frame>, inactive: bool) -> Option<WindowHandle> {
    if !s.persists() || s.window.id.is_empty() {
        return None;
    }
    // A torn-out tab's window joins the saved list only if it stays (`life`).
    if w.life.saved() {
        let mut ids = super::saved_ids();
        ids.push(w.id.clone());
        store::save_ids(&ids);
    }
    let spec = window(w, frame, &s.diagnostics);
    Some(rune::open_window(if inactive { spec.inactive() } else { spec }))
}

/// Window `w`'s spec: the app, at `frame` when it has one.
fn window(w: Window, frame: Option<Frame>, diagnostics: &crate::diagnostics::Diagnostics) -> WindowSpec<App> {
    let (id, life) = (w.id.clone(), w.life);
    let diagnostics = diagnostics.clone();
    let app_diagnostics = diagnostics.clone();
    let app = crate::rune_app(AppSpec { families: crate::appearance::launch_families(), ..crate::spec(App::new) }).init(move || {
        let mut app = App::new_in(w);
        app.diagnostics = app_diagnostics;
        app
    });
    let handle = app.handle();
    let ready = handle.clone();
    let mut motion_log = crate::motion_log::Log::default();
    let (encoded, drawn) = (diagnostics.clone(), diagnostics);
    let app = app
        .on_ready(move |_| {
            // The first frame is up: the storage's second stage is taken off the UI thread.
            crate::storage::presented();
            // Stream batches wake the window for a quiet poll, which draws only what they change.
            let wake = crate::launch::inbox_waker(ready.clone());
            ready.update_if(move |s, tx| {
                s.launch.inbox_wake = Some(wake);
                crate::launch::ready(s, tx)
            })
        })
        .on_frame_encoded(move |f| {
            encoded.encoded(f);
            crate::telemetry::encoded(f);
        })
        .on_frame_drawn(move |rt| {
            drawn.drawn(rt);
            if let Some(frame) = crate::telemetry::last_frame() {
                crate::telemetry::drawn(rt.state.painted_session(), frame);
                crate::telemetry::stream_drawn(&rt.state.window.id, frame.0);
            }
            crate::launch::trace::drawn(&rt.state);
            motion_log.frame(rt);
        })
        // Back on screen (the Dock icon, the Window menu): no longer the hidden last window.
        .on_focus(|s, focused, _| {
            s.window.hidden &= !focused;
            if focused {
                super::focused(&s.window.id);
            }
        })
        .on_quit(|s, _| s.save_on_quit())
        .on_close(|s, _| {
            s.updater.closing();
            s.save_on_close();
        });
    let mut spec = WindowSpec::from_app(format!("window-{id}"), app);
    if let Some(f) = frame {
        spec.options.position = f.x.zip(f.y);
        spec.options.physical_size = Some((f.width, f.height));
        spec = spec.maximized(f.maximized || f.full_screen);
    }
    spec.on_created(move |native| register(&id, native, handle, life))
}

/// `f`, while it still fits on one of the displays (Desktop's `resolveWindowState`); a
/// maximized window keeps that on the default size.
fn fit(f: Frame, displays: &[Display]) -> Option<Frame> {
    let within = |d: &Display| {
        let (x, y, w, h) = (d.position.0, d.position.1, d.size.0 as i32, d.size.1 as i32);
        // A frame saved without a place (Electron's is optional) opens where the system puts it.
        let (Some(fx), Some(fy)) = (f.x, f.y) else { return true };
        fx >= x - 16 && fy >= y - 16 && fx + f.width as i32 <= x + w + 16 && fy + f.height as i32 <= y + h + 16
    };
    if displays.iter().any(within) {
        return Some(f);
    }
    let primary = displays.iter().find(|d| d.primary).or(displays.first())?;
    let scale = primary.scale;
    let (w, h) = ((DEFAULT_SIZE.0 as f64 * scale) as u32, (DEFAULT_SIZE.1 as f64 * scale) as u32);
    let x = primary.position.0 + (primary.size.0 as i32 - w as i32).max(0) / 2;
    let y = primary.position.1 + (primary.size.1 as i32 - h as i32).max(0) / 2;
    Some(Frame { x: Some(x), y: Some(y), width: w, height: h, ..f })
}
