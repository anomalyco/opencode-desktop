//! TOCTOU-proof pre-open component walk + open-handle kernel path verification and workspace root
//! pinning (`packages/gui-plugins/src/browser/disk/{handle-path,contained-file,preview-scope}.ts`, #63002).
//!
//! Every local preview read:
//! 1. Validates the workspace-relative path against `browser_policy::preview::{preview_path_valid, preview_allowed}`.
//! 2. Verifies the workspace root directory still matches its pinned `(canonical_path, volume/dev,
//!    file_index/inode[, creation/birth_time])` before opening.
//! 3. Walks every intermediate directory component strictly relative to the parent handle WITHOUT
//!    following symlinks/junctions/mount points:
//!    - Windows: opens the verified pinned root handle and walks each child segment via
//!      `NtCreateFile` (`OBJECT_ATTRIBUTES.RootDirectory`) with `FILE_OPEN_REPARSE_POINT`,
//!      inspecting `GetFileInformationByHandleEx(FileAttributeTagInfo)` to allow ONLY non-redirecting
//!      Windows Cloud Filter tags (`IO_REPARSE_TAG_CLOUD*` / `IO_REPARSE_TAG_ONEDRIVE`) while
//!      rejecting every name-surrogate/redirecting/unknown tag (symlinks, junctions, mount points,
//!      `APPEXECLINK`), eliminating both outbound SMB/NTLMv2 handshakes and full-path check-then-use
//!      walk races.
//!    - Unix (macOS/Linux): opens the pinned root directory fd and walks each intermediate segment
//!      with `openat(..., O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)` (preventing `/dev` opens
//!      or symlink traversal before touching the leaf).
//! 4. Opens the leaf file relative to the verified parent directory handle without following a leaf
//!    symlink/reparse point (`O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC` via `openat` on Unix;
//!    `NtCreateFile` with `FILE_OPEN_REPARSE_POINT` on Windows, hydrating Cloud Filter placeholders
//!    only after verifying their `FileAttributeTagInfo` reparse tag and re-verifying post-open).
//! 5. Resolves the kernel's final path for that exact open handle (`GetFinalPathNameByHandleW` on
//!    Windows accepting local `\\?\<drive>:\...` only, `fcntl(F_GETPATH)` on macOS,
//!    `/proc/self/fd/<fd>` on Linux), checks regular disk file (`FILE_TYPE_DISK` / `S_ISREG`),
//!    compares physically (case-sensitively even on NTFS) against the pinned root, and re-checks
//!    `preview_path_valid` + `preview_allowed` + root pin AFTER open.
//! 6. Hands that exact verified `std::fs::File` to the stream handler — never re-opening by path.

use std::fs::File;
use std::path::Path;

use browser_policy::preview::{MAX_PREVIEW_FILE_BYTES, preview_allowed, preview_path_valid};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootPin {
    /// Physical canonical root path (normalized `/` separators, no trailing `/` except `/`).
    pub canonical_path: String,
    /// OS-level stable directory identity:
    /// - Windows: `canonical_path + dwVolumeSerialNumber + nFileIndexHigh/Low + ftCreationTime`
    /// - macOS:   `canonical_path + st_dev + st_ino + st_birthtime/nsec`
    /// - Linux:   `canonical_path + st_dev + st_ino` (never mutable `st_ctime`)
    pub identity: String,
}

pub enum OpenOutcome {
    Opened { file: File, verified_rel: String, size: u64 },
    Denied,
    Missing,
}

