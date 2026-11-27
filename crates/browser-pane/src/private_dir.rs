//! A short folder that only the current Windows user can open, for the engine's socket when the
//! user's temporary folder is too long for one and its volume keeps no 8.3 names (8.3 turned off,
//! ReFS, a Dev Drive). Without it such a user has no browser.
//!
//! `%ProgramData%\opencode-browser\<16 hex of the user's SID>`, created with a protected DACL that
//! grants full control to the user and SYSTEM only. A folder already there is used only when the
//! user owns it and it is no reparse point; its DACL is then set again. Another user can't plant
//! one: the owner check refuses it.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetSecurityInfo,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

#[allow(unsafe_code)]
pub(crate) fn private_short_dir() -> std::io::Result<PathBuf> {
    let user = UserSid::current()?;
    let root = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let parent = root.join("opencode-browser");
    std::fs::create_dir_all(&parent)?;
    refuse_reparse(&parent)?;
    let digest = Sha256::digest(user.text.as_bytes());
    let dir = parent.join(digest[..8].iter().map(|b| format!("{b:02x}")).collect::<String>());

    let sddl = wide(&format!("O:{0}D:P(A;OICI;FA;;;{0})(A;OICI;FA;;;SY)", user.text));
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` is a NUL-terminated SDDL string; `sd` is freed by `LocalFree` below.
    unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
        .map_err(std::io::Error::other)?;
    let result = create_or_adopt(&dir, &user, sd);
    // SAFETY: allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(sd.0)));
    }
    result.map(|()| dir)
}

#[allow(unsafe_code)]
fn create_or_adopt(dir: &Path, user: &UserSid, sd: PSECURITY_DESCRIPTOR) -> std::io::Result<()> {
    let path = wide_path(dir);
    let attrs = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: NUL-terminated path; `attrs` points at a valid self-relative descriptor for the call.
    match unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&attrs)) } {
        Ok(()) => return Ok(()),
        Err(e) if e.code() != ERROR_ALREADY_EXISTS.to_hresult() => return Err(std::io::Error::other(e)),
        Err(_) => {}
    }
    // Already there: ours only if this user owns it and it is no reparse point; then its DACL again.
    // SAFETY: NUL-terminated path; the handle is closed by `Handle::drop`.
    let handle = Handle(
        unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                (READ_CONTROL | WRITE_DAC).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
        }
        .map_err(std::io::Error::other)?,
    );
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `handle.0` is an open handle.
    unsafe { GetFileInformationByHandle(handle.0, &mut info) }.map_err(std::io::Error::other)?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(denied("the private socket folder is a reparse point"));
    }
    let mut owner = PSID::default();
    let mut owner_sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `handle` has READ_CONTROL; `owner` points into `owner_sd`, freed below.
    let rc = unsafe {
        GetSecurityInfo(handle.0, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut owner_sd))
    };
    if rc.0 != 0 {
        return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
    }
    // SAFETY: both SIDs are valid for the duration of the call.
    let ours = unsafe { EqualSid(owner, user.sid()) }.is_ok();
    // SAFETY: allocated by `GetSecurityInfo`.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(owner_sd.0)));
    }
    if !ours {
        return Err(denied("the private socket folder belongs to another user"));
    }
    let mut present = false.into();
    let mut defaulted = false.into();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `sd` is a valid descriptor; `dacl` points into it.
    unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) }.map_err(std::io::Error::other)?;
    // SAFETY: `handle` has WRITE_DAC; `dacl` stays valid while `sd` lives (the caller frees it later).
    let rc = unsafe {
        SetSecurityInfo(handle.0, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, None, None, Some(dacl), None)
    };
    if rc.0 != 0 {
        return Err(std::io::Error::from_raw_os_error(rc.0 as i32));
    }
    Ok(())
}

fn refuse_reparse(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || crate::installer::is_reparse_or_symlink(path) {
        return Err(denied("the socket folder's parent is a reparse point"));
    }
    Ok(())
}

fn denied(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, msg)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(Some(0)).collect()
}

struct Handle(HANDLE);

impl Drop for Handle {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        // SAFETY: an owned handle from `CreateFileW` / `OpenProcessToken`.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The current user's SID, as the token's bytes and as text (`S-1-5-21-…`).
struct UserSid {
    token_user: Vec<u8>,
    text: String,
}

impl UserSid {
    #[allow(unsafe_code)]
    fn current() -> std::io::Result<Self> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo-handle of this process; `token` is closed by `Handle::drop`.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(std::io::Error::other)?;
        let token = Handle(token);
        let mut len = 0u32;
        // SAFETY: a size query; it fails with ERROR_INSUFFICIENT_BUFFER and sets `len`.
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
        let mut token_user = vec![0u8; len as usize];
        // SAFETY: `token_user` holds `len` bytes.
        unsafe { GetTokenInformation(token.0, TokenUser, Some(token_user.as_mut_ptr().cast()), len, &mut len) }
            .map_err(std::io::Error::other)?;
        // SAFETY: `token_user` now starts with a `TOKEN_USER` whose SID points inside the buffer.
        let sid = unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let mut text = PWSTR::null();
        // SAFETY: `sid` is valid; `text` is freed by `LocalFree` below.
        unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(std::io::Error::other)?;
        // SAFETY: `text` is a NUL-terminated string from `ConvertSidToStringSidW`.
        let s = unsafe { text.to_string() }.map_err(std::io::Error::other)?;
        // SAFETY: allocated by `ConvertSidToStringSidW`.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        }
        Ok(Self { token_user, text: s })
    }

    #[allow(unsafe_code)]
    fn sid(&self) -> PSID {
        // SAFETY: `token_user` starts with a `TOKEN_USER` written by `GetTokenInformation`.
        unsafe { (*self.token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}
