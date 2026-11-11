//! `Process`: programs a plugin starts, supervised by the host (`plugin_host::process`: window
//! hidden, in the app's job object, killed when the plugin goes). `Spawn` returns a handle whose
//! `Output` and `Exited` signals fire as its threads report; `RunAsync` waits for the whole run.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mlua::{FromLua, Lua, MultiValue, Table, Value};
use opencode_plugin_spec as spec;
use rune_luau::{Completer, Data, Host};

use super::{async_method, map, method};
use crate::plugin_host::cx;
use crate::plugin_host::plugin::Half;
use crate::plugin_host::vm::Vmx;
use crate::plugin_host::process::{self as supervisor, Control, Event};

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

static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "Spawn", move |lua, spec: Value| spawn(lua, &v, spec))?;
    async_method(lua, vmx, t, "RunAsync", "Process.RunAsync")
}

pub fn asyncs(host: &Host<'_>, t: &Table, half: &Rc<Half>) -> mlua::Result<()> {
    let owner = Rc::downgrade(half);
    t.raw_set("Process.RunAsync", host.async_function("Process:RunAsync", move |lua, a, done| run(lua, a, done, &owner))?)?;
    let half = Rc::downgrade(half);
    t.raw_set(
        "Process.WaitAsync",
        host.async_function("ProcessHandle:WaitAsync", move |lua, a, done| {
            let (id,): (u64,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
            // A process that already exited drops done, which resumes the task with nil.
            if let Some(h) = half.upgrade()
                && let Some(p) = h.procs.borrow_mut().get_mut(&id)
            {
                p.waiters.push(done);
            }
            Ok(())
        })?,
    )?;
    Ok(())
}

fn decode(lua: &Lua, spec: Value) -> mlua::Result<supervisor::Spec> {
    let s: spec::ProcessSpec = Data::from_lua(spec, lua)?.decode().map_err(|e| mlua::Error::runtime(format!("ProcessSpec: {e}")))?;
    Ok(supervisor::Spec {
        program: s.program,
        args: s.args.unwrap_or_default(),
        env: s.env.unwrap_or_default().into_iter().collect(),
        cwd: s.cwd.unwrap_or_default(),
        stdin: s.stdin.unwrap_or_default().into_bytes(),
        keep_stdin: s.keep_stdin.unwrap_or(false),
        timeout: s.timeout.filter(|t| *t > 0.0).map(Duration::from_secs_f64),
        encoding: s.encoding.map(|e| match e {
            spec::TextEncoding::Utf16le => supervisor::Encoding::Utf16Le,
            _ => supervisor::Encoding::Utf8,
        }),
        stderr_file: s.stderr_file.unwrap_or(false),
    })
}

/// `ProcessExit.timedOut` and `.killed`, from how the supervisor saw it stop.
fn stopped(control: Option<&Control>) -> (bool, bool) {
    match control.and_then(Control::end) {
        Some(supervisor::End::TimedOut) => (true, false),
        Some(supervisor::End::Killed) => (false, true),
        None => (false, false),
    }
}

fn spawn(lua: &Lua, vmx: &Rc<Vmx>, spec: Value) -> mlua::Result<Table> {
    let spec = decode(lua, spec)?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let inbox = vmx.half.inbox.clone();
    let waker = cx::app().and_then(|app| app.plugins.waker.clone());
    let control = supervisor::launch(spec, move |event| {
        if let Ok(mut q) = inbox.lock() {
            q.push((id, event));
        }
        if let Some(w) = &waker {
            w();
        }
    });
    let output: Value = vmx.script_signal.call(())?;
    let exited: Value = vmx.script_signal.call(())?;
    vmx.half.procs.borrow_mut().insert(id, Proc { vm: vmx.vm.get(), control: control.clone(), output: output.clone(), exited: exited.clone(), waiters: Vec::new() });
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
    let wait: mlua::Function = vmx.asyncs.raw_get("Process.WaitAsync")?;
    let wrap: mlua::Function = lua.load("local f, id = ... ; return function() return f(id) end").call((wait, id))?;
    h.raw_set("WaitAsync", wrap)?;
    Ok(h)
}

/// What the host fires for the half's processes since the last poll: `(signal, args)`, and the
/// waiters to resume. Exited processes are forgotten.
pub fn drain(half: &Half, vm: u64) -> Vec<(Value, Data)> {
    let events = match half.inbox.lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut procs = half.procs.borrow_mut();
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

/// Kills every process of the half (it turned off, failed or reloaded), `RunAsync`'s too.
pub fn reap(half: &Half, keep_vm: Option<u64>) {
    half.procs.borrow_mut().retain(|_, p| {
        let keep = Some(p.vm) == keep_vm;
        if !keep {
            p.control.kill();
        }
        keep
    });
    half.runs.borrow_mut().retain(|(vm, control)| {
        let keep = Some(*vm) == keep_vm && control.running();
        if !keep {
            control.kill();
        }
        keep
    });
}

/// `Process:RunAsync(spec)`: runs to the end, then resumes with a `ProcessOutput`.
fn run(lua: &Lua, a: MultiValue, done: Completer, owner: &std::rc::Weak<Half>) -> mlua::Result<()> {
    let (spec,): (Value,) = mlua::FromLuaMulti::from_lua_multi(a, lua)?;
    let spec = decode(lua, spec)?;
    let out: Arc<Mutex<(String, String, Option<Completer>)>> = Arc::new(Mutex::new((String::new(), String::new(), Some(done))));
    let control: Arc<Mutex<Option<Control>>> = Arc::default();
    let held = control.clone();
    let launched = supervisor::launch(spec, move |event| {
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
    if let (Some(half), Some(vm)) = (owner.upgrade(), lua.app_data_ref::<Rc<Vmx>>().map(|v| v.vm.get())) {
        let mut runs = half.runs.borrow_mut();
        runs.retain(|(_, c)| c.running());
        runs.push((vm, launched));
    }
    Ok(())
}
