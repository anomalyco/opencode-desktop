//! Authoritative OS-level local path verification and safe file-manager reveal shared across
//! `browser-policy`, `features/open-in`, `features/files`, `crates/session-view`,
//! `crates/opencode-plugin-runtime`, `apps/desktop`, and `apps/browser-engine`.
//!
//! Every caller that touches a user- or model-influenced local path (`reveal`, `open_in`,
//! `location_card`, `images/menu`, local video playback, workspace `file://` confinement) MUST
//! validate through [`verify_local_path`] or [`verify_within_root`] BEFORE calling `exists()`,
//! `is_dir()`, `metadata()`, `canonicalize()`, `ILCreateFromPathW`, or spawning a file manager:
//!
//! - **Windows:** pure string validation -> `QueryDosDeviceW` (`subst`-to-UNC & network redirector
//!   refusal before `GetDriveTypeW`) -> `GetDriveTypeW == DRIVE_FIXED` -> component-wise open walk
//!   with `FILE_READ_ATTRIBUTES` + `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT`
//!   inspecting `FileAttributeTagInfo` (`is_allowed_local_reparse_tag`: permits only non-redirecting
//!   Cloud Filter / legacy OneDrive / WOF / Dedup tags, rejecting all junctions/symlinks/mount
//!   points/unknown tags before touching any child) -> `GetFinalPathNameByHandleW` confirming local
//!   `\\?\<drive>:\...` (never `\\?\UNC\`).
//! - **macOS / Linux:** pure string validation + autofs/network prefix rejection (`/net`,
//!   `/Network`, `/misc`, `/afs`, `/run/user/*/gvfs`) -> component-wise `openat(..., O_NOFOLLOW)`
//!   from `/` verifying `fstat` (`S_ISDIR`/`S_ISREG`) and `fstatfs` (`MNT_LOCAL` on macOS,
//!   non-network/non-autofs magic on Linux) at every step.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fmt;

use crate::local_path::{is_macos_bundle_path, safe_local_reveal_path};

/// A local filesystem path verified without following symlinks, junctions, `subst`-to-UNC
/// mappings, or network/autofs mounts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedLocalPath {
    /// Native OS path verified by the kernel (no UNC, no device namespace, no reparse points).
    pub path: String,
    pub is_dir: bool,
    pub is_file: bool,
}

/// Why [`verify_local_path`] or [`reveal_in_file_manager`] refused a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalPathError {
    /// Failed pure string checks (UNC, device path, `..`, controls/bidi, autofs prefix).
    UnsafePath,
    /// Drive or mount is not a local fixed drive (`DRIVE_FIXED` / `MNT_LOCAL`) or is a `subst`
    /// alias targeting a network share / redirector.
    NonLocalDrive,
    /// An intermediate directory or the leaf itself is a symlink, junction, or reparse point.
    SymlinkOrReparse,
    /// Verified local drive/ancestors, but the target path does not exist on disk.
    NotFound,
    /// The OS file manager refused or failed to reveal the verified path.
    RevealFailed(String),
}

impl fmt::Display for LocalPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafePath => f.write_str("Refusing non-local or unsafe path"),
            Self::NonLocalDrive => f.write_str("Only local fixed drives can be accessed"),
            Self::SymlinkOrReparse => f.write_str("Refusing symlink or reparse point"),
            Self::NotFound => f.write_str("Path doesn't exist"),
            Self::RevealFailed(why) => f.write_str(why),
        }
    }
}

/// Pure drive/mount pre-check without touching the target path itself.
///
/// On Windows, inspects `QueryDosDeviceW("X:")` (refusing `\??\UNC\`, MUP/Lanman/WebDAV
/// redirectors, and `subst` targets pointing at non-fixed drives or reparse points) before calling
/// `GetDriveTypeW("X:\\") == DRIVE_FIXED`. On Unix, rejects autofs prefixes; full mount-local
/// verification runs inside [`verify_local_path`].
pub fn is_local_fixed_drive(raw: &str) -> bool {
    let Some(clean) = safe_local_reveal_path(raw) else {
        return false;
    };
    #[cfg(windows)]
    {
        windows::is_safe_fixed_drive(&clean, 0)
    }
    #[cfg(unix)]
    {
        let _ = clean;
        true
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = clean;
        false
    }
}

/// Verifies that `raw` names an existing regular file or directory on a local fixed drive / local
/// mount with zero symlinks, junctions, or disallowed reparse points in any component.
///
/// Safe to call on attacker-influenced paths: refuses UNC, device namespaces, mapped drives,
/// `subst`-to-UNC, autofs, and junctions BEFORE any filesystem call can initiate network traffic.
pub fn verify_local_path(raw: &str) -> Result<VerifiedLocalPath, LocalPathError> {
    let Some(clean) = safe_local_reveal_path(raw) else {
        return Err(LocalPathError::UnsafePath);
    };
    #[cfg(windows)]
    {
        windows::verify_path(&clean, 0)
    }
    #[cfg(unix)]
    {
        unix::verify_path(&clean)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = clean;
        Err(LocalPathError::UnsafePath)
    }
}

