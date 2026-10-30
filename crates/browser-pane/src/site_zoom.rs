//! Page zoom remembered per site, like Chrome: zoom a site once and its pages open at that
//! zoom again, in every tab and after a relaunch. Sites at 100% aren't stored.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

#[derive(Default)]
pub(crate) struct SiteZoom {
    levels: HashMap<String, f64>,
    file: Option<PathBuf>,
}

impl SiteZoom {
    /// Reads `file` and saves every change back to it.
    pub(crate) fn load(file: PathBuf) -> Self {
        let levels = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| v.as_object().cloned())
            .map(|o| o.into_iter().filter_map(|(k, v)| Some((k, v.as_f64()?))).collect())
            .unwrap_or_default();
        Self { levels, file: Some(file) }
    }

    /// The zoom for `url`'s site, 1.0 when it has none (or isn't a site, like `about:blank`).
    pub(crate) fn get(&self, url: &str) -> f64 {
        site_of(url).and_then(|s| self.levels.get(&s).copied()).unwrap_or(1.0)
    }

    pub(crate) fn set(&mut self, url: &str, zoom: f64) {
        let Some(site) = site_of(url) else { return };
        let changed = if (zoom - 1.0).abs() < 0.001 {
            self.levels.remove(&site).is_some()
        } else {
            self.levels.insert(site, zoom) != Some(zoom)
        };
        if changed && let Some(file) = &self.file {
            let map: serde_json::Map<String, Value> =
                self.levels.iter().map(|(k, v)| (k.clone(), Value::from(*v))).collect();
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(file, Value::Object(map).to_string());
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
