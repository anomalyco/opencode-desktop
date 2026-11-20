//! One plugin half's state shared with its bindings: what its setup contributed, the signals
//! the host fires into it, its editors, its cached server reads and what its surfaces read.
//! The bindings hold it as `Rc<Half>`; the host holds the same `Rc` beside the script.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Function, Value};

pub use opencode_plugin_runtime::contribs::{Contrib, Contribs, Kind};
pub use opencode_plugin_runtime::luau::Resource;

use crate::plugin_host::cx::Dep;

pub struct Half {
    pub id: Rc<str>,
    pub kind: Kind,
    pub tier: crate::plugin_host::Tier,
    /// Ownership token tracked by process-wide services (`askpass`) to prune entries when dropped.
    pub token: Rc<()>,
    /// A migrated bundled half explicitly invalidates its surfaces; other 1.0 plugins keep the implicit redraw.
    pub owns_redraw: bool,
    /// The VM whose setup filled `made`, waiting for the host to adopt it.
    pub made: RefCell<Option<(u64, Contribs)>>,
    /// What the running VM contributed.
    pub contribs: RefCell<Contribs>,
    /// Host signals by VM and name (`Sessions.Changed`), made as a field is first read.
    pub signals: RefCell<HashMap<(u64, String), Value>>,
    /// `Server:OnEvent(prefix)` signals, by VM.
    pub events: RefCell<Vec<(u64, String, Value)>>,
    /// Fires a script signal, by VM: `function(s, ...) s:Fire(...) end`.
    pub fire: RefCell<HashMap<u64, Function>>,
    /// The editors the running setup made, by creation order: the n-th is the window's n-th
    /// field of this plugin, kept across reloads.
    pub editors_made: Cell<usize>,
    pub resources: RefCell<HashMap<String, Resource>>,
    /// What each surface read when it last ran, by surface id.
    pub reads: RefCell<HashMap<String, Vec<Dep>>>,
    pub catalogs: Rc<RefCell<crate::plugin_host::Catalogs>>,
    /// Supervised processes (`Process:Spawn` and `Process:RunAsync`) owned by this half.
    pub procs: Rc<opencode_plugin_runtime::process::service::Procs>,
    /// What the half holds of the host's process-wide services (display locks, prompt
    /// listeners), by VM, let go with the half.
    pub held: RefCell<Vec<(u64, crate::plugin_host::services::Held)>>,
    /// Bumped when anything a surface may read from this half changes outside a render.
    pub epoch: Cell<u64>,
    /// What each watched signal saw last (`services::watch`).
    pub seen: RefCell<HashMap<String, u64>>,
    /// What `Sessions.Changed` read when it last compared the sessions' versions.
    pub(crate) sessions_read: Cell<Option<crate::plugin_host::services::SessionInputs>>,
    /// Shell slots whose render failed in the running VM: they draw nothing until a reload.
    pub failed: RefCell<std::collections::HashSet<String>>,
}

impl Half {
    pub fn new(id: &str, kind: Kind, tier: crate::plugin_host::Tier) -> Rc<Half> {
        Rc::new(Half {
            id: id.into(),
            kind,
            tier,
            token: Rc::new(()),
            owns_redraw: super::redraw::explicit(tier, id),
            made: RefCell::default(),
            contribs: RefCell::default(),
            signals: RefCell::default(),
            events: RefCell::default(),
            fire: RefCell::default(),
            editors_made: Cell::new(0),
            resources: RefCell::default(),
            reads: RefCell::default(),
            catalogs: Rc::default(),
            procs: Rc::default(),
            held: RefCell::default(),
            epoch: Cell::new(0),
            seen: RefCell::default(),
            sessions_read: Cell::default(),
            failed: RefCell::default(),
        })
    }

    /// Takes what VM `vm` contributed, if its setup is the one that ran last. Forgets the
    /// signals, events and cached values of the VM it replaces.
    pub fn adopt(&self, vm: Option<u64>) -> bool {
        let made = self.made.borrow_mut().take();
        match (made, vm) {
            (Some((made_by, contribs)), Some(vm)) if made_by == vm => {
                *self.contribs.borrow_mut() = contribs;
                self.signals.borrow_mut().retain(|(v, _), _| *v == vm);
                self.events.borrow_mut().retain(|(v, _, _)| *v == vm);
                self.fire.borrow_mut().retain(|v, _| *v == vm);
                self.resources.borrow_mut().values_mut().for_each(|r| r.value = None);
                self.reads.borrow_mut().clear();
                self.failed.borrow_mut().clear();
                self.epoch.set(self.epoch.get() + 1);
                true
            }
            _ => false,
        }
    }
}
