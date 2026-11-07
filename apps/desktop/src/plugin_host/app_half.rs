//! A plugin's app half, once per process (`plugins-design.md` 2.1: one `AppPlugin` per
//! process, one `WindowPlugin` per window). The first window to set a plugin up runs its app
//! setup; every later window's instance of that plugin library shares the same half, its app
//! menu items and the services it provided, and runs only its own window setup. The half goes
//! when the last window holding it lets go.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use opencode_plugin::abi::{DropFn, DropStateFn};

use super::instance::ContribRec;
use super::loader::LoadedLibrary;

/// A service the app half provided: its contract id, type hash and value.
pub struct Service {
    pub id: String,
    pub type_hash: u64,
    pub value: usize,
    drop_value: Option<DropFn>,
}

impl Service {
    pub fn new(id: String, type_hash: u64, value: usize, drop_value: Option<DropFn>) -> Service {
        Service { id, type_hash, value, drop_value }
    }
}

pub struct AppHalf {
    pub state: *mut c_void,
    /// Its app menu items.
    pub contribs: Vec<ContribRec>,
    /// The services it provided, which every window's contracts list without owning them.
    pub services: Vec<Service>,
    drop_state: Option<DropStateFn>,
    /// Which build it is (swap::library_identity), so a rebuilt library gets a new half.
    entry: u64,
    /// Dropped last, after the state and every callback.
    pub lib: Arc<LoadedLibrary>,
}

impl AppHalf {
    pub fn new(state: *mut c_void, drop_state: Option<DropStateFn>, entry: u64, lib: Arc<LoadedLibrary>) -> AppHalf {
        AppHalf { state, contribs: Vec::new(), services: Vec::new(), drop_state, entry, lib }
    }
}

impl Drop for AppHalf {
    fn drop(&mut self) {
        // Contributions first: their captures may point into the state.
        self.contribs.clear();
        for s in self.services.drain(..) {
            if let Some(drop_value) = s.drop_value {
                // SAFETY: the provider's destructor for exactly this value, called once.
                unsafe { drop_value(s.value) };
            }
        }
        let (Some(drop_state), false) = (self.drop_state, self.state.is_null()) else { return };
        // SAFETY: the state this half's setup returned, dropped once; the plugin catches a panic.
        let out = unsafe { drop_state(std::mem::replace(&mut self.state, std::ptr::null_mut())) };
        if let Err((_, message)) = out.into_result() {
            super::tasks::log(format!("opencode-plugin: dropping a plugin's app state failed: {message}"));
        }
    }
}

thread_local! {
    /// Each plugin's app half, while a window holds it (the UI thread's, which every window shares).
    static HALVES: RefCell<HashMap<String, Weak<AppHalf>>> = RefCell::default();
}

/// Plugin `id`'s running app half, if it's this code's (`entry`).
pub fn find(id: &str, entry: u64) -> Option<Rc<AppHalf>> {
    HALVES.with(|h| h.borrow().get(id).and_then(Weak::upgrade)).filter(|half| half.entry == entry)
}

/// Plugin `id`'s app half from now on, for the windows that set it up after this.
pub fn publish(id: &str, half: &Rc<AppHalf>) {
    HALVES.with(|h| {
        let mut h = h.borrow_mut();
        h.retain(|_, w| w.strong_count() > 0);
        h.insert(id.to_owned(), Rc::downgrade(half));
    });
}
