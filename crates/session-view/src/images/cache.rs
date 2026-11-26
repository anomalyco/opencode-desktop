//! Bounded thumbnail scaling, pixel-weight estimation, and explicit fetch/slot state for
//! `MediaStore`. Keeps decoded RGBA8 mip chains within a strict CPU memory budget during
//! scrolling and tab switches while preserving layout dimensions and encoded bytes for instant
//! re-decode.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Weak};

use rune::{Image, ImageCache, Waker};

use super::{Fetch, Fetched};

/// Maximum logical side (points) for timeline gallery thumbnails (single tiles fit 320×240 pt;
/// bento hero tiles up to ~480 pt). At 2× DPI this stays <= 960 px, packing into Rune's shared
/// 1024×1024 atlas shelf pages instead of standalone exact-size CPU/GPU pages.
pub const THUMB_MAX_PT: f32 = 480.0;
/// Maximum logical side (points) for markdown reply images.
pub const MD_MAX_PT: f32 = 720.0;
/// Maximum logical side (points) for favicons.
pub const FAVICON_MAX_PT: f32 = 32.0;

/// Maximum decoded CPU pixel bytes kept in `MediaStore` (inline + `media:` refs combined).
pub const PIXEL_BUDGET: usize = 48 << 20;
/// Tighter decoded pixel budget kept when resting (leaves room for a full 2× viewport).
pub const REST_PIXEL_BUDGET: usize = 24 << 20;
/// Maximum encoded `media:` ref bytes kept in `MediaStore`.
pub const REF_BYTES_BUDGET: usize = 24 << 20;
/// Cap on auxiliary metadata caches (`ref_sizes`, `inline_sizes`, `shas`).
pub const MAX_META_ENTRIES: usize = 512;

/// Current display scale factor for thumbnail sizing, clamped to a safe range.
pub(crate) fn display_scale() -> f32 {
    crate::scroller::device_scale().clamp(1.0, 4.0)
}

/// Target device pixel side matching `rune_image::ImageCache::load_scaled`.
pub(crate) fn target_px(size_pt: f32, scale: f32) -> u32 {
    let raw = size_pt * scale;
    if !raw.is_finite() || raw <= 1.0 { 1 } else { raw.ceil().clamp(1.0, 8192.0) as u32 }
}

/// The image's upright header dimensions without decoding its pixels.
pub(crate) fn dimensions(bytes: &[u8]) -> Option<[u32; 2]> {
    use image::ImageDecoder;
    use image::metadata::Orientation::{Rotate90, Rotate90FlipH, Rotate270, Rotate270FlipH};
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    let turned = matches!(decoder.orientation(), Ok(Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH));
    (w > 0 && h > 0).then_some(if turned { [h, w] } else { [w, h] })
}

/// Decoded RGBA8 + linear-light mip-chain weight in bytes for an image downscaled to fit
/// `max_px` square. Falls back to a conservative estimate when header dimensions aren't readable
/// (e.g. SVG/AVIF) so every entry still counts against the budget.
pub(crate) fn scaled_weight(size: Option<[u32; 2]>, max_px: u32) -> usize {
    let Some([w, h]) = size else {
        let side = max_px.min(512) as usize;
        return (side * side * 16 / 3).max(64 * 1024);
    };
    let longest = w.max(h);
    let (nw, nh) = if max_px == 0 || longest <= max_px {
        (w as usize, h as usize)
    } else if w >= h {
        let nh = ((f64::from(h) * f64::from(max_px)) / f64::from(w)).round().clamp(1.0, f64::from(max_px)) as usize;
        (max_px as usize, nh)
    } else {
        let nw = ((f64::from(w) * f64::from(max_px)) / f64::from(h)).round().clamp(1.0, f64::from(max_px)) as usize;
        (nw, max_px as usize)
    };
    (nw.saturating_mul(nh).saturating_mul(16) / 3).max(4096)
}

fn is_animated_mime(mime: &str) -> bool {
    matches!(mime, "image/gif" | "image/webp" | "image/apng")
}

