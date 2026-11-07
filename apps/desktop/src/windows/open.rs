//! Opening windows: every window saved at the last quit at launch (Desktop's `restore`), and a
//! new one in the running app (`window.new`, a torn-out tab).

use std::collections::HashMap;

use opencode_kit::model::{Inbox, Pending, Project, SessionInfo, Transcript};
use rune::{AppSpec, Display, WindowHandle, WindowSpec};

use super::store::{self, Frame};
use super::{Window, register};
use crate::tabs::Tab;
use crate::{App, Route};

/// Desktop's window size when none was saved.
const DEFAULT_SIZE: (f32, f32) = (1280.0, 800.0);

/// What a window opened from another starts with, so it shows its tabs at once: the opener's
/// session data, and the tabs it moves.
pub struct Seed {
    pub tabs: Vec<Tab>,
    pub route: Route,
    pub drafts: HashMap<String, String>,
    projects: Vec<Project>,
    sessions: Vec<SessionInfo>,
    running: std::collections::HashSet<String>,
    requests: std::collections::HashSet<(String, String)>,
    pending: Vec<Pending>,
    inboxes: HashMap<String, Inbox>,
    transcripts: HashMap<String, Transcript>,
    default_dir: String,
}

impl Seed {
    /// `s`'s session data, with `tabs` (their drafts and the transcripts of their sessions'
    /// families) moving into the new window, `route` showing.
    pub fn from(s: &App, tabs: Vec<Tab>, route: Route) -> Seed {
        let moved = |id: &str| tabs.iter().any(|t| t.id == s.root_of(id));
        Seed {
            drafts: tabs.iter().filter_map(|t| Some((t.id.clone(), s.draft_text(&t.id)?))).collect(),
            transcripts: s.transcripts.iter().filter(|(id, _)| moved(id)).map(|(k, v)| (k.clone(), v.clone())).collect(),
            tabs,
            route,
            projects: s.projects.clone(),
            sessions: s.sessions.clone(),
            running: s.running.clone(),
            requests: s.requests.clone(),
            pending: s.pending.clone(),
            inboxes: s.inboxes.clone(),
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
        self.projects = seed.projects;
        self.sessions = seed.sessions;
        self.running = seed.running;
        self.requests = seed.requests;
        self.pending = seed.pending;
        self.inboxes = seed.inboxes;
        self.transcripts.extend(seed.transcripts);
        self.default_dir = seed.default_dir;
        self.tabs = seed.tabs;
        self.restore.drafts.extend(seed.drafts);
        self.route = seed.route;
        self.set_media_fetch();
        self.show_draft();
        self.save_tabs();
    }

    /// The composer words kept for tab `id`, the open one's from the composer.
    pub(crate) fn draft_text(&self, id: &str) -> Option<String> {
        let text = if self.restore.shown.as_deref() == Some(id) { Some(self.composer.text.clone()) } else { self.restore.drafts.get(id).cloned() };
        text.filter(|t| !t.trim().is_empty())
    }
}

/// Opens every window saved at the last quit, or one for a new profile, each where it was.
pub fn launch(diagnostics: crate::diagnostics::Diagnostics) {
    let saved = store::has_ids();
    let mut ids = store::ids();
    if ids.is_empty() {
        ids.push(store::new_id());
    }
    store::save_ids(ids.clone());
    rune::run_windows(move |displays: &[Display]| {
        ids.iter()
            .enumerate()
            .map(|(i, id)| {
                let frame = store::load_frame(id).and_then(|f| fit(f, displays));
                window(Window::new(id.clone(), i == 0 && !saved), frame, &diagnostics).boxed()
            })
            .collect::<Vec<_>>()
    });
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
    let mut ids: Vec<String> = super::OPEN.with(|o| o.borrow().iter().map(|w| w.id.clone()).collect());
    ids.push(w.id.clone());
    store::save_ids(ids);
    let spec = window(w, frame, &s.diagnostics);
    Some(rune::open_window(if inactive { spec.inactive() } else { spec }))
}

/// Window `w`'s spec: the app, at `frame` when it has one.
fn window(w: Window, frame: Option<Frame>, diagnostics: &crate::diagnostics::Diagnostics) -> WindowSpec<App> {
    let id = w.id.clone();
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
        .on_ready(move |_| ready.update_if(|s, tx| { crate::launch::ready(s, tx); false }))
        .on_frame_encoded(move |f| {
            encoded.encoded(f);
            crate::telemetry::encoded(f);
        })
        .on_frame_drawn(move |rt| {
            drawn.drawn(rt);
            if let Some(frame) = crate::telemetry::last_frame() {
                crate::telemetry::drawn(rt.state.painted_session(), frame);
            }
            motion_log.frame(rt);
        })
        .on_close(|s, _| s.save_on_close());
    let mut spec = WindowSpec::from_app(format!("window-{id}"), app);
    if let Some(f) = frame {
        spec.options.position = Some((f.x, f.y));
        spec.options.physical_size = Some((f.width, f.height));
        spec = spec.maximized(f.maximized || f.full_screen);
    }
    spec.on_created(move |native| register(&id, native, handle))
}

/// `f`, while it still fits on one of the displays (Desktop's `resolveWindowState`); a
/// maximized window keeps that on the default size.
fn fit(f: Frame, displays: &[Display]) -> Option<Frame> {
    let within = |d: &Display| {
        let (x, y, w, h) = (d.position.0, d.position.1, d.size.0 as i32, d.size.1 as i32);
        f.x >= x - 16 && f.y >= y - 16 && f.x + f.width as i32 <= x + w + 16 && f.y + f.height as i32 <= y + h + 16
    };
    if displays.iter().any(within) {
        return Some(f);
    }
    let primary = displays.iter().find(|d| d.primary).or(displays.first())?;
    let scale = primary.scale;
    let (w, h) = ((DEFAULT_SIZE.0 as f64 * scale) as u32, (DEFAULT_SIZE.1 as f64 * scale) as u32);
    let x = primary.position.0 + (primary.size.0 as i32 - w as i32).max(0) / 2;
    let y = primary.position.1 + (primary.size.1 as i32 - h as i32).max(0) / 2;
    Some(Frame { x, y, width: w, height: h, ..f })
}
