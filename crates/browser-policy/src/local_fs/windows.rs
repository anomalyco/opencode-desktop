#![allow(unsafe_code)]

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
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_GET_REPARSE_POINT;

use super::{LocalPathError, VerifiedLocalPath};
use crate::local_path::{is_allowed_local_reparse_tag, safe_local_reveal_path};

const DRIVE_FIXED: u32 = 3;
const MAX_SUBST_DEPTH: u8 = 4;
const MAX_ROOT_ALIAS_HOPS: u8 = 8;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const SYMLINK_FLAG_RELATIVE: u32 = 0x0000_0001;
const MAX_REPARSE_BUFFER: usize = 16_384;

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

fn open_raw_no_reparse(path: &std::path::Path) -> Result<(std::fs::File, BY_HANDLE_FILE_INFORMATION, Option<u32>), LocalPathError> {
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
    let reparse_tag = if (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
        let mut tag_info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileAttributeTagInfo,
                (&raw mut tag_info).cast(),
                std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(LocalPathError::SymlinkOrReparse);
        }
        Some(tag_info.ReparseTag)
    } else {
        None
    };
    Ok((file, info, reparse_tag))
}

fn open_no_reparse(path: &std::path::Path) -> Result<(std::fs::File, BY_HANDLE_FILE_INFORMATION), LocalPathError> {
    let (file, info, tag) = open_raw_no_reparse(path)?;
    if let Some(tag) = tag
        && !is_allowed_local_reparse_tag(tag)
    {
        return Err(LocalPathError::SymlinkOrReparse);
    }
    Ok((file, info))
}

/// Reads the NTFS symlink or mount-point/junction substitute target directly from the open
/// `FILE_FLAG_OPEN_REPARSE_POINT` handle via `FSCTL_GET_REPARSE_POINT` (pure local NTFS metadata;
/// never traverses the target or initiates SMB/network traffic).
fn read_reparse_target(handle: HANDLE, expected_tag: u32) -> Option<(String, bool)> {
    if expected_tag != IO_REPARSE_TAG_MOUNT_POINT && expected_tag != IO_REPARSE_TAG_SYMLINK {
        return None;
    }
    let mut buf = vec![0u8; MAX_REPARSE_BUFFER];
    let mut returned = 0u32;
    // SAFETY: `handle` is open on the local reparse point and `buf` is valid writable memory.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            buf.as_mut_ptr().cast(),
            buf.len() as u32,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    let len = returned as usize;
    if ok == 0 || len < 16 || len > buf.len() {
        return None;
    }
    let tag = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if tag != expected_tag {
        return None;
    }
    let sub_off = u16::from_le_bytes([buf[8], buf[9]]) as usize;
    let sub_len = u16::from_le_bytes([buf[10], buf[11]]) as usize;
    let (base, relative) = if tag == IO_REPARSE_TAG_SYMLINK {
        if len < 20 {
            return None;
        }
        let flags = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
        (20usize, (flags & SYMLINK_FLAG_RELATIVE) != 0)
    } else {
        (16usize, false)
    };
    let start = base.checked_add(sub_off)?;
    let end = start.checked_add(sub_len)?;
    if sub_len == 0 || sub_len % 2 != 0 || end > len {
        return None;
    }
    let wide: Vec<u16> = buf[start..end]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some((String::from_utf16(&wide).ok()?, relative))
}

