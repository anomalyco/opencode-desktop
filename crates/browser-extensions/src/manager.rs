//! High-level extension staging, atomic swap-with-rollback, startup integrity verification, and
//! managed-directory lifecycle (`profile/index.ts` + `profile/install.ts`).
//!
//! Persisted `StoredExtension` lists are passed in and returned by value so the caller stores them
//! through `opencode-storage` (never direct UI-thread JSON writes, honoring `holds_profile()`).

use std::path::{Path, PathBuf};

use base64::Engine as _;

use crate::crx::{
    download_crx, extension_id, generate_local_spki_key, is_valid_ext_id, parse_crx, random_token,
    store_target,
};
use crate::integrity::{
    copy_dir_safe, extension_tree_digest, pin_manifest_key, strip_metadata_dir,
    verify_extension_dir,
};
use crate::manifest::{
    display_name, extension_access, extension_compatibility, extension_icon_data_url,
    extension_page_url, read_manifest,
};
use crate::review::{is_downgrade, review_access};
use crate::sources::{PlatformDirs, chromium_profile_directory, profile_extension_dirs};
use crate::zip::unpack_zip;
use crate::{
    CompatibilityLevel, ExtensionSource, InstallFailure, InstallReview, ProfileExtension,
    StoreKind, StoredExtension, VerifiedStartupSpec,
};

const STAGING_PREFIX: &str = ".staging-";

/// Staged package prepared for user review; consumed once by [`ExtensionManager::approve_staged`].
#[derive(Clone, Debug)]
pub struct StagedInstall {
    pub token: String,
    pub staged_dir: PathBuf,
    pub record: StoredExtension,
    pub review: InstallReview,
}

/// Handle to a backup directory created during [`ExtensionManager::approve_staged`] so a caller
/// can finalize or roll back cleanly if engine load fails.
#[derive(Clone, Debug)]
pub struct SwappedInstall {
    pub record: StoredExtension,
    pub profile: ProfileExtension,
    pub backup_dir: Option<PathBuf>,
}

/// Result of verifying all enabled [`StoredExtension`] entries before spawning the engine.
#[derive(Clone, Debug, Default)]
pub struct StartupVerification {
    pub specs: Vec<VerifiedStartupSpec>,
    pub changed_ids: Vec<String>,
}

/// Manages unpacked extension directories under `<userData>/browser-extensions`.
#[derive(Clone, Debug)]
pub struct ExtensionManager {
    root: PathBuf,
}

