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
        win::is_safe_fixed_drive(&clean, 0)
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
/// mount with zero symlinks, junctions, or reparse points in any component.
///
/// Safe to call on attacker-influenced paths: refuses UNC, device namespaces, mapped drives,
/// `subst`-to-UNC, autofs, and junctions BEFORE any filesystem call can initiate network traffic.
pub fn verify_local_path(raw: &str) -> Result<VerifiedLocalPath, LocalPathError> {
    let Some(clean) = safe_local_reveal_path(raw) else {
        return Err(LocalPathError::UnsafePath);
    };
    #[cfg(windows)]
    {
        win::verify_path(&clean, 0)
    }
    #[cfg(unix)]
    {
        unix_impl::verify_path(&clean)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = clean;
        Err(LocalPathError::UnsafePath)
    }
}

/// Verifies that `target_lex` stays strictly inside local directory `root` with zero symlinks,
/// junctions, or reparse points along every existing component.
///
/// When `require_existing_leaf` is `true` (e.g. local video playback), the leaf must exist as a
/// regular non-reparse file. When `false` (e.g. `file_url_within` checking a candidate path), a
/// missing leaf is accepted once `root` and every existing ancestor component inside `root` have
/// been verified non-reparse and local.
pub fn verify_within_root(target_lex: &str, root: &str, require_existing_leaf: bool) -> Option<String> {
    if safe_local_reveal_path(root).is_none() {
        return None;
    }
    #[cfg(windows)]
    {
        win::verify_within_root(target_lex, root, require_existing_leaf)
    }
    #[cfg(unix)]
    {
        unix_impl::verify_within_root(target_lex, root, require_existing_leaf)
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
        unix_impl::has_macos_bundle_marker(verified_path)
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
pub fn reveal_in_file_manager(raw: &str) -> Result<VerifiedLocalPath, LocalPathError> {
    let verified = verify_local_path(raw)?;
    #[cfg(windows)]
    {
        if win::shell_select(&verified.path) {
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

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::ffi::OsStr;
    use std::fs::OpenOptions;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_TYPE_DISK, FileAttributeTagInfo,
        GetDriveTypeW, GetFileInformationByHandle, GetFileInformationByHandleEx, GetFileType,
        GetFinalPathNameByHandleW, QueryDosDeviceW, VOLUME_NAME_DOS,
    };

    use super::{LocalPathError, VerifiedLocalPath};
    use crate::local_path::{is_allowed_local_reparse_tag, safe_local_reveal_path};

    const DRIVE_FIXED: u32 = 3;
    const MAX_SUBST_DEPTH: u8 = 4;

    fn wide_nul(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain([0]).collect()
    }

    fn query_dos_device(drive_letter: char) -> Option<String> {
        let dev = wide_nul(&format!("{drive_letter}:"));
        let mut buf = vec![0u16; 4096];
        // SAFETY: `dev` is NUL-terminated UTF-16 and `buf` has capacity `buf.len()`.
        let len = unsafe { QueryDosDeviceW(dev.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if len == 0 || len > buf.len() {
            return None;
        }
        let end = buf[..len].iter().position(|&u| u == 0).unwrap_or(len);
        String::from_utf16(&buf[..end]).ok()
    }

    fn is_network_nt_device(target: &str) -> bool {
        let lower = target.to_ascii_lowercase();
        lower.starts_with(r"\??\unc\")
            || lower.starts_with(r"\device\mup")
            || lower.starts_with(r"\device\lanmanredirector")
            || lower.starts_with(r"\device\webdavredirector")
            || lower.starts_with(r"\device\rdpdr")
            || lower.starts_with(r"\device\hgfs")
            || lower.starts_with(r"\device\vboxminirdr")
            || lower.starts_with(r"\device\vmci")
    }

    /// Checks `QueryDosDeviceW("X:")` BEFORE `GetDriveTypeW("X:\\") == DRIVE_FIXED` so a `subst`
    /// drive targeting `\\server\share` (or targeting a local junction to UNC) is rejected without
    /// ever initiating an SMB/WebDAV handshake.
    pub(super) fn is_safe_fixed_drive(path: &str, depth: u8) -> bool {
        if depth >= MAX_SUBST_DEPTH {
            return false;
        }
        let b = path.as_bytes();
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || (b[2] != b'\\' && b[2] != b'/') {
            return false;
        }
        let letter = (b[0] as char).to_ascii_uppercase();
        let Some(nt_target) = query_dos_device(letter) else {
            return false;
        };
        if nt_target.is_empty() || is_network_nt_device(&nt_target) {
            return false;
        }
        // `subst X: C:\dir` reports `\??\C:\dir`. Verify the underlying target recursively so
        // `subst X: C:\junction_to_unc` is refused before touching anything on `X:`.
        if let Some(subst_target) = nt_target.strip_prefix(r"\??\") {
            if safe_local_reveal_path(subst_target).is_none() {
                return false;
            }
            return verify_path(subst_target, depth + 1).is_ok_and(|v| v.is_dir);
        }
        let root = wide_nul(&format!("{letter}:\\"));
        // SAFETY: `root` is a valid NUL-terminated UTF-16 drive root string.
        unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
    }

    fn final_dos_path(handle: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
        // SAFETY: `handle` is an open Win32 file handle and `buf` is valid writable memory.
        let len = unsafe { GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, flags) } as usize;
        if len == 0 || len >= buf.len() {
            return None;
        }
        let raw = String::from_utf16(&buf[..len]).ok()?;
        let dos = raw.strip_prefix(r"\\?\")?;
        let b = dos.as_bytes();
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
            return None;
        }
        Some(dos.to_owned())
    }

    fn open_no_reparse(path: &std::path::Path) -> Result<(std::fs::File, BY_HANDLE_FILE_INFORMATION), LocalPathError> {
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => LocalPathError::NotFound,
                _ => LocalPathError::UnsafePath,
            })?;
        let handle = file.as_raw_handle() as HANDLE;
        // SAFETY: `handle` is owned by `file` for the duration of these queries.
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err(LocalPathError::UnsafePath);
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(LocalPathError::UnsafePath);
        }
        if (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            let mut tag_info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                GetFileInformationByHandleEx(
                    handle,
                    FileAttributeTagInfo,
                    (&raw mut tag_info).cast(),
                    std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
                )
            };
            if ok == 0 || !is_allowed_local_reparse_tag(tag_info.ReparseTag) {
                return Err(LocalPathError::SymlinkOrReparse);
            }
        }
        Ok((file, info))
    }

    pub(super) fn verify_path(clean: &str, depth: u8) -> Result<VerifiedLocalPath, LocalPathError> {
        let native = clean.replace('/', "\\");
        if !is_safe_fixed_drive(&native, depth) {
            return Err(LocalPathError::NonLocalDrive);
        }
        let drive = format!("{}:\\", native.as_bytes()[0].to_ascii_uppercase() as char);
        let tail = &native[3..];
        let segs: Vec<&str> = tail.split('\\').filter(|s| !s.is_empty() && *s != ".").collect();
        let mut cur = PathBuf::from(&drive);
        if segs.is_empty() {
            let (file, info) = open_no_reparse(&cur)?;
            let is_dir = (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
            if !is_dir {
                return Err(LocalPathError::UnsafePath);
            }
            let final_path = final_dos_path(file.as_raw_handle() as HANDLE).ok_or(LocalPathError::NonLocalDrive)?;
            return Ok(VerifiedLocalPath { path: final_path, is_dir: true, is_file: false });
        }
        for (idx, seg) in segs.iter().enumerate() {
            if *seg == ".." {
                return Err(LocalPathError::UnsafePath);
            }
            cur.push(seg);
            let (file, info) = open_no_reparse(&cur)?;
            let is_dir = (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
            let is_last = idx + 1 == segs.len();
            if !is_last && !is_dir {
                return Err(LocalPathError::NotFound);
            }
            if is_last {
                let final_path = final_dos_path(file.as_raw_handle() as HANDLE).ok_or(LocalPathError::NonLocalDrive)?;
                return Ok(VerifiedLocalPath { path: final_path, is_dir, is_file: !is_dir });
            }
        }
        Err(LocalPathError::UnsafePath)
    }

    pub(super) fn verify_within_root(target_lex: &str, root: &str, require_existing_leaf: bool) -> Option<String> {
        let verified_root = verify_path(&safe_local_reveal_path(root)?, 0).ok()?;
        if !verified_root.is_dir {
            return None;
        }
        let root_fwd = verified_root.path.replace('\\', "/").trim_end_matches('/').to_owned();
        let raw_root_fwd = root.replace('\\', "/").trim_end_matches('/').to_owned();
        let target_clean = safe_local_reveal_path(target_lex)?;
        let target_fwd = target_clean.replace('\\', "/");
        let root_lower = root_fwd.to_ascii_lowercase();
        let raw_root_lower = raw_root_fwd.to_ascii_lowercase();
        let target_lower = target_fwd.to_ascii_lowercase();
        let strip_prefix = |prefix: &str| -> Option<&str> {
            if target_lower == prefix {
                Some("")
            } else if target_lower.starts_with(prefix) && target_fwd.as_bytes().get(prefix.len()) == Some(&b'/') {
                Some(&target_fwd[prefix.len() + 1..])
            } else {
                None
            }
        };
        let rel = strip_prefix(&root_lower).or_else(|| strip_prefix(&raw_root_lower))?;
        if rel.is_empty() {
            return (!require_existing_leaf).then_some(verified_root.path);
        }
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        let mut cur = PathBuf::from(&verified_root.path);
        for (idx, seg) in segs.iter().enumerate() {
            if *seg == ".." {
                return None;
            }
            cur.push(seg);
            match open_no_reparse(&cur) {
                Ok((file, info)) => {
                    let is_dir = (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
                    let is_last = idx + 1 == segs.len();
                    if !is_last && !is_dir {
                        return None;
                    }
                    let final_path = final_dos_path(file.as_raw_handle() as HANDLE)?;
                    let final_lower = final_path.replace('\\', "/").to_ascii_lowercase();
                    if !final_lower.starts_with(&root_lower)
                        || final_lower.as_bytes().get(root_lower.len()) != Some(&b'/')
                    {
                        return None;
                    }
                    if is_last {
                        if require_existing_leaf && is_dir {
                            return None;
                        }
                        return Some(final_path);
                    }
                }
                Err(LocalPathError::NotFound) if !require_existing_leaf => {
                    // Every existing ancestor under `root` is verified non-reparse; a not-yet-
                    // created leaf cannot traverse a junction or escape `root`.
                    return Some(cur.to_string_lossy().into_owned());
                }
                Err(_) => return None,
            }
        }
        None
    }

    /// Selects `path` in File Explorer via binary PIDL (`SHOpenFolderAndSelectItems`) using
    /// `(parent_pidl, 1, &child_pidl, 0)` so `.zip`, `.cab`, `.search-ms`, `dir.{CLSID}`, `.lnk`,
    /// `.url`, executables, and directories are ALWAYS selected in their parent folder rather than
    /// executed or opened into a shell namespace container.
    pub(super) fn shell_select(path: &str) -> bool {
        use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
        use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
        use windows_sys::Win32::UI::Shell::{
            ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID, SHOpenFolderAndSelectItems,
        };
        let wide = wide_nul(path);
        // SAFETY: COM apartment balanced with `CoUninitialize`, valid NUL-terminated UTF-16 path,
        // and all allocated PIDLs freed before returning.
        unsafe {
            let hr = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
            let full = ILCreateFromPathW(wide.as_ptr());
            if full.is_null() {
                if hr >= 0 {
                    CoUninitialize();
                }
                return false;
            }
            let parent = ILClone(full);
            let ok = if !parent.is_null() && ILRemoveLastID(parent) != 0 {
                let child = ILFindLastID(full);
                let items: [*const ITEMIDLIST; 1] = [child];
                SHOpenFolderAndSelectItems(parent, 1, items.as_ptr(), 0) >= 0
            } else {
                SHOpenFolderAndSelectItems(full, 0, std::ptr::null(), 0) >= 0
            };
            if !parent.is_null() {
                ILFree(parent);
            }
            ILFree(full);
            if hr >= 0 {
                CoUninitialize();
            }
            ok
        }
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix_impl {
    use std::ffi::CString;

    use super::{LocalPathError, VerifiedLocalPath};
    use crate::local_path::{is_unix_autofs_path, safe_local_reveal_path};

    struct OwnedFd(i32);
    impl Drop for OwnedFd {
        fn drop(&mut self) {
            if self.0 >= 0 {
                // SAFETY: `self.0` is a valid owned file descriptor.
                unsafe { libc::close(self.0) };
            }
        }
    }

    fn fstat_of(fd: i32) -> Option<libc::stat> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `st` is valid writable memory for `fstat`.
        let rc = unsafe { libc::fstat(fd, st.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        // SAFETY: `fstat` succeeded and initialized `st`.
        Some(unsafe { st.assume_init() })
    }

    fn is_local_mount(fd: i32) -> bool {
        let mut stfs = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: `stfs` is valid writable memory for `fstatfs`.
        if unsafe { libc::fstatfs(fd, stfs.as_mut_ptr()) } != 0 {
            return false;
        }
        // SAFETY: `fstatfs` succeeded and initialized `stfs`.
        let stfs = unsafe { stfs.assume_init() };
        #[cfg(target_os = "macos")]
        {
            (stfs.f_flags & libc::MNT_LOCAL as u32) != 0
        }
        #[cfg(target_os = "linux")]
        {
            let magic = stfs.f_type as u64;
            !matches!(
                magic,
                0x6969 // NFS_SUPER_MAGIC
                    | 0x517B // SMB_SUPER_MAGIC
                    | 0xFE53_4D42 // SMB2_MAGIC_NUMBER
                    | 0xFF53_4D42 // CIFS_MAGIC_NUMBER
                    | 0x0187 // AUTOFS_SUPER_MAGIC
                    | 0x564C // NCP_SUPER_MAGIC
                    | 0x7375_7245 // CODA_SUPER_MAGIC
                    | 0x5346_414F // AFS_SUPER_MAGIC
                    | 0x0102_1997 // V9FS_MAGIC
                    | 0x00C3_6400 // CEPH_SUPER_MAGIC
            )
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = stfs;
            true
        }
    }

    /// On macOS, `/tmp`, `/var`, and `/etc` are standard root symlinks to `/private/*`. Normalize
    /// only those three OS root aliases before the strict `O_NOFOLLOW` walk so legitimate `/tmp`
    /// paths work while every other symlink is refused.
    fn normalize_unix_root(clean: &str) -> String {
        #[cfg(target_os = "macos")]
        {
            for alias in ["/tmp", "/var", "/etc"] {
                if clean == alias {
                    return format!("/private{alias}");
                }
                if let Some(rest) = clean.strip_prefix(alias)
                    && rest.starts_with('/')
                {
                    return format!("/private{alias}{rest}");
                }
            }
        }
        clean.to_owned()
    }

    fn verify_path_open(clean: &str, require_dir: bool) -> Result<(VerifiedLocalPath, OwnedFd), LocalPathError> {
        if is_unix_autofs_path(clean) {
            return Err(LocalPathError::NonLocalDrive);
        }
        let walked = normalize_unix_root(clean);
        let root_c = CString::new("/").map_err(|_| LocalPathError::UnsafePath)?;
        // SAFETY: constant NUL-terminated path "/".
        let root_fd = unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
        if root_fd < 0 {
            return Err(LocalPathError::UnsafePath);
        }
        let mut held = OwnedFd(root_fd);
        if !is_local_mount(held.0) {
            return Err(LocalPathError::NonLocalDrive);
        }
        let segs: Vec<&str> = walked.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        if segs.is_empty() {
            return Ok((VerifiedLocalPath { path: "/".into(), is_dir: true, is_file: false }, held));
        }
        for (idx, seg) in segs.iter().enumerate() {
            if *seg == ".." {
                return Err(LocalPathError::UnsafePath);
            }
            let c_seg = CString::new(*seg).map_err(|_| LocalPathError::UnsafePath)?;
            let is_last = idx + 1 == segs.len();
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | if !is_last || require_dir { libc::O_DIRECTORY } else { libc::O_NONBLOCK };
            // SAFETY: `held.0` is an open directory fd and `c_seg` is a NUL-terminated component.
            let next = unsafe { libc::openat(held.0, c_seg.as_ptr(), flags) };
            if next < 0 {
                let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                return Err(match err {
                    libc::ENOENT => LocalPathError::NotFound,
                    libc::ELOOP => LocalPathError::SymlinkOrReparse,
                    _ => LocalPathError::UnsafePath,
                });
            }
            held = OwnedFd(next);
            if !is_local_mount(held.0) {
                return Err(LocalPathError::NonLocalDrive);
            }
            let st = fstat_of(held.0).ok_or(LocalPathError::UnsafePath)?;
            let kind = st.st_mode & libc::S_IFMT;
            if is_last {
                let is_dir = kind == libc::S_IFDIR;
                let is_file = kind == libc::S_IFREG;
                if (require_dir && !is_dir) || (!is_dir && !is_file) {
                    return Err(LocalPathError::UnsafePath);
                }
                return Ok((VerifiedLocalPath { path: walked, is_dir, is_file }, held));
            }
            if kind != libc::S_IFDIR {
                return Err(LocalPathError::NotFound);
            }
        }
        Err(LocalPathError::UnsafePath)
    }

    pub(super) fn verify_path(clean: &str) -> Result<VerifiedLocalPath, LocalPathError> {
        verify_path_open(clean, false).map(|(verified, _)| verified)
    }

    pub(super) fn verify_within_root(target_lex: &str, root: &str, require_existing_leaf: bool) -> Option<String> {
        let (verified_root, mut held) = verify_path_open(&safe_local_reveal_path(root)?, true).ok()?;
        let target_norm = normalize_unix_root(&safe_local_reveal_path(target_lex)?);
        let root_trim = verified_root.path.trim_end_matches('/');
        let rel = if target_norm == root_trim {
            ""
        } else {
            let prefix = format!("{root_trim}/");
            target_norm.strip_prefix(&prefix)?
        };
        if rel.is_empty() {
            return (!require_existing_leaf).then_some(verified_root.path);
        }
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
        for (idx, seg) in segs.iter().enumerate() {
            if *seg == ".." {
                return None;
            }
            let c_seg = CString::new(*seg).ok()?;
            let is_last = idx + 1 == segs.len();
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | if is_last { libc::O_NONBLOCK } else { libc::O_DIRECTORY };
            // SAFETY: `held.0` is an open directory fd and `c_seg` is a NUL-terminated component.
            let next = unsafe { libc::openat(held.0, c_seg.as_ptr(), flags) };
            if next < 0 {
                let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                if err == libc::ENOENT && !require_existing_leaf {
                    return Some(target_norm);
                }
                return None;
            }
            held = OwnedFd(next);
            if !is_local_mount(held.0) {
                return None;
            }
            let st = fstat_of(held.0)?;
            let kind = st.st_mode & libc::S_IFMT;
            if is_last {
                if require_existing_leaf && kind != libc::S_IFREG {
                    return None;
                }
                if kind != libc::S_IFREG && kind != libc::S_IFDIR {
                    return None;
                }
                return Some(target_norm);
            }
            if kind != libc::S_IFDIR {
                return None;
            }
        }
        None
    }

    /// Checks macOS directory bundle markers (`Contents/Info.plist`, `Info.plist`, or the HFS+/APFS
    /// Finder `kHasBundle` bit `0x2000`) on an already-verified local non-symlink directory.
    #[cfg(target_os = "macos")]
    pub(super) fn has_macos_bundle_marker(verified_path: &str) -> bool {
        let p = std::path::Path::new(verified_path);
        if p.join("Contents").join("Info.plist").exists() || p.join("Info.plist").exists() {
            return true;
        }
        let Ok(c_path) = CString::new(verified_path) else { return true };
        #[repr(C)]
        struct FinderInfoBuf {
            length: u32,
            finder_info: [u8; 32],
        }
        let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
        attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT as u16;
        attrs.commonattr = libc::ATTR_CMN_FNDRINFO;
        let mut out = FinderInfoBuf { length: 0, finder_info: [0; 32] };
        // SAFETY: `c_path`, `attrs`, and `out` are valid pointers with matching buffer size.
        let rc = unsafe {
            libc::getattrlist(
                c_path.as_ptr(),
                (&raw mut attrs).cast(),
                (&raw mut out).cast(),
                std::mem::size_of::<FinderInfoBuf>(),
                libc::FSOPT_NOFOLLOW as u64,
            )
        };
        if rc == 0 && out.length as usize >= std::mem::size_of::<FinderInfoBuf>() {
            // Folder FinderInfo stores big-endian `frFlags` at bytes 8..10; `kHasBundle` is 0x2000.
            let flags = u16::from_be_bytes([out.finder_info[8], out.finder_info[9]]);
            if (flags & 0x2000) != 0 {
                return true;
            }
        }
        false
    }
}
