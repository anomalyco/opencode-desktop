//! Each browser's icon for Settings > Browser (`browser_pane::cookies::icons`), read off the UI
//! thread: the first ask starts a worker for that browser and answers "not yet"; when the worker
//! ends it wakes the app, and the count of finished browsers moves `Dep::Browser`'s version, so the
//! page asks again. A browser is read once a run, whether or not it has an icon.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use browser_pane::cookies::icons::{AppIcon, application, application_icon};

/// What is known of one browser's icon: read, with or without one.
pub type Known = Option<Arc<AppIcon>>;

#[derive(Default)]
pub struct AppIcons {
    done: Arc<Mutex<HashMap<String, Known>>>,
    asked: RefCell<HashSet<String>>,
}

impl AppIcons {
    /// The icon of browser `source_id` once read (`Some(None)`: it has none), or `None` while it is
    /// being read, starting that read on the first ask.
    pub fn get(&self, source_id: &str, waker: Option<rune::Waker>) -> Option<Known> {
        if let Some(known) = self.done.lock().ok().and_then(|d| d.get(source_id).cloned()) {
            return Some(known);
        }
        if !self.asked.borrow_mut().insert(source_id.to_owned()) {
            return None;
        }
        let done = self.done.clone();
        let id = source_id.to_owned();
        let spawned = std::thread::Builder::new().name("browser-app-icon".into()).spawn(move || {
            let icon = application(&id).and_then(|path| application_icon(&path)).map(Arc::new);
            if let Ok(mut d) = done.lock() {
                d.insert(id, icon);
            }
            if let Some(w) = &waker {
                w();
            }
        });
        if spawned.is_err()
            && let Ok(mut d) = self.done.lock()
        {
            d.insert(source_id.to_owned(), None);
        }
        None
    }

    /// How many browsers' icons are read: part of `Dep::Browser`'s version.
    pub fn finished(&self) -> usize {
        self.done.lock().map(|d| d.len()).unwrap_or(0)
    }
}