/// Pins a workspace root directory by opening it and capturing its canonical final path + OS
/// volume/inode identity. Returns `None` if the directory does not exist, is not a safe
/// workspace root, or cannot be verified by the OS.
pub fn pin_root(workspace: &str, home: Option<&str>) -> Option<RootPin> {
    if !browser_policy::is_safe_workspace_root(workspace, home) {
        return None;
    }
    // Resolve local symlinks/junctions via reparse metadata / `openat(O_NOFOLLOW)` BEFORE calling
    // `os::pin_dir` so an alias targeting UNC (`\\server\share`) or Unix autofs (`/net/...`) is
    // refused with zero network touch.
    let verified = browser_policy::resolve_local_root(workspace).ok()?;
    let pin = os::pin_dir(Path::new(&verified.path))?;
    if !browser_policy::is_safe_workspace_root(&pin.canonical_path, home) {
        return None;
    }
    if let Some(h) = home
        && let Some(canon_home) = browser_policy::resolve_local_home(h)
        && !browser_policy::is_safe_workspace_root(&pin.canonical_path, Some(&canon_home))
    {
        return None;
    }
    Some(pin)
}

/// Opens `rel_path` under `workspace` (pinned by `pin`) after walking every intermediate
/// directory without following links, then verifies confinement strictly via the open OS handle.
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
    let (file, final_path, size) = match os::open_regular_and_resolve(pin, rel_path) {
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
    if !preview_path_valid(&verified_rel, cfg!(windows))
        || !preview_allowed(&pin.canonical_path, document, &verified_rel, entry, home)
    {
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
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED,
        FILE_READ_ATTRIBUTES, FILE_TYPE_DISK, FileAttributeTagInfo,
        GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandle,
        GetFileInformationByHandleEx, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    use super::{OpenErr, RootPin, relative_inside};

    const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
    const FILE_OPEN: u32 = 1;
    const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
    const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
    const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
    const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_SHARE_ALL: u32 = 0x0000_0007;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const FILE_TRAVERSE: u32 = 0x0000_0020;
    /// Standard read access + `SYNCHRONIZE` matching `OpenOptions::read(true)`.
    const FILE_READ_SYNC: u32 = 0x0012_0089;
    const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034u32 as i32;
    const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xC000_003Au32 as i32;
    const STATUS_NO_SUCH_FILE: i32 = 0xC000_000Fu32 as i32;

    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum_length: u16,
        buffer: *mut u16,
    }

    #[repr(C)]
    struct ObjectAttributes {
        length: u32,
        root_directory: HANDLE,
        object_name: *mut UnicodeString,
        attributes: u32,
        security_descriptor: *mut c_void,
        security_quality_of_service: *mut c_void,
    }

    #[repr(C)]
    struct IoStatusBlock {
        status_or_ptr: usize,
        information: usize,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtCreateFile(
            file_handle: *mut HANDLE,
            desired_access: u32,
            object_attributes: *mut ObjectAttributes,
            io_status_block: *mut IoStatusBlock,
            allocation_size: *mut i64,
            file_attributes: u32,
            share_access: u32,
            create_disposition: u32,
            create_options: u32,
            ea_buffer: *mut c_void,
            ea_length: u32,
        ) -> i32;
    }

    /// Inspects `FileAttributeTagInfo` whenever `FILE_ATTRIBUTE_REPARSE_POINT` is set.
    /// Returns `Ok(true)` for an allowed non-redirecting local reparse entry (Cloud Filter,
    /// legacy OneDrive, WOF compression, or NTFS deduplication), `Ok(false)` for a regular
    /// non-reparse entry, and `Err(OpenErr::Denied)` for symlinks, junctions, mount points,
    /// `APPEXECLINK`, or unknown tags.
    fn verify_reparse_tag(handle: HANDLE, attrs: u32) -> Result<bool, OpenErr> {
        if (attrs & FILE_ATTRIBUTE_REPARSE_POINT.0) == 0 {
            return Ok(false);
        }
        let mut tag_info = FILE_ATTRIBUTE_TAG_INFO::default();
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&mut tag_info as *mut FILE_ATTRIBUTE_TAG_INFO).cast::<c_void>(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if ok.is_err() || !browser_policy::is_allowed_local_reparse_tag(tag_info.ReparseTag) {
            return Err(OpenErr::Denied);
        }
        Ok(true)
    }

    /// Opens a single child path segment strictly relative to an already-verified parent directory
    /// handle via `NtCreateFile` (`OBJECT_ATTRIBUTES.RootDirectory`), eliminating the Windows
    /// intermediate-walk check-then-use race.
    fn nt_open_relative(
        parent: HANDLE,
        segment: &str,
        desired_access: u32,
        create_options: u32,
    ) -> Result<File, OpenErr> {
        if segment.is_empty() || segment.contains(['/', '\\', '\0']) {
            return Err(OpenErr::Denied);
        }
        let mut wide: Vec<u16> = std::ffi::OsStr::new(segment).encode_wide().collect();
        let byte_len = u16::try_from(wide.len().saturating_mul(2)).map_err(|_| OpenErr::Denied)?;
        let mut uname = UnicodeString {
            length: byte_len,
            maximum_length: byte_len,
            buffer: wide.as_mut_ptr(),
        };
        let mut oa = ObjectAttributes {
            length: size_of::<ObjectAttributes>() as u32,
            root_directory: parent,
            object_name: &mut uname,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut iosb = IoStatusBlock { status_or_ptr: 0, information: 0 };
        let mut handle = HANDLE::default();
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                desired_access,
                &mut oa,
                &mut iosb,
                std::ptr::null_mut(),
                0,
                FILE_SHARE_ALL,
                FILE_OPEN,
                create_options,
                std::ptr::null_mut(),
                0,
            )
        };
        if status < 0 || handle.is_invalid() {
            return Err(match status {
                STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND | STATUS_NO_SUCH_FILE => {
                    OpenErr::NotFound
                }
                _ => OpenErr::Denied,
            });
        }
        // SAFETY: `handle` is a newly created valid Win32 file/dir handle owned exclusively by us.
        Ok(unsafe { File::from_raw_handle(handle.0) })
    }

    fn final_path_of(handle: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 32_768];
        let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
        let len = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, flags) } as usize;
        if len == 0 || len >= buf.len() {
            return None;
        }
        let raw = String::from_utf16(&buf[..len]).ok()?;
        // Accept strictly local DOS drive paths (`\\?\C:\...`); refuse `\\?\UNC\`, device
        // namespaces, and volume GUID paths so previews can never touch SMB/UNC shares.
        let dos = raw.strip_prefix(r"\\?\")?;
        let b = dos.as_bytes();
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
            return None;
        }
        Some(dos.replace('\\', "/"))
    }

    fn info_of(handle: HANDLE) -> Option<BY_HANDLE_FILE_INFORMATION> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(handle, &mut info).ok()? };
        Some(info)
    }

    fn identity_of(canonical_path: &str, info: &BY_HANDLE_FILE_INFORMATION) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            canonical_path,
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime,
        )
    }

    pub fn pin_dir(dir: &Path) -> Option<RootPin> {
        // Resolve the workspace root once (allowing a workspace opened via a directory alias),
        // then capture the physical directory's handle identity and final path.
        let file = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
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
        let identity = identity_of(&canonical_path, &info);
        Some(RootPin { canonical_path, identity })
    }

    pub fn open_regular_and_resolve(pin: &RootPin, rel_path: &str) -> Result<(File, String, u64), OpenErr> {
        let segments: Vec<&str> = rel_path.split('/').collect();
        if segments.is_empty() {
            return Err(OpenErr::Denied);
        }
        // Open pinned root handle (using `\\?\` extended path for >260 char safety) and verify its
        // identity matches `pin` before walking children.
        let root_win = format!(r"\\?\{}", pin.canonical_path.replace('/', "\\"));
        let mut cur_dir = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0 | FILE_TRAVERSE | SYNCHRONIZE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(Path::new(&root_win))
            .map_err(|_| OpenErr::Denied)?;
        {
            let rh = HANDLE(cur_dir.as_raw_handle());
            if unsafe { GetFileType(rh) } != FILE_TYPE_DISK {
                return Err(OpenErr::Denied);
            }
            let rinfo = info_of(rh).ok_or(OpenErr::Denied)?;
            let rfinal = final_path_of(rh).ok_or(OpenErr::Denied)?;
            if identity_of(&rfinal, &rinfo) != pin.identity {
                return Err(OpenErr::Denied);
            }
        }

        // Parent-handle-relative walk via `NtCreateFile`: each intermediate directory is opened
        // relative to `cur_dir` with `FILE_OPEN_REPARSE_POINT`, rejecting symlinks/junctions/mount
        // points while permitting non-redirecting Cloud Filter folders (OneDrive/iCloud/Dropbox/GDrive).
        for &seg in &segments[..segments.len() - 1] {
            let mut next_dir = nt_open_relative(
                HANDLE(cur_dir.as_raw_handle()),
                seg,
                FILE_READ_ATTRIBUTES.0 | FILE_TRAVERSE | SYNCHRONIZE,
                FILE_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            )?;
            let mut h = HANDLE(next_dir.as_raw_handle());
            if unsafe { GetFileType(h) } != FILE_TYPE_DISK {
                return Err(OpenErr::Denied);
            }
            let mut info = info_of(h).ok_or(OpenErr::Denied)?;
            if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) == 0 {
                return Err(OpenErr::Denied);
            }
            let is_cloud_dir = verify_reparse_tag(h, info.dwFileAttributes)?;
            if is_cloud_dir {
                // Re-open verified Cloud Filter placeholder directory relative to `cur_dir` WITHOUT
                // `FILE_OPEN_REPARSE_POINT` so `cldflt.sys` can traverse/populate children, then re-verify.
                drop(next_dir);
                next_dir = nt_open_relative(
                    HANDLE(cur_dir.as_raw_handle()),
                    seg,
                    FILE_READ_ATTRIBUTES.0 | FILE_TRAVERSE | SYNCHRONIZE,
                    FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
                )?;
                h = HANDLE(next_dir.as_raw_handle());
                if unsafe { GetFileType(h) } != FILE_TYPE_DISK {
                    return Err(OpenErr::Denied);
                }
                info = info_of(h).ok_or(OpenErr::Denied)?;
                if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) == 0 {
                    return Err(OpenErr::Denied);
                }
                let _ = verify_reparse_tag(h, info.dwFileAttributes)?;
            }
            let dir_final = final_path_of(h).ok_or(OpenErr::Denied)?;
            if relative_inside(&pin.canonical_path, &dir_final).is_none() {
                return Err(OpenErr::Denied);
            }
            cur_dir = next_dir;
        }

        // Open the leaf file relative to `cur_dir` with `FILE_OPEN_REPARSE_POINT`.
        let leaf = segments[segments.len() - 1];
        let mut file = nt_open_relative(
            HANDLE(cur_dir.as_raw_handle()),
            leaf,
            FILE_READ_SYNC,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
        )?;
        let mut handle = HANDLE(file.as_raw_handle());
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err(OpenErr::Denied);
        }
        let mut info = info_of(handle).ok_or(OpenErr::Denied)?;
        if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
            return Err(OpenErr::Denied);
        }
        let is_cloud = verify_reparse_tag(handle, info.dwFileAttributes)?;
        let pre_size = ((info.nFileSizeHigh as u64) << 32) | (info.nFileSizeLow as u64);
        if pre_size > super::MAX_PREVIEW_FILE_BYTES {
            return Err(OpenErr::Denied);
        }
        if is_cloud {
            // Re-open Cloud Filter placeholder relative to the same verified `cur_dir` WITHOUT
            // `FILE_OPEN_REPARSE_POINT` so `cldflt.sys` hydrates its bytes, then re-verify handle.
            drop(file);
            file = nt_open_relative(
                HANDLE(cur_dir.as_raw_handle()),
                leaf,
                FILE_READ_SYNC,
                FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT,
            )?;
            handle = HANDLE(file.as_raw_handle());
            if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
                return Err(OpenErr::Denied);
            }
            info = info_of(handle).ok_or(OpenErr::Denied)?;
            if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0 {
                return Err(OpenErr::Denied);
            }
            let _ = verify_reparse_tag(handle, info.dwFileAttributes)?;
        }
        let final_path = final_path_of(handle).ok_or(OpenErr::Denied)?;
        let size = ((info.nFileSizeHigh as u64) << 32) | (info.nFileSizeLow as u64);
        Ok((file, final_path, size))
    }
}

