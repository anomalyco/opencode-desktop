//! Browser cookie profile management, cross-browser/file cookie import, Choose-sites domain
//! grouping, and startup disk-persistence verification (`UP-095` `#63002` Phase 3b).
//!
//! - Pure parsers (`format`, `sites`) and `BrowserHost` IPC hooks (`host`) compile everywhere
//!   (including `wasm32` and `--no-default-features`).
//! - OS database snapshots (`database`), Chromium/Firefox/Safari readers, and OS key-store
//!   decryption (`sources`) compile strictly under `#[cfg(feature = "engine")]`.

#[cfg(feature = "engine")]
pub mod aes128;
#[cfg(feature = "engine")]
pub mod chromium;
#[cfg(feature = "engine")]
pub mod database;
#[cfg(feature = "engine")]
pub mod firefox;
pub mod format;
pub mod host;
#[cfg(feature = "engine")]
pub mod icons;
#[cfg(feature = "engine")]
pub mod safari;
pub mod sites;
#[cfg(feature = "engine")]
pub mod sources;

use serde::{Deserialize, Serialize};

use crate::protocol::WireCookie;

/// Short-lived probe cookie written at startup to verify that real OS cookie encryption can write
/// to disk (`profile/cookie.ts`, `profile/probe.ts`). Never shown to pages or extensions.
pub const PROBE_COOKIE_HOST: &str = "cookie-check.opencode.invalid";
pub const PROBE_COOKIE_URL: &str = "https://cookie-check.opencode.invalid/";
pub const PROBE_COOKIE_NAME: &str = "opencode_cookie_check";

/// Maximum cookie export file size accepted by `import_cookie_file` (`profile/index.ts:496`).
pub const MAX_COOKIE_FILE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEngineKind {
    Chromium,
    Firefox,
    Safari,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserProfileInfo {
    pub id: String,
    pub name: String,
}

/// An external browser installed on this machine with at least one profile holding cookies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserSource {
    pub id: String,
    pub name: String,
    pub engine: BrowserEngineKind,
    pub profiles: Vec<BrowserProfileInfo>,
}

/// A cookie host entry read without decrypting or inspecting cookie names/values (Choose sites).
#[derive(Clone, Debug, PartialEq)]
pub struct CookieHost {
    pub host: String,
    pub expires: Option<f64>,
    pub used: f64,
}

/// One row of the Choose-sites dialog: registrable domain + count of importable cookies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSite {
    pub site: String,
    pub cookies: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteListOrder {
    Recent,
    Name,
}

/// Why reading an external browser or cookie file failed (`profile/ipc.ts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportFailure {
    Key,
    Access,
    Locked,
    Read,
    Format,
    Storage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SiteList {
    Listed {
        sites: Vec<ProfileSite>,
        order: SiteListOrder,
    },
    Failed {
        reason: ImportFailure,
    },
}

/// What reading an external browser or cookie file produced before writing to the engine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CookieRead {
    pub cookies: Vec<WireCookie>,
    /// Values protected by source-browser-only app-bound encryption (`v20` on Windows Chrome/Edge).
    pub protected: u32,
    /// Rows that failed decryption, UTF-8 validation, or parsing.
    pub failed: u32,
    /// Rows skipped on purpose (e.g. CHIPS partitioned cookies).
    pub skipped: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImportResult {
    Imported {
        imported: u32,
        skipped: u32,
        protected: u32,
        failed: u32,
    },
    Failed {
        reason: ImportFailure,
    },
}

/// Events emitted by the engine in response to `BrowserHost` profile/cookie requests.
#[derive(Clone, Debug, PartialEq)]
pub enum ProfileEvent {
    CookiesListed {
        request_id: u64,
        cookies: Vec<WireCookie>,
    },
    CookiesImported {
        request_id: u64,
        imported: u32,
        failed: u32,
    },
    ProbeReady {
        request_id: u64,
        written: bool,
        cache_dir: String,
    },
    ActionDone {
        request_id: u64,
        ok: bool,
    },
}

/// Reads and parses an exported cookie file (Netscape `cookies.txt`, Chrome JSON array, or
/// Playwright storage state), rejecting symlinks/non-regular files and capping reads at
/// `MAX_COOKIE_FILE_BYTES` (32 MiB) directly on the open file handle (no metadata->read TOCTOU).
#[cfg(feature = "engine")]
pub fn read_cookie_file(path: &std::path::Path) -> Result<CookieRead, ImportFailure> {
    let bytes = database::read_regular_bounded(path, MAX_COOKIE_FILE_BYTES)?;
    let text = String::from_utf8(bytes).map_err(|_| ImportFailure::Format)?;
    format::parse_cookie_file(&text).ok_or(ImportFailure::Format)
}
