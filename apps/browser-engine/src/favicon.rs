//! Per-tab favicon fetching via CEF's `BrowserHost::download_image` (`UP-109` `#65094`,
//! `packages/gui-plugins/src/browser/{icon,icon-image}.ts`).
//!
//! Fetches candidate favicon URLs through Chromium's own network/partition stack (auto-decoding
//! `.ico`, `.png`, `.jpg`, `.webp`, `.gif` to a <= 32 px `cef::Image`), encodes as a bounded
//! `data:image/png;base64,...` data URL (<= 64 KiB), and guards against stale navigations using
//! a per-tab document generation counter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use cef::rc::Rc as _;
use cef::{
    Browser, CefString, DownloadImageCallback, Image, ImplBinaryValue, ImplBrowser,
    ImplBrowserHost, ImplDownloadImageCallback, ImplImage, WrapDownloadImageCallback,
    wrap_download_image_callback,
};

use crate::output::Output;
use crate::wire::{EngineMsg, TabSnapshot};

/// Twice the 16 px drawn size so tab icons stay crisp on 2x displays (`icon-image.ts`).
const ICON_SIZE: u32 = 32;
/// Maximum encoded `data:image/png;base64,...` length carried over IPC (`ipc.ts` `MAX_ICON_URL`).
const MAX_ICON_URL: usize = 65_536;
const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/// Tracks the active document generation for a tab so late favicon callbacks from an earlier
/// navigation never overwrite the current page's icon.
#[derive(Default)]
pub struct TabFavicon {
    generation: AtomicU64,
}

impl TabFavicon {
    /// Called on main-frame address changes to clear any stale favicon and invalidate in-flight loads.
    pub fn on_navigation(&self, snap: &mut TabSnapshot, origin_changed: bool) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        if origin_changed {
            snap.favicon = None;
        }
    }

    /// Starts fetching the first allowed candidate favicon URL for the current document generation.
    /// Preview tabs enforce strict isolation: artifacts fetch zero favicons (`chromium.ts:644-645`),
    /// and local/remote workspace previews permit ONLY `data:image/*` or their own `oc-preview://`
    /// grant (never `http://` or `https://`).
    pub fn fetch_candidates(
        self: &Arc<Self>,
        browser: &Browser,
        candidates: Vec<String>,
        preview: Option<&Arc<crate::preview::PreviewGrant>>,
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    ) {
        if preview.is_some_and(|g| g.is_artifact()) {
            return;
        }
        let allowed: Vec<String> = candidates
            .into_iter()
            .filter(|u| is_allowed_favicon_url(u, preview))
            .take(4)
            .collect();
        if allowed.is_empty() {
            return;
        }
        let generation = self.generation.load(Ordering::Relaxed);
        self.try_next(browser, allowed, 0, generation, snap, out_tx);
    }

    fn try_next(
        self: &Arc<Self>,
        browser: &Browser,
        candidates: Vec<String>,
        index: usize,
        generation: u64,
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    ) {
        let Some(url) = candidates.get(index) else { return };
        let Some(host) = browser.host() else { return };
        let mut cb = FaviconDownloadCallback::new(
            Arc::clone(self),
            browser.clone(),
            candidates.clone(),
            index,
            generation,
            snap,
            out_tx,
        );
        host.download_image(
            Some(&CefString::from(url.as_str())),
            1, // is_favicon
            ICON_SIZE,
            0, // bypass_cache
            Some(&mut cb),
        );
    }
}

fn is_allowed_favicon_url(url: &str, preview: Option<&Arc<crate::preview::PreviewGrant>>) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_ICON_URL {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let is_safe_data = lower.starts_with("data:image/png;base64,")
        || lower.starts_with("data:image/jpeg;base64,")
        || lower.starts_with("data:image/x-icon;base64,")
        || lower.starts_with("data:image/vnd.microsoft.icon;base64,");
    if let Some(grant) = preview {
        if grant.is_artifact() || grant.is_revoked() {
            return false;
        }
        return is_safe_data || grant.compute_display_url(trimmed).is_some();
    }
    is_safe_data || browser_policy::destination_origin(trimmed).is_some()
}

fn encode_png_data_url(image: &Image) -> Option<String> {
    if image.is_empty() != 0 {
        return None;
    }
    let (mut w, mut h) = (0, 0);
    let bin = image.as_png(1.0, 1, Some(&mut w), Some(&mut h))?;
    let size = bin.size();
    if size < PNG_MAGIC.len() || size > 48 * 1024 {
        return None;
    }
    let mut bytes = vec![0u8; size];
    let read = bin.data(Some(&mut bytes), 0);
    if read < PNG_MAGIC.len() || !bytes.starts_with(&PNG_MAGIC) {
        return None;
    }
    bytes.truncate(read);
    let data_url = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    (data_url.len() <= MAX_ICON_URL).then_some(data_url)
}

wrap_download_image_callback! {
    struct FaviconDownloadCallback {
        favicon: Arc<TabFavicon>,
        browser: Browser,
        candidates: Vec<String>,
        index: usize,
        generation: u64,
        snap: Arc<Mutex<TabSnapshot>>,
        out_tx: Output,
    }

    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self,
            _image_url: Option<&CefString>,
            http_status_code: ::std::os::raw::c_int,
            image: Option<&mut Image>,
        ) {
            if self.favicon.generation.load(Ordering::Relaxed) != self.generation {
                return;
            }
            let status_ok = http_status_code == 0 || (200..300).contains(&http_status_code);
            if status_ok
                && let Some(img) = image
                && let Some(data_url) = encode_png_data_url(img)
            {
                let mut s = self.snap.lock().unwrap();
                if s.favicon.as_deref() != Some(data_url.as_str()) {
                    s.favicon = Some(data_url);
                    let _ = self.out_tx.send(EngineMsg::TabUpdate { tab: s.clone() });
                }
                return;
            }
            if self.index + 1 < self.candidates.len() {
                self.favicon.try_next(
                    &self.browser,
                    self.candidates.clone(),
                    self.index + 1,
                    self.generation,
                    Arc::clone(&self.snap),
                    self.out_tx.clone(),
                );
            }
        }
    }
}