#[cfg(unix)]
mod os {
    use std::ffi::CString;
    use std::fs::{File, OpenOptions};
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::{AsRawFd, FromRawFd};
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

    fn identity_from_stat(canonical_path: &str, st: &libc::stat) -> String {
        #[cfg(target_os = "macos")]
        {
            format!(
                "{}:{}:{}:{}:{}",
                canonical_path, st.st_dev, st.st_ino, st.st_birthtime, st.st_birthtime_nsec
            )
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Never pin mutable `st_ctime` on Linux (changes on every file creation/rename in root).
            format!("{}:{}:{}", canonical_path, st.st_dev, st.st_ino)
        }
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
        let identity = identity_from_stat(&canonical_path, &st);
        Some(RootPin { canonical_path, identity })
    }

    struct OwnedFd(i32);
    impl Drop for OwnedFd {
        fn drop(&mut self) {
            if self.0 >= 0 {
                unsafe { libc::close(self.0) };
            }
        }
    }

    pub fn open_regular_and_resolve(pin: &RootPin, rel_path: &str) -> Result<(File, String, u64), OpenErr> {
        let segments: Vec<&str> = rel_path.split('/').collect();
        if segments.is_empty() {
            return Err(OpenErr::Denied);
        }
        let root_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(Path::new(&pin.canonical_path))
            .map_err(|_| OpenErr::Denied)?;
        let root_st = fstat_of(root_file.as_raw_fd()).ok_or(OpenErr::Denied)?;
        let root_final = final_path_of(root_file.as_raw_fd()).ok_or(OpenErr::Denied)?;
        if identity_from_stat(&root_final, &root_st) != pin.identity {
            return Err(OpenErr::Denied);
        }

        // Walk each intermediate directory component strictly via `openat(..., O_NOFOLLOW | O_DIRECTORY)`
        // so intermediate symlinks (e.g. pointing to `/dev` or outside the workspace) are never followed.
        let mut cur_raw = root_file.as_raw_fd();
        let mut _held: Option<OwnedFd> = None;
        for &seg in &segments[..segments.len() - 1] {
            let c_seg = CString::new(seg).map_err(|_| OpenErr::Denied)?;
            let next = unsafe {
                libc::openat(
                    cur_raw,
                    c_seg.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if next < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                return Err(if errno == libc::ENOENT { OpenErr::NotFound } else { OpenErr::Denied });
            }
            cur_raw = next;
            _held = Some(OwnedFd(next));
        }

        let c_leaf = CString::new(segments[segments.len() - 1]).map_err(|_| OpenErr::Denied)?;
        let leaf_fd = unsafe {
            libc::openat(
                cur_raw,
                c_leaf.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if leaf_fd < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            return Err(if errno == libc::ENOENT { OpenErr::NotFound } else { OpenErr::Denied });
        }
        // SAFETY: `leaf_fd` is a newly opened, valid file descriptor owned exclusively by us.
        let file = unsafe { File::from_raw_fd(leaf_fd) };
        let st = fstat_of(file.as_raw_fd()).ok_or(OpenErr::Denied)?;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFREG {
            return Err(OpenErr::Denied);
        }
        let final_path = final_path_of(file.as_raw_fd()).ok_or(OpenErr::Denied)?;
        let size = u64::try_from(st.st_size).map_err(|_| OpenErr::Denied)?;
        Ok((file, final_path, size))
    }
}
