//! What a plugin asks of the system through the host: processes it supervises, HTTP requests,
//! Desktop's menus and dialogs, and the app's own facts (its options, its server's version).
//!
//! - [`ProcessSpec`]: a process the host starts with its window hidden and kills when the
//!   plugin unloads (and on Windows when the app exits, however it does). Its output and exit
//!   arrive as [`ProcessEvent`]s on the half that started it; [`ActionCx::run_process`] collects
//!   them into one [`ProcessOutput`].
//! - [`Fetch`]: an HTTP request off the app's thread, answered as a [`FetchResult`].
//! - [`MenuSpec`]: Desktop's menu beside an element, answered with the picked row's id.

use std::cell::RefCell;

use crate::abi::{
    AbiOwnedBytes, AbiStatus, AbiStr, Callback, EVENT_FETCH, EVENT_PROCESS_EXIT, EVENT_PROCESS_OUTPUT, PROCESS_KEEP_STDIN, ProcessSpecAbi,
    REQUEST_CLOSE_DIALOG, REQUEST_CONNECT_SERVER, REQUEST_OPEN_DIALOG, STATE_DIALOG, STATE_SERVER_CURRENT, TEXT_ARG, TEXT_EXE, TEXT_SERVER_HEALTH,
    TEXT_SERVER_VERSION,
};
use crate::app::AppSetupCx;
use crate::boundary::{ctx, drop_packed, host};
use crate::cx::{ActionCx, ViewCx, WindowSetupCx, action_callback};

/// A process to start.
#[derive(Clone, Debug, Default)]
pub struct ProcessSpec {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: String,
    stdin: String,
    keep_stdin: bool,
    timeout_ms: u64,
}

impl ProcessSpec {
    pub fn new(program: impl Into<String>) -> Self {
        ProcessSpec { program: program.into(), ..Self::default() }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Adds `key=value` to the app's environment for it.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Written to its stdin first; stdin then closes unless [`Self::keep_stdin`].
    pub fn stdin(mut self, input: impl Into<String>) -> Self {
        self.stdin = input.into();
        self
    }

    /// Stdin stays open for [`ActionCx::write_process`].
    pub fn keep_stdin(mut self) -> Self {
        self.keep_stdin = true;
        self
    }

    /// Killed after `ms` (its exit says it timed out).
    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn arg_list(&self) -> &[String] {
        &self.args
    }

    fn with_abi<R>(&self, f: impl FnOnce(&ProcessSpecAbi) -> R) -> R {
        let args = self.args.join("\0");
        let env = self.env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("\0");
        let abi = ProcessSpecAbi {
            program: AbiStr::new(&self.program),
            args: AbiStr::new(&args),
            env: AbiStr::new(&env),
            cwd: AbiStr::new(&self.cwd),
            stdin: AbiStr::new(&self.stdin),
            flags: if self.keep_stdin { PROCESS_KEEP_STDIN } else { 0 },
            _pad: 0,
            timeout_ms: self.timeout_ms,
        };
        f(&abi)
    }
}

/// A running (or finished) process the host supervises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Process(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// How a process ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessExit {
    /// Its exit code, when it has one.
    pub code: Option<i64>,
    /// Why it couldn't start (or be waited on).
    pub error: Option<String>,
    /// Its timeout killed it.
    pub timed_out: bool,
    /// A kill (the plugin's, or an unload) ended it.
    pub killed: bool,
}

pub enum ProcessEvent<'a> {
    Output { process: Process, stream: Stream, text: &'a str },
    Exit { process: Process, exit: ProcessExit },
}

/// A finished process's collected output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit: ProcessExit,
}

impl ProcessOutput {
    /// It ran and exited with 0.
    pub fn ok(&self) -> bool {
        self.exit.code == Some(0)
    }
}

/// A process event's output (its stream and text) or exit, owned.
type Owned = Result<(Stream, String), ProcessExit>;

