//! Bundle swap and retirement on Windows.
//!
//! Preferred path (`Mode::Transacted`): both directory renames commit together in one Kernel
//! Transaction Manager (KTM / TxF) transaction (`Swap`), so the install path is never absent, and
//! retirement unlinks the previous bundle inside a transaction guarded by an exclusive-write handle
//! on its executable.
//!
//! Runtime fallback (`Mode::Plain`): TxF exists only on local NTFS and can be disabled by policy
//! or blocked by filter drivers; Windows Dev Drives (ReFS), exFAT/FAT32 and network shares return
//! not-supported errors either on `CreateTransaction` or on the first transacted file call that
//! enlists the volume. On those exact error codes (`tx_not_supported`), `publish` and `retire`
//! drop the unused transaction and run `PlainSwap` / `retire_plain`:
//! - Each same-volume `fs::rename` is metadata-atomic on its own, so neither `root`, `stage` nor
//!   `previous` is ever half-written or mixed.
//! - **Weaker guarantee on the plain path:** `root` is briefly absent between `MoveOldAside` and
//!   `PlaceNew` (microseconds in normal operation, or until repaired if the helper is killed at
//!   `OldAside`). A launch aimed at `root\opencode-desktop.exe` in that instant fails honestly at
//!   OS `CreateProcessW` (`ERROR_FILE_NOT_FOUND` / `ERROR_PATH_NOT_FOUND`) rather than ever
//!   starting half a bundle. If `PlaceNew` returns an error, `RestoreOld` puts `previous` back at
//!   `root`; if the helper is killed at `OldAside`, `windows::repair_interrupted` restores `root`
//!   from the verified stage or `previous` before any `.old-*` folder can be retired.
//! - Plain retirement opens `old\opencode-desktop.exe` for exclusive write (`FILE_SHARE_DELETE`
//!   only) before touching any sibling file: a mapped executable refuses with
//!   `ERROR_SHARING_VIOLATION`, keeping every asset intact; once opened, the executable is
//!   unlinked first before closing the guard handle and removing the remaining files.
#![allow(unsafe_code)]

use std::io::Read as _;
use std::os::windows::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
    io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use std::{fs, io, path::Path, ptr};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
        ERROR_NOT_SUPPORTED, ERROR_NOT_SUPPORTED_ON_DAX, ERROR_RM_DISCONNECTED,
        ERROR_RM_NOT_ACTIVE, ERROR_TRANSACTED_MAPPING_UNSUPPORTED_REMOTE,
        ERROR_TRANSACTIONAL_OPEN_NOT_ALLOWED, ERROR_TRANSACTIONMANAGER_NOT_FOUND,
        ERROR_TRANSACTIONMANAGER_NOT_ONLINE, ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE, GENERIC_WRITE,
        INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    },
    Storage::FileSystem::*,
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Transacted,
    Plain,
}

impl Mode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Transacted => "transacted",
            Self::Plain => "plain-fallback",
        }
    }
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain([0]).collect()
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Only explicit "KTM / TxF not supported or disabled on this volume" codes fall back to plain
/// renames. Sharing, locking, permission, missing-path, conflict and disk-space errors are real
/// failures and never fall back.
fn tx_not_supported(err: &io::Error) -> bool {
    let Some(code) = err.raw_os_error().and_then(|c| u32::try_from(c).ok()) else { return false };
    matches!(
        code,
        ERROR_INVALID_FUNCTION
            | ERROR_NOT_SUPPORTED
            | ERROR_CALL_NOT_IMPLEMENTED
            | ERROR_NOT_SUPPORTED_ON_DAX
            | ERROR_TRANSACTIONMANAGER_NOT_FOUND
            | ERROR_TRANSACTIONMANAGER_NOT_ONLINE
            | ERROR_RM_NOT_ACTIVE
            | ERROR_RM_DISCONNECTED
            | ERROR_TRANSACTIONS_UNSUPPORTED_REMOTE
            | ERROR_TRANSACTIONAL_OPEN_NOT_ALLOWED
            | ERROR_TRANSACTED_MAPPING_UNSUPPORTED_REMOTE
    )
}

