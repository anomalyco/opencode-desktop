#![allow(unsafe_code)]

use std::ffi::CString;

use super::{LocalPathError, VerifiedLocalPath};
use crate::local_path::{is_unix_autofs_path, safe_local_reveal_path};

const MAX_ROOT_ALIAS_HOPS: u8 = 8;

pub(super) struct OwnedFd(i32);
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

fn readlink_at(dir_fd: i32, c_seg: &CString) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: `dir_fd` is an open directory fd, `c_seg` is NUL-terminated, and `buf` is writable.
    let len = unsafe { libc::readlinkat(dir_fd, c_seg.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if len <= 0 || len as usize >= buf.len() {
        return None;
    }
    String::from_utf8(buf[..len as usize].to_vec()).ok()
}

fn join_unix_symlink(prefix_segs: &[&str], link_target: &str, rest: &[&str]) -> Option<String> {
    let mut stack: Vec<String> = if link_target.starts_with('/') {
        Vec::new()
    } else {
        prefix_segs.iter().map(|s| (*s).to_owned()).collect()
    };
    for seg in link_target.split('/').chain(rest.iter().copied()) {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s.to_owned()),
        }
    }
    let resolved = if stack.is_empty() { "/".to_owned() } else { format!("/{}", stack.join("/")) };
    if is_unix_autofs_path(&resolved) {
        return None;
    }
    safe_local_reveal_path(&resolved)
}

/// Resolves a local workspace root directory once up front: allows symlink aliases whose target
/// resolves to another verified `MNT_LOCAL` / non-network directory (common on macOS), refusing
/// autofs and network mounts BEFORE traversing them. Returns the verified directory's `OwnedFd`
/// so callers never re-open `root` by path without `O_NOFOLLOW`.
pub(super) fn resolve_root_open(clean: &str, hops: u8) -> Result<(VerifiedLocalPath, OwnedFd), LocalPathError> {
    if hops >= MAX_ROOT_ALIAS_HOPS {
        return Err(LocalPathError::SymlinkOrReparse);
    }
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
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_DIRECTORY;
        // SAFETY: `held.0` is an open directory fd and `c_seg` is a NUL-terminated component.
        let next = unsafe { libc::openat(held.0, c_seg.as_ptr(), flags) };
        if next < 0 {
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            // macOS returns ELOOP (or ENOTDIR on O_DIRECTORY + symlink on some BSDs) for symlinks.
            if err == libc::ELOOP || err == libc::ENOTDIR {
                if let Some(target) = readlink_at(held.0, &c_seg) {
                    let next_clean = join_unix_symlink(&segs[..idx], &target, &segs[idx + 1..])
                        .ok_or(LocalPathError::NonLocalDrive)?;
                    return resolve_root_open(&next_clean, hops + 1);
                }
            }
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
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR {
            return Err(LocalPathError::UnsafePath);
        }
        if idx + 1 == segs.len() {
            return Ok((VerifiedLocalPath { path: walked, is_dir: true, is_file: false }, held));
        }
    }
    Err(LocalPathError::UnsafePath)
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
    let root_clean = safe_local_reveal_path(root)?;
    let (verified_root, mut held) = resolve_root_open(&root_clean, 0).ok()?;
    let target_norm = normalize_unix_root(&safe_local_reveal_path(target_lex)?);
    let root_trim = verified_root.path.trim_end_matches('/');
    let raw_root_norm = normalize_unix_root(&root_clean);
    let raw_root_trim = raw_root_norm.trim_end_matches('/');
    let strip_prefix = |prefix: &str| -> Option<&str> {
        if target_norm == prefix {
            Some("")
        } else {
            target_norm.strip_prefix(&format!("{prefix}/"))
        }
    };
    let rel = strip_prefix(root_trim).or_else(|| strip_prefix(raw_root_trim))?;
    if rel.is_empty() {
        return (!require_existing_leaf).then_some(verified_root.path);
    }
    let resolved_target = format!("{root_trim}/{rel}");
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
                return Some(resolved_target);
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
            return Some(resolved_target);
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
