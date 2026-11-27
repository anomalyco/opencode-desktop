//! Which sites keep local storage in the engine's own profile: a private copy of the profile's
//! `Local Storage/leveldb` (`snapshot_directory`: no lock on the live files) opened with a LevelDB
//! reader, which replays the copied log, so an item deleted is gone here too. Chromium keeps each
//! item as `_<storage key>\0<item key>` and describes a storage key in `META:` / `METAACCESS:` rows
//! (`components/services/storage/dom_storage/local_storage_impl.cc`): only the item rows are data.
//! Values are never read. Clear browsing data checks this, as DevTools reports no size for local
//! storage.

use std::collections::BTreeSet;
use std::path::Path;

use rusty_leveldb::{DB, LdbIterator, Options};

use super::ImportFailure;
use super::database::{SnapshotScratch, snapshot_directory};

/// A local-storage folder larger than this is not copied (the check then fails).
const MAX_LOCAL_STORAGE_BYTES: u64 = 256 * 1024 * 1024;

/// The storage keys holding at least one local-storage item in `profile`'s shared profile
/// (`<profile>/Default/Local Storage/leveldb`), extensions' (`chrome-extension://`) left out:
/// empty when the profile has no local storage yet.
pub fn sites_with_local_storage(profile: &Path, scratch: &SnapshotScratch) -> Result<Vec<String>, ImportFailure> {
    let folder = profile.join("Default").join("Local Storage").join("leveldb");
    if !folder.is_dir() {
        return Ok(Vec::new());
    }
    snapshot_directory(&folder, scratch, MAX_LOCAL_STORAGE_BYTES, |copy| {
        let options = Options { create_if_missing: false, paranoid_checks: false, ..Options::default() };
        let mut db = DB::open(copy, options).map_err(|_| ImportFailure::Read)?;
        let mut it = db.new_iter().map_err(|_| ImportFailure::Read)?;
        let mut sites = BTreeSet::new();
        while let Some((key, _)) = it.next() {
            if let Some(site) = item_site(&key)
                && !site.starts_with("chrome-extension://")
            {
                sites.insert(site);
            }
        }
        drop(it);
        let _ = db.close();
        Ok(sites.into_iter().collect())
    })
}

/// The storage key of an item row (`_<storage key>\0<item key>`), `None` for every other row.
fn item_site(key: &[u8]) -> Option<String> {
    let rest = key.strip_prefix(b"_")?;
    let end = rest.iter().position(|b| *b == 0)?;
    // A key that is not UTF-8 still names data: it is reported (lossily), never skipped.
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}
