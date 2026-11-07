//! What a handler asked of the app that needs more than its own call: toggling its panels,
//! server requests off the app's thread and saved files, each answering through the callback
//! the plugin passed.

use opencode_plugin::abi::{AbiEvent, AbiStr, EVENT_RESPONSE, EVENT_SAVED};
use rune::Tx;

use super::instance::{CallbackEnv, Scope};
use super::run::{Ran, act};
use crate::App;
use crate::jobs::Apply;

/// A request in flight: whose callback runs when it answers, on which half.
pub struct Pending {
    slot: usize,
    generation: u32,
    scope: Scope,
    done: CallbackEnv,
}

/// An HTTP request a plugin asked for (`HostApiVtable::fetch`).
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub timeout: std::time::Duration,
    pub delay: std::time::Duration,
}

/// Opens slot `i`'s panel `panel` in the routed session, or closes it when it shows there
/// (Desktop's `layout.toggle`).
pub fn toggle_panel(app: &mut App, i: usize, panel: &str, tx: &mut Tx) {
    let Some(session) = app.active_session_id().map(str::to_owned) else { return };
    let plugin = app.plugins.slots[i].manifest.id.clone();
    let key = crate::panels::panel_key(super::view::PANEL, &format!("{plugin}.{panel}"));
    tx.instant();
    if super::vtable::panel_state(app, &plugin, panel, &session) == 3 {
        app.close_side_tab(&key, tx);
    } else {
        app.open_side_tab(&key, None, false, false, tx);
    }
}

fn hold(app: &mut App, i: usize, scope: Scope, done: CallbackEnv) -> Option<u64> {
    let generation = app.plugins.slots[i].instance.as_ref()?.generation;
    let host = &mut app.plugins;
    host.next_pending += 1;
    host.pending.insert(host.next_pending, Pending { slot: i, generation, scope, done });
    Some(host.next_pending)
}

/// Sends `request` off the app's thread, on a plugin worker; `done` gets the answer on `scope`.
pub fn fetch(app: &mut App, i: usize, scope: Scope, request: FetchRequest, done: CallbackEnv) {
    let Some(n) = hold(app, i, scope, done) else { return };
    let deliver = app.deliverer();
    super::tasks::run(move || {
        if !request.delay.is_zero() {
            std::thread::sleep(request.delay);
        }
        let (status, answer) = send(&request);
        deliver(Box::new(move |s: &mut App, tx: &mut Tx| answer_status(s, n, status, answer, tx)));
    });
}

/// Sends `r`: its status (0 when nothing answered) and body, or the error.
fn send(r: &FetchRequest) -> (u16, Result<String, String>) {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(r.timeout)).http_status_as_error(false).build().into();
    let mut req = ureq::http::Request::builder().method(r.method.as_str()).uri(r.url.as_str());
    for (k, v) in &r.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let sent = if r.body.is_empty() { req.body(()).map(|req| agent.run(req)) } else { req.body(r.body.clone()).map(|req| agent.run(req)) };
    let sent = match sent {
        Ok(sent) => sent,
        Err(e) => return (0, Err(e.to_string())),
    };
    match sent {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            match resp.body_mut().with_config().limit(16 << 20).read_to_string() {
                Ok(body) => (status, Ok(body)),
                Err(e) => (status, Err(e.to_string())),
            }
        }
        Err(e) => (0, Err(e.to_string())),
    }
}

fn answer_status(app: &mut App, n: u64, status: u16, answer: Result<String, String>, tx: &mut Tx) {
    answer_with_status(app, n, opencode_plugin::abi::EVENT_FETCH, u32::from(status), answer, tx);
}

/// Opens Desktop's menu `spec` (the SDK's `MenuSpec` lines) beside `anchor`; `done` hears the
/// pick, or "" when it's dismissed.
pub fn menu(app: &mut App, i: usize, scope: Scope, anchor: &str, spec: &str, done: CallbackEnv, tx: &mut Tx) {
    let Some(n) = hold(app, i, scope, done) else { return };
    let menu = parse_menu(spec);
    crate::popup::open(app, menu, anchor.to_owned(), tx, move |s: &mut App, picked: Option<&str>, tx: &mut Tx| {
        answer_with(s, n, opencode_plugin::abi::EVENT_MENU, Ok(picked.unwrap_or_default().to_owned()), tx);
    });
}

fn parse_menu(spec: &str) -> crate::popup::Menu {
    let mut lines = spec.lines().peekable();
    let mut menu = crate::popup::Menu::new("");
    if let Some(head) = lines.peek().filter(|l| l.starts_with("menu\t")) {
        let f: Vec<&str> = head.split('\t').collect();
        menu = crate::popup::Menu::new(f.get(1).copied().unwrap_or_default());
        if f.get(2) == Some(&"end") {
            menu = menu.align(rune::Align::End);
        }
        if let Some(w) = f.get(3).and_then(|w| w.parse::<f32>().ok()).filter(|w| *w > 0.0) {
            menu = menu.min_width(w);
        }
        lines.next();
    }
    for line in lines {
        let f: Vec<&str> = line.split('\t').collect();
        menu = match f.first().copied() {
            Some("sep") => menu.separator(),
            Some("group") => menu.group(f.get(1).copied().unwrap_or_default()),
            Some("item") => {
                let flags = f.get(3).copied().unwrap_or_default();
                let item = crate::popup::Item::new(f.get(1).copied().unwrap_or_default(), f.get(2).copied().unwrap_or_default());
                menu.item(item.enabled(!flags.contains('d')).checked(flags.contains('c')).danger(flags.contains('x')))
            }
            _ => menu,
        };
    }
    menu
}

