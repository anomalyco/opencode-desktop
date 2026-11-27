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
use super::vm;
use crate::App;
use crate::plugin_host::{Catalogs, Tier, console, cx, services};

pub struct AppHalf {
    pub script: Script,
    pub half: Rc<Half>,
    updater_rev: Cell<u64>,
}

thread_local! {
    static HALVES: RefCell<Vec<AppHalf>> = const { RefCell::new(Vec::new()) };
    /// The frame clock of the last poll: a second window polling in the same tick skips.
    static POLLED: Cell<f64> = const { Cell::new(f64::NAN) };
    /// Hash of all app halves' menubar items after the last refresh.
    static MENUBAR_SIG: Cell<u64> = const { Cell::new(0) };
}

/// Starts plugin `manifest`'s app half from `entry`, unless it runs already.
pub fn ensure(app: &App, tier: Tier, manifest: &Manifest, files: &[(String, String)], catalogs: Catalogs, sources: rune_luau::Sources) {
    if HALVES.with_borrow(|h| h.iter().any(|a| *a.half.id == *manifest.id)) {
        return;
    }
    let half = Half::new(&manifest.id, Kind::App, tier);
    *half.catalogs.borrow_mut() = catalogs;
    super::sources::intern_icons(&manifest.id, files);
    let script = cx::enter(app, || Script::with_options(&format!("{}@app", manifest.id), sources, vm::options(half.clone())));
    if let Some(w) = app.plugins.waker.clone() {
        script.set_waker(w);
    }
    if let Some(e) = script.error() {
        console::push(&manifest.id, rune_luau::LogLevel::Error, &format!("app.luau: {}", e.message));
    }
    half.adopt(script.vm_id());
    HALVES.with_borrow_mut(|h| h.push(AppHalf { script, half, updater_rev: Cell::new(0) }));
}

/// Whether plugin `id`'s app half runs.
pub fn runs(id: &str) -> bool {
    HALVES.with_borrow(|h| h.iter().any(|a| &*a.half.id == id))
}

/// Stops plugin `id`'s app half (it turned off).
pub fn stop(id: &str) {
    let gone = HALVES.with_borrow_mut(|h| h.iter().position(|a| &*a.half.id == id).map(|i| h.remove(i)));
    if let Some(a) = gone {
        services::reap(&a.half, None);
        services::store::withdraw_owner(id, Kind::App, None);
    }
}

/// Polls every app half once per tick.
pub fn poll(app: &App, now: f64, tx: &mut Tx) -> bool {
    if POLLED.get() == now {
        return false;
    }
    POLLED.set(now);
    let updater_rev = crate::updater::revision_untracked(app);
    let changed = HALVES.with_borrow(|halves| {
        halves.iter().fold(false, |changed, a| {
            let before = a.half.epoch.get();
            let vm = a.script.vm_id().unwrap_or(0);
            let prompts = services::askpass::drain(&a.half, vm).into_iter().filter_map(|(name, args)| Some((a.half.signals.borrow().get(&(vm, name))?.clone(), args)));
            let mut fired: Vec<_> = services::process::drain(&a.half, vm).into_iter().chain(prompts).collect();
            if a.updater_rev.get() != updater_rev {
                a.updater_rev.set(updater_rev);
                if let Some(sig) = a.half.signals.borrow().get(&(vm, "Updater.StateChanged".to_owned())).cloned() {
                    fired.push((sig, crate::updater::state_data(app)));
                }
            }
            for (signal, args) in fired {
                if let Some(fire) = a.half.fire.borrow().get(&vm).cloned() {
                    let args = match args {
                        rune_luau::Data::List(items) => items,
                        other => vec![other],
                    };
                    cx::enter(app, || {
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
            let ran = cx::enter(app, || a.script.poll(now, tx));
            changed || before != a.half.epoch.get() || (!a.half.owns_redraw && ran)
        })
    });
    let sig = menubar_sig();
    if MENUBAR_SIG.get() != sig {
        MENUBAR_SIG.set(sig);
        crate::menubar::refresh();
    }
    changed
}

fn menubar_sig() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    HALVES.with_borrow(|halves| {
        for a in halves {
            for item in &a.half.contribs.borrow().menubar_items {
                item.key.hash(&mut h);
                item.spec.label.hash(&mut h);
                item.spec.enabled.hash(&mut h);
            }
        }
    });
    h.finish()
}

/// Runs `f` on every running app half.
pub fn each<R>(mut f: impl FnMut(&AppHalf) -> Option<R>) -> Vec<R> {
    HALVES.with_borrow(|h| h.iter().filter_map(&mut f).collect())
}
