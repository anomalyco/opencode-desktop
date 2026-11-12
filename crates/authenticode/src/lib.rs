//! Windows' Authenticode check of a downloaded file (`WinVerifyTrust`, the same policy Explorer
//! and SmartScreen apply), and the signer's name from its certificate.
#![cfg(windows)]
// Windows' trust API is unsafe by nature; each block says what it relies on.
#![allow(unsafe_code)]

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::Cryptography::{
    CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL, CERT_CHAIN_CONTEXT, CERT_CHAIN_PARA, CERT_CONTEXT, CERT_INFO, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
    CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE, CERT_TRUST_CTL_IS_NOT_TIME_VALID, CERT_TRUST_IS_NOT_TIME_NESTED, CERT_TRUST_IS_NOT_TIME_VALID, CERT_TRUST_IS_OFFLINE_REVOCATION,
    CERT_TRUST_REVOCATION_STATUS_UNKNOWN, CMSG_SIGNER_CERT_INFO_PARAM, CertCloseStore, CertFreeCertificateChain, CertFreeCertificateContext, CertGetCertificateChain, CertGetNameStringW,
    CertGetSubjectCertificateFromStore, CryptMsgClose, CryptMsgGetParam, CryptQueryObject, HCERTSTORE,
};
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

/// The name on the signature embedded in `path`, when its certificate chains to a root Windows
/// trusts. Unlike [`signer`] it doesn't hash the file, so it's quick enough for startup: it says
/// who signed a build, not that the file is intact. The certificate's validity period isn't
/// checked, since Artifact Signing certificates last days and only the signature's timestamp
/// keeps them valid.
pub fn embedded_signer(path: &Path) -> Option<String> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let (mut encoding, mut store, mut msg): (u32, HCERTSTORE, *mut core::ffi::c_void) = (0, core::ptr::null_mut(), core::ptr::null_mut());
    // SAFETY: `wide` is a NUL-terminated path that outlives the call; the out-pointers are ours.
    let found = unsafe {
        CryptQueryObject(
            CERT_QUERY_OBJECT_FILE,
            wide.as_ptr().cast(),
            CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED,
            CERT_QUERY_FORMAT_FLAG_BINARY,
            0,
            &mut encoding,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            &mut store,
            &mut msg,
            core::ptr::null_mut(),
        )
    };
    if found == 0 {
        return None;
    }
    // SAFETY: `store` and `msg` are the open signature's, closed right after.
    let name = unsafe { trusted_signer(store, msg, encoding) };
    // SAFETY: closes what CryptQueryObject opened.
    unsafe {
        CryptMsgClose(msg);
        CertCloseStore(store, 0);
    }
    name
}

/// The signer's certificate of the signed message `msg`, from its certificates `store`, named
/// when it chains to a trusted root.
unsafe fn trusted_signer(store: HCERTSTORE, msg: *mut core::ffi::c_void, encoding: u32) -> Option<String> {
    // SAFETY: every pointer comes from the open message and store; each context is freed before
    // returning.
    unsafe {
        let mut size = 0u32;
        if CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, core::ptr::null_mut(), &mut size) == 0 {
            return None;
        }
        // u64 words keep CERT_INFO aligned.
        let mut info = vec![0u64; (size as usize).div_ceil(8)];
        if CryptMsgGetParam(msg, CMSG_SIGNER_CERT_INFO_PARAM, 0, info.as_mut_ptr().cast(), &mut size) == 0 {
            return None;
        }
        let cert = CertGetSubjectCertificateFromStore(store, encoding, info.as_ptr().cast::<CERT_INFO>());
        if cert.is_null() {
            return None;
        }
        let para = CERT_CHAIN_PARA { cbSize: size_of::<CERT_CHAIN_PARA>() as u32, ..Default::default() };
        let mut chain: *mut CERT_CHAIN_CONTEXT = core::ptr::null_mut();
        let built = CertGetCertificateChain(core::ptr::null_mut(), cert, core::ptr::null(), store, &para, CERT_CHAIN_CACHE_ONLY_URL_RETRIEVAL, core::ptr::null(), &mut chain) != 0;
        let ignored = CERT_TRUST_IS_NOT_TIME_VALID | CERT_TRUST_IS_NOT_TIME_NESTED | CERT_TRUST_CTL_IS_NOT_TIME_VALID | CERT_TRUST_REVOCATION_STATUS_UNKNOWN | CERT_TRUST_IS_OFFLINE_REVOCATION;
        let trusted = built && !chain.is_null() && (*chain).TrustStatus.dwErrorStatus & !ignored == 0;
        if !chain.is_null() {
            CertFreeCertificateChain(chain);
        }
        let name = trusted.then(|| cert_name(cert)).flatten();
        CertFreeCertificateContext(cert);
        name
    }
}

/// The certificate's subject name as [`signer`] and [`embedded_signer`] report it.
unsafe fn cert_name(cert: *const CERT_CONTEXT) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: `cert` is a live certificate context; `buf` is ours.
    let n = unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, core::ptr::null(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n - 1]))
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
        cert_name(cert)
    }
}