/// The event `cx` carries, its text copied out so the handler may use `cx` freely.
fn process_event(cx: &ActionCx) -> Option<(Process, Owned)> {
    let e = cx.event();
    let process = Process(e.mods);
    match e.kind {
        EVENT_PROCESS_OUTPUT => Some((process, Ok((if e.x > 1.5 { Stream::Stderr } else { Stream::Stdout }, cx.input().to_owned())))),
        EVENT_PROCESS_EXIT => {
            let code = cx.input().parse().ok();
            let error = Some(cx.input_extra().to_owned()).filter(|s| !s.is_empty());
            Some((process, Err(ProcessExit { code, error, timed_out: e.y > 0.5 && e.y < 1.5, killed: e.y > 1.5 })))
        }
        _ => None,
    }
}

fn spawn_with(spec: &ProcessSpec, events: Callback) -> Option<Process> {
    // SAFETY: setup and handler contexts accept it; the spec's strings are alive for the call;
    // the host takes `events` when it accepts.
    let handle = spec.with_abi(|abi| unsafe { (host().process_spawn)(ctx(), abi, events) });
    if handle == 0 {
        drop_packed(events.data, events.drop_data);
        return None;
    }
    Some(Process(handle))
}

fn events_callback<P: 'static>(on: impl Fn(&mut P, &mut ActionCx, ProcessEvent) + 'static) -> Callback {
    action_callback::<P, _>(move |s: &mut P, cx: &mut ActionCx| match process_event(cx) {
        Some((process, Ok((stream, text)))) => on(s, cx, ProcessEvent::Output { process, stream, text: &text }),
        Some((process, Err(exit))) => on(s, cx, ProcessEvent::Exit { process, exit }),
        None => cx.unchanged(),
    })
}

fn collecting<P: 'static>(done: impl FnOnce(&mut P, &mut ActionCx, ProcessOutput) + 'static) -> Callback {
    let out = RefCell::new(ProcessOutput::default());
    let done = RefCell::new(Some(done));
    events_callback::<P>(move |s, cx, e| match e {
        ProcessEvent::Output { stream: Stream::Stdout, text, .. } => out.borrow_mut().stdout.push_str(text),
        ProcessEvent::Output { stream: Stream::Stderr, text, .. } => out.borrow_mut().stderr.push_str(text),
        ProcessEvent::Exit { exit, .. } => {
            let mut output = std::mem::take(&mut *out.borrow_mut());
            output.exit = exit;
            if let Some(done) = done.borrow_mut().take() {
                done(s, cx, output);
            }
        }
    })
}

macro_rules! process_methods {
    ($p:ident) => {
        /// Starts `spec`; `on` hears its output and its exit on this half's state. `None` when the
        /// host refused it (a program that can't start still answers, with its exit).
        pub fn spawn_process<$p: 'static>(&mut self, spec: &ProcessSpec, on: impl Fn(&mut $p, &mut ActionCx, ProcessEvent) + 'static) -> Option<Process> {
            spawn_with(spec, events_callback::<$p>(on))
        }

        /// Starts `spec` and collects its output; `done` runs once with all of it as it exits.
        pub fn run_process<$p: 'static>(&mut self, spec: &ProcessSpec, done: impl FnOnce(&mut $p, &mut ActionCx, ProcessOutput) + 'static) -> Option<Process> {
            spawn_with(spec, collecting::<$p>(done))
        }
    };
}

impl ActionCx {
    process_methods!(P);

    /// Writes `input` to the process's stdin, then closes it when `close`.
    pub fn write_process(&mut self, process: Process, input: &str, close: bool) -> bool {
        // SAFETY: handler contexts accept it; `input` is alive for the call.
        unsafe { (host().process_write)(ctx(), process.0, AbiStr::new(input), u8::from(close)) == AbiStatus::Ok }
    }

    /// Kills the process; its exit event follows.
    pub fn kill_process(&mut self, process: Process) {
        // SAFETY: handler contexts accept it.
        unsafe { (host().process_kill)(ctx(), process.0) };
    }

