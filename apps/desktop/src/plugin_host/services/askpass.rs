//! `Askpass` (the app half only): a loopback endpoint for `SSH_ASKPASS` (Desktop's
//! `askpass.ts`). `ssh` runs the app's own executable as its askpass helper
//! (`crate::askpass`), which sends each prompt here with the listener's token and prints the
//! answer the user gave. One prompt at a time, each for up to 5 minutes; a helper that goes away
//! clears its prompt. The endpoint runs on a thread of its own until `Close` or its half goes.
//!
//! A listener's life on the UI thread is [`Listener`]: listening (with the prompt it shows, if
//! any) or closed. The thread hands it notes; the app half's poll steps it and fires the
//! listener's signals.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use rune::time::Instant;

use mlua::{Function, Lua, Table, Value};
use rune_luau::Data;

use super::{Held, map, method, signal};
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;

/// What the endpoint's thread tells the UI thread.
enum Note {
    Prompt { id: String, text: String, confirm: bool },
    Cleared(String),
}

/// What the endpoint's thread and the UI thread share.
#[derive(Default)]
struct Shared {
    notes: Mutex<Vec<Note>>,
    /// Answers by prompt id; `None` cancels.
    answers: Mutex<Vec<(String, Option<String>)>>,
    answered: Condvar,
    closed: AtomicBool,
}

/// A listener as the UI thread steps it.
#[derive(Debug, PartialEq, Eq)]
enum Listener {
    Listening { prompt: Option<String> },
    Closed,
}

enum Event {
    Prompted(String),
    Cleared(String),
    Close,
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    /// Fire `Prompted` for the prompt the note carries.
    Show,
    /// Fire `Cleared` with the prompt's id.
    Clear(String),
    /// Stop the endpoint's thread.
    Stop,
}

impl Listener {
    fn step(self, event: Event) -> (Listener, Vec<Effect>) {
        match (self, event) {
            (Listener::Closed, _) => (Listener::Closed, Vec::new()),
            (Listener::Listening { prompt }, Event::Close) => (Listener::Closed, prompt.into_iter().map(Effect::Clear).chain([Effect::Stop]).collect()),
            (Listener::Listening { .. }, Event::Prompted(id)) => (Listener::Listening { prompt: Some(id) }, vec![Effect::Show]),
            (Listener::Listening { prompt: Some(p) }, Event::Cleared(id)) if p == id => (Listener::Listening { prompt: None }, vec![Effect::Clear(id)]),
            (listening, Event::Cleared(_)) => (listening, Vec::new()),
        }
    }
}

struct Entry {
    half: std::rc::Weak<Half>,
    vm: u64,
    state: Listener,
    shared: Arc<Shared>,
}

thread_local! {
    static LISTENERS: RefCell<HashMap<u64, Entry>> = RefCell::default();
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Builds the listener table in Luau: its prompts as objects with `Respond`, over the raw
/// signals the host fires.
const LISTENER: &str = r#"
local raw, rawCleared, respond, close, env, newSignal = ...
local listener = { Env = env, Prompted = newSignal(), Cleared = newSignal() }
raw:Connect(function(d)
    listener.Prompted:Fire({
        Id = d.id,
        Prompt = d.text,
        Secret = not d.confirm,
        Respond = function(_, answer)
            respond(d.id, answer)
        end,
    })
end)
rawCleared:Connect(function(id)
    listener.Cleared:Fire(id)
end)
function listener.Close()
    close()
end
return listener
"#;

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "Listen", move |lua, ()| listen(lua, &v))
}

fn listen(lua: &Lua, vmx: &Rc<Vmx>) -> mlua::Result<Value> {
    let socket = TcpListener::bind("127.0.0.1:0").map_err(|e| mlua::Error::runtime(format!("Askpass:Listen: {e}")))?;
    socket.set_nonblocking(true).map_err(|e| mlua::Error::runtime(format!("Askpass:Listen: {e}")))?;
    let port = socket.local_addr().map_err(|e| mlua::Error::runtime(format!("Askpass:Listen: {e}")))?.port();
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let token = super::app::random_id();
    let shared = Arc::new(Shared::default());
    let waker = crate::plugin_host::cx::with_app(|app| app.plugins.waker.clone()).flatten();
    let (s, t) = (shared.clone(), token.clone());
    std::thread::Builder::new()
        .name("plugin-askpass".into())
        .spawn(move || serve(&socket, &t, &s, waker.as_ref()))
        .map_err(|e| mlua::Error::runtime(format!("Askpass:Listen: {e}")))?;
    LISTENERS.with_borrow_mut(|l| l.insert(id, Entry { half: Rc::downgrade(&vmx.half), vm: vmx.vm.get(), state: Listener::Listening { prompt: None }, shared: shared.clone() }));
    vmx.half.held.borrow_mut().push((vmx.vm.get(), Held::Askpass(id)));
    let env = lua.create_table()?;
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let display = std::env::var_os("DISPLAY").map(|d| d.to_string_lossy().into_owned()).filter(|d| !d.is_empty()).unwrap_or_else(|| "opencode".into());
    for (k, v) in [("SSH_ASKPASS", exe), ("SSH_ASKPASS_REQUIRE", "force".into()), ("DISPLAY", display), ("OPENCODE_SSH_ASKPASS_PORT", port.to_string()), ("OPENCODE_SSH_ASKPASS_TOKEN", token)] {
        env.raw_set(k, v)?;
    }
    let raw = signal(lua, vmx, &format!("Askpass.{id}.Prompted"))?;
    let raw_cleared = signal(lua, vmx, &format!("Askpass.{id}.Cleared"))?;
    let answers = shared.clone();
    let respond = lua.create_function(move |_, (prompt, answer): (String, Option<String>)| {
        if let Ok(mut a) = answers.answers.lock() {
            a.push((prompt, answer));
        }
        answers.answered.notify_all();
        Ok(())
    })?;
    let close = lua.create_function(move |_, ()| {
        close(id);
        Ok(())
    })?;
    let new_signal = vmx.script_signal.clone();
    let built: Function = lua.load(LISTENER).set_name("=@opencode/askpass").into_function()?;
    built.call((raw, raw_cleared, respond, close, env, new_signal))
}