impl ExtensionManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Removes leftover `.staging-*` directories from earlier crashes (`install.ts:123-131`).
    pub fn sweep_staging(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(s) = name.to_str()
                && s.starts_with(STAGING_PREFIX)
                && entry.file_type().is_ok_and(|ft| !ft.is_symlink())
            {
                std::fs::remove_dir_all(entry.path()).ok();
            }
        }
    }

    /// Verifies every enabled extension on disk before passing `--extension <id>:<sha256>:<dir>`
    /// to `opencode-browser-engine` (`profile/index.ts:228-246`).
    pub fn verify_enabled_for_startup(&self, stored: &[StoredExtension]) -> StartupVerification {
        let mut out = StartupVerification::default();
        let Ok(canon_root) = std::fs::canonicalize(&self.root) else {
            for rec in stored.iter().filter(|r| r.enabled) {
                out.changed_ids.push(rec.id.clone());
            }
            return out;
        };
        for rec in stored.iter().filter(|r| r.enabled) {
            if !is_valid_ext_id(&rec.id) {
                out.changed_ids.push(rec.id.clone());
                continue;
            }
            let Some(digest) = rec.digest.as_deref() else {
                out.changed_ids.push(rec.id.clone());
                continue;
            };
            let expected_dir = self.root.join(&rec.id);
            let dir_path = PathBuf::from(&rec.directory);
            let canon_ok = dir_path == expected_dir
                && std::fs::canonicalize(&dir_path).ok().as_deref() == Some(canon_root.join(&rec.id).as_path());
            if canon_ok && verify_extension_dir(&dir_path, &rec.id, digest) {
                out.specs.push(VerifiedStartupSpec {
                    id: rec.id.clone(),
                    sha256: digest.to_owned(),
                    dir: rec.directory.clone(),
                });
            } else {
                out.changed_ids.push(rec.id.clone());
            }
        }
        out
    }

    /// Describes a stored extension record for `Settings › Browser › Extensions`.
    pub fn describe(&self, record: &StoredExtension, changed: bool, error: Option<String>) -> Option<ProfileExtension> {
        if changed || record.digest.is_none() {
            return Some(ProfileExtension {
                id: record.id.clone(),
                name: record.name.clone().unwrap_or_else(|| record.id.clone()),
                version: String::new(),
                enabled: false,
                source: record.source,
                icon: None,
                popup: None,
                options: None,
                error,
                changed: true,
                compatibility: crate::ExtensionCompatibility {
                    level: CompatibilityLevel::Partial,
                    cause: None,
                    missing: Vec::new(),
                },
                access: Vec::new(),
            });
        }
        let dir = Path::new(&record.directory);
        let manifest = read_manifest(dir)?;
        Some(ProfileExtension {
            id: record.id.clone(),
            name: display_name(dir, &manifest),
            version: manifest.version.clone(),
            enabled: record.enabled && error.is_none(),
            source: record.source,
            icon: extension_icon_data_url(dir, &manifest),
            popup: manifest
                .action_default_popup
                .as_deref()
                .and_then(|f| extension_page_url(&record.id, f)),
            options: manifest
                .options_page
                .as_deref()
                .and_then(|f| extension_page_url(&record.id, f)),
            error,
            changed: false,
            compatibility: extension_compatibility(&manifest),
            access: extension_access(&manifest),
        })
    }

    /// Downloads, verifies CRX3 signatures, unpacks into staging, checks downgrade, and builds a
    /// single-use review token (`profile/index.ts:588-602`).
    pub fn stage_store_install(
        &self,
        url_or_id: &str,
        chrome_version: &str,
        stored: &[StoredExtension],
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let target = store_target(url_or_id).ok_or((InstallFailure::Download, None))?;
        let crx_bytes = download_crx(&target, chrome_version).ok_or((InstallFailure::Download, None))?;
        self.stage_verified_crx(&crx_bytes, &target.id, target.store, stored)
    }

    /// Verifies already-fetched CRX3 bytes, unpacks into staging, checks downgrade, and builds a
    /// single-use review token.
    pub fn stage_verified_crx(
        &self,
        crx_bytes: &[u8],
        expected_id: &str,
        store: StoreKind,
        stored: &[StoredExtension],
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let verified = parse_crx(crx_bytes, expected_id).ok_or((InstallFailure::Download, None))?;
        let target_dir = self.root.join(expected_id);
        let record = StoredExtension {
            id: expected_id.to_owned(),
            directory: target_dir.to_string_lossy().into_owned(),
            enabled: true,
            source: ExtensionSource::Store,
            origin: None,
            local_key: None,
            digest: None,
            name: None,
            version: None,
            grants: None,
        };
        self.stage_common(record, Some(store), stored, |staging| {
            unpack_zip(verified.zip, staging).map_err(|_| InstallFailure::Manifest)?;
            if !strip_metadata_dir(staging) {
                return Err(InstallFailure::Manifest);
            }
            pin_manifest_key(staging, &verified.key).map_err(|_| InstallFailure::Manifest)
        })
    }

    /// Copies an extension from an installed Chromium browser profile into staging with a private
    /// profile-local ECDSA P-256 key (`profile/index.ts:561-586`).
    pub fn stage_browser_install(
        &self,
        dirs: &PlatformDirs,
        browser_id: &str,
        profile_id: &str,
        source_ext_id: &str,
        stored: &[StoredExtension],
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let profile_dir = chromium_profile_directory(dirs, browser_id, profile_id)
            .ok_or((InstallFailure::Manifest, None))?;
        let (_, version_dir) = profile_extension_dirs(&profile_dir)
            .into_iter()
            .find(|(id, _)| id == source_ext_id)
            .ok_or((InstallFailure::Manifest, None))?;
        let origin = version_dir
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or((InstallFailure::Manifest, None))?;
        self.stage_local_copy(&version_dir, ExtensionSource::Browser, origin, stored)
    }

    /// Copies a developer unpacked folder into staging with a private profile-local ECDSA P-256
    /// key (`profile/index.ts:561-586`).
    pub fn stage_folder_install(
        &self,
        folder: &Path,
        stored: &[StoredExtension],
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let canon = std::fs::canonicalize(folder).map_err(|_| (InstallFailure::Manifest, None))?;
        let origin = canon.to_string_lossy().into_owned();
        self.stage_local_copy(&canon, ExtensionSource::Folder, origin, stored)
    }

    fn stage_local_copy(
        &self,
        source_dir: &Path,
        source: ExtensionSource,
        origin: String,
        stored: &[StoredExtension],
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let previous = stored
            .iter()
            .find(|r| r.source == source && r.origin.as_deref() == Some(origin.as_str()));
        let key_der = match previous.and_then(|p| p.local_key.as_deref()) {
            Some(b64) => base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| (InstallFailure::Manifest, None))?,
            None => generate_local_spki_key().ok_or((InstallFailure::Manifest, None))?,
        };
        let id = extension_id(&key_der);
        let target_dir = self.root.join(&id);
        let record = StoredExtension {
            id,
            directory: target_dir.to_string_lossy().into_owned(),
            enabled: true,
            source,
            origin: Some(origin),
            local_key: Some(base64::engine::general_purpose::STANDARD.encode(&key_der)),
            digest: None,
            name: None,
            version: None,
            grants: None,
        };
        self.stage_common(record, None, stored, |staging| {
            copy_dir_safe(source_dir, staging).map_err(|_| InstallFailure::Manifest)?;
            pin_manifest_key(staging, &key_der).map_err(|_| InstallFailure::Manifest)
        })
    }

    fn stage_common(
        &self,
        mut record: StoredExtension,
        store: Option<StoreKind>,
        stored: &[StoredExtension],
        fill: impl FnOnce(&Path) -> Result<(), InstallFailure>,
    ) -> Result<StagedInstall, (InstallFailure, Option<String>)> {
        let previous = stored
            .iter()
            .find(|item| item.directory == record.directory || item.id == record.id);
        if let Some(prev) = previous
            && (prev.source != record.source
                || prev.directory != record.directory
                || prev.id != record.id
                || prev.origin != record.origin)
        {
            return Err((InstallFailure::Collision, None));
        }

        std::fs::create_dir_all(&self.root).map_err(|_| (InstallFailure::Manifest, None))?;
        let nonce = random_token().ok_or((InstallFailure::Manifest, None))?;
        let staging = self.root.join(format!("{STAGING_PREFIX}{nonce}"));
        std::fs::create_dir(&staging).map_err(|_| (InstallFailure::Manifest, None))?;

        let cleanup = |dir: &Path| {
            std::fs::remove_dir_all(dir).ok();
        };
        if let Err(err) = fill(&staging) {
            cleanup(&staging);
            return Err((err, None));
        }
        let Some(manifest) = read_manifest(&staging) else {
            cleanup(&staging);
            return Err((InstallFailure::Manifest, None));
        };
        if manifest.is_theme {
            cleanup(&staging);
            return Err((InstallFailure::Manifest, None));
        }
        // Refuse store version downgrades against an already-installed store extension.
        if record.source == ExtensionSource::Store
            && let Some(prev_ver) = previous.and_then(|p| p.version.as_deref())
            && is_downgrade(prev_ver, &manifest.version)
        {
            cleanup(&staging);
            return Err((
                InstallFailure::Downgrade,
                Some(format!("Refusing to downgrade from {} to {}.", prev_ver, manifest.version)),
            ));
        }
        let Some(digest) = extension_tree_digest(&staging) else {
            cleanup(&staging);
            return Err((InstallFailure::Manifest, None));
        };

        let name = display_name(&staging, &manifest);
        let compatibility = extension_compatibility(&manifest);
        let access_review = review_access(&manifest, previous.and_then(|p| p.grants.as_ref()));
        let icon = extension_icon_data_url(&staging, &manifest);
        let token = random_token().ok_or_else(|| {
            cleanup(&staging);
            (InstallFailure::Manifest, None)
        })?;

        record.name = Some(name.clone());
        record.version = Some(manifest.version.clone());
        record.digest = Some(digest);
        record.grants = Some(access_review.grants.clone());

        let review = InstallReview {
            unsupported: compatibility.level == CompatibilityLevel::Unsupported,
            token: token.clone(),
            id: record.id.clone(),
            source: record.source,
            store,
            name,
            version: manifest.version,
            icon,
            replaces: previous.map(|p| p.name.clone().unwrap_or_else(|| p.id.clone())),
            compatibility,
            grants: access_review.grants,
            sites: access_review.sites,
            added: access_review.added,
        };
        Ok(StagedInstall { token, staged_dir: staging, record, review })
    }

    /// Re-verifies the staged directory's SHA-256 tree digest (detecting any swap between review
    /// display and user click) and atomically swaps it into `<root>/<id>` with backup/rollback.
    pub fn approve_staged(&self, staged: &StagedInstall) -> Result<SwappedInstall, InstallFailure> {
        if staged.review.compatibility.cause == Some(crate::UnsupportedCause::ManifestV2) {
            std::fs::remove_dir_all(&staged.staged_dir).ok();
            return Err(InstallFailure::Load);
        }
        let Some(expected_digest) = staged.record.digest.as_deref() else {
            std::fs::remove_dir_all(&staged.staged_dir).ok();
            return Err(InstallFailure::Expired);
        };
        if !verify_extension_dir(&staged.staged_dir, &staged.record.id, expected_digest) {
            std::fs::remove_dir_all(&staged.staged_dir).ok();
            return Err(InstallFailure::Expired);
        }
        let target = self.root.join(&staged.record.id);
        let backup = self.root.join(format!(
            "{}-previous",
            staged
                .staged_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(".staging-backup")
        ));
        let had_existing = match std::fs::rename(&target, &backup) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => {
                std::fs::remove_dir_all(&staged.staged_dir).ok();
                return Err(InstallFailure::Manifest);
            }
        };
        if std::fs::rename(&staged.staged_dir, &target).is_err() {
            std::fs::remove_dir_all(&staged.staged_dir).ok();
            if had_existing {
                std::fs::rename(&backup, &target).ok();
            }
            return Err(InstallFailure::Manifest);
        }
        let Some(profile) = self.describe(&staged.record, false, None) else {
            std::fs::remove_dir_all(&target).ok();
            if had_existing {
                std::fs::rename(&backup, &target).ok();
            }
            return Err(InstallFailure::Manifest);
        };
        Ok(SwappedInstall {
            record: staged.record.clone(),
            profile,
            backup_dir: had_existing.then_some(backup),
        })
    }

    /// Removes the backup created by [`approve_staged`] after the engine successfully loads it.
    pub fn finalize_swap(&self, swap: &SwappedInstall) {
        if let Some(backup) = &swap.backup_dir {
            std::fs::remove_dir_all(backup).ok();
        }
    }

    /// Rolls back a swapped install if Chromium fails to load it (`profile/index.ts:404-410`).
    pub fn rollback_swap(&self, swap: &SwappedInstall) {
        let target = self.root.join(&swap.record.id);
        std::fs::remove_dir_all(&target).ok();
        if let Some(backup) = &swap.backup_dir {
            std::fs::rename(backup, &target).ok();
        }
    }

    /// Deletes a managed extension directory strictly inside `self.root` (`profile/index.ts:617`).
    /// Never deletes a user's source folder outside `self.root`.
    pub fn remove_managed_dir(&self, record: &StoredExtension) {
        if !is_valid_ext_id(&record.id) {
            return;
        }
        if record.source == ExtensionSource::Folder && record.local_key.is_none() {
            return;
        }
        let expected = self.root.join(&record.id);
        if Path::new(&record.directory) != expected {
            return;
        }
        if let (Ok(canon_root), Ok(canon_target)) = (
            std::fs::canonicalize(&self.root),
            std::fs::canonicalize(&expected),
        ) && canon_target == canon_root.join(&record.id)
        {
            std::fs::remove_dir_all(&canon_target).ok();
        }
    }
}
