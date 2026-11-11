//! Rune's glyph masks across launches (`rune::text::mask_cache`), kept as one row of the
//! database's cache table: read once the storage is in hand and offered to every text system,
//! and written when Rune hands over the masks in use (a few seconds after launch, then as they
//! change), on Rune's thread. A run kept in memory (headless, fixtures) keeps none.

const KEY: &str = "rune.text.masks";
const VERSION: i64 = 1;

/// After `storage::start`: wires the cache to the database.
pub fn start() {
    if !crate::storage::on_disk() {
        return;
    }
    rune::text::mask_cache::save_with(|bytes| crate::storage::with_storage(move |s| s.caches().put(KEY, VERSION, bytes.into())));
    crate::storage::with_storage(|s| s.caches().get(KEY, VERSION, Box::new(|bytes| {
        if let Some(bytes) = bytes { rune::text::mask_cache::offer(&bytes); }
    })));
}