/// Closes listener `id` (`Close`, or its half went away).
pub fn close(id: u64) {
    let Some(shared) = LISTENERS.with_borrow_mut(|l| {
        let e = l.get_mut(&id)?;
        let (next, effects) = std::mem::replace(&mut e.state, Listener::Closed).step(Event::Close);
        e.state = next;
        effects.contains(&Effect::Stop).then(|| e.shared.clone())
    }) else {
        return;
    };
    shared.closed.store(true, Ordering::Release);
    shared.answered.notify_all();
}

/// The prompts that came and went for the listeners of `half`'s VM `vm` since the last poll, as
/// `(signal name, payload)`. Closed listeners are forgotten.
pub fn drain(half: &Rc<Half>, vm: u64) -> Vec<(String, Data)> {
    let mut out = Vec::new();
    LISTENERS.with_borrow_mut(|all| {
        for (id, e) in all.iter_mut().filter(|(_, e)| e.vm == vm && e.half.upgrade().is_some_and(|h| Rc::ptr_eq(&h, half))) {
            let notes = e.shared.notes.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default();
            for note in notes {
                let (event, payload) = match note {
                    Note::Prompt { id: p, text, confirm } => (Event::Prompted(p.clone()), Some(map([("id", Data::String(p)), ("text", Data::String(text)), ("confirm", Data::Bool(confirm))]))),
                    Note::Cleared(p) => (Event::Cleared(p), None),
                };
                let (next, effects) = std::mem::replace(&mut e.state, Listener::Closed).step(event);
                e.state = next;
                for effect in effects {
                    match effect {
                        Effect::Show => out.push((format!("Askpass.{id}.Prompted"), payload.clone().unwrap_or(Data::Nil))),
                        Effect::Clear(p) => out.push((format!("Askpass.{id}.Cleared"), Data::String(p))),
                        Effect::Stop => {}
                    }
                }
            }
        }
        all.retain(|_, e| e.state != Listener::Closed && e.half.strong_count() > 0);
    });
    out
}

/// The endpoint: accepts a helper, reads its request, hands the prompt over and waits for the
/// answer, one helper at a time.
fn serve(socket: &TcpListener, token: &str, shared: &Shared, waker: Option<&rune::Waker>) {
    let mut next = 0u64;
    let note = |n: Note| {
        if let Ok(mut all) = shared.notes.lock() {
            all.push(n);
        }
        if let Some(w) = waker {
            w();
        }
    };
    while !shared.closed.load(Ordering::Acquire) {
        let Ok((stream, _)) = socket.accept() else {
            std::thread::sleep(Duration::from_millis(50));
            continue;
        };
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let Ok(reading) = stream.try_clone() else { continue };
        let mut line = String::new();
        if BufReader::new(reading).take(16_384).read_line(&mut line).is_err() || !line.ends_with('\n') {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_default();
        if v["token"].as_str() != Some(token) {
            continue;
        }
        next += 1;
        let id = format!("p{next}");
        note(Note::Prompt { id: id.clone(), text: v["text"].as_str().unwrap_or_default().to_owned(), confirm: v["confirm"].as_bool().unwrap_or(false) });
        let answer = wait(shared, &id);
        if let Some(value) = answer {
            let mut stream = stream;
            stream.write_all(format!("{}\n", serde_json::json!({ "value": value })).as_bytes()).ok();
        }
        note(Note::Cleared(id));
    }
}

/// Waits up to 5 minutes for prompt `id`'s answer; `None` when cancelled, closed or timed out.
fn wait(shared: &Shared, id: &str) -> Option<String> {
    let started = Instant::now();
    let mut answers = shared.answers.lock().ok()?;
    while !shared.closed.load(Ordering::Acquire) && started.elapsed() < Duration::from_secs(300) {
        if let Some(at) = answers.iter().position(|(p, _)| p == id) {
            return answers.remove(at).1;
        }
        answers = shared.answered.wait_timeout(answers, Duration::from_millis(500)).ok()?.0;
    }
    None
}
