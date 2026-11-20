//! Chromium-family cookie reader and OS-key decryption (`profile/chromium.ts`).
//!
//! - Windows: `v10`/`v11` AES-256-GCM via `ring::aead` (key unwrapped from `Local State` via DPAPI);
//!   `v20` app-bound encryption reported cleanly as `Protected`.
//! - macOS & Linux: `v10`/`v11` AES-128-CBC via `aes128` + `ring::pbkdf2::PBKDF2_HMAC_SHA1`.
//! - Chromium DB v24+: strips the 32-byte `SHA256(host)` prefix before UTF-8 decoding.
//! - Skips CHIPS partitioned cookies (`top_frame_site_key` non-empty) and counts damaged rows.

use std::num::NonZeroU32;
use std::path::Path;

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::digest::{SHA256, digest};
use ring::pbkdf2;

use super::aes128::decrypt_cbc_pkcs7;
use super::database::{SnapshotScratch, snapshot_database};
use super::format::chromium_time;
use super::{CookieHost, CookieRead, ImportFailure};
use crate::protocol::{CookieSameSite, WireCookie};

#[derive(Clone)]
pub enum ChromiumKey {
    Gcm([u8; 32]),
    Cbc { v10: [u8; 16], v11: Option<[u8; 16]> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decrypted {
    Value(String),
    Protected,
    Failed,
}

const WELL_FORMED: &str = "\
typeof(host_key) = 'text' AND \
typeof(name) = 'text' AND \
typeof(value) = 'text' AND \
typeof(encrypted_value) = 'blob' AND \
typeof(path) = 'text' AND \
typeof(expires_utc) = 'integer' AND \
typeof(has_expires) = 'integer' AND \
typeof(is_secure) = 'integer' AND \
typeof(is_httponly) = 'integer' AND \
typeof(samesite) = 'integer' AND \
typeof(top_frame_site_key) IN ('text', 'null')";

/// Derives Chromium's AES-128-CBC key from a Safe Storage password (1003 rounds on macOS, 1 on Linux).
pub fn derive_cbc_key(password: &[u8], iterations: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    let Some(iters) = NonZeroU32::new(iterations.max(1)) else {
        return out;
    };
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA1, iters, b"saltysalt", password, &mut out);
    out
}

/// Decrypts one Chromium `encrypted_value` blob, stripping the DB v24+ `SHA256(host)` prefix.
pub fn decrypt_chromium(encrypted: &[u8], key: &ChromiumKey, host: &str) -> Decrypted {
    if encrypted.len() < 3 {
        return Decrypted::Failed;
    }
    let prefix = &encrypted[..3];
    if prefix == b"v20" {
        return Decrypted::Protected;
    }
    if prefix != b"v10" && prefix != b"v11" {
        return Decrypted::Failed;
    }
    let bytes = match key {
        ChromiumKey::Gcm(raw_key) => decrypt_gcm(raw_key, encrypted),
        ChromiumKey::Cbc { v10, v11 } => {
            let secret = if prefix == b"v11" {
                match v11 {
                    Some(k) => k,
                    None => return Decrypted::Failed,
                }
            } else {
                v10
            };
            decrypt_cbc_pkcs7(secret, &[0x20u8; 16], &encrypted[3..])
        }
    };
    let Some(plain) = bytes else {
        return Decrypted::Failed;
    };
    let host_hash = digest(&SHA256, host.as_bytes());
    let text_bytes = if plain.len() >= 32 && plain[..32] == *host_hash.as_ref() {
        &plain[32..]
    } else {
        &plain[..]
    };
    match String::from_utf8(text_bytes.to_vec()) {
        Ok(value) => Decrypted::Value(value),
        Err(_) => Decrypted::Failed,
    }
}

fn decrypt_gcm(raw_key: &[u8; 32], encrypted: &[u8]) -> Option<Vec<u8>> {
    // 3-byte version ("v10"/"v11") + 12-byte nonce + ciphertext + 16-byte tag.
    if encrypted.len() < 3 + 12 + 16 {
        return None;
    }
    let unbound = UnboundKey::new(&AES_256_GCM, raw_key).ok()?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::try_assume_unique_for_key(&encrypted[3..15]).ok()?;
    let mut in_out = encrypted[15..].to_vec();
    let plain = key.open_in_place(nonce, Aad::empty(), &mut in_out).ok()?;
    Some(plain.to_vec())
}

/// Reads only the plain-text host & timestamp columns for the Choose-sites list (zero decryption,
/// never asks for DPAPI/Keychain).
pub fn read_chromium_hosts(
    database: &Path,
    scratch: &SnapshotScratch,
) -> Result<Vec<CookieHost>, ImportFailure> {
    snapshot_database(database, scratch, |conn| {
        let mut stmt = conn.prepare(
            "SELECT host_key, expires_utc, has_expires, last_access_utc \
             FROM cookies \
             WHERE (top_frame_site_key IS NULL OR top_frame_site_key = '') \
               AND typeof(host_key) = 'text'",
        )?;
        let rows = stmt.query_map([], |row| {
            let host: String = row.get(0)?;
            let expires_utc: i64 = row.get(1)?;
            let has_expires: i64 = row.get(2)?;
            let last_access_utc: i64 = row.get(3)?;
            Ok(CookieHost {
                host,
                expires: if has_expires != 0 {
                    chromium_time(expires_utc)
                } else {
                    None
                },
                used: chromium_time(last_access_utc).unwrap_or(0.0),
            })
        })?;
        rows.collect()
    })
}

struct ChromiumRow {
    host_key: String,
    name: String,
    value: String,
    encrypted_value: Vec<u8>,
    path: String,
    expires_utc: i64,
    has_expires: i64,
    is_secure: i64,
    is_httponly: i64,
    samesite: i64,
    top_frame_site_key: Option<String>,
}

/// Reads and decrypts a Chromium profile's cookie database from a private snapshot.
/// When `keep` is `Some`, only matching hosts are decrypted and counted.
pub fn read_chromium_cookies<F>(
    database: &Path,
    key: &ChromiumKey,
    scratch: &SnapshotScratch,
    mut keep: Option<F>,
) -> Result<CookieRead, ImportFailure>
where
    F: FnMut(&str) -> bool,
{
    let (rows, damaged) = snapshot_database(database, scratch, |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT host_key, name, value, encrypted_value, path, expires_utc, \
                    has_expires, is_secure, is_httponly, samesite, top_frame_site_key \
             FROM cookies WHERE {WELL_FORMED}"
        ))?;
        let good = stmt
            .query_map([], |row| {
                Ok(ChromiumRow {
                    host_key: row.get(0)?,
                    name: row.get(1)?,
                    value: row.get(2)?,
                    encrypted_value: row.get(3)?,
                    path: row.get(4)?,
                    expires_utc: row.get(5)?,
                    has_expires: row.get(6)?,
                    is_secure: row.get(7)?,
                    is_httponly: row.get(8)?,
                    samesite: row.get(9)?,
                    top_frame_site_key: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut bad_stmt = conn.prepare(&format!(
            "SELECT CASE WHEN typeof(host_key) = 'text' THEN host_key END FROM cookies WHERE NOT ({WELL_FORMED})"
        ))?;
        let bad = bad_stmt
            .query_map([], |row| row.get::<_, Option<String>>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok((good, bad))
    })?;

    let mut failed = 0u32;
    for host in damaged {
        let count_it = match (&mut keep, host.as_deref()) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(pred), Some(h)) => pred(h),
        };
        if count_it {
            failed = failed.saturating_add(1);
        }
    }

    let mut out = CookieRead {
        cookies: Vec::new(),
        protected: 0,
        failed,
        skipped: 0,
    };

    for row in rows {
        if let Some(pred) = keep.as_mut()
            && !pred(&row.host_key)
        {
            continue;
        }
        if row
            .top_frame_site_key
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        {
            out.skipped = out.skipped.saturating_add(1);
            continue;
        }
        let val = if !row.value.is_empty() || row.encrypted_value.is_empty() {
            Decrypted::Value(row.value)
        } else {
            decrypt_chromium(&row.encrypted_value, key, &row.host_key)
        };
        let value = match val {
            Decrypted::Value(v) => v,
            Decrypted::Protected => {
                out.protected = out.protected.saturating_add(1);
                continue;
            }
            Decrypted::Failed => {
                out.failed = out.failed.saturating_add(1);
                continue;
            }
        };
        let same_site = match row.samesite {
            0 => CookieSameSite::NoRestriction,
            1 => CookieSameSite::Lax,
            2 => CookieSameSite::Strict,
            _ => CookieSameSite::Unspecified,
        };
        out.cookies.push(WireCookie {
            host: row.host_key,
            name: row.name,
            value,
            path: if row.path.is_empty() {
                "/".to_owned()
            } else {
                row.path
            },
            secure: row.is_secure != 0,
            http_only: row.is_httponly != 0,
            expires: if row.has_expires != 0 {
                chromium_time(row.expires_utc)
            } else {
                None
            },
            same_site,
        });
    }

    Ok(out)
}
