//! Page zoom remembered per site, like Chrome: zoom a site once and its pages open at that
//! zoom again, in every tab and after a relaunch. Sites at 100% aren't stored.
//!
//! The app keeps the levels (its database, as JSON text): this crate reads them through the app
//! once a page first opens and hands every change back, touching no file and starting no thread.

use std::collections::HashMap;

use serde_json::Value;

type Levels = HashMap<String, f64>;

/// Where the app keeps the levels: `read` gives their JSON text, `write` takes the new text.
pub struct ZoomStore {
    pub read: Box<dyn Fn() -> Option<String>>,
    pub write: Box<dyn Fn(String)>,
}

/// Where the levels are: in memory only, kept by the app but not read yet, or read.
pub(crate) enum SiteZoom {
    /// Nowhere else: the levels last as long as the app.
    Memory { levels: Levels },
    /// Kept by `store`, not read yet.
    Unread { store: ZoomStore },
    /// Read; each change goes back to `store`.
    Read { levels: Levels, store: ZoomStore },
}

impl Default for SiteZoom {
    fn default() -> Self {
        Self::Memory { levels: Levels::new() }
    }
}

impl SiteZoom {
    /// Keeps the levels in `store`, read once a page first opens.
    pub(crate) fn remember(store: ZoomStore) -> Self {
        Self::Unread { store }
    }

    /// A page is opening: the levels come in, so its zoom is known by the time the page loads.
    pub(crate) fn prepare(&mut self) {
        if !matches!(self, Self::Unread { .. }) {
            return;
        }
        let Self::Unread { store } = std::mem::take(self) else { return };
        let levels = (store.read)().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).map(|o| o.into_iter().filter_map(|(k, v)| Some((k, v.as_f64()?))).collect()).unwrap_or_default();
        *self = Self::Read { levels, store };
    }

    /// The zoom for `url`'s site, 1.0 when it has none (or isn't a site, like `about:blank`).
    pub(crate) fn get(&mut self, url: &str) -> f64 {
        self.prepare();
        match (self, site_of(url)) {
            (Self::Memory { levels } | Self::Read { levels, .. }, Some(site)) => levels.get(&site).copied().unwrap_or(1.0),
            _ => 1.0,
        }
    }

    pub(crate) fn set(&mut self, url: &str, zoom: f64) {
        self.prepare();
        let Some(site) = site_of(url) else { return };
        let (levels, store) = match self {
            Self::Memory { levels } => (levels, None),
            Self::Read { levels, store } => (levels, Some(store)),
            Self::Unread { .. } => return,
        };
        let changed = if (zoom - 1.0).abs() < 0.001 { levels.remove(&site).is_some() } else { levels.insert(site, zoom) != Some(zoom) };
        if changed && let Some(store) = store {
            let map: serde_json::Map<String, Value> = levels.iter().map(|(k, v)| (k.clone(), Value::from(*v))).collect();
            (store.write)(Value::Object(map).to_string());
        }
    }
}

/// `url`'s host and port, lowercased: `localhost:3000`, `github.com`. `None` without one.
pub(crate) fn site_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}
