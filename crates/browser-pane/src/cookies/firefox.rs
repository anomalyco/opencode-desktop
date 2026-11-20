//! Firefox-family (Firefox, Zen, LibreWolf, Floorp) `profiles.ini` and `cookies.sqlite` reader
//! (`profile/firefox.ts`). Reads only the default container (`originAttributes = ''`).

use std::path::{Path, PathBuf};

use rusqlite::params_from_iter;

use super::database::{SnapshotScratch, snapshot_database};
use super::{CookieHost, CookieRead, ImportFailure};
use crate::protocol::{CookieSameSite, WireCookie};

#[derive(Clone, Debug)]
pub struct FirefoxProfileEntry {
    pub id: String,
    pub name: String,
    pub directory: PathBuf,
}

/// Parses a Firefox-family `profiles.ini` into resolved profile entries.
pub fn firefox_profiles(root: &Path, ini: &str) -> Vec<FirefoxProfileEntry> {
    let mut out = Vec::new();
    let mut in_profile = false;
    let mut name: Option<String> = None;
    let mut path_val: Option<String> = None;
    let mut is_relative = true;

    let flush = |out: &mut Vec<FirefoxProfileEntry>,
                 name: &mut Option<String>,
                 path_val: &mut Option<String>,
                 is_relative: &mut bool| {
        if let Some(val) = path_val.take()
            && !val.is_empty()
        {
            let directory = if *is_relative {
                let rel = Path::new(&val);
                if rel.is_absolute()
                    || rel
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    *name = None;
                    *is_relative = true;
                    return;
                }
                root.join(rel)
            } else {
                PathBuf::from(&val)
            };
            let display = name
                .take()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| val.clone());
            out.push(FirefoxProfileEntry {
                id: val,
                name: display,
                directory,
            });
        } else {
            *name = None;
        }
        *is_relative = true;
    };

    for raw in ini.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('[')
            && let Some(header) = rest.strip_suffix(']')
        {
            if in_profile {
                flush(&mut out, &mut name, &mut path_val, &mut is_relative);
            }
            in_profile = header.starts_with("Profile");
            continue;
        }
        if !in_profile {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "Name" => name = Some(v.trim().to_owned()),
                "Path" => path_val = Some(v.trim().to_owned()),
                "IsRelative" => is_relative = v.trim() != "0",
                _ => {}
            }
        }
    }
    if in_profile {
        flush(&mut out, &mut name, &mut path_val, &mut is_relative);
    }
    out
}

/// Firefox 139+ stores milliseconds; earlier versions store seconds (`firefox.ts:66`).
fn firefox_expiry(raw: i64) -> Option<f64> {
    if raw <= 0 {
        return None;
    }
    let val = raw as f64;
    Some(if val > 1e11 { val / 1000.0 } else { val })
}

/// Reads plain-text cookie hosts & timestamps from Firefox's `cookies.sqlite` (no values read).
pub fn read_firefox_hosts(
    profile_dir: &Path,
    scratch: &SnapshotScratch,
) -> Result<Vec<CookieHost>, ImportFailure> {
    let db = profile_dir.join("cookies.sqlite");
    snapshot_database(&db, scratch, |conn| {
        let mut stmt = conn.prepare(
            "SELECT host, expiry, lastAccessed FROM moz_cookies \
             WHERE originAttributes = '' AND typeof(host) = 'text'",
        )?;
        let rows = stmt.query_map([], |row| {
            let host: String = row.get(0)?;
            let expiry: i64 = row.get(1)?;
            let last_accessed: i64 = row.get(2)?;
            Ok(CookieHost {
                host,
                expires: firefox_expiry(expiry),
                used: (last_accessed as f64) / 1_000_000.0,
            })
        })?;
        rows.collect()
    })
}

/// Reads cookies from Firefox's `cookies.sqlite`, optionally restricted to hosts accepted by `keep`.
pub fn read_firefox_cookies<F>(
    profile_dir: &Path,
    scratch: &SnapshotScratch,
    mut keep: Option<F>,
) -> Result<CookieRead, ImportFailure>
where
    F: FnMut(&str) -> bool,
{
    let db = profile_dir.join("cookies.sqlite");
    let columns = "SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite \
                   FROM moz_cookies WHERE originAttributes = ''";
    let cookies = snapshot_database(&db, scratch, |conn| {
        let map_row = |row: &rusqlite::Row<'_>| -> Result<WireCookie, rusqlite::Error> {
            let host: String = row.get(0)?;
            let name: String = row.get(1)?;
            let value: String = row.get(2)?;
            let path: String = row.get(3)?;
            let expiry: i64 = row.get(4)?;
            let secure = row.get::<_, i64>(5)? != 0;
            let http_only = row.get::<_, i64>(6)? != 0;
            let ss: i64 = row.get(7)?;
            let same_site = match ss {
                1 => CookieSameSite::Lax,
                2 => CookieSameSite::Strict,
                _ if secure => CookieSameSite::NoRestriction,
                _ => CookieSameSite::Unspecified,
            };
            Ok(WireCookie {
                host,
                name,
                value,
                path: if path.is_empty() { "/".to_owned() } else { path },
                secure,
                http_only,
                expires: firefox_expiry(expiry),
                same_site,
            })
        };

        let Some(pred) = keep.as_mut() else {
            let mut stmt = conn.prepare(columns)?;
            let rows = stmt.query_map([], map_row)?;
            return rows.collect::<Result<Vec<_>, _>>();
        };

        // Read values only for chosen hosts, chunked at 500 parameters per SQLite statement.
        let mut distinct = conn.prepare(
            "SELECT DISTINCT host FROM moz_cookies WHERE originAttributes = '' AND typeof(host) = 'text'",
        )?;
        let hosts: Vec<String> = distinct
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|h| pred(h))
            .collect();

        let mut all = Vec::new();
        for chunk in hosts.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let sql = format!("{columns} AND host IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), map_row)?;
            for item in rows {
                all.push(item?);
            }
        }
        Ok(all)
    })?;

    Ok(CookieRead {
        cookies,
        protected: 0,
        failed: 0,
        skipped: 0,
    })
}
