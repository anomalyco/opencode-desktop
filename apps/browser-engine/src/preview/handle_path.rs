//! TOCTOU-proof open-handle kernel path verification and workspace root pinning
//! (`packages/gui-plugins/src/browser/disk/{handle-path,contained-file,preview-scope}.ts`, #63002).
//!
//! Every local preview read:
//! 1. Validates the workspace-relative path against `browser_policy::preview::preview_allowed`.
//! 2. Verifies the workspace root directory still matches its pinned `(canonical_path, volume/dev,
//!    file_index/inode, birth_time)` before opening.
//! 3. Opens the target file ONCE without following a leaf symlink/reparse point (`O_NOFOLLOW |
//!    O_NONBLOCK` on Unix; `FILE_FLAG_OPEN_REPARSE_POINT` on Windows).
//! 4. Resolves the kernel's final path for that exact open handle (`GetFinalPathNameByHandleW` on
//!    Windows, `fcntl(F_GETPATH)` on macOS, `/proc/self/fd/<fd>` on Linux), checks it is a regular
//!    disk file (`FILE_TYPE_DISK` / `S_ISREG`), compares physically (case-sensitively even on NTFS)
//!    against the pinned root, and re-checks `preview_allowed` + root pin AFTER open.
//! 5. Hands that exact verified `std::fs::File` to the stream handler — never re-opening by path.

use std::fs::File;
use std::path::Path;

use browser_policy::preview::{MAX_PREVIEW_FILE_BYTES, preview_allowed, preview_path_valid};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootPin {
    /// Physical canonical root path (normalized `/` separators, no trailing `/` except `/`).
    pub canonical_path: String,
    /// OS-level stable directory identity (`volume/dev + file_index/inode + birth_time`).
    pub identity: String,
}

pub enum OpenOutcome {
    Opened { file: File, verified_rel: String, size: u64 },
    Denied,
    Missing,
}

/// Pins a workspace root directory by opening it and capturing its canonical final path + OS
/// volume/inode/birth identity. Returns `None` if the directory does not exist, is not a safe
/// workspace root, or cannot be verified by the OS.
pub fn pin_root(workspace: &str, home: Option<&str>) -> Option<RootPin> {
    if !browser_policy::is_safe_workspace_root(workspace, home) {
        return None;
    }
    let pin = os::pin_dir(Path::new(workspace))?;
    if !browser_policy::is_safe_workspace_root(&pin.canonical_path, home) {
        return None;
    }
    Some(pin)
}

/// Opens `rel_path` under `workspace` (pinned by `pin`) and verifies confinement strictly via the
/// open OS file handle.
pub fn open_verified_file(
    workspace: &str,
    pin: &RootPin,
    document: &str,
    rel_path: &str,
    entry: bool,
    home: Option<&str>,
) -> OpenOutcome {
    if !preview_path_valid(rel_path, cfg!(windows)) {
        return OpenOutcome::Denied;
    }
    if !preview_allowed(workspace, document, rel_path, entry, home) {
        return OpenOutcome::Denied;
    }
    let Some(before) = os::pin_dir(Path::new(workspace)) else {
        return OpenOutcome::Denied;
    };
    if before != *pin {
        return OpenOutcome::Denied;
    }
    let target = Path::new(&pin.canonical_path).join(rel_path);
    let (file, final_path, size) = match os::open_regular_and_resolve(&target) {
        Ok(ok) => ok,
        Err(OpenErr::NotFound) => return OpenOutcome::Missing,
        Err(OpenErr::Denied) => return OpenOutcome::Denied,
    };
    if size > MAX_PREVIEW_FILE_BYTES {
        return OpenOutcome::Denied;
    }
    let Some(verified_rel) = relative_inside(&pin.canonical_path, &final_path) else {
        return OpenOutcome::Denied;
    };
    if !preview_allowed(&pin.canonical_path, document, &verified_rel, entry, home) {
        return OpenOutcome::Denied;
    }
    let Some(after) = os::pin_dir(Path::new(workspace)) else {
        return OpenOutcome::Denied;
    };
    if after != *pin {
        return OpenOutcome::Denied;
    }
    OpenOutcome::Opened { file, verified_rel, size }
}

/// Exact case-sensitive physical containment check (`contained-file.ts:40-43`).
fn relative_inside(root: &str, final_path: &str) -> Option<String> {
    let root_norm = root.trim_end_matches('/');
    let prefix = format!("{root_norm}/");
    let rel = final_path.strip_prefix(&prefix)?;
    if rel.is_empty() || rel.starts_with('/') {
        return None;
    }
    Some(rel.to_owned())
}

