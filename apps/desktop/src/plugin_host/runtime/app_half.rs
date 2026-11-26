//! `AppPluginHost`: one `app.luau` VM per plugin. A different revision waits until the
//! last owner of the running revision leaves. Each VM polls only from a window that owns it.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
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
    revision: Revision,
    polled: Cell<f64>,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Revision {
    id: String,
    tier: Tier,
    hash: String,
    has_app: bool,
}

impl Revision {
    pub(super) fn new(manifest: &Manifest, tier: Tier, files: &[(String, String)]) -> Self {
        Self { id: manifest.id.clone(), tier, hash: super::sources::content_hash(files), has_app: manifest.app.is_some() }
    }
}

/// Holds keyed by slot token, including the one bundled plugin warmed before adoption.
#[derive(Default)]
pub(crate) struct Owners {
    slots: RefCell<Vec<(Rc<()>, Revision)>>,
    staged: RefCell<Option<(Rc<()>, Revision)>>,
}

impl Owners {
    pub(super) fn hold(&self, slot: &Rc<()>, revision: &Revision) {
        let mut slots = self.slots.borrow_mut();
        if slots.iter().any(|(token, held)| Rc::ptr_eq(token, slot) && held == revision) {
            return;
        }
        COUNTS.with_borrow_mut(|counts| *counts.entry(revision.clone()).or_default() += 1);
        slots.push((slot.clone(), revision.clone()));
    }

    pub(super) fn stage(&self, revision: &Revision) {
        let slot = Rc::new(());
        self.hold(&slot, revision);
        let previous = self.staged.replace(Some((slot.clone(), revision.clone())));
        if let Some((previous, _)) = previous {
            self.release(&previous);
        }
    }

    pub(super) fn adopt_staged(&self, slot: &Rc<()>, revision: &Revision) {
        let matches = self.staged.borrow().as_ref().is_some_and(|(_, held)| held == revision);
        if matches && let Some((staged, _)) = self.staged.take() {
            self.hold(slot, revision);
            self.release(&staged);
        }
    }

    pub(super) fn staged_for(&self, id: &str) -> Option<Revision> {
        self.staged.borrow().as_ref().filter(|(_, held)| held.id == id).map(|(_, held)| held.clone())
    }

    pub(super) fn discard_staged(&self, revision: &Revision) {
        let matches = self.staged.borrow().as_ref().is_some_and(|(_, held)| held == revision);
        if matches && let Some((staged, _)) = self.staged.take() {
            self.release(&staged);
        }
    }

    pub(super) fn release(&self, slot: &Rc<()>) {
        self.release_except(slot, None);
    }

    pub(super) fn release_except(&self, slot: &Rc<()>, keep: Option<&Revision>) {
        let gone: Vec<_> = self.slots.borrow_mut()
            .extract_if(.., |(token, held)| Rc::ptr_eq(token, slot) && Some(&*held) != keep)
            .map(|(_, revision)| revision)
            .collect();
        for revision in gone {
            leave(&revision);
        }
    }

    fn owns(&self, revision: &Revision) -> bool {
        self.slots.borrow().iter().any(|(_, held)| held == revision)
    }
}

impl Drop for Owners {
    fn drop(&mut self) {
        for (_, revision) in self.slots.get_mut().drain(..) {
            leave(&revision);
        }
    }
}

/// Drops one process-wide hold on plugin `id`, stopping its app half and what its server sources
/// listed once the last window let go.
fn leave(revision: &Revision) {
    let Some(last_for_id) = COUNTS
        .try_with(|counts| {
            let mut counts = counts.borrow_mut();
            match counts.get_mut(revision) {
                Some(n) if *n > 1 => {
                    *n -= 1;
                    None
                }
                Some(_) => {
                    counts.remove(revision);
                    Some(!counts.keys().any(|other| other.id == revision.id))
                }
                None => None,
            }
        })
        .ok().flatten() else { return };
    stop(revision);
    if last_for_id {
        crate::plugin_host::server_sources::forget(&revision.id);
    }
}