/// Shows `path` in the system's file manager, selected when it's a file (`desktop.reveal`); a
/// headless run opens nothing.
pub fn reveal(path: &str) {
    if crate::headless() || path.is_empty() {
        return;
    }
    let file = std::path::Path::new(path).is_file();
    #[cfg(target_os = "windows")]
    let spawned = if file { std::process::Command::new("explorer.exe").arg(format!("/select,{path}")).spawn() } else { std::process::Command::new("explorer.exe").arg(path).spawn() };
    #[cfg(target_os = "macos")]
    let spawned = if file { std::process::Command::new("/usr/bin/open").args(["-R", path]).spawn() } else { std::process::Command::new("/usr/bin/open").arg(path).spawn() };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(if file { std::path::Path::new(path).parent().map_or(path, |p| p.to_str().unwrap_or(path)) } else { path }).spawn();
    if let Err(e) = spawned {
        super::tasks::log(format!("reveal {path}: {e}"));
    }
}

/// Sends `method path` with `body` to the server off the app's thread; `done` gets the answer.
pub fn server(app: &mut App, i: usize, scope: Scope, method: String, path: String, body: String, done: CallbackEnv) {
    let Some(n) = hold(app, i, scope, done) else { return };
    let send = move |srv: &opencode_kit::client::Server, path: &str| {
        // `BYTES` reads a file's raw bytes (`abi::METHOD_BYTES`), which needn't be text.
        let answer = if method == opencode_plugin::abi::METHOD_BYTES {
            srv.get_bytes(path)
        } else if method.eq_ignore_ascii_case("GET") {
            srv.get_text(path).map(String::into_bytes)
        } else {
            let body = (!body.is_empty()).then(|| serde_json::from_str(&body).unwrap_or(serde_json::Value::Null));
            // A plugin keeps its own deadline (a model's reply may take minutes), so the request
            // only gives up once nothing could still want it.
            srv.call_within(&method.to_ascii_uppercase(), path, body, std::time::Duration::from_secs(600)).map(|v| v.to_string().into_bytes())
        };
        Box::new(move |s: &mut App, tx: &mut Tx| answer_bytes(s, n, EVENT_RESPONSE, u32::from(answer.is_err()), answer, tx)) as Apply
    };
    // `local:` asks the app's own server, even while the window is connected to another.
    match (path.strip_prefix(opencode_plugin::abi::LOCAL_SERVER_PREFIX), app.own_server.clone()) {
        (Some(local), Some(own)) => {
            let (local, deliver) = (local.to_owned(), app.deliverer());
            super::tasks::run(move || {
                let apply = send(&own, &local);
                deliver(apply);
            });
        }
        (local, _) => {
            let path = local.map_or(path.clone(), str::to_owned);
            app.job(move |srv| send(srv, &path));
        }
    }
}

/// Saves `content` as `name` in the Downloads folder; `done` gets its path.
pub fn save(app: &mut App, i: usize, name: &str, content: &str, done: CallbackEnv, tx: &mut Tx) {
    let Some(n) = hold(app, i, Scope::Window, done) else { return };
    let saved = if app.persists() { opencode_kit::client::save_download(name, content) } else { Err("This window doesn't save files".to_owned()) };
    answer_with(app, n, EVENT_SAVED, saved, tx);
}

/// Runs request `n`'s callback with its answer, while the plugin generation that asked runs.
fn answer_with(app: &mut App, n: u64, kind: u32, answer: Result<String, String>, tx: &mut Tx) {
    let mods = u32::from(answer.is_err());
    answer_with_status(app, n, kind, mods, answer, tx);
}

fn answer_with_status(app: &mut App, n: u64, kind: u32, mods: u32, answer: Result<String, String>, tx: &mut Tx) {
    answer_bytes(app, n, kind, mods, answer.map(String::into_bytes), tx);
}

/// [`answer_with_status`] with an answer that may not be text (`abi::METHOD_BYTES`).
fn answer_bytes(app: &mut App, n: u64, kind: u32, mods: u32, answer: Result<Vec<u8>, String>, tx: &mut Tx) {
    let Some(p) = app.plugins.pending.remove(&n) else { return };
    let Some(func) = p.done.cb.func else { return };
    let (text, extra): (&[u8], &str) = match &answer {
        Ok(body) => (body, ""),
        Err(e) => (&[], e.as_str()),
    };
    let view = |s: &[u8]| AbiStr { ptr: s.as_ptr(), len: s.len(), _lt: std::marker::PhantomData };
    let event = AbiEvent { kind, mods, text: view(text), extra: view(extra.as_bytes()), ..AbiEvent::default() };
    let data = p.done.cb.data;
    // SAFETY: the plugin's own callback with its data, on the state of the generation that asked;
    // the event's strings outlive the call.
    let ran = act(app, tx, p.slot, p.scope, Some(p.generation), |state, ctx| unsafe { func(state, ctx, data, &event) });
    if matches!(ran, Ran::Done(_, true)) {
        tx.instant();
    }
}
