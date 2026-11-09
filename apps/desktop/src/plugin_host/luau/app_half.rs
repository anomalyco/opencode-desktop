//! `AppPluginHost`: each plugin's app half (`app.luau`), once per process. Every window runs on
//! the UI thread, so the halves live in a thread-local the windows share: the first window that
//! loads a plugin starts its app half, and every window's poll offers to poll the halves, which
//! run at most once per tick of the frame clock however many windows are open.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use opencode_plugin_spec::Manifest;
use rune::Tx;
use rune_luau::Script;

use super::plugin::{Half, Kind};
use super::{cx, vm};
use crate::App;

pub struct AppHalf {
    pub script: Script,
    pub half: Rc<Half>,
}

thread_local! {
    static HALVES: RefCell<Vec<AppHalf>> = const { RefCell::new(Vec::new()) };
    /// The frame clock of the last poll: a second window polling in the same tick skips.
    static POLLED: Cell<f64> = const { Cell::new(f64::NAN) };
}

/// Starts plugin `manifest`'s app half from `entry`, unless it runs already.
pub fn ensure(app: &App, manifest: &Manifest, files: &[(String, String)], entry: &str) {
    if HALVES.with_borrow(|h| h.iter().any(|a| *a.half.id == *manifest.id)) {
        return;
    }
    let half = Half::new(&manifest.id, Kind::App);
    *half.catalogs.borrow_mut() = super::sources::catalogs(files);
    super::sources::intern_icons(&manifest.id, files);
    let sources = super::sources::script_sources(files, &manifest.id, entry);
    let script = cx::with_app(app, || Script::with_options(&format!("{}@app", manifest.id), sources, vm::options(half.clone())));
    if let Some(w) = app.plugins.luau.waker.clone() {
        script.set_waker(w);
    }
    if let Some(e) = script.error() {
        super::console::push(&manifest.id, rune_luau::LogLevel::Error, &format!("app.luau: {}", e.message));
    }
    half.adopt(script.vm_id());
    HALVES.with_borrow_mut(|h| h.push(AppHalf { script, half }));
}

/// Stops plugin `id`'s app half (it turned off).
pub fn stop(id: &str) {
    let gone = HALVES.with_borrow_mut(|h| h.iter().position(|a| &*a.half.id == id).map(|i| h.remove(i)));
    if let Some(a) = gone {
        super::services::reap(&a.half, None);
        super::services::store::withdraw_owner(id, Kind::App, None);
    }
}

/// Polls every app half once per tick.
pub fn poll(app: &App, now: f64, tx: &mut Tx) -> bool {
    if POLLED.get() == now {
        return false;
    }
    POLLED.set(now);
    HALVES.with_borrow(|halves| {
        halves.iter().fold(false, |changed, a| {
            let vm = a.script.vm_id().unwrap_or(0);
            let prompts = super::services::askpass::drain(&a.half, vm).into_iter().filter_map(|(name, args)| Some((a.half.signals.borrow().get(&(vm, name))?.clone(), args)));
            let fired: Vec<_> = super::services::process::drain(&a.half, vm).into_iter().chain(prompts).collect();
            for (signal, args) in fired {
                if let Some(fire) = a.half.fire.borrow().get(&vm).cloned() {
                    let args = match args {
                        rune_luau::Data::List(items) => items,
                        other => vec![other],
                    };
                    cx::with_app(app, || {
                        let values = a.script.with_lua(|lua| {
                            let mut v = mlua::MultiValue::new();
                            v.push_back(signal.clone());
                            for item in args {
                                v.push_back(mlua::IntoLua::into_lua(item, lua)?);
                            }
                            Ok::<_, mlua::Error>(v)
                        });
                        if let Some(Ok(values)) = values {
                            a.script.spawn_fn(&fire, values, None).ok();
                        }
                    });
                }
            }
            changed | cx::with_app(app, || a.script.poll(now, tx))
        })
    })
}

/// Runs `f` on every running app half.
pub fn each<R>(mut f: impl FnMut(&AppHalf) -> Option<R>) -> Vec<R> {
    HALVES.with_borrow(|h| h.iter().filter_map(&mut f).collect())
}