    /// Sends `request` off the app's thread; `done` runs once with the answer.
    pub fn fetch<P: 'static>(&mut self, request: &Fetch, done: impl FnOnce(&mut P, &mut ActionCx, FetchResult) + 'static) -> bool {
        let done = RefCell::new(Some(done));
        let cb = action_callback::<P, _>(move |s: &mut P, cx: &mut ActionCx| {
            if cx.event().kind != EVENT_FETCH {
                return;
            }
            let result = FetchResult { status: cx.event().mods as u16, body: cx.input().to_owned(), error: Some(cx.input_extra().to_owned()).filter(|e| !e.is_empty()) };
            if let Some(done) = done.borrow_mut().take() {
                done(s, cx, result);
            }
        });
        let headers = request.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n");
        // SAFETY: handler contexts accept it; the strings are alive for the call; the host takes `cb`.
        let ok = unsafe {
            (host().fetch)(
                ctx(),
                AbiStr::new(&request.method),
                AbiStr::new(&request.url),
                AbiStr::new(&headers),
                AbiStr::new(&request.body),
                request.timeout_ms,
                request.delay_ms,
                cb,
            )
        } == AbiStatus::Ok;
        if !ok {
            drop_packed(cb.data, cb.drop_data);
        }
        ok
    }

    /// Opens Desktop's menu `menu` beside the element keyed `anchor`; `done` runs once it
    /// closes, with the picked row's id (`None` when dismissed).
    pub fn open_menu<P: 'static>(&mut self, anchor: &str, menu: &MenuSpec, done: impl FnOnce(&mut P, &mut ActionCx, Option<&str>) + 'static) -> bool {
        let done = RefCell::new(Some(done));
        let cb = action_callback::<P, _>(move |s: &mut P, cx: &mut ActionCx| {
            let picked = cx.input().to_owned();
            if let Some(done) = done.borrow_mut().take() {
                done(s, cx, Some(picked.as_str()).filter(|p| !p.is_empty()));
            }
        });
        let spec = menu.encode();
        // SAFETY: handler contexts accept it; the strings are alive for the call; the host takes `cb`.
        let ok = unsafe { (host().open_menu)(ctx(), AbiStr::new(anchor), AbiStr::new(&spec), cb) } == AbiStatus::Ok;
        if !ok {
            drop_packed(cb.data, cb.drop_data);
        }
        ok
    }

    /// Opens this plugin's dialog `id` (`Contribution::dialog`), closing any other.
    pub fn open_dialog(&mut self, id: &str) {
        // SAFETY: handler contexts accept it.
        unsafe { (host().request)(ctx(), REQUEST_OPEN_DIALOG, AbiStr::new(id)) };
    }

    /// Closes this plugin's open dialog.
    pub fn close_dialog(&mut self) {
        // SAFETY: as `open_dialog`.
        unsafe { (host().request)(ctx(), REQUEST_CLOSE_DIALOG, AbiStr::EMPTY) };
    }

    /// Whether this plugin's dialog `id` is open.
    pub fn dialog_open(&self, id: &str) -> bool {
        state(STATE_DIALOG, id)
    }

    /// Connects the window to this plugin's server `id` (ready and listed) and goes to its Home.
    pub fn connect_server(&mut self, id: &str) {
        // SAFETY: as `open_dialog`.
        unsafe { (host().request)(ctx(), REQUEST_CONNECT_SERVER, AbiStr::new(id)) };
    }
}

impl<S: 'static> AppSetupCx<S> {
    process_methods!(P);

    /// As `ActionCx::set_servers`.
    pub fn set_servers(&mut self, source: &str, entries: &[crate::contrib::ServerEntry], ready: bool) {
        crate::cx::set_servers(source, entries, ready);
    }
}