/// Resolves a known-local workspace root directory ONCE up front (at start-up / session pin),
/// permitting a symlink or junction alias pointing to another verified local fixed-drive /
/// `MNT_LOCAL` directory while strictly refusing UNC, device namespaces, mapped drives,
/// `subst`-to-UNC, autofs, or any symlink/junction whose target escapes to a network share
/// (without ever following it onto SMB or autofs).
///
/// Callers MUST store and pass the returned physical path (never the raw alias) into all later
/// `verify_local_path` / `verify_within_root` calls so re-pointing the alias after start-up
/// cannot move the workspace.
pub fn resolve_local_root(raw: &str) -> Result<VerifiedLocalPath, LocalPathError> {
    let Some(clean) = safe_local_reveal_path(raw) else {
        return Err(LocalPathError::UnsafePath);
    };
    #[cfg(windows)]
    {
        windows::resolve_root(&clean, 0)
    }
    #[cfg(unix)]
    {
        unix::resolve_root_open(&clean, 0).map(|(verified, _)| verified)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = clean;
        Err(LocalPathError::UnsafePath)
    }
}

/// Verifies that `target_lex` stays strictly inside local directory `root` with zero symlinks,
/// junctions, or disallowed reparse points along every existing component (including `root`).
///
/// Callers supporting aliased workspace roots MUST resolve `root` once via
/// [`crate::resolve_safe_local_workspace_root`] and pass that pinned physical root here; this
/// function never resolves aliases on its own.
pub fn verify_within_root(target_lex: &str, root: &str, require_existing_leaf: bool) -> Option<String> {
    if safe_local_reveal_path(root).is_none() {
        return None;
    }
    #[cfg(windows)]
    {
        windows::verify_within_root(target_lex, root, require_existing_leaf)
    }
    #[cfg(unix)]
    {
        unix::verify_within_root(target_lex, root, require_existing_leaf)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (target_lex, require_existing_leaf);
        None
    }
}

/// Returns `true` if `verified_path` is a macOS bundle/package (by suffix, `Info.plist`, or
/// Finder `kHasBundle` bit). Caller must have already verified `verified_path` via
/// [`verify_local_path`].
pub fn is_macos_bundle(verified_path: &str) -> bool {
    if is_macos_bundle_path(verified_path) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        unix::has_macos_bundle_marker(verified_path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Safely reveals `raw` in the OS file manager without ever executing the target or touching UNC,
/// mapped drives, `subst`-to-UNC, symlinks/junctions, or autofs mounts:
/// - **Windows:** verifies with [`verify_local_path`], then calls `SHOpenFolderAndSelectItems`
///   with `(parent_pidl, 1, &child_pidl, 0)` (selects `.lnk`/`.url`/`.zip`/`.search-ms`/
///   `dir.{CLSID}`/executables in their parent folder; never spawns `explorer.exe`).
/// - **macOS:** verifies with [`verify_local_path`], then runs `/usr/bin/open -R -- <path>`
///   (never launches bundles or symlinks).
/// - **Linux:** verifies with [`verify_local_path`], then runs `xdg-open` on the directory (or
///   parent directory for a file; never executes `.desktop` files).
///
/// A proof run that records external opens ([`crate::harness_log`]) records it instead.
pub fn reveal_in_file_manager(raw: &str) -> Result<VerifiedLocalPath, LocalPathError> {
    let verified = verify_local_path(raw)?;
    if crate::harness_log::record("reveal", &[("path", &verified.path)]) {
        return Ok(verified);
    }
    #[cfg(windows)]
    {
        if windows::shell_select(&verified.path) {
            return Ok(verified);
        }
        Err(LocalPathError::RevealFailed("Couldn't reveal this path in File Explorer".into()))
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/bin/open")
            .args(["-R", "--", &verified.path])
            .output()
            .map_err(|e| LocalPathError::RevealFailed(e.to_string()))?;
        if out.status.success() {
            Ok(verified)
        } else {
            Err(LocalPathError::RevealFailed(String::from_utf8_lossy(&out.stderr).trim().to_owned()))
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if verified.is_dir {
            verified.path.as_str()
        } else {
            std::path::Path::new(&verified.path)
                .parent()
                .and_then(std::path::Path::to_str)
                .unwrap_or(verified.path.as_str())
        };
        let out = std::process::Command::new("xdg-open")
            .arg(target)
            .output()
            .map_err(|e| LocalPathError::RevealFailed(e.to_string()))?;
        if out.status.success() {
            Ok(verified)
        } else {
            Err(LocalPathError::RevealFailed(String::from_utf8_lossy(&out.stderr).trim().to_owned()))
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = verified;
        Err(LocalPathError::RevealFailed("Revealing local files is not supported in the browser".into()))
    }
}
