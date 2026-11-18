//! Rune's glyph masks across launches (`rune::text::mask_cache`), kept as one row of the
//! database's cache table: read on the open's thread as soon as SQLite is open (before the
//! first frame, which uses them if they're in time) and offered to every text system, and
//! written when Rune hands over the masks in use (a few seconds after launch, then as they
//! change), on Rune's thread. A run kept in memory (headless, fixtures) keeps none.

use opencode_storage::StorageConfig;

const KEY: &str = "rune.text.masks";
const VERSION: i64 = 1;

/// The open reads the row early (`storage::config`).
pub fn early(config: StorageConfig) -> StorageConfig {
    config.early_cache(KEY, VERSION, |bytes| {
        rune::text::mask_cache::offer(&bytes);
        rune::marks::mark("storage.early_cache");
    })
}

/// After `storage::start`: what Rune saves goes to the database (an unchanged row writes nothing).
#[cfg(not(target_arch = "wasm32"))]
pub fn start() {
    if !crate::storage::on_disk() {
        return;
    }
    rune::text::mask_cache::save_with(|bytes| crate::storage::with_storage(move |s| s.caches().put(KEY, VERSION, bytes.into())));
}