impl<S: 'static> WindowSetupCx<S> {
    process_methods!(P);

    /// As `ActionCx::set_servers`.
    pub fn set_servers(&mut self, source: &str, entries: &[crate::contrib::ServerEntry], ready: bool) {
        crate::cx::set_servers(source, entries, ready);
    }
}

impl<P: 'static> ViewCx<P> {
    /// Whether this plugin's dialog `id` is open.
    pub fn dialog_open(&self, id: &str) -> bool {
        state(STATE_DIALOG, id)
    }

    /// Whether the window is connected to this plugin's server `id`.
    pub fn server_current(&self, id: &str) -> bool {
        state(STATE_SERVER_CURRENT, id)
    }
}

fn state(what: u32, arg: &str) -> bool {
    // SAFETY: render, handler and poll contexts accept it; the strings are alive for the call.
    unsafe { (host().host_state)(ctx(), what, AbiStr::new(arg), AbiStr::EMPTY) != 0 }
}

fn text(what: u32, arg: &str) -> Option<String> {
    let mut out = AbiOwnedBytes::EMPTY;
    // SAFETY: any context accepts it; `out` is a valid, empty out pointer the host fills.
    let some = unsafe { (host().host_text)(ctx(), what, AbiStr::new(arg), &mut out) } != 0;
    some.then(|| out.to_string_lossy())
}

/// Writes `message` to the app's log (`[plugin] …`), as Desktop's `ctx.log`.
pub fn log(error: bool, message: &str) {
    let level = if error { crate::abi::LOG_ERROR } else { crate::abi::LOG_INFO };
    // SAFETY: any thread may log; `message` is alive for the call.
    unsafe { (host().log)(level, AbiStr::new(message)) };
}

/// The value of command-line option `name` (`--name value`).
pub fn arg(name: &str) -> Option<String> {
    text(TEXT_ARG, name)
}

/// The version of the app's own server, once it answered.
pub fn server_version() -> Option<String> {
    text(TEXT_SERVER_VERSION, "").filter(|v| !v.is_empty())
}

/// This plugin's server `id`'s version once it answered healthy.
pub fn server_health(id: &str) -> Option<String> {
    text(TEXT_SERVER_HEALTH, id).filter(|v| !v.is_empty())
}

/// The app's executable.
pub fn exe() -> Option<String> {
    text(TEXT_EXE, "")
}

/// The build's release channel: `local` (a development build), `beta` or `prod`.
pub fn channel() -> String {
    text(crate::abi::TEXT_CHANNEL, "").unwrap_or_else(|| "local".into())
}

/// The folder the app writes its logs to.
pub fn logs_dir() -> Option<String> {
    text(crate::abi::TEXT_LOGS_DIR, "")
}

/// The folder the app saves downloads to.
pub fn downloads_dir() -> Option<String> {
    text(crate::abi::TEXT_DOWNLOADS_DIR, "")
}

/// The app process's memory as JSON (see `abi::TEXT_MEMORY`).
pub fn memory() -> Option<String> {
    text(crate::abi::TEXT_MEMORY, "")
}

/// The routed session, as JSON (see `abi::TEXT_ACTIVE_SESSION`); `None` when none is open.
pub fn active_session() -> Option<String> {
    text(crate::abi::TEXT_ACTIVE_SESSION, "")
}

/// A developer tool's state (see `abi::TEXT_DEV`).
pub fn dev(name: &str) -> Option<String> {
    text(crate::abi::TEXT_DEV, name)
}

impl ActionCx {
    /// Drives a developer tool (see `abi::REQUEST_DEV`).
    pub fn dev(&mut self, what: &str) {
        // SAFETY: handler contexts accept it.
        unsafe { (host().request)(ctx(), crate::abi::REQUEST_DEV, AbiStr::new(what)) };
    }
}

/// An experiment's setting (see `abi::TEXT_EXPERIMENT`).
pub fn experiment(name: &str) -> Option<String> {
    text(crate::abi::TEXT_EXPERIMENT, name)
}

