//! Page zoom remembered per site, like Chrome: zoom a site once and its pages open at that
//! zoom again, in every tab and after a relaunch. Sites at 100% aren't stored.
//!
//! Nothing touches the file until a page opens: a launch reads none of it before its first frame,
//! and the read and every write run off the UI thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use serde_json::Value;

type Levels = HashMap<String, f64>;

/// Where the levels are: in memory only, in a file not read yet, being read, or read.
pub(crate) enum SiteZoom {
    /// No file: the levels last as long as the app.
    Memory { levels: Levels },
    /// Kept in `file`, not read yet.
    Unread { file: PathBuf },
    /// `file` is being read on a thread of its own.
    Reading { file: PathBuf, read: Receiver<Levels> },
    /// Read; each change goes to `file` through `writer`'s thread.
    Read { levels: Levels, writer: Sender<String> },
}

impl Default for SiteZoom {
    fn default() -> Self {
        Self::Memory { levels: Levels::new() }
    }
}

impl SiteZoom {
    /// Keeps the levels in `file`, read once a page first opens.
    pub(crate) fn remember(file: PathBuf) -> Self {
        Self::Unread { file }
    }

    /// A page is opening: starts reading the file, so its zoom is in by the time the page loads.
    pub(crate) fn prepare(&mut self) {
        if let Self::Unread { file } = self {
            let file = std::mem::take(file);
            let (tx, read) = channel();
            let path = file.clone();
            let spawned = std::thread::Builder::new().name("browser-site-zoom-read".into()).spawn(move || {
                let _ = tx.send(read_levels(&path));
            });
            *self = match spawned {
                Ok(_) => Self::Reading { file, read },
                Err(_) => Self::Memory { levels: Levels::new() },
            };
        }
    }

    /// The zoom for `url`'s site, 1.0 when it has none (or isn't a site, like `about:blank`).
    pub(crate) fn get(&mut self, url: &str) -> f64 {
        site_of(url).and_then(|s| self.levels().get(&s).copied()).unwrap_or(1.0)
    }

    pub(crate) fn set(&mut self, url: &str, zoom: f64) {
        let Some(site) = site_of(url) else { return };
        let levels = self.levels();
        let changed = if (zoom - 1.0).abs() < 0.001 {
            levels.remove(&site).is_some()
        } else {
            levels.insert(site, zoom) != Some(zoom)
        };
        if changed && let Self::Read { levels, writer } = self {
            let map: serde_json::Map<String, Value> = levels.iter().map(|(k, v)| (k.clone(), Value::from(*v))).collect();
            let _ = writer.send(Value::Object(map).to_string());
        }
    }

    /// The levels, once the read is in (it waits for a read still running, which is a small file
    /// started as the page opened).
    fn levels(&mut self) -> &mut Levels {
        self.prepare();
        if let Self::Reading { file, read } = self {
            let levels = read.recv().unwrap_or_default();
            *self = Self::Read { levels, writer: spawn_writer(std::mem::take(file)) };
        }
        match self {
            Self::Memory { levels } | Self::Read { levels, .. } => levels,
            Self::Unread { .. } | Self::Reading { .. } => unreachable!("prepare and the read above leave neither"),
        }
    }
}

fn read_levels(file: &PathBuf) -> Levels {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|o| o.into_iter().filter_map(|(k, v)| Some((k, v.as_f64()?))).collect())
        .unwrap_or_default()
}

/// One thread writes `file`, the newest contents it was sent each time.
fn spawn_writer(file: PathBuf) -> Sender<String> {
    let (tx, rx) = channel::<String>();
    let _ = std::thread::Builder::new().name("browser-site-zoom-write".into()).spawn(move || {
        while let Ok(mut contents) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                contents = newer;
            }
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&file, contents);
        }
    });
    tx
}

/// `url`'s host and port, lowercased: `localhost:3000`, `github.com`. `None` without one.
pub(crate) fn site_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}
