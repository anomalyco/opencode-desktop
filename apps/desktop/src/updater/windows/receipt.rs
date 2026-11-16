//! The verified archive is staged, not installed. A process with the database promotes the
//! record only when that same tree is observed at the install path. If the helper dies after
//! publication but before logging (or before a window opens), the same observation repairs it.
use std::path::Path;
use serde::{Deserialize, Serialize};
use super::{bundle, super::engine::NAMESPACE};

const STAGE: &str = "rune.stagedBundle";
const INSTALL: &str = "rune.installedBundle";

#[derive(Serialize, Deserialize)]
pub(super) struct Release { pub version: String, pub sha256: String, identity: String }

fn read(key: &str) -> Option<Release> { serde_json::from_value(crate::storage::json(NAMESPACE, key)?).ok() }

pub(super) fn stage(path: &Path, version: &str, sha256: &str) -> Result<(), String> {
    let identity = bundle::identity(path).map_err(|e| format!("could not identify staged bundle: {e}"))?;
    crate::storage::set_json(NAMESPACE, STAGE, &serde_json::json!(Release { version: version.into(), sha256: sha256.into(), identity }));
    Ok(())
}

pub(super) fn staged_matches(path: &Path, version: &str, sha256: &str) -> bool {
    read(STAGE).is_some_and(|r| r.version == version && r.sha256 == sha256 && bundle::identity(path).ok().as_ref() == Some(&r.identity))
}

pub(super) fn at(root: &Path) -> Option<Release> {
    let actual = bundle::identity(root).ok()?;
    if let Some(r) = read(STAGE).filter(|r| r.identity == actual) {
        crate::storage::set_json(NAMESPACE, INSTALL, &serde_json::json!(r));
        super::super::engine::record_installed(&r.version, &r.sha256);
        crate::storage::delete(NAMESPACE, STAGE);
        return Some(r);
    }
    read(INSTALL).filter(|r| r.identity == actual)
}