/// An image handed to the host, for [`crate::ui::image`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageId(pub u32);

/// Copies an encoded image (PNG, JPEG …) into the host, which decodes it off the app's thread.
/// Call it on the app's thread (setup, a render, a handler or poll); `None` when refused.
pub fn image(encoded: &[u8]) -> Option<ImageId> {
    // SAFETY: `encoded` is alive for the call; the host copies it.
    let id = unsafe { (host().image)(ctx(), AbiStr::from_bytes(encoded), 0, 0) };
    (id != 0).then_some(ImageId(id))
}

/// Copies `width` × `height` RGBA8 pixels (straight alpha) into the host.
pub fn image_rgba(pixels: &[u8], width: u32, height: u32) -> Option<ImageId> {
    if width == 0 || height == 0 || pixels.len() != width as usize * height as usize * 4 {
        return None;
    }
    // SAFETY: as `image`.
    let id = unsafe { (host().image)(ctx(), AbiStr::from_bytes(pixels), width, height) };
    (id != 0).then_some(ImageId(id))
}

/// Lets the host free an image once nothing draws it.
pub fn image_drop(id: ImageId) {
    // SAFETY: any callback on the app's thread accepts it.
    unsafe { (host().image_drop)(ctx(), id.0) };
}

/// An HTTP request.
#[derive(Clone, Debug)]
pub struct Fetch {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    timeout_ms: u64,
    delay_ms: u64,
}

impl Fetch {
    pub fn get(url: impl Into<String>) -> Self {
        Fetch { method: "GET".into(), url: url.into(), headers: Vec::new(), body: String::new(), timeout_ms: 15_000, delay_ms: 0 }
    }

    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// `Authorization: Basic` for `user:password`.
    pub fn basic_auth(self, user: &str, password: &str) -> Self {
        let token = base64(format!("{user}:{password}").as_bytes());
        self.header("Authorization", format!("Basic {token}"))
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    /// Sent after `ms` (a poll's next try).
    pub fn delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

/// An HTTP answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchResult {
    /// 0 when nothing answered.
    pub status: u16,
    pub body: String,
    pub error: Option<String>,
}

impl FetchResult {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for c in input.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= c.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

/// One of Desktop's menus (`Menu` in `@opencode/ui`).
#[derive(Clone, Debug, Default)]
pub struct MenuSpec {
    label: String,
    end: bool,
    min_w: f32,
    rows: Vec<String>,
}

impl MenuSpec {
    /// `label` names it for assistive technology.
    pub fn new(label: impl Into<String>) -> Self {
        MenuSpec { label: label.into(), ..Self::default() }
    }

    /// A group's title above the rows that follow (`Menu.GroupLabel`).
    pub fn group(mut self, title: &str) -> Self {
        self.rows.push(format!("group\t{}", clean(title)));
        self
    }

    pub fn item(self, id: &str, title: &str) -> Self {
        self.item_with(id, title, true, false)
    }

    /// An item, offered only when `enabled`, in the critical ink when `danger`.
    pub fn item_with(mut self, id: &str, title: &str, enabled: bool, danger: bool) -> Self {
        let flags = format!("{}{}", if enabled { "" } else { "d" }, if danger { "x" } else { "" });
        self.rows.push(format!("item\t{}\t{}\t{flags}", clean(id), clean(title)));
        self
    }

    pub fn separator(mut self) -> Self {
        self.rows.push("sep".into());
        self
    }

    /// Lined up with its anchor's end (`placement="bottom-end"`).
    pub fn align_end(mut self) -> Self {
        self.end = true;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_w = w;
        self
    }

    fn encode(&self) -> String {
        let head = format!("menu\t{}\t{}\t{}", clean(&self.label), if self.end { "end" } else { "start" }, self.min_w);
        std::iter::once(head).chain(self.rows.iter().cloned()).collect::<Vec<_>>().join("\n")
    }
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n'], " ")
}
