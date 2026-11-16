//! Publishing a verified portable tree never exposes an empty install path. The two directory
//! renames commit together; on a volume without transactions, fail with both trees untouched.
#![allow(unsafe_code)]

use std::{fs, io, path::Path, ptr};
use std::os::windows::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}, io::{AsRawHandle, FromRawHandle, OwnedHandle}};
use windows_sys::Win32::{Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GENERIC_WRITE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0}, Storage::FileSystem::*, System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject}};

fn wide(p: &Path) -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() }
fn check(ok: i32) -> io::Result<()> { if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) } }

struct Transaction(OwnedHandle);
impl Transaction {
    fn begin() -> io::Result<Self> {
        // SAFETY: default transaction options, no borrowed pointers. Closing the last handle
        // before commit (including process termination) aborts the transaction.
        let h = unsafe { CreateTransaction(ptr::null_mut(), ptr::null_mut(), 0, 0, 0, 0, ptr::null()) };
        if h == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(h) }))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        // SAFETY: terminated paths and a live transaction; no progress callback.
        check(unsafe { MoveFileTransactedW(wide(from).as_ptr(), wide(to).as_ptr(), None, ptr::null(), 0, self.0.as_raw_handle()) })
    }
    fn commit(self) -> io::Result<()> { check(unsafe { CommitTransaction(self.0.as_raw_handle()) }) }

    fn unlink(&self, path: &Path) -> io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 { return Err(io::Error::other("not following an update reparse point")); }
        if !meta.is_dir() {
            return check(unsafe { DeleteFileTransactedW(wide(path).as_ptr(), self.0.as_raw_handle()) });
        }
        for entry in fs::read_dir(path)? { self.unlink(&entry?.path())?; }
        check(unsafe { RemoveDirectoryTransactedW(wide(path).as_ptr(), self.0.as_raw_handle()) })
    }
}

/// Transactional states. Before Publish the installed name still resolves to the old tree to
/// every other process. After Publish it resolves only to the complete verified stage.
enum Swap { Staged, OldAside, NewPlaced, Published }
enum Effect { SetOldAside, PlaceNew, Publish, Done }
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

pub(in crate::updater) fn publish(root: &Path, stage: &Path, previous: &Path) -> io::Result<()> {
    let tx = Transaction::begin()?;
    let mut state = Swap::Staged;
    loop {
        let (next, effect) = state.step();
        match effect {
            Effect::SetOldAside => tx.rename(root, previous)?,
            Effect::PlaceNew => tx.rename(stage, root)?,
            Effect::Publish => return tx.commit(),
            Effect::Done => return Ok(()),
        }
        state = next;
    }
}

/// No deletion starts unless the executable can be opened for exclusive write (a mapped
/// image refuses this). Keep the handle to exclude a racing launch until the tree is gone.
/// Locked siblings, sharing failures and interrupted cleanup roll back *all* unlinks.
pub(in crate::updater) fn retire(old: &Path) -> io::Result<()> {
    let tx = Transaction::begin()?;
    // Enlist the guard in *this* transaction; a non-transacted writer conflicts with our own
    // delete. Its exclusive write access is denied for a mapped executable, including when
    // the loader has closed its ordinary file handles.
    let h = unsafe { CreateFileTransactedW(wide(&old.join(super::EXE)).as_ptr(), GENERIC_WRITE, FILE_SHARE_DELETE, ptr::null(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, ptr::null_mut(), tx.0.as_raw_handle(), ptr::null(), ptr::null()) };
    if h == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); }
    let image = unsafe { OwnedHandle::from_raw_handle(h) };
    // Enlist the executable's deletion before releasing its guard, but close the handle
    // before removing its parent directory. Windows keeps a delete-pending child in the
    // directory until the last open file handle goes away.
    let exe = old.join(super::EXE);
    check(unsafe { DeleteFileTransactedW(wide(&exe).as_ptr(), tx.0.as_raw_handle()) })?;
    drop(image);
    for e in fs::read_dir(old)? {
        let path = e?.path();
        if path != exe { tx.unlink(&path)?; }
    }
    check(unsafe { RemoveDirectoryTransactedW(wide(old).as_ptr(), tx.0.as_raw_handle()) })?;
    tx.commit()
}

/// File identity survives a same-volume rename. Bind the archive to both its verified
/// directory and its executable, not to the spelling of the path or a helper exit status.
pub(in crate::updater) fn identity(dir: &Path) -> io::Result<String> {
    fn id(path: &Path) -> io::Result<String> {
        let f = fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
        let mut info = std::mem::MaybeUninit::uninit();
        // SAFETY: valid file handle, OS initializes info on success.
        check(unsafe { GetFileInformationByHandle(f.as_raw_handle(), info.as_mut_ptr()) })?;
        let i = unsafe { info.assume_init() };
        if i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || (i.nFileIndexHigh == 0 && i.nFileIndexLow == 0) { return Err(io::Error::other("bundle has no stable file identity")); }
        Ok(format!("{:08x}:{:08x}{:08x}", i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow))
    }
    Ok(format!("{}/{}", id(dir)?, id(&dir.join(super::EXE))?))
}

/// True only once `pid` has completely exited (its handle table is closed, or the PID no longer exists).
pub(super) fn process_gone(pid: u32) -> bool {
    // SAFETY: queries synchronization state only and closes any returned handle once.
    let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if h.is_null() {
        return io::Error::last_os_error().raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32);
    }
    let done = unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 };
    unsafe { CloseHandle(h) };
    done
}
