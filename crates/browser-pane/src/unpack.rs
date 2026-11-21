//! Unpacks the engine archive as it arrives: zstd (at most an 8 MB window, so unpacking needs
//! little memory) around a ustar archive of plain files under `OpenCode Browser Engine/`. Any
//! other entry (a link, a device, an absolute or `..` path) fails the install, so an archive can
//! only ever write inside the folder it unpacks into.

use std::collections::HashSet;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::download::Cut;
use crate::installer::{ENGINE_BUNDLE_NAME, Fault, err_write};

/// The largest zstd window this unpacks, as a power of two (package-windows.ps1 compresses at
/// level 19: 8 MB).
const MAX_WINDOW_LOG: u32 = 23;

pub(crate) enum UnpackError {
    /// It isn't an archive this unpacks, or the stream under it failed (the download's reader
    /// then knows why).
    Format,
    /// Writing a file failed.
    Write(Fault),
    /// The decoder or its buffer couldn't get memory (a computer out of commit): the bytes so far
    /// stay, and a retry goes on.
    Memory,
}

/// Unpacks `archive` into `dest` (a fresh staging directory owned by the caller), calling `unpacked` with each file's path once
/// it's whole and closed. Returns once the archive's end marker is read.
pub(crate) fn tar_zst(archive: &mut dyn Read, dest: &Path, unpacked: &mut dyn FnMut(&Path)) -> Result<(), UnpackError> {
    // The decoder reads its input in large pieces (a download's reads are syscalls).
    // Making the decoder reads nothing: it fails only when its context can't be allocated.
    let mut zstd = zstd::stream::read::Decoder::new(archive).map_err(|_| UnpackError::Memory)?;
    zstd.window_log_max(MAX_WINDOW_LOG).map_err(|_| UnpackError::Format)?;
    if !std::fs::symlink_metadata(dest).map_err(|e| UnpackError::Write(err_write(&e)))?.is_dir() {
        return Err(UnpackError::Format);
    }
    let mut made = HashSet::from([dest.to_owned()]);
    let mut buf = Vec::new();
    #[cfg(feature = "faults")]
    fault::memory()?;
    buf.try_reserve_exact(1 << 20).map_err(|_| UnpackError::Memory)?;
    buf.resize(1 << 20, 0);
    let mut header = [0u8; 512];
    loop {
        read_exact(&mut zstd, &mut header)?;
        if header.iter().all(|&b| b == 0) {
            return Ok(());
        }
        let entry = parse(&header).ok_or(UnpackError::Format)?;
        let path = dest.join(&entry.path);
        match entry.kind {
            Kind::Dir => make_dir(&path, &mut made)?,
            Kind::File { mode } => {
                make_dir(path.parent().ok_or(UnpackError::Format)?, &mut made)?;
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                // Only the execute bits come from the archive; creation respects the user's umask.
                #[cfg(unix)]
                options.mode(0o644 | (mode & 0o111));
                #[cfg(not(unix))]
                let _ = mode;
                // Unbuffered: each write is a whole piece of up to 1 MB already.
                let mut out = options.open(&path).map_err(create_error)?;
                let mut left = entry.size;
                while left > 0 {
                    let n = left.min(buf.len() as u64) as usize;
                    read_exact(&mut zstd, &mut buf[..n])?;
                    write(&mut out, &buf[..n]).map_err(|e| UnpackError::Write(err_write(&e)))?;
                    left -= n as u64;
                }
                out.flush().map_err(|e| UnpackError::Write(err_write(&e)))?;
                drop(out);
                unpacked(&path);
            }
        }
        // Entries end on a 512-byte boundary.
        let pad = (512 - entry.size % 512) % 512;
        read_exact(&mut zstd, &mut buf[..pad as usize])?;
    }
}

/// Create each component ourselves, including parents omitted by the packager. An existing
/// object in staging could be a link, a duplicate file, or a filesystem alias of an earlier name.
fn make_dir(path: &Path, made: &mut HashSet<PathBuf>) -> Result<(), UnpackError> {
    if made.contains(path) {
        return Ok(());
    }
    make_dir(path.parent().ok_or(UnpackError::Format)?, made)?;
    let mut dir = DirBuilder::new();
    #[cfg(unix)]
    dir.mode(0o755);
    dir.recursive(false).create(path).map_err(create_error)?;
    made.insert(path.to_owned());
    Ok(())
}

fn create_error(e: std::io::Error) -> UnpackError {
    if e.kind() == std::io::ErrorKind::AlreadyExists {
        UnpackError::Format
    } else {
        UnpackError::Write(err_write(&e))
    }
}

fn write(out: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(feature = "faults")]
    fault::write()?;
    out.write_all(bytes)
}