/// Proof-only fault switch for forcing the non-transacted fallback path on an NTFS machine.
///
/// Guaranteed absent from shipping builds by two independent compile-time gates:
/// 1. `#[cfg(debug_assertions)]`: `[profile.release]` (`Cargo.toml`) compiles with Cargo's
///    default `debug-assertions = false`, eliminating this function and its call sites from the
///    compiled binary.
/// 2. `build_channel() == "local"` (`option_env!("OPENCODE_RUNE_BUILD_CHANNEL")`): release CI
///    sets `"dev"` or `"stable"`, so even an optimized build with assertions enabled ignores it.
#[cfg(debug_assertions)]
fn debug_txf_fault(at_create: bool) -> Option<io::Error> {
    if crate::build_channel() != "local" {
        return None;
    }
    let mode = std::env::var_os("OPENCODE_RUNE_DISABLE_TXF")?;
    let s = mode.to_string_lossy();
    match (at_create, s.as_ref()) {
        (true, "create") => Some(io::Error::from_raw_os_error(ERROR_RM_NOT_ACTIVE as i32)),
        (false, "1" | "enlist") => Some(io::Error::from_raw_os_error(ERROR_NOT_SUPPORTED as i32)),
        _ => None,
    }
}

struct Transaction(OwnedHandle);

