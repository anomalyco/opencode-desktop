//! Links opened with the app (`App::on_open_url`), on the app's own scheme (`opencode-rune://`,
//! see `launch::product`). OpenCode Desktop's `opencode://` stays OpenCode Desktop's: Rune never
//! registers it, so such a link opens OpenCode Desktop, or nothing when only Rune is installed.
//!
//! - `open-project?directory=<folder>` (and the older `open?project=<folder>`) opens the folder as a
//!   project with a new session, as Open project does; `new-session?directory=<folder>&prompt=<text>`
//!   does the same with the prompt typed in, not sent: OpenCode Desktop's links
//!   (`packages/app/src/shell/deep-links/machine.ts`, opencode#65332). Links open in the order they came.
//! - `notification/<id>?…` is a clicked notification (`rune_notify::click_from_url`): its
//!   session opens, the app coming forward, also when the click started it.
//! - `console/authorized?window=<id>` is the OpenCode Console's return link after a sign-in:
//!   the window that started it comes forward (Desktop's `consoleReturnWindow`).
//! - `new-window`, `new-session` and `session?id=<id>` are the jump list's (`jump_list`): a new
//!   window, a new session in the window in front, a session on the app's own server.
//!
//! The window comes forward for each, as Desktop's `focusWindow` does. A link that arrives
//! before the server connects (one that started the app) waits for it.
//!
//! Windows registers the scheme at install (`win_install`) and hands each link to the running app
//! through `rune::single_instance`; macOS gets it from the bundle's `CFBundleURLTypes`.

use rune::Tx;
use rune::machine::Effects;

use crate::reconnect::ServerId;
use crate::{App, Source};

/// A link waiting for its server: the folder an `open?project=` link names, on the server it
/// opens on (the app's own: a folder on this computer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub server: ServerId,
    pub dir: String,
    /// `new-session`'s prompt, typed into the new session's composer, not sent.
    pub prompt: Option<String>,
}

/// Links the system handed the app, held until their server can open them, as OpenCode Desktop
/// holds a link until a window is ready for it (`packages/desktop/src/main/deep-links/machine.ts`:
/// `queued` until `window.ready`, then `delivered`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeepLinks {
    /// Links whose server isn't connected yet, oldest first.
    waiting: Vec<Queued>,
    /// A `new-session` link's folder and prompt, until the session it opened shows its composer.
    prompt: Option<(String, String, f64)>,
}

pub enum DeepLinkEvent {
    Received(Queued),
    /// `server` is connected: its links can open.
    Ready { server: ServerId },
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeepLinkEffect {
    /// The window comes forward, as Desktop's `window.focus` command.
    Focus,
    /// Opens `dir` as a project with a new session on `server`, `prompt` typed into it.
    Open { server: ServerId, dir: String, prompt: Option<String> },
}

impl DeepLinks {
    pub fn step(&mut self, event: DeepLinkEvent, ready: impl Fn(&str) -> bool) -> Effects<DeepLinkEffect> {
        let mut queue = std::mem::take(&mut self.waiting);
        let mut effects = Effects::none();
        match event {
            DeepLinkEvent::Received(link) => {
                effects.push(DeepLinkEffect::Focus);
                queue.push(link);
            }
            DeepLinkEvent::Ready { .. } => {}
        }
        let (now, later): (Vec<_>, Vec<_>) = queue.into_iter().partition(|q| ready(&q.server));
        for q in now {
            effects.push(DeepLinkEffect::Open { server: q.server, dir: q.dir, prompt: q.prompt });
        }
        self.waiting = later;
        effects
    }