enum OpenErr {
    NotFound,
    Denied,
}

#[cfg(windows)]
mod os {
    use std::fs::{File, OpenOptions};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
        FILE_TYPE_DISK, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandle, GetFileType,
        GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    use super::{OpenErr, RootPin};

    fn final_path_of(handle: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
        let len = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, flags) } as usize;
        if len == 0 || len >= buf.len() {
            return None;
        }
        let raw = String::from_utf16(&buf[..len]).ok()?;
        let cleaned = if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            format!("//{unc}")
        } else if let Some(dos) = raw.strip_prefix(r"\\?\") {
            let b = dos.as_bytes();
            if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\' {
                dos.to_owned()
            } else {
                return None;
            }
        } else {
            return None;
        };
        Some(cleaned.replace('\\', "/"))
    }

    fn info_of(handle: HANDLE) -> Option<BY_HANDLE_FILE_INFORMATION> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(handle, &mut info).ok()? };
        Some(info)
    }

    pub fn pin_dir(dir: &Path) -> Option<RootPin> {
        // Resolve the workspace root once (allowing a workspace opened via a directory alias),
        // then capture the physical directory's handle identity and final path.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(dir)
            .ok()?;
        let handle = HANDLE(file.as_raw_handle());
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return None;
        }
        let info = info_of(handle)?;
        if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) == 0 {
            return None;
        }
        let canonical_path = final_path_of(handle)?;
        let identity = format!(
            "{}:{}:{}:{}:{}:{}",
            canonical_path,
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime,
        );
        Some(RootPin { canonical_path, identity })
    }

    pub fn open_regular_and_resolve(target: &Path) -> Result<(File, String, u64), OpenErr> {
        // Refuse opening a leaf symlink/reparse point without following it.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(target)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => OpenErr::NotFound,
                _ => OpenErr::Denied,
            })?;
        let handle = HANDLE(file.as_raw_handle());
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err(OpenErr::Denied);
        }
        let info = info_of(handle).ok_or(OpenErr::Denied)?;
        if (info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)) != 0 {
            return Err(OpenErr::Denied);
        }
        let final_path = final_path_of(handle).ok_or(OpenErr::Denied)?;
        let size = ((info.nFileSizeHigh as u64) << 32) | (info.nFileSizeLow as u64);
        Ok((file, final_path, size))
    }
}

#[cfg(unix)]
mod os {
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    use super::{OpenErr, RootPin};

    #[cfg(target_os = "macos")]
    fn final_path_of(fd: i32) -> Option<String> {
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        let rc = unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0)?;
        let s = std::str::from_utf8(&buf[..end]).ok()?;
        s.starts_with('/').then(|| s.to_owned())
    }

    #[cfg(target_os = "linux")]
    fn final_path_of(fd: i32) -> Option<String> {
        let path = std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()?;
        let s = path.into_os_string().into_string().ok()?;
        if !s.starts_with('/') || s.ends_with(" (deleted)") {
            return None;
        }
        Some(s)
    }

    fn fstat_of(fd: i32) -> Option<libc::stat> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        let rc = unsafe { libc::fstat(fd, st.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        Some(unsafe { st.assume_init() })
    }

    pub fn pin_dir(dir: &Path) -> Option<RootPin> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(dir)
            .ok()?;
        let fd = file.as_raw_fd();
        let st = fstat_of(fd)?;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR || st.st_ino == 0 {
            return None;
        }
        let canonical_path = final_path_of(fd)?;
        #[cfg(target_os = "macos")]
        let birth = format!("{}:{}", st.st_birthtime, st.st_birthtime_nsec);
        #[cfg(not(target_os = "macos"))]
        let birth = format!("{}:{}", st.st_ctime, st.st_ctime_nsec);
        let identity = format!("{}:{}:{}:{birth}", canonical_path, st.st_dev, st.st_ino);
        Some(RootPin { canonical_path, identity })
    }

    pub fn open_regular_and_resolve(target: &Path) -> Result<(File, String, u64), OpenErr> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(target)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => OpenErr::NotFound,
                _ => OpenErr::Denied,
            })?;
        let fd = file.as_raw_fd();
        let st = fstat_of(fd).ok_or(OpenErr::Denied)?;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err(OpenErr::Denied);
        }
        let final_path = final_path_of(fd).ok_or(OpenErr::Denied)?;
        let size = u64::try_from(st.st_size).map_err(|_| OpenErr::Denied)?;
        Ok((file, final_path, size))
    }
}