impl Transaction {
    fn begin() -> io::Result<Self> {
        #[cfg(debug_assertions)]
        if let Some(err) = debug_txf_fault(true) {
            return Err(err);
        }
        // SAFETY: default transaction options, no borrowed pointers. Closing the handle before
        // commit (including process termination) rolls the transaction back.
        let h = unsafe { CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null()) };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateTransaction` succeeded and returned a valid, owned transaction handle.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(h) }))
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        #[cfg(debug_assertions)]
        if let Some(err) = debug_txf_fault(false) {
            return Err(err);
        }
        // SAFETY: NUL-terminated paths and a live transaction handle; no progress callback.
        check(unsafe { MoveFileTransactedW(wide(from).as_ptr(), wide(to).as_ptr(), None, ptr::null(), 0, self.0.as_raw_handle()) })
    }

    fn open_exclusive(&self, exe: &Path) -> io::Result<OwnedHandle> {
        #[cfg(debug_assertions)]
        if let Some(err) = debug_txf_fault(false) {
            return Err(err);
        }
        // SAFETY: NUL-terminated path and live transaction; exclusive write refuses any mapped exe.
        let h = unsafe {
            CreateFileTransactedW(
                wide(exe).as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
                self.0.as_raw_handle(),
                ptr::null(),
                ptr::null(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `CreateFileTransactedW` succeeded and returned a valid, owned file handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(h) })
    }

    fn commit(self) -> io::Result<()> {
        // SAFETY: `self.0` is a live transaction handle owned by `self`, committed once before drop.
        check(unsafe { CommitTransaction(self.0.as_raw_handle()) })
    }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("not following an update reparse point"));
        }
        if !meta.is_dir() {
            // SAFETY: `wide(path)` is NUL-terminated for this call and `self.0` is a live transaction handle.
            return check(unsafe { DeleteFileTransactedW(wide(path).as_ptr(), self.0.as_raw_handle()) });
        }
        for entry in fs::read_dir(path)? {
            self.unlink(&entry?.path())?;
        }
        // SAFETY: `wide(path)` is NUL-terminated for this call and `self.0` is a live transaction handle.
        check(unsafe { RemoveDirectoryTransactedW(wide(path).as_ptr(), self.0.as_raw_handle()) })
    }
}

/// Transacted swap states (`Mode::Transacted`).
enum Swap {
    Staged,
    OldAside,
    NewPlaced,
    Published,
}
enum Effect {
    SetOldAside,
    PlaceNew,
    Publish,
    Done,
}
impl Swap {
    fn step(self) -> (Self, Effect) {
        match self {
            Self::Staged => (Self::OldAside, Effect::SetOldAside),
            Self::OldAside => (Self::NewPlaced, Effect::PlaceNew),
            Self::NewPlaced => (Self::Published, Effect::Publish),
            Self::Published => (self, Effect::Done),
        }
    }
}

/// Non-transacted swap states (`Mode::Plain`; see module docs for the weaker between-rename window).
enum PlainSwap {
    Staged,
    OldAside,
    Placing,
    RollingBack,
    Finished,
}
enum PlainEvent {
    Advanced,
    Failed,
}
enum PlainEffect {
    MoveOldAside,
    PlaceNew,
    RestoreOld,
    Done,
}
impl PlainSwap {
    fn step(self, event: PlainEvent) -> (Self, PlainEffect) {
        match (self, event) {
            (Self::Staged, PlainEvent::Advanced) => (Self::OldAside, PlainEffect::MoveOldAside),
            (Self::OldAside, PlainEvent::Advanced) => (Self::Placing, PlainEffect::PlaceNew),
            (Self::Placing, PlainEvent::Advanced) => (Self::Finished, PlainEffect::Done),
            (Self::Placing, PlainEvent::Failed) => (Self::RollingBack, PlainEffect::RestoreOld),
            _ => (Self::Finished, PlainEffect::Done),
        }
    }
}

pub(crate) fn publish(root: &Path, stage: &Path, previous: &Path) -> io::Result<Mode> {
    let tx = match Transaction::begin() {
        Ok(tx) => tx,
        Err(e) if tx_not_supported(&e) => return publish_plain(root, stage, previous).map(|()| Mode::Plain),
        Err(e) => return Err(e),
    };
    let mut state = Swap::Staged;
    loop {
        let (next, effect) = state.step();
        match effect {
            Effect::SetOldAside => {
                if let Err(e) = tx.rename(root, previous) {
                    drop(tx);
                    if tx_not_supported(&e) {
                        return publish_plain(root, stage, previous).map(|()| Mode::Plain);
                    }
                    return Err(e);
                }
            }
            Effect::PlaceNew => tx.rename(stage, root)?,
            Effect::Publish => return tx.commit().map(|()| Mode::Transacted),
            Effect::Done => return Ok(Mode::Transacted),
        }
        state = next;
    }
}

fn publish_plain(root: &Path, stage: &Path, previous: &Path) -> io::Result<()> {
    let mut state = PlainSwap::Staged;
    let mut event = PlainEvent::Advanced;
    let mut err: Option<io::Error> = None;
    loop {
        let (next, effect) = state.step(event);
        state = next;
        event = PlainEvent::Advanced;
        match effect {
            PlainEffect::MoveOldAside => fs::rename(root, previous)?,
            PlainEffect::PlaceNew => {
                if let Err(e) = fs::rename(stage, root) {
                    err = Some(e);
                    event = PlainEvent::Failed;
                }
            }
            PlainEffect::RestoreOld => {
                fs::rename(previous, root).ok();
            }
            PlainEffect::Done => return err.map_or(Ok(()), Err),
        }
    }
}

pub(crate) fn retire(old: &Path) -> io::Result<Mode> {
    if fs::symlink_metadata(old)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("not following an update reparse point"));
    }
    let tx = match Transaction::begin() {
        Ok(tx) => tx,
        Err(e) if tx_not_supported(&e) => return retire_plain(old).map(|()| Mode::Plain),
        Err(e) => return Err(e),
    };
    let exe = old.join(super::EXE);
    let image = match tx.open_exclusive(&exe) {
        Ok(h) => Some(h),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            drop(tx);
            if tx_not_supported(&e) {
                return retire_plain(old).map(|()| Mode::Plain);
            }
            return Err(e);
        }
    };
    if let Some(image) = image {
        // SAFETY: `wide(&exe)` is NUL-terminated for this call and `tx.0` is a live transaction handle.
        check(unsafe { DeleteFileTransactedW(wide(&exe).as_ptr(), tx.0.as_raw_handle()) })?;
        drop(image);
    }
    for e in fs::read_dir(old)? {
        let path = e?.path();
        if path != exe {
            tx.unlink(&path)?;
        }
    }
    // SAFETY: `wide(old)` is NUL-terminated for this call and `tx.0` is a live transaction handle.
    check(unsafe { RemoveDirectoryTransactedW(wide(old).as_ptr(), tx.0.as_raw_handle()) })?;
    tx.commit().map(|()| Mode::Transacted)
}

fn retire_plain(old: &Path) -> io::Result<()> {
    if fs::symlink_metadata(old)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("not following an update reparse point"));
    }
    let exe = old.join(super::EXE);
    match fs::OpenOptions::new().write(true).share_mode(FILE_SHARE_DELETE).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&exe) {
        Ok(image) => {
            // Unlink the executable while holding the exclusive-write guard, then close the
            // handle so the executable is gone before any sibling asset is removed.
            // SAFETY: `wide(&exe)` is a valid NUL-terminated UTF-16 path for the duration of the call.
            check(unsafe { DeleteFileW(wide(&exe).as_ptr()) })?;
            drop(image);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    unlink_plain(old)
}

fn unlink_plain(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("not following an update reparse point"));
    }
    if !meta.is_dir() {
        return fs::remove_file(path);
    }
    for entry in fs::read_dir(path)? {
        unlink_plain(&entry?.path())?;
    }
    fs::remove_dir(path)
}

/// Identifies a verified bundle across a same-volume rename on NTFS, ReFS (128-bit `FileIdInfo`),
/// and filesystems without stable file IDs (`exe_len` + `exe_sha256`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BundleId {
    #[serde(default)]
    pub file_id: Option<String>,
    #[serde(default)]
    pub exe_len: u64,
    #[serde(default)]
    pub exe_sha256: String,
}

pub(crate) fn identify(dir: &Path) -> io::Result<BundleId> {
    let exe = dir.join(super::EXE);
    if fs::symlink_metadata(dir)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("bundle directory is a reparse point"));
    }
    let meta = fs::symlink_metadata(&exe)?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
        return Err(io::Error::other("bundle executable is missing or a reparse point"));
    }
    let (exe_len, exe_sha256) = hash_file(&exe)?;
    let file_id = pair_file_id(dir, &exe);
    Ok(BundleId { file_id, exe_len, exe_sha256 })
}

pub(crate) fn matches_id(dir: &Path, expected: &BundleId) -> bool {
    let exe = dir.join(super::EXE);
    let Ok(dir_meta) = fs::symlink_metadata(dir) else { return false };
    let Ok(exe_meta) = fs::symlink_metadata(&exe) else { return false };
    if (dir_meta.file_attributes() | exe_meta.file_attributes()) & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !exe_meta.is_file() {
        return false;
    }
    if expected.exe_len != 0 && exe_meta.len() != expected.exe_len {
        return false;
    }
    if let Some(want) = &expected.file_id
        && pair_file_id(dir, &exe).as_ref() == Some(want)
    {
        return true;
    }
    !expected.exe_sha256.is_empty() && hash_file(&exe).is_ok_and(|(len, sha)| len == expected.exe_len && sha == expected.exe_sha256)
}

fn pair_file_id(dir: &Path, exe: &Path) -> Option<String> {
    Some(format!("{}/{}", file_id_128(dir)?, file_id_128(exe)?))
}

pub(crate) fn current_file_id(dir: &Path) -> Option<String> {
    pair_file_id(dir, &dir.join(super::EXE))
}

fn file_id_128(path: &Path) -> Option<String> {
    let f = fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path).ok()?;
    let mut info = std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: valid handle; `FILE_ID_INFO` matches `FileIdInfo` and covers 64-bit NTFS and 128-bit ReFS IDs.
    let ok = unsafe { GetFileInformationByHandleEx(f.as_raw_handle(), FileIdInfo, info.as_mut_ptr().cast(), std::mem::size_of::<FILE_ID_INFO>() as u32) };
    if ok == 0 {
        return None;
    }
    // SAFETY: `GetFileInformationByHandleEx` succeeded and wrote a complete `FILE_ID_INFO` into `info`.
    let info = unsafe { info.assume_init() };
    if info.FileId.Identifier == [0u8; 16] {
        return None;
    }
    let hex: String = info.FileId.Identifier.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{:016x}:{hex}", info.VolumeSerialNumber))
}

fn hash_file(path: &Path) -> io::Result<(u64, String)> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buf[..n]);
    }
    let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((total, hex))
}

/// True only once `pid` has completely exited (its handle table is closed, or the PID no longer exists).
pub(super) fn process_gone(pid: u32) -> bool {
    // SAFETY: queries synchronization state only and closes any returned handle once.
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32);
    }
    // SAFETY: `h` is a valid non-null process handle with `PROCESS_SYNCHRONIZE` access returned by `OpenProcess`.
    let done = unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 };
    // SAFETY: `h` is owned by this function and closed once after waiting.
    unsafe { CloseHandle(h) };
    done
}
