//! Standalone widgets: one widget in a small window of its own, as a mini app. "Extract as
//! standalone app" on a widget in the timeline opens one, as does the app's link
//! `<scheme>://widget?server=<url>&instance=<session>:<message>:<call>` (what a widget dragged onto
//! the desktop starts, [`bundle`]; a launch while the app runs hands it the link).
//!
//! The window is one of the app's windows (`windows::open`) on the widget's server, showing the
//! widget alone in place of the shell. It follows the widget instance as the Mini apps dashboard
//! does, from the same store (`opencode_feature_widgets::instances`, kept by the plugin's
//! `changed` events): an update the agent posts (`replaces`) shows here as it does in the timeline
//! and on the dashboard. The widget runs here as it does there; what it sends goes to its own
//! session. The window never joins the windows saved for the next launch (`Life::Standalone`).

#[cfg(target_os = "macos")]
pub mod bundle;

use std::collections::HashSet;

use opencode_kit::model::instance_key;
use opencode_kit::servers::ServerKey;
use rune::{Cx, El, Tx, col, row, text};

use crate::App;
use crate::session::widgets::{WidgetsHost as _, instances};

/// The titlebar's height in a standalone window: the traffic lights and the widget's name.
const TITLE_H: f32 = 32.0;
/// What [`opencode_feature_widgets::Widgets::set_outside`] knows this window's widget by.
const OUTSIDE: &str = "standalone";

/// The widget a standalone window shows: the instance its key names now, on its server.
#[derive(Clone, Debug, PartialEq)]
pub struct Standalone {
    pub server: ServerKey,
    /// Its instance key (`sessionID:messageID:callID`, [`instance_key`]).
    pub instance: String,
    /// The widget's size where it was taken from (the timeline), in points: the window's
    /// content opens at it.
    pub size: Option<[f32; 2]>,
}

impl Standalone {
    /// Widget `id` ([`opencode_kit::model::widget_key`]) of `session` on `server`.
    pub fn of(server: ServerKey, session: &str, id: &str) -> Standalone {
        let (message, call) = id.split_once('/').unwrap_or((id, ""));
        Standalone { server, instance: instance_key(session, message, call), size: None }
    }

    /// Opens at widget `id`'s size in this window's timeline, if it was laid out.
    pub fn sized(mut self, s: &App, id: &str) -> Standalone {
        self.size = s.widgets().laid(id);
        self
    }

    /// Its window's size in points: the widget's, plus the titlebar, within reason.
    pub fn window_size(&self) -> [f32; 2] {
        let [w, h] = self.size.unwrap_or([460.0, 508.0]);
        [w.clamp(320.0, 1200.0), (h + TITLE_H).clamp(200.0, 900.0)]
    }

    /// The app's link that opens it, with its server's address `base`.
    pub fn url(&self, base: &str) -> String {
        let size = self.size.map(|[w, h]| format!("&size={}x{}", w.round(), h.round())).unwrap_or_default();
        format!("{}://widget?server={}&instance={}{size}", crate::launch::product().scheme(), percent(base), percent(&self.instance))
    }
}

/// The server address, instance key and size a `<own>://widget?…` link names.
pub fn parse(url: &str, own: &str) -> Option<(String, String, Option<[f32; 2]>)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (path, query) = rest.split_once('?')?;
    if path.trim_end_matches('/') != "widget" {
        return None;
    }
    let param = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(name).and_then(|v| v.strip_prefix('='))).map(unpercent);
    let (server, instance) = (param("server")?, param("instance")?);
    let parts: Vec<&str> = instance.split(':').collect();
    let ok = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(|c| c.is_control() || c == '/');
    let size = param("size").and_then(|v| {
        let (w, h) = v.split_once('x')?;
        Some([w.parse::<f32>().ok().filter(|w| w.is_finite())?, h.parse::<f32>().ok().filter(|h| h.is_finite())?])
    });
    (parts.len() == 3 && parts.iter().all(|p| ok(p))).then_some((server, instance, size))
}

/// Whether this process was started to open a standalone widget (its link among the arguments).
pub fn launched() -> bool {
    let product = crate::launch::product();
    std::env::args().any(|a| parse(&a, product.scheme()).is_some())
}

/// How long a link's widget waits for its server to be listed (a cold start connects first).
const WAIT_S: f64 = 30.0;

/// Opens a link's widget standalone, on the listed server at its address. While that server
/// isn't listed yet (the app just started), the link waits for it ([`drain`]), at most [`WAIT_S`].
pub fn open_link(s: &App, base: &str, instance: String, size: Option<[f32; 2]>) {
    match listed(base) {
        Some(server) => open(s, Standalone { server, instance, size }),
        None => WAITING.with_borrow_mut(|w| w.push((base.to_owned(), instance, size, *s.clock + WAIT_S))),
    }
}

