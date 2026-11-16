//! The verified archive is staged, not installed. A process with the database promotes the
//! record only when that same tree is observed at the install path. Profiles from older builds
//! that wrote `rune.installedRelease` at staging without `rune.stagedBundle` keep working: when
//! the newly installed version runs, `engine::read_installed_sha` matches `rune.installedRelease`
//! as before without reporting a false `Installed`, and if the old build never swapped, the
//! running old version mismatches `rune.installedRelease` and stages cleanly.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::bundle::{self, BundleId};
use super::super::engine::NAMESPACE;

const STAGE: &str = "rune.stagedBundle";
const INSTALL: &str = "rune.installedBundle";
const APPLY_FAILED: &str = "apply-failed.json";

#[derive(Serialize, Deserialize)]
pub(in crate::updater) struct Release {
    pub version: String,
    pub sha256: String,
    pub bundle: BundleId,
}

#[derive(Serialize, Deserialize)]
struct ApplyFailure {
    version: String,
    error: String,
}

pub(super) fn staged() -> Option<Release> {
    read(STAGE)
}

fn read(key: &str) -> Option<Release> {
    serde_json::from_value(crate::storage::json(NAMESPACE, key)?).ok()
}

pub(super) fn stage(path: &Path, version: &str, sha256: &str) -> Result<(), String> {
    let bundle = bundle::identify(path).map_err(|e| format!("could not identify staged bundle: {e}"))?;
    crate::storage::set_json(NAMESPACE, STAGE, &serde_json::json!(Release { version: version.into(), sha256: sha256.into(), bundle }));
    Ok(())
}

pub(super) fn staged_matches(path: &Path, version: &str, sha256: &str) -> bool {
    read(STAGE).is_some_and(|r| r.version == version && r.sha256 == sha256 && bundle::matches_id(path, &r.bundle))
}

pub(super) fn at(root: &Path) -> Option<Release> {
    if let Some(mut r) = read(STAGE).filter(|r| bundle::matches_id(root, &r.bundle)) {
        // Refresh `file_id` at `root` so even on filesystems (FAT32/exFAT) whose synthetic IDs
        // move across a directory rename, later launches match `file_id` without re-hashing.
        r.bundle.file_id = bundle::current_file_id(root);
        crate::storage::set_json(NAMESPACE, INSTALL, &serde_json::json!(r));
        super::super::engine::record_installed(&r.version, &r.sha256);
        crate::storage::delete(NAMESPACE, STAGE);
        return Some(r);
    }
    read(INSTALL).filter(|r| bundle::matches_id(root, &r.bundle))
}

pub(in crate::updater) fn record_apply_failure(updates: &Path, version: &str, error: &str) {
    std::fs::create_dir_all(updates).ok();
    let payload = serde_json::to_vec(&ApplyFailure { version: version.into(), error: error.into() }).unwrap_or_default();
    std::fs::write(updates.join(APPLY_FAILED), payload).ok();
}

pub(in crate::updater) fn clear_apply_failure(updates: &Path) {
    std::fs::remove_file(updates.join(APPLY_FAILED)).ok();
}

pub(super) fn take_apply_failure(updates: &Path, running_version: &str, installed_version: Option<&str>) -> Option<String> {
    let path = updates.join(APPLY_FAILED);
    let raw = std::fs::read(&path).ok()?;
    std::fs::remove_file(&path).ok();
    let f: ApplyFailure = serde_json::from_slice(&raw).ok()?;
    if f.version == running_version || installed_version == Some(f.version.as_str()) {
        return None;
    }
    eprintln!("updater: previous helper couldn't install {}: {}", f.version, f.error);
    Some(opencode_i18n::t!("rune.updater.applyFailed", version = &f.version, why = crate::status_toast::plain(&f.error)).to_string())
}