    pub fn is_waiting(&self) -> bool { !self.waiting.is_empty() }
}

/// `App::on_open_url`.
#[cfg(not(target_arch = "wasm32"))]
pub fn open(s: &mut App, url: &str, tx: &mut Tx) {
    if let Some(click) = rune_notify::click_from_url(url) {
        crate::notifications::clicked(s, &click);
        return;
    }
    if let Some(id) = console_return(url, crate::launch::product().scheme()) {
        match crate::windows::handle(&id) {
            Some(w) => w.focus(),
            None => rune_notify::bring_to_front(),
        }
        return;
    }
    match jump(url, crate::launch::product().scheme()) {
        Some(Jump::NewWindow) => {
            crate::windows::new_window(s);
            return;
        }
        Some(Jump::NewSession) => {
            rune_notify::bring_to_front();
            s.close_settings();
            s.new_session(tx);
            return;
        }
        Some(Jump::Session(id)) => {
            crate::notifications::clicked(s, &rune_notify::Click { id: String::new(), info: vec![("session".into(), id), ("server".into(), String::new())] });
            return;
        }
        None => {}
    }
    match (project(url, crate::launch::product().scheme()), server_of(s)) {
        (Some((dir, prompt)), Some(server)) => {
            let ready = ready_servers(s);
            let effects = s.services.deep_links.step(DeepLinkEvent::Received(Queued { server, dir, prompt }), |server| ready.iter().any(|r| r == server));
            run(s, effects, tx);
        }
        _ => eprintln!("opencode-rune: ignored a link it doesn't open: {url}"),
    }
}

/// `App::on_open_url`: browsers deliver query parameters via `apps/web`, never OS URL schemes.
#[cfg(target_arch = "wasm32")]
pub fn open(_: &mut App, _: &str, _: &mut Tx) {}

/// Opens the links waiting for a server once it connects (from `poll`).
pub fn drain(s: &mut App, tx: &mut Tx) -> bool {
    let typed = type_prompt(s);
    if !s.services.deep_links.is_waiting() {
        return typed;
    }
    let ready = ready_servers(s);
    let Some(server) = ready.first().cloned() else { return false };
    let effects = s.services.deep_links.step(DeepLinkEvent::Ready { server }, |server| ready.iter().any(|r| r == server));
    run(s, effects, tx)
}

fn run(s: &mut App, effects: Effects<DeepLinkEffect>, tx: &mut Tx) -> bool {
    let mut changed = false;
    for effect in effects {
        match effect {
            #[cfg(not(target_arch = "wasm32"))]
            DeepLinkEffect::Focus if s.live() => rune_notify::bring_to_front(),
            DeepLinkEffect::Focus => {}
            DeepLinkEffect::Open { server, dir, prompt } => {
                s.close_settings();
                // The project opens on its server, whichever the window shows.
                crate::servers::runtime::switch(s, &opencode_kit::servers::ServerKey::new(&server));
                s.open_project_dir(&dir, true, tx);
                if let Some(prompt) = prompt {
                    s.services.deep_links.prompt = Some((dir, prompt, *s.clock));
                }
                changed = true;
            }
        }
    }
    changed
}

/// The server a link opens on: the app's own (a folder on this machine).
#[cfg(not(target_arch = "wasm32"))]
fn server_of(_: &App) -> Option<ServerId> {
    Some(opencode_kit::servers::ServerKey::SIDECAR.to_owned())
}

/// The servers the window has a half of that can take writes: a link waits only for its own
/// server, not for the one the window shows.
fn ready_servers(s: &mut App) -> Vec<ServerId> {
    let can_write = |s: &mut App| matches!(&*s.source, Source::Live { writes: Some(_), .. } | Source::Fixture);
    s.checkout.keys().into_iter().filter(|k| crate::servers::runtime::with_server(s, k, can_write).unwrap_or(false)).map(|k| k.as_str().to_owned()).collect()
}

/// A `new-session` link's prompt goes into the composer of the session it opened, once that
/// session shows (the project opens through the server first). Dropped after 30 s, or when the
/// composer already holds something.
fn type_prompt(s: &mut App) -> bool {
    let Some((dir, prompt, at)) = s.services.deep_links.prompt.clone() else { return false };
    if *s.clock - at > 30.0 {
        s.services.deep_links.prompt = None;
        return false;
    }
    let crate::Route::Session { id, .. } = &*s.route else { return false };
    let same = |a: &str, b: &str| a.trim_end_matches(['/', '\\']).eq_ignore_ascii_case(b.trim_end_matches(['/', '\\']));
    if !s.session(id).is_some_and(|i| same(&i.directory, &dir)) {
        return false;
    }
    s.services.deep_links.prompt = None;
    if !s.composer.text.is_empty() {
        return false;
    }
    *s.composer = rune::TextEdit::new(prompt);
    true
}

/// The folder (and `new-session`'s prompt) a project link on the app's scheme `own` names:
/// `open-project?directory=`, `new-session?directory=&prompt=`, or the older `open?project=`.
#[cfg(not(target_arch = "wasm32"))]
fn project(url: &str, own: &str) -> Option<(String, Option<String>)> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (host, query) = rest.split_once('?')?;
    let param = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(name).and_then(|v| v.strip_prefix('='))).map(unpercent);
    let (dir, prompt) = match host.trim_end_matches('/') {
        "open" => (param("project")?, None),
        "open-project" => (param("directory")?, None),
        "new-session" => (param("directory")?, param("prompt").filter(|p| !p.is_empty())),
        _ => return None,
    };
    (!dir.trim().is_empty() && !dir.chars().any(char::is_control)).then_some((dir, prompt))
}
/// The jump list's entries (`jump_list`), besides `open?project=`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, PartialEq, Eq)]
enum Jump {
    NewWindow,
    NewSession,
    /// `session?id=<id>`: a session on the app's own server.
    Session(String),
}

#[cfg(not(target_arch = "wasm32"))]
fn jump(url: &str, own: &str) -> Option<Jump> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    match path.trim_end_matches('/') {
        "new-window" => Some(Jump::NewWindow),
        // `new-session?directory=` is a project link (`project`); bare, it's the jump list's.
        "new-session" if !query.split('&').any(|pair| pair.starts_with("directory=")) => Some(Jump::NewSession),
        "session" => {
            let id = query.split('&').find_map(|pair| pair.strip_prefix("id=")).map(unpercent)?;
            (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)).then_some(Jump::Session(id))
        }
        _ => None,
    }
}

/// The window a `<own>://console/authorized?window=<id>` link names.
#[cfg(not(target_arch = "wasm32"))]
fn console_return(url: &str, own: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    if !scheme.eq_ignore_ascii_case(own) {
        return None;
    }
    let (path, query) = rest.split_once('?')?;
    if path != "console/authorized" {
        return None;
    }
    let id = query.split('&').find_map(|pair| pair.strip_prefix("window=")).map(unpercent)?;
    (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)).then_some(id)
}

/// A query value: `%XX` escapes and `+` for a space, as `URLSearchParams` reads it.
#[cfg(not(target_arch = "wasm32"))]
fn unpercent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
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
