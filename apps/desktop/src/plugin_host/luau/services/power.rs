//! `Power`: keeps the display awake while any plugin holds a lock (Desktop's
//! `powerSaveBlocker.start("prevent-display-sleep")`). One blocker serves the whole process:
//! every window runs on the UI thread, where Windows' execution state lives; on macOS and Linux
//! a supervised `caffeinate -d` or `systemd-inhibit` runs while it's held. A lock goes with the
//! plugin half that took it.
// The OS call that keeps the display awake; the block says what it relies on.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use mlua::{Lua, Table};

use super::{Held, method};
use crate::plugin_host::luau::vm::Vmx;

/// The display blocker: released, or held by a set of locks.
#[derive(Debug, PartialEq, Eq)]
enum Blocker {
    Released,
    Held { locks: BTreeSet<u64> },
}

enum Event {
    Acquire(u64),
    Release(u64),
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Hold,
    Let,
}

impl Blocker {
    fn step(self, event: Event) -> (Blocker, Option<Effect>) {
        match (self, event) {
            (Blocker::Released, Event::Acquire(id)) => (Blocker::Held { locks: BTreeSet::from([id]) }, Some(Effect::Hold)),
            (Blocker::Held { mut locks }, Event::Acquire(id)) => {
                locks.insert(id);
                (Blocker::Held { locks }, None)
            }
            (Blocker::Held { mut locks }, Event::Release(id)) => {
                locks.remove(&id);
                if locks.is_empty() { (Blocker::Released, Some(Effect::Let)) } else { (Blocker::Held { locks }, None) }
            }
            (Blocker::Released, Event::Release(_)) => (Blocker::Released, None),
        }
    }
}

thread_local! {
    static BLOCKER: RefCell<Blocker> = const { RefCell::new(Blocker::Released) };
    #[cfg(not(target_os = "windows"))]
    static INHIBITOR: RefCell<Option<crate::plugin_host::process::Control>> = const { RefCell::new(None) };
}

static NEXT: AtomicU64 = AtomicU64::new(1);

fn dispatch(event: Event) {
    let effect = BLOCKER.with_borrow_mut(|b| {
        let (next, effect) = std::mem::replace(b, Blocker::Released).step(event);
        *b = next;
        effect
    });
    match effect {
        Some(Effect::Hold) => platform::hold(true),
        Some(Effect::Let) => platform::hold(false),
        None => {}
    }
}

pub fn build(lua: &Lua, vmx: &Rc<Vmx>, t: &Table) -> mlua::Result<()> {
    let v = vmx.clone();
    method(lua, t, "KeepAwake", move |lua, _reason: Option<String>| {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        dispatch(Event::Acquire(id));
        v.half.held.borrow_mut().push((v.vm.get(), Held::Power(id)));
        let lock = lua.create_table()?;
        let half = Rc::downgrade(&v.half);
        method(lua, &lock, "Release", move |_, ()| {
            release(id);
            if let Some(h) = half.upgrade() {
                h.held.borrow_mut().retain(|(_, held)| *held != Held::Power(id));
            }
            Ok(())
        })?;
        Ok(lock)
    })
}

/// Lets lock `id` go (released, or its half went away).
pub fn release(id: u64) {
    dispatch(Event::Release(id));
}

#[cfg(target_os = "windows")]
mod platform {
    use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, SetThreadExecutionState};

    /// The UI thread holds it; every window runs there.
    pub fn hold(on: bool) {
        // SAFETY: plain flags in, the previous state out.
        unsafe { SetThreadExecutionState(if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS }) };
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::plugin_host::process::{Spec, launch};

    /// `caffeinate -d` (macOS) or `systemd-inhibit` (Linux), for as long as it's held.
    pub fn hold(on: bool) {
        super::INHIBITOR.with_borrow_mut(|held| {
            if let Some(p) = held.take() {
                p.kill();
            }
            if !on {
                return;
            }
            let (program, args): (&str, Vec<String>) = if cfg!(target_os = "macos") {
                ("caffeinate", vec!["-d".into(), "-w".into(), std::process::id().to_string()])
            } else {
                ("systemd-inhibit", ["--what=idle", "--who=OpenCode", "--why=Keep screen active", "sleep", "infinity"].map(str::to_owned).to_vec())
            };
            let spec = Spec { program: program.into(), args, env: Vec::new(), cwd: String::new(), stdin: Vec::new(), keep_stdin: false, timeout: None, encoding: None, stderr_file: false };
            *held = Some(launch(spec, |_| {}));
        });
    }
}
