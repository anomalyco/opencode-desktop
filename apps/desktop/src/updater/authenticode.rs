//! Windows' Authenticode check of a downloaded file (`WinVerifyTrust`, the same policy Explorer
//! and SmartScreen apply), and the signer's name from its certificate.
// Windows' trust API is unsafe by nature; each block says what it relies on.
#![allow(unsafe_code)]

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::Cryptography::{CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW};
use windows_sys::Win32::Security::WinTrust::{
    WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
    WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
};

/// `TRUST_E_NOSIGNATURE`: the file carries no signature.
const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100_u32 as i32;

/// The name of the file's trusted signer, or why Windows doesn't trust it.
pub fn signer(path: &Path) -> Result<String, String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file = WINTRUST_FILE_INFO { cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: wide.as_ptr(), ..Default::default() };
    let mut data = WINTRUST_DATA {
        cbStruct: size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        // Revocation needs the network and the release's certificate is checked by name below.
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ..Default::default()
    };
    data.Anonymous.pFile = &mut file;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    // SAFETY: `data` and `file` (and the path they point at) outlive the call; no window.
    let status = unsafe { WinVerifyTrust(core::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    let name = (status == 0).then(|| signer_name(data.hWVTStateData)).flatten();
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    // SAFETY: closes the state the verify call opened, with the same structures.
    unsafe { WinVerifyTrust(core::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
    match status {
        0 => name.ok_or_else(|| "its signer couldn't be read".to_owned()),
        TRUST_E_NOSIGNATURE => Err("it isn't signed".into()),
        code => Err(format!("Windows doesn't trust its signature (0x{:08X})", code as u32)),
    }
}

fn signer_name(state: HANDLE) -> Option<String> {
    // SAFETY: `state` is the open verify state; the helpers return pointers into it, read before
    // it closes.
    unsafe {
        let prov = WTHelperProvDataFromStateData(state);
        if prov.is_null() {
            return None;
        }
        let sgnr = WTHelperGetProvSignerFromChain(prov, 0, 0, 0);
        if sgnr.is_null() || (*sgnr).csCertChain == 0 || (*sgnr).pasCertChain.is_null() {
            return None;
        }
        let cert = (*(*sgnr).pasCertChain).pCert;
        if cert.is_null() {
            return None;
        }
        let mut buf = [0u16; 512];
        let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, core::ptr::null(), buf.as_mut_ptr(), buf.len() as u32) as usize;
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n - 1]))
    }
}