/// Harness builds only (the `faults` feature): `OPENCODE_BROWSER_FAULT_DISK_FULL=<n>` makes the
/// process's nth unpack write fail as a full disk does (ERROR_DISK_FULL, ENOSPC), once, so the
/// pane's disk-full case can be shown without filling a disk.
#[cfg(feature = "faults")]
mod fault {
    use std::sync::atomic::{AtomicU64, Ordering};

    static WRITES: AtomicU64 = AtomicU64::new(0);
    static MEMORY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// `OPENCODE_BROWSER_FAULT_NO_MEMORY=1`: the process's first unpack finds no memory for its
    /// buffer, as on a computer out of commit.
    pub(super) fn memory() -> Result<(), super::UnpackError> {
        let on = std::env::var_os("OPENCODE_BROWSER_FAULT_NO_MEMORY").is_some_and(|v| v == "1");
        if on && !MEMORY.swap(true, Ordering::Relaxed) { Err(super::UnpackError::Memory) } else { Ok(()) }
    }

    pub(super) fn write() -> std::io::Result<()> {
        let fail_at = std::env::var_os("OPENCODE_BROWSER_FAULT_DISK_FULL").and_then(|n| n.to_str()?.parse::<u64>().ok());
        if fail_at != Some(WRITES.fetch_add(1, Ordering::Relaxed) + 1) {
            return Ok(());
        }
        #[cfg(windows)]
        return Err(std::io::Error::from_raw_os_error(112));
        #[cfg(unix)]
        return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
    }
}

fn read_exact(r: &mut dyn Read, buf: &mut [u8]) -> Result<(), UnpackError> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Err(UnpackError::Format),
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            // zstd's ZSTD_error_memory_allocation, which isn't the archive's fault.
            Err(e) if e.to_string().contains("Allocation") => return Err(UnpackError::Memory),
            Err(_) => return Err(UnpackError::Format),
        }
    }
    Ok(())
}

enum Kind {
    File { mode: u32 },
    Dir,
}

struct Entry {
    path: PathBuf,
    size: u64,
    kind: Kind,
}

/// A ustar header: a plain file or a folder under the engine's folder, its checksum right.
fn parse(h: &[u8; 512]) -> Option<Entry> {
    let field = |range: std::ops::Range<usize>| {
        let f = &h[range];
        std::str::from_utf8(&f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]).ok()
    };
    let octal = |range| u64::from_str_radix(field(range)?.trim(), 8).ok();
    if &h[257..262] != b"ustar" {
        return None;
    }
    let sum: u64 = h.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) }).sum();
    if octal(148..156)? != sum {
        return None;
    }
    let size = octal(124..136)?;
    let kind = match h[156] {
        b'0' | 0 => Kind::File { mode: u32::try_from(octal(100..108)?).ok()? },
        b'5' if size == 0 => Kind::Dir,
        _ => return None,
    };
    let (prefix, name) = (field(345..500)?, field(0..100)?);
    let full = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
    let full = match kind {
        Kind::Dir => full.strip_suffix('/').unwrap_or(&full),
        Kind::File { .. } => full.as_str(),
    };
    let mut parts = full.split('/');
    if parts.next()? != ENGINE_BUNDLE_NAME || !parts.all(safe_name) {
        return None;
    }
    if matches!(kind, Kind::File { .. }) && full == ENGINE_BUNDLE_NAME {
        return None;
    }
    Some(Entry { path: PathBuf::from(full), size, kind })
}

fn safe_name(name: &str) -> bool {
    // Both packagers use portable filenames. Exclude dot components, alternate streams,
    // DOS device names and names Windows would silently trim or interpret as a separator.
    if name.is_empty() || name.ends_with(['.', ' ']) || name.contains(['\\', ':', '<', '>', '"', '|', '?', '*']) || name.chars().any(char::is_control) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ').to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return false;
    }
    let port = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"));
    !port.is_some_and(|n| matches!(n.as_bytes(), [b'0'..=b'9']) || matches!(n, "¹" | "²" | "³"))
}

impl UnpackError {
    /// How the install ends (when the download's reader didn't fail first): a broken archive is
    /// never resumed, a failed write is.
    pub(crate) fn cut(self) -> Cut {
        match self {
            UnpackError::Format => Cut::Rejected(Fault::Corrupt),
            UnpackError::Write(msg) => Cut::Failed(msg),
            UnpackError::Memory => Cut::Failed(Fault::NoMemory),
        }
    }
}

