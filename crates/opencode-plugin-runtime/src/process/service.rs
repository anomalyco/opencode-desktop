//! Luau `Process` service (`Process:Spawn`, `Process:RunAsync`) built over [`super`] supervision.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{FromLua, Function, Lua, MultiValue, Table, Value};
use opencode_plugin_spec as spec;
use rune_luau::{Completer, Data, Host};

use super::{Control, Encoding, End, Event, Spec, launch};
use crate::luau::{map, method};

/// Output and exits waiting for the next poll, by process id.
pub type Inbox = Arc<Mutex<Vec<(u64, Event)>>>;

/// One running process: its control, the VM that started it, its signals and its waiters.
pub struct Proc {
    pub vm: u64,
    pub control: Control,
    pub output: Value,
    pub exited: Value,
    pub waiters: Vec<Completer>,
}

/// All supervised processes (`Spawn` and `RunAsync`) belonging to one plugin half.
#[derive(Default)]
pub struct Procs {
    procs: RefCell<HashMap<u64, Proc>>,
    inbox: Inbox,
    runs: RefCell<Vec<(u64, Control)>>,
}

impl Procs {
    /// Whether this half has any live spawned or `RunAsync` processes.
    pub fn is_busy(&self) -> bool {
        !self.procs.borrow().is_empty() || !self.runs.borrow().is_empty()
    }

    /// What the host fires for this half's processes since the last poll: `(signal, args)`, and
    /// the waiters to resume. Exited processes are forgotten.
    pub fn drain(&self, vm: u64) -> Vec<(Value, Data)> {
        let events = match self.inbox.lock() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        let mut procs = self.procs.borrow_mut();
        for (id, event) in events {
            let Some(p) = procs.get_mut(&id).filter(|p| p.vm == vm) else { continue };
            match event {
                Event::Output { stream, text } => out.push((p.output.clone(), Data::List(vec![Data::String(if stream == 2 { "stderr" } else { "stdout" }.into()), Data::String(text)]))),
                Event::Exit { code, error } => {
                    let (timed_out, killed) = stopped(Some(&p.control));
                    let exit = map([
                        ("code", code.map_or(Data::Nil, Data::Integer)),
                        ("error", if error.is_empty() { Data::Nil } else { Data::String(error) }),
                        ("timedOut", Data::Bool(timed_out)),
                        ("killed", Data::Bool(killed)),
                    ]);
                    out.push((p.exited.clone(), Data::List(vec![exit.clone()])));
                    if let Some(p) = procs.remove(&id) {
                        p.waiters.into_iter().for_each(|w| w.complete(exit.clone()));
                    }
                }
            }
        }
        out
    }

    /// Kills every process of this half whose VM is not `keep_vm` (`RunAsync`'s too).
    pub fn reap(&self, keep_vm: Option<u64>) {
        self.procs.borrow_mut().retain(|_, p| {
            let keep = Some(p.vm) == keep_vm;
            if !keep {
                p.control.kill();
            }
            keep
        });
        self.runs.borrow_mut().retain(|(vm, control)| {
            let keep = Some(*vm) == keep_vm && control.running();
            if !keep {
                control.kill();
            }
            keep
        });
    }
}

static NEXT: AtomicU64 = AtomicU64::new(1);

/// Populates the `Process` service table (`Spawn` and `RunAsync`).
pub fn build(
    lua: &Lua,
    t: &Table,
    procs: Rc<Procs>,
    get_vm: impl Fn() -> u64 + 'static,
    script_signal: Function,
    asyncs_table: Table,
    get_waker: fn() -> Option<rune::Waker>,
) -> mlua::Result<()> {
    let wait_async = asyncs_table.clone();
    method(lua, t, "Spawn", move |lua, spec: Value| spawn(lua, &procs, get_vm(), &script_signal, &wait_async, get_waker, spec))?;
    let f: Function = asyncs_table.raw_get("Process.RunAsync")?;
    let wrap: Function = crate::chunk::load(lua, "local f = ... ; return function(_, ...) return f(...) end").call(f)?;
    t.raw_set("RunAsync", wrap)
}