/// Decodes a thumbnail into `cache`, downscaling oversized rasters before mipmap generation via
/// `ImageCache::load_scaled`. Small animated GIF/WebP/APNG images already within `max_pt * scale`
/// keep full `load` so timeline animations continue to play without risking 256 MiB 4K loops.
/// Effective `max_pt` for markdown images: tall portrait images (`h > w`) scale up their
/// long-side target so their decoded width still fills the column up to `max_pt` (capped at 1440 pt).
pub(crate) fn md_effective_pt(size: Option<[u32; 2]>, max_pt: f32) -> f32 {
    match size {
        Some([w, h]) if h > w && w > 0 => (max_pt * (h as f32 / w as f32)).min(1440.0),
        _ => max_pt,
    }
}

pub(crate) fn load_thumb(
    cache: &ImageCache,
    bytes: Arc<[u8]>,
    mime: &str,
    size: Option<[u32; 2]>,
    max_pt: f32,
    scale: f32,
) -> (Image, u32) {
    let max_px = target_px(max_pt, scale);
    // Small SVGs rasterize at 2× their intrinsic size via `load` (avoiding upscaling a 24×24 SVG
    // to 960 px), and small animated GIF/WebP/APNG within `max_px` keep playing inline.
    let keep_native = mime == "image/svg+xml"
        || (is_animated_mime(mime) && size.is_some_and(|[w, h]| w.max(h) <= max_px));
    let image = if keep_native { cache.load(bytes) } else { cache.load_scaled(bytes, max_pt, scale) };
    (image, max_px)
}

/// Unified per-ref entry state inside `MediaStore`.
pub(crate) enum RefSlot {
    /// Encoded bytes kept without decoded CPU pixels; re-decodes without refetching.
    Held { bytes: Arc<[u8]>, size: Option<[u32; 2]>, last_used: u64 },
    /// Decoded thumbnail pixels plus optional encoded bytes (`None` when byte budget stripped them
    /// so an active decoded image stays visible without triggering an HTTP re-fetch loop).
    Decoded { image: Image, bytes: Option<Arc<[u8]>>, size: Option<[u32; 2]>, max_px: u32, weight: usize, last_used: u64, last_frame: u64 },
}

pub(crate) enum Demote {
    Unchanged,
    ToHeld(Arc<[u8]>, usize),
    DropEntry(usize),
}

impl RefSlot {
    pub(crate) fn bytes(&self) -> Option<&Arc<[u8]>> {
        match self {
            RefSlot::Held { bytes, .. } => Some(bytes),
            RefSlot::Decoded { bytes, .. } => bytes.as_ref(),
        }
    }

    pub(crate) fn bytes_len(&self) -> usize {
        self.bytes().map_or(0, |b| b.len())
    }

    pub(crate) fn size(&self) -> Option<[u32; 2]> {
        match self {
            RefSlot::Held { size, .. } => *size,
            RefSlot::Decoded { size, image, .. } => (*size).or_else(|| image.size()),
        }
    }

    pub(crate) fn last_used(&self) -> u64 {
        match self {
            RefSlot::Held { last_used, .. } | RefSlot::Decoded { last_used, .. } => *last_used,
        }
    }

    pub(crate) fn last_frame(&self) -> u64 {
        match self {
            RefSlot::Held { .. } => 0,
            RefSlot::Decoded { last_frame, .. } => *last_frame,
        }
    }

    pub(crate) fn pixel_weight(&self) -> usize {
        match self {
            RefSlot::Held { .. } => 0,
            RefSlot::Decoded { weight, .. } => *weight,
        }
    }

