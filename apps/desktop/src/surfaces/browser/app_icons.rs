//! Each browser's icon for Settings > Browser (`browser_pane::cookies::icons`), read off the UI
//! thread: the first ask starts a worker for that browser and answers "not yet"; when the worker
//! ends it says so (`Landed`) and wakes the app, whose poll then reports a change
//! (`App::poll_browser`), and the count of finished browsers moves `Dep::Browser`'s version, so
//! the page asks again. A browser is read once a run, whether or not it has an icon.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use browser_pane::cookies::icons::{AppIcon, application, application_icon};

/// A worker's news for the UI thread: a read or a listing ended since the app last took it. The
/// wake alone draws nothing: the app builds its view again only when a poll reports a change.
#[derive(Clone, Default)]
struct Landed(Arc<AtomicBool>);

impl Landed {
    fn tell(&self, waker: &Option<rune::Waker>) {
        self.0.store(true, Ordering::Release);
        if let Some(w) = waker {
            w();
        }
    }

    fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

/// What is known of one browser's icon: read, with or without one.
pub type Known = Option<Arc<AppIcon>>;

#[derive(Default)]
pub struct AppIcons {
    done: Arc<Mutex<HashMap<String, Known>>>,
    asked: RefCell<HashSet<String>>,
    landed: Landed,
    /// The other browsers and their profiles (`ImportSources`).
    pub sources: ImportSources,
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
        let landed = self.landed.clone();
        let id = source_id.to_owned();
        let spawned = std::thread::Builder::new().name("browser-app-icon".into()).spawn(move || {
            let icon = application(&id).and_then(|path| application_icon(&path)).map(Arc::new);
            if let Ok(mut d) = done.lock() {
                d.insert(id, icon);
            }
            landed.tell(&waker);
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

    /// Whether an icon's read or a listing of the browsers ended since the last call: the poll
    /// reports it as a change, so the page that asked is built again with it.
    pub fn take_landed(&self) -> bool {
        // Both are taken: `|` and not `||`.
        self.landed.take() | self.sources.landed.take()
    }
}

/// The other browsers and their profiles (`cookies::sources::list_sources`), listed off the UI
/// thread: it reads each browser's `Local State` / `profiles.ini`, which can be large. An ask returns
/// the last listing at once (`None` before the first is in: Desktop's `sourcesLoaded`,
/// `gui-plugins/src/browser/jobs.ts:313`) and, when that is older than `FRESH`, starts
/// another; its end says so (`Landed`), wakes the app and moves `Dep::Browser`'s version, so the
/// page and the pane's sign-in offer draw again with it.
#[derive(Default)]
pub struct ImportSources {
    listed: Arc<Mutex<Option<(rune::time::Instant, Vec<browser_pane::cookies::BrowserSource>)>>>,
    listing: Arc<AtomicBool>,
    generation: Arc<AtomicUsize>,
    landed: Landed,
}

const FRESH: std::time::Duration = std::time::Duration::from_secs(5);

impl ImportSources {
    pub fn get(&self, waker: Option<rune::Waker>) -> Option<Vec<browser_pane::cookies::BrowserSource>> {
        let current = self.listed.lock().ok().and_then(|l| l.clone());
        let stale = current.as_ref().is_none_or(|(at, _)| at.elapsed() >= FRESH);
        if stale && !self.listing.swap(true, Ordering::AcqRel) {
            let listed = self.listed.clone();
            let listing = self.listing.clone();
            let generation = self.generation.clone();
            let landed = self.landed.clone();
            let spawned = std::thread::Builder::new().name("browser-import-sources".into()).spawn(move || {
                let sources = browser_pane::cookies::sources::list_sources();
                if let Ok(mut l) = listed.lock() {
                    *l = Some((rune::time::Instant::now(), sources));
                }
                generation.fetch_add(1, Ordering::AcqRel);
                listing.store(false, Ordering::Release);
                landed.tell(&waker);
            });
            if spawned.is_err() {
                self.listing.store(false, Ordering::Release);
                // No worker to list them: none are listed, which the page can say.
                if let Ok(mut l) = self.listed.lock()
                    && l.is_none()
                {
                    *l = Some((rune::time::Instant::now(), Vec::new()));
                }
            }
        }
        current.map(|(_, s)| s)
    }

    /// Moves each time a listing lands: part of `Dep::Browser`'s version.
    pub fn generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }
}