/// Opens the waiting links whose server is listed now, and lets go of those that waited too
/// long. Whether any still waits.
pub fn drain(s: &App) -> bool {
    let waiting = WAITING.with_borrow_mut(std::mem::take);
    for (base, instance, size, until) in waiting {
        match listed(&base) {
            Some(server) => open(s, Standalone { server, instance, size }),
            None if *s.clock < until => WAITING.with_borrow_mut(|w| w.push((base, instance, size, until))),
            None => eprintln!("opencode-rune: no server at {base} for a standalone widget"),
        }
    }
    WAITING.with_borrow(|w| !w.is_empty())
}

/// The listed server at address `base`.
fn listed(base: &str) -> Option<ServerKey> {
    let same = |a: &str| a.trim_end_matches('/').eq_ignore_ascii_case(base.trim_end_matches('/'));
    crate::servers::runtime::listed().into_iter().find(|l| same(&l.server.base)).map(|l| l.key)
}

thread_local! {
    /// Links waiting for their server: its address, the instance, its size, and until when.
    static WAITING: std::cell::RefCell<Vec<(String, String, Option<[f32; 2]>, f64)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// The standalone windows opened, by window id (one that closed has no handle any more).
    static OPENED: std::cell::RefCell<Vec<(String, Standalone)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens widget `w` standalone in a window of its own, or brings the one showing it forward.
pub fn open(s: &App, w: Standalone) {
    let shown = OPENED.with_borrow(|o| o.iter().filter(|(_, x)| x.server == w.server && x.instance == w.instance).find_map(|(id, _)| crate::windows::handle(id)));
    if let Some(handle) = shown {
        handle.focus();
        return;
    }
    if let Some((id, _)) = crate::windows::open_standalone(s, w.clone()) {
        OPENED.with_borrow_mut(|o| {
            o.retain(|(id, _)| crate::windows::handle(id).is_some());
            o.push((id, w));
        });
    }
}

/// The standalone window opened: it works against its widget's server and asks for the server's
/// widgets.
pub fn ready(s: &mut App, _tx: &mut Tx) {
    let Some(w) = s.window.standalone.clone() else { return };
    crate::servers::runtime::switch(s, &w.server);
    instances::want(s);
}

/// Each frame in a standalone window: the widget its key names now (the newest of its line)
/// opens and runs here, as a dashboard tile's does. Whether that changed.
pub fn poll(s: &mut App) -> bool {
    let Some(w) = s.window.standalone.clone() else { return false };
    let known = s.widgets().instances.resolve(&w.instance).is_some_and(|i| s.widgets().entry(&i.state.key()).is_some());
    let running: HashSet<String> = instances::open(s, &w.instance).into_iter().collect();
    let opened = !known && !running.is_empty();
    opened | s.widgets_mut().set_outside(OUTSIDE, running)
}

/// The macOS titlebar of a standalone window: the traffic lights over its name.
pub fn titlebar() -> rune::Titlebar {
    rune::Titlebar::new(TITLE_H).traffic_lights(12.0, 9.0)
}

/// A standalone window's view in place of the shell: the widget's name in the titlebar, and the
/// widget filling the rest.
pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let p = crate::palette::pal(s.dark(cx));
    let Some(w) = &s.window.standalone else { return col() };
    let size = cx.layout_size("standalone-body").unwrap_or([480.0, 480.0]);
    let label = title(s, &w.instance).unwrap_or_default();
    let title = row()
        .key("standalone-title")
        .w_full()
        .h(TITLE_H)
        .center()
        .child(text(label).font(crate::view::UI_MED).color(p.muted).truncate());
    let body = rune::el().key("standalone-body").w_full().grow(1.0).min_h(0.0).child(crate::session::widgets::window_view(s, &p, &w.instance, size));
    col().key("standalone").w_full().h_full().fill(p.bg_base).child(title).child(body)
}

/// What the widget instance `key` names now is called (its window, its app on the desktop): its
/// title, else its session's when it has none of its own (it would go by its widget template's
/// name).
pub fn title(s: &App, key: &str) -> Option<String> {
    let i = s.widgets().instances.resolve(key)?;
    i.state.title.clone().or_else(|| i.session_title.clone()).or_else(|| Some(i.state.name.clone()))
}

/// A query value as a link carries it: unreserved characters as they are, the rest `%XX`.
fn percent(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// A query value: `%XX` escapes and `+` for a space.
fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len()).then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 3;
            }
            (b'+', None) => {
                out.push(b' ');
                i += 1;
            }
            (c, None) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