    pub(crate) fn demote(&mut self) -> Demote {
        match self {
            RefSlot::Held { .. } => Demote::Unchanged,
            RefSlot::Decoded { bytes: Some(bytes), size, weight, last_used, .. } => {
                let b = bytes.clone();
                let freed = *weight;
                *self = RefSlot::Held { bytes: b.clone(), size: *size, last_used: *last_used };
                Demote::ToHeld(b, freed)
            }
            RefSlot::Decoded { bytes: None, weight, .. } => Demote::DropEntry(*weight),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
type FetchQueue = (Mutex<Vec<String>>, Condvar);

/// Explicit lifecycle state of `Media`'s ref-fetch worker.
#[derive(Default)]
enum FetcherState {
    #[default]
    Unset,
    Ready { fetch: Fetch },
    #[cfg(not(target_arch = "wasm32"))]
    Running { fetch: Fetch, queue: Arc<FetchQueue> },
}

enum FetcherEvent {
    SetFetch(Fetch),
    #[cfg(not(target_arch = "wasm32"))]
    Start(Arc<FetchQueue>),
}

fn step_fetcher(state: FetcherState, event: FetcherEvent) -> FetcherState {
    match (state, event) {
        (_, FetcherEvent::SetFetch(fetch)) => FetcherState::Ready { fetch },
        #[cfg(not(target_arch = "wasm32"))]
        (FetcherState::Ready { fetch }, FetcherEvent::Start(queue)) => FetcherState::Running { fetch, queue },
        #[cfg(not(target_arch = "wasm32"))]
        (state, FetcherEvent::Start(_)) => state,
    }
}

/// Ref fetches on one LIFO worker thread so the newest (bottom of transcript) image lands first.
#[derive(Default)]
pub struct Media {
    fetcher: std::cell::RefCell<FetcherState>,
    pub(super) state: Arc<Mutex<HashMap<String, Fetched>>>,
    pub(super) landed: Arc<AtomicBool>,
}

impl Media {
    pub(super) fn set_fetch(&mut self, fetch: Fetch) {
        let mut f = self.fetcher.borrow_mut();
        *f = step_fetcher(std::mem::take(&mut *f), FetcherEvent::SetFetch(fetch));
    }

    pub fn get(&self, hash: &str, waker: Option<&Waker>) -> Fetched {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = state.get(hash) {
            return f.clone();
        }
        let fetch = match &*self.fetcher.borrow() {
            FetcherState::Unset => {
                drop(state);
                return Fetched::Failed("no server".into());
            }
            FetcherState::Ready { fetch } => fetch.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            FetcherState::Running { fetch, .. } => fetch.clone(),
        };
        state.insert(hash.to_owned(), Fetched::Loading);
        drop(state);

        #[cfg(target_arch = "wasm32")]
        {
            let (state, landed, hash) = (self.state.clone(), self.landed.clone(), hash.to_owned());
            let wake = waker.cloned().unwrap_or_else(|| Arc::new(|| {}));
            rune::task::spawn("opencode-media", move || {
                let result = match fetch(&hash) {
                    Ok(bytes) => Fetched::Ready(bytes.into()),
                    Err(e) => Fetched::Failed(e),
                };
                state.lock().unwrap_or_else(|e| e.into_inner()).insert(hash, result);
                landed.store(true, Ordering::Release);
                wake();
            })
            .ok();
            Fetched::Loading
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let q = {
                let mut f = self.fetcher.borrow_mut();
                if let FetcherState::Running { queue, .. } = &*f {
                    queue.clone()
                } else {
                    let q = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
                    let wake = waker.cloned().unwrap_or_else(|| Arc::new(|| {}));
                    spawn_fetcher(Arc::downgrade(&q), self.state.clone(), self.landed.clone(), fetch, wake);
                    *f = step_fetcher(std::mem::take(&mut *f), FetcherEvent::Start(q.clone()));
                    q
                }
            };
            q.0.lock().unwrap_or_else(|e| e.into_inner()).push(hash.to_owned());
            q.1.notify_one();
            Fetched::Loading
        }
    }

    pub(super) fn forget(&self, hash: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).remove(hash);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_fetcher(
    queue: Weak<FetchQueue>,
    state: Arc<Mutex<HashMap<String, Fetched>>>,
    landed: Arc<AtomicBool>,
    fetch: Fetch,
    wake: Waker,
) {
    if let Err(e) = std::thread::Builder::new().name("opencode-media".into()).spawn(move || {
        loop {
            let Some(q) = queue.upgrade() else { return };
            let next = {
                let mut stack = q.0.lock().unwrap_or_else(|e| e.into_inner());
                if stack.is_empty() {
                    stack = match q.1.wait_timeout(stack, std::time::Duration::from_secs(1)) {
                        Ok((g, _)) => g,
                        Err(e) => e.into_inner().0,
                    };
                }
                stack.pop()
            };
            drop(q);
            let Some(hash) = next else { continue };
            let result = match fetch(&hash) {
                Ok(bytes) => Fetched::Ready(bytes.into()),
                Err(e) => Fetched::Failed(e),
            };
            state.lock().unwrap_or_else(|e| e.into_inner()).insert(hash, result);
            landed.store(true, Ordering::Release);
            wake();
        }
    }) {
        panic!("spawn media thread: {e}");
    }
}

/// Clears `map` once it exceeds `MAX_META_ENTRIES` to bound auxiliary metadata memory.
pub(crate) fn cap_map<K, V>(map: &mut HashMap<K, V>) {
    if map.len() > MAX_META_ENTRIES {
        map.clear();
    }
}

pub(crate) enum Pin<'a> {
    None,
    Inline((usize, usize)),
    Ref(&'a str),
}

pub(crate) enum RefEvict {
    Demoted(String, Arc<[u8]>),
    Forgot(String),
}

/// Evicts least-recently-used decoded thumbnails across `images` and `refs` until total decoded
/// pixel weight fits `budget`, never evicting `pin` or any image touched on `cur_frame` (which
/// prevents same-frame re-decode thrash and preserves visible images across `App::rest`).
pub(crate) fn evict_pixels(
    images: &mut HashMap<(usize, usize), super::InlineEntry>,
    refs: &mut HashMap<String, RefSlot>,
    budget: usize,
    pin: Pin<'_>,
    cur_frame: Option<u64>,
) -> Vec<RefEvict> {
    let mut total: usize = images.values().map(|e| e.prepared.weight()).sum::<usize>()
        + refs.values().map(RefSlot::pixel_weight).sum::<usize>();
    if total <= budget {
        return Vec::new();
    }
    enum Victim { Inline((usize, usize)), Ref(String) }
    let mut changes = Vec::new();
    while total > budget {
        let oldest_inline = images
            .iter()
            .filter(|(k, e)| !matches!(pin, Pin::Inline(pk) if pk == **k) && cur_frame != Some(e.last_frame))
            .min_by_key(|(_, e)| e.last_used)
            .map(|(&k, e)| (k, e.last_used));
        let oldest_ref = refs
            .iter()
            .filter(|(k, s)| s.pixel_weight() > 0 && !matches!(pin, Pin::Ref(pr) if pr == k.as_str()) && cur_frame != Some(s.last_frame()))
            .min_by_key(|(_, s)| s.last_used())
            .map(|(k, s)| (k.clone(), s.last_used()));
        let victim = match (oldest_inline, oldest_ref) {
            (Some((k, ti)), Some((r, tr))) => if ti <= tr { Victim::Inline(k) } else { Victim::Ref(r) },
            (Some((k, _)), None) => Victim::Inline(k),
            (None, Some((r, _))) => Victim::Ref(r),
            (None, None) => break,
        };
        match victim {
            Victim::Inline(k) => {
                if let Some(removed) = images.remove(&k) {
                    total = total.saturating_sub(removed.prepared.weight());
                }
            }
            Victim::Ref(hash) => {
                let demote = refs.get_mut(&hash).map(RefSlot::demote).unwrap_or(Demote::Unchanged);
                match demote {
                    Demote::Unchanged => {}
                    Demote::ToHeld(bytes, freed) => {
                        total = total.saturating_sub(freed);
                        changes.push(RefEvict::Demoted(hash, bytes));
                    }
                    Demote::DropEntry(freed) => {
                        refs.remove(&hash);
                        total = total.saturating_sub(freed);
                        changes.push(RefEvict::Forgot(hash));
                    }
                }
            }
        }
    }
    changes
}

/// Evicts oldest encoded `media:` ref bytes once total cached ref bytes exceed `budget`.
/// Drops `Held` entries completely; strips encoded `bytes` from active `Decoded` entries without
/// deleting them so visible images never enter an HTTP re-fetch loop.
pub(crate) fn evict_ref_bytes(refs: &mut HashMap<String, RefSlot>, budget: usize, pin: Option<&str>) -> Vec<String> {
    let mut total: usize = refs.values().map(RefSlot::bytes_len).sum();
    if total <= budget {
        return Vec::new();
    }
    let mut dropped = Vec::new();
    while total > budget {
        let oldest = refs
            .iter()
            .filter(|(k, s)| s.bytes_len() > 0 && pin != Some(k.as_str()))
            .min_by_key(|(_, s)| (s.pixel_weight() > 0, s.last_used()))
            .map(|(k, s)| (k.clone(), s.pixel_weight() > 0));
        let Some((hash, is_decoded)) = oldest else { break };
        if is_decoded {
            if let Some(RefSlot::Decoded { bytes, .. }) = refs.get_mut(&hash)
                && let Some(b) = bytes.take()
            {
                total = total.saturating_sub(b.len());
            }
        } else if let Some(slot) = refs.remove(&hash) {
            total = total.saturating_sub(slot.bytes_len());
            dropped.push(hash);
        }
    }
    dropped
}