/// Registers `Process.RunAsync` and `Process.WaitAsync` on `t` with the VM's [`Host`].
pub fn asyncs(host: &Host<'_>, t: &Table, procs: &Rc<Procs>, get_vm: fn(&Lua) -> Option<u64>) -> mlua::Result<()> {
    let owner = Rc::downgrade(procs);
    t.raw_set("Process.RunAsync", host.async_function("Process:RunAsync", move |lua, a, done| run(lua, a, done, &owner, get_vm))?)?;
    let owner = Rc::downgrade(procs);
    t.raw_set(
        "Process.WaitAsync",
        host.async_function("ProcessHandle:WaitAsync", move |lua, a, done| {
            let (id,): (u64,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
            // A process that already exited drops done, which resumes the task with nil.
            if let Some(h) = owner.upgrade()
                && let Some(p) = h.procs.borrow_mut().get_mut(&id)
            {
                p.waiters.push(done);
            }
            Ok(())
        })?,
    )
}

fn decode(lua: &Lua, spec: Value) -> mlua::Result<Spec> {
    let s: spec::ProcessSpec = Data::from_lua(spec, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("ProcessSpec: {e}")))?;
    Ok(Spec {
        program: s.program,
        args: s.args.unwrap_or_default(),
        env: s.env.unwrap_or_default().into_iter().collect(),
        cwd: s.cwd.unwrap_or_default(),
        stdin: s.stdin.unwrap_or_default().into_bytes(),
        keep_stdin: s.keep_stdin.unwrap_or(false),
        timeout: s.timeout.filter(|t| *t > 0.0).map(Duration::from_secs_f64),
        encoding: s.encoding.map(|e| match e {
            spec::TextEncoding::Utf16le => Encoding::Utf16Le,
            _ => Encoding::Utf8,
        }),
        stderr_file: s.stderr_file.unwrap_or(false),
    })
}

/// `ProcessExit.timedOut` and `.killed`, from how the supervisor saw it stop.
fn stopped(control: Option<&Control>) -> (bool, bool) {
    match control.and_then(Control::end) {
        Some(End::TimedOut) => (true, false),
        Some(End::Killed) => (false, true),
        None => (false, false),
    }
}

fn spawn(
    lua: &Lua,
    procs: &Rc<Procs>,
    vm: u64,
    script_signal: &Function,
    asyncs_table: &Table,
    get_waker: fn() -> Option<rune::Waker>,
    spec: Value,
) -> mlua::Result<Table> {
    let spec = decode(lua, spec)?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let inbox = procs.inbox.clone();
    let waker = get_waker();
    let control = launch(spec, move |event| {
        if let Ok(mut q) = inbox.lock() {
            q.push((id, event));
        }
        if let Some(w) = &waker {
            w();
        }
    });
    let output: Value = script_signal.call(())?;
    let exited: Value = script_signal.call(())?;
    procs.procs.borrow_mut().insert(id, Proc { vm, control: control.clone(), output: output.clone(), exited: exited.clone(), waiters: Vec::new() });
    let h = lua.create_table()?;
    h.raw_set("Id", id)?;
    h.raw_set("Output", output)?;
    h.raw_set("Exited", exited)?;
    let c = control.clone();
    method(lua, &h, "Running", move |_, ()| Ok(c.running()))?;
    let c = control.clone();
    method(lua, &h, "Write", move |_, (text, close): (String, Option<bool>)| Ok(c.write(text.as_bytes(), close.unwrap_or(false))))?;
    let c = control;
    method(lua, &h, "Kill", move |_, ()| {
        c.kill();
        Ok(())
    })?;
    let wait: Function = asyncs_table.raw_get("Process.WaitAsync")?;
    let wrap: Function = crate::chunk::load(lua, "local f, id = ... ; return function() return f(id) end").call((wait, id))?;
    h.raw_set("WaitAsync", wrap)?;
    Ok(h)
}

/// `Process:RunAsync(spec)`: runs to the end, then resumes with a `ProcessOutput`.
fn run(lua: &Lua, a: MultiValue, done: Completer, owner: &Weak<Procs>, get_vm: fn(&Lua) -> Option<u64>) -> mlua::Result<()> {
    let (spec,): (Value,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let spec = decode(lua, spec)?;
    let out: Arc<Mutex<(String, String, Option<Completer>)>> = Arc::new(Mutex::new((String::new(), String::new(), Some(done))));
    let control: Arc<Mutex<Option<Control>>> = Arc::default();
    let held = control.clone();
    let launched = launch(spec, move |event| {
        let Ok(mut o) = out.lock() else { return };
        match event {
            Event::Output { stream: 2, text } => o.1.push_str(&text),
            Event::Output { text, .. } => o.0.push_str(&text),
            Event::Exit { code, error } => {
                let (timed_out, _) = stopped(held.lock().ok().as_ref().and_then(|c| c.as_ref()));
                let (stdout, stderr) = (std::mem::take(&mut o.0), std::mem::take(&mut o.1));
                if let Some(done) = o.2.take() {
                    done.complete(map([
                        ("ok", Data::Bool(code == Some(0) && error.is_empty())),
                        ("code", code.map_or(Data::Nil, Data::Integer)),
                        ("stdout", Data::String(stdout)),
                        ("stderr", Data::String(stderr)),
                        ("error", if error.is_empty() { Data::Nil } else { Data::String(error) }),
                        ("timedOut", Data::Bool(timed_out)),
                    ]));
                }
            }
        }
    });
    if let Ok(mut c) = control.lock() {
        *c = Some(launched.clone());
    }
    if let (Some(procs), Some(vm)) = (owner.upgrade(), get_vm(lua)) {
        let mut runs = procs.runs.borrow_mut();
        runs.retain(|(_, c)| c.running());
        runs.push((vm, launched));
    }
    Ok(())
}
