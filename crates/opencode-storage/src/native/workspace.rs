//! Evidence for reconciling edits made with no writable store. A pathname, matching boot
//! packet and a newly available lease are not proof that this is still the same database.
//! Keep the actual file from the initial read alive so a recycled file ID cannot match.
//! This is Windows-only; opening another descriptor on POSIX could release SQLite's locks.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::sync::Arc;
use windows_sys::Win32::Storage::FileSystem::{FILE_ID_INFO, FileIdInfo, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandleEx};

#[derive(Clone, Debug)]
pub struct OriginalFile(Arc<File>);

impl OriginalFile {
    pub(super) fn new(file:File) -> Self { Self(Arc::new(file)) }
    pub(super) fn file(&self) -> &File { &self.0 }

    /// Never creates, moves or repairs a file. A replacement, a missing file, or a volume
    /// that can't supply a stable ID cannot authorize automatic reconciliation. The caller
    /// must retain the returned no-delete-sharing handle and acquire the normal writer lease
    /// before comparing or writing draft rows. Identity is not a substitute for that lease.
    pub(super) fn same_existing_file(&self,path:&Path) -> io::Result<File> {
        let current=OpenOptions::new().read(true).share_mode(FILE_SHARE_READ|FILE_SHARE_WRITE).open(path)?;
        if file_id(&current)?!=file_id(&self.0)? { return Err(io::Error::other("the saved workspace was replaced")); }
        Ok(current)
    }
}

fn file_id(f:&File) -> io::Result<(u64,[u8;16])> {
    let mut info=std::mem::MaybeUninit::<FILE_ID_INFO>::uninit();
    // SAFETY: a live file handle and a buffer of exactly FILE_ID_INFO's size. Only read
    // the output when GetFileInformationByHandleEx says it initialized that buffer.
    #[allow(unsafe_code)]
    let info=unsafe {
        if GetFileInformationByHandleEx(f.as_raw_handle(),FileIdInfo,info.as_mut_ptr().cast(),std::mem::size_of::<FILE_ID_INFO>() as u32)==0 {
            return Err(io::Error::last_os_error());
        }
        info.assume_init()
    };
    let id=info.FileId.Identifier;
    if id==[0;16] { return Err(io::Error::other("no stable database file ID")); }
    Ok((info.VolumeSerialNumber,id))
}