thread_local! {
    static HALVES: RefCell<Vec<AppHalf>> = const { RefCell::new(Vec::new()) };
    /// How many windows hold each plugin's app half.
    static COUNTS: RefCell<BTreeMap<Revision, usize>> = const { RefCell::new(BTreeMap::new()) };
    /// Hash of all app halves' menubar items after the last refresh.
    static MENUBAR_SIG: Cell<u64> = const { Cell::new(0) };
}

/// Starts plugin `manifest`'s app half from `entry`, unless it runs already.
pub(super) fn ensure(app: &App, revision: Revision, files: &[(String, String)], catalogs: Catalogs, sources: rune_luau::Sources) {
    if !available(&revision) || runs(&revision) {
        return;
    }
    let id = &revision.id;
    let half = Half::new(id, Kind::App, revision.tier);
    *half.catalogs.borrow_mut() = catalogs;
    super::sources::intern_icons(id, files);
    let script = cx::enter(app, || Script::with_options(&format!("{id}@app"), sources, vm::options(half.clone())));
    if let Some(w) = app.plugins.waker.clone() {
        script.set_waker(w);
    }
    if let Some(e) = script.error() {
        console::push(id, rune_luau::LogLevel::Error, &format!("app.luau: {}", e.message));
    }
    half.adopt(script.vm_id());
    HALVES.with_borrow_mut(|h| h.push(AppHalf { script, half, updater_rev: Cell::new(0), revision, polled: Cell::new(f64::NAN) }));
}

pub(super) fn runs(revision: &Revision) -> bool {
    HALVES.with_borrow(|h| h.iter().any(|a| &a.revision == revision))
}

pub(super) fn available(revision: &Revision) -> bool {
    COUNTS.with_borrow(|counts| counts.keys().all(|held| {
        held.id != revision.id || held == revision || (!held.has_app && !revision.has_app)
    }))
}

fn stop(revision: &Revision) {
    let gone = HALVES.try_with(|h| {
        let mut halves = h.try_borrow_mut().ok()?;
        let at = halves.iter().position(|a| &a.revision == revision)?;
        Some(halves.remove(at))
    });
    if let Ok(Some(a)) = gone {
        services::store::withdraw_owner(&revision.id, Kind::App, None);
        services::reap(&a.half, None);
    }
}

/// Polls every app half once per tick.
pub fn poll(app: &App, now: f64, tx: &mut Tx) -> bool {
    let updater_rev = crate::updater::revision_untracked(app);
    let changed = HALVES.with_borrow(|halves| {
        halves.iter().fold(false, |changed, a| {
            if !app.plugins.shared_plugin_owners.owns(&a.revision) || a.polled.replace(now) == now {
                return changed;
            }
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

pub(crate) fn each_in<R>(app: &App, mut f: impl FnMut(&AppHalf) -> Option<R>) -> Vec<R> {
    each(|half| app.plugins.shared_plugin_owners.owns(&half.revision).then(|| f(half)).flatten())
}

/// A dormant slot may dispatch to an already running copy of its admitted app half.
pub(crate) fn attach_dormant(app: &mut App, id: &str) {
    let locally_known = |app: &App| app.plugins.list.iter().any(|p| {
        p.manifest.id == id && !matches!(p.life, crate::plugin_host::Life::Refused(_))
    });
    if !locally_known(app) {
        super::boot::adopt_later_id(app, id);
    }
    let Some(p) = app.plugins.list.iter().find(|p| {
        p.manifest.id == id && matches!(p.life, crate::plugin_host::Life::Dormant) && p.trust.allows() && p.manifest.app.is_some()
    }) else { return };
    let revision = Revision::new(&p.manifest, p.tier, &p.origin.files());
    if runs(&revision) {
        app.plugins.shared_plugin_owners.hold(&p.half.token, &revision);
    }
}