fn resolve_reparse_hop(parent_dir: &str, target: &str, relative: bool, rest: &[&str]) -> Option<String> {
    let resolved_base = if relative {
        if target.starts_with(['\\', '/']) || target.contains(':') {
            return None;
        }
        let parent_fwd = parent_dir.replace('\\', "/");
        if parent_fwd.len() < 3 || !parent_fwd.as_bytes()[0].is_ascii_alphabetic() || parent_fwd.as_bytes()[1] != b':' {
            return None;
        }
        let drive = &parent_fwd[..2];
        let mut stack: Vec<String> = parent_fwd[2..]
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(str::to_owned)
            .collect();
        for seg in target.replace('\\', "/").split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    stack.pop()?;
                }
                s => stack.push(s.to_owned()),
            }
        }
        if stack.is_empty() {
            format!("{drive}/")
        } else {
            format!("{drive}/{}", stack.join("/"))
        }
    } else {
        // Strip `\??\` (NT object prefix used by local NTFS junctions/symlinks such as
        // `\??\D:\repos\proj`). `\??\UNC\...` becomes `UNC\...` and is refused by
        // `safe_local_reveal_path` below without ever contacting SMB.
        let stripped = target.strip_prefix(r"\??\").unwrap_or(target);
        stripped.replace('\\', "/")
    };
    let mut full = resolved_base.trim_end_matches('/').to_owned();
    for seg in rest {
        full.push('/');
        full.push_str(seg);
    }
    if full.len() == 2 && full.as_bytes()[1] == b':' {
        full.push('/');
    }
    safe_local_reveal_path(&full)
}

/// Resolves a local workspace root directory once up front: allows a symlink/junction alias whose
/// NTFS target stays on a verified local fixed drive (common in `subst`-free dev setups), while
/// refusing UNC/network/mapped-drive targets BEFORE following them.
pub(super) fn resolve_root(clean: &str, hops: u8) -> Result<VerifiedLocalPath, LocalPathError> {
    if hops >= MAX_ROOT_ALIAS_HOPS {
        return Err(LocalPathError::SymlinkOrReparse);
    }
    let native = clean.replace('/', "\\");
    if !is_safe_fixed_drive(&native, 0) {
        return Err(LocalPathError::NonLocalDrive);
    }
    let drive = format!("{}:\\", native.as_bytes()[0].to_ascii_uppercase() as char);
    let segs: Vec<&str> = native[3..].split('\\').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut cur = PathBuf::from(&drive);
    if segs.is_empty() {
        let (file, info) = open_no_reparse(&cur)?;
        if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 {
            return Err(LocalPathError::UnsafePath);
        }
        let final_path = final_dos_path(file.as_raw_handle() as HANDLE).ok_or(LocalPathError::NonLocalDrive)?;
        return Ok(VerifiedLocalPath { path: final_path, is_dir: true, is_file: false });
    }
    for (idx, seg) in segs.iter().enumerate() {
        if *seg == ".." {
            return Err(LocalPathError::UnsafePath);
        }
        let parent_str = cur.to_string_lossy().into_owned();
        cur.push(seg);
        let (file, info, tag) = open_raw_no_reparse(&cur)?;
        if let Some(tag) = tag
            && !is_allowed_local_reparse_tag(tag)
        {
            let (target, relative) = read_reparse_target(file.as_raw_handle() as HANDLE, tag)
                .ok_or(LocalPathError::SymlinkOrReparse)?;
            let next_clean = resolve_reparse_hop(&parent_str, &target, relative, &segs[idx + 1..])
                .ok_or(LocalPathError::NonLocalDrive)?;
            return resolve_root(&next_clean, hops + 1);
        }
        let is_dir = (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        if !is_dir {
            return Err(LocalPathError::UnsafePath);
        }
        if idx + 1 == segs.len() {
            let final_path = final_dos_path(file.as_raw_handle() as HANDLE).ok_or(LocalPathError::NonLocalDrive)?;
            return Ok(VerifiedLocalPath { path: final_path, is_dir: true, is_file: false });
        }
    }
    Err(LocalPathError::UnsafePath)
}

pub(super) fn verify_path(clean: &str, depth: u8) -> Result<VerifiedLocalPath, LocalPathError> {
    let native = clean.replace('/', "\\");
    if !is_safe_fixed_drive(&native, depth) {
        return Err(LocalPathError::NonLocalDrive);
    }
    let drive = format!("{}:\\", native.as_bytes()[0].to_ascii_uppercase() as char);
    let segs: Vec<&str> = native[3..].split('\\').filter(|s| !s.is_empty() && *s != ".").collect();
    let mut cur = PathBuf::from(&drive);
    if segs.is_empty() {
        let (file, info) = open_no_reparse(&cur)?;
        if (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 {
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
    let verified_root = resolve_root(&safe_local_reveal_path(root)?, 0).ok()?;
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