/// Grants `ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`) and `ALL APPLICATION PACKAGES`
/// (`S-1-15-2-1`) inherited read/execute (`(OI)(CI)(RX)`) on the unpacked Windows browser engine
/// directory (`official/packages/desktop/resources/windows/installer.nsh:29-31`). Required because
/// `%LOCALAPPDATA%` grants access only to SYSTEM, Administrators, and the current user, which
/// blocks Chromium's LPAC / AppContainer sandboxed subprocesses from loading `libcef.dll` and
/// `opencode-browser-engine.dll`.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn grant_sandbox_rx(dir: &Path) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Authorization::{
        ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GetExplicitEntriesFromAclW,
        GetNamedSecurityInfoW, GRANT_ACCESS, SE_FILE_OBJECT, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows::Win32::Security::{
        ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, EqualSid, OBJECT_INHERIT_ACE,
        PSECURITY_DESCRIPTOR, PSID,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    };
    use windows::core::{PCWSTR, PWSTR};

    // Never follow a symlink or junction when adjusting directory ACLs.
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    if !meta.is_dir() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0) != 0 {
        return;
    }

    let wide: Vec<u16> = OsStr::new(dir).encode_wide().chain(std::iter::once(0)).collect();
    let mut old_dacl: *mut ACL = std::ptr::null_mut();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: valid NUL-terminated UTF-16 path; `sd` is freed via `LocalFree` below.
    let err = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(wide.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut old_dacl),
            None,
            &mut sd,
        )
    };
    if err.0 != 0 {
        return;
    }

    let mut sids: Vec<PSID> = Vec::with_capacity(2);
    for sid_str in ["S-1-15-2-2", "S-1-15-2-1"] {
        let sid_wide: Vec<u16> = sid_str.encode_utf16().chain(std::iter::once(0)).collect();
        let mut psid = PSID::default();
        // SAFETY: converts well-known SID string; freed via `LocalFree` below.
        if unsafe { ConvertStringSidToSidW(PCWSTR(sid_wide.as_ptr()), &mut psid) }.is_ok()
            && !psid.0.is_null()
        {
            sids.push(psid);
        }
    }

    let access = (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE).0;
    let inheritance = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;

    // Fast path: skip rewriting NTFS ACLs on every launch when both ACEs are already present.
    let mut missing: Vec<PSID> = sids.clone();
    if !old_dacl.is_null() && !missing.is_empty() {
        let mut count = 0u32;
        let mut existing: *mut EXPLICIT_ACCESS_W = std::ptr::null_mut();
        // SAFETY: `old_dacl` is owned by `sd`; `existing` is freed via `LocalFree` below.
        if unsafe { GetExplicitEntriesFromAclW(old_dacl, &mut count, &mut existing) }.0 == 0
            && !existing.is_null()
        {
            // SAFETY: `GetExplicitEntriesFromAclW` guarantees `count` initialized entries.
            let slice = unsafe { std::slice::from_raw_parts(existing, count as usize) };
            missing.retain(|&want| {
                !slice.iter().any(|e| {
                    e.grfAccessMode == GRANT_ACCESS
                        && (e.grfAccessPermissions & access) == access
                        && (e.grfInheritance & inheritance) == inheritance
                        && e.Trustee.TrusteeForm == TRUSTEE_IS_SID
                        && !e.Trustee.ptstrName.0.is_null()
                        && unsafe { EqualSid(PSID(e.Trustee.ptstrName.0.cast()), want) }.is_ok()
                })
            });
            // SAFETY: `existing` was allocated by `GetExplicitEntriesFromAclW`.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(existing.cast())));
            }
        }
    }

    if !missing.is_empty() {
        let entries: Vec<EXPLICIT_ACCESS_W> = missing
            .iter()
            .map(|&psid| EXPLICIT_ACCESS_W {
                grfAccessPermissions: access,
                grfAccessMode: GRANT_ACCESS,
                grfInheritance: inheritance,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: Default::default(),
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: Default::default(),
                    ptstrName: PWSTR(psid.0.cast()),
                },
            })
            .collect();
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `entries` and `old_dacl` are valid for this call; `new_dacl` freed via `LocalFree`.
        let acl_err = unsafe { SetEntriesInAclW(Some(&entries), Some(old_dacl), &mut new_dacl) };
        if acl_err.0 == 0 && !new_dacl.is_null() {
            // SAFETY: updates the directory DACL so inherited ACEs propagate to child files.
            let _ = unsafe {
                SetNamedSecurityInfoW(
                    PCWSTR(wide.as_ptr()),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(new_dacl),
                    None,
                )
            };
            // SAFETY: `new_dacl` was allocated by `SetEntriesInAclW`.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(new_dacl.cast())));
            }
        }
    }

    for psid in sids {
        // SAFETY: each `psid` was allocated by `ConvertStringSidToSidW`.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(psid.0)));
        }
    }
    if !sd.0.is_null() {
        // SAFETY: `sd` was allocated by `GetNamedSecurityInfoW`.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
    }
}
