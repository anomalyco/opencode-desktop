//! Retiring imported files: after a verified import they're moved (renamed, never copied or
//! deleted) into one `retired-YYYY-MM-DD` folder in the data folder, relative paths kept. The
//! folders waiting to be purged are a row in the database (`state(rune.storage, retired)`, with
//! `FLAG_RETIRED` in the header saying the row exists), so a normal launch never looks at them;
//! a folder goes after 14 days in which every start was clean, and a start whose import didn't
//! verify starts its 14 days again.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Driver;
use super::machine::VerifiedPaths;
use crate::native::schema::{self, FLAG_RETIRED};
use crate::native::sql;

const KEEP_MS: i64 = 14 * 24 * 60 * 60 * 1000;
const NAME: &str = "rune.storage";
const KEY: &str = "retired";

/// Moves `verified` into today's retired folder; returns how many couldn't be moved (they stay).
pub(super) fn retire(d: &mut Driver<'_>, verified: &VerifiedPaths) -> usize {
    let Some(dir) = d.dir.clone() else { return verified.paths().len() };
    let name = format!("retired-{}", date(d.now_ms));
    let folder = dir.join(&name);
    let mut failed = 0;
    for path in verified.paths() {
        let rel = path.strip_prefix(&dir).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from("Updates").join(path.file_name().unwrap_or_default()));
        let target = free(folder.join(rel));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match std::fs::rename(path, &target) {
            Ok(()) => d.summary.retired += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => failed += 1,
        }
    }
    // `plugins-state\` goes once its last store moved (`remove_dir` keeps a folder with anything in it).
    std::fs::remove_dir(dir.join("plugins-state")).ok();
    if d.summary.retired > 0 {
        let mut folders = folders(d);
        if !folders.iter().any(|f| f["name"] == name.as_str()) {
            folders.push(json!({ "name": name, "since_ms": d.now_ms }));
        }
        record(d, &folders);
    }
    failed
}

/// After the first frame of a run: purges the folders whose 14 clean days are up, or restarts
/// their clock when this start wasn't clean.
pub(super) fn sweep(d: &mut Driver<'_>, clean: bool) {
    let Some(dir) = d.dir.clone() else { return };
    let now = d.now_ms;
    let kept: Vec<Value> = folders(d)
        .into_iter()
        .filter_map(|mut f| {
            let since = f["since_ms"].as_i64().unwrap_or(now);
            if !clean {
                f["since_ms"] = json!(now);
                return Some(f);
            }
            if now - since < KEEP_MS {
                return Some(f);
            }
            let path = dir.join(f["name"].as_str().unwrap_or_default());
            match std::fs::remove_dir_all(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Some(f),
                _ => None,
            }
        })
        .collect();
    record(d, &kept);
}

fn folders(d: &Driver<'_>) -> Vec<Value> {
    let text: Option<String> = d.conn.query_row("SELECT value FROM state WHERE name = ?1 AND key = ?2", [NAME, KEY], |r| r.get(0)).ok();
    text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["folders"].as_array().cloned()).unwrap_or_default()
}

fn record(d: &mut Driver<'_>, folders: &[Value]) {
    let value = (!folders.is_empty()).then(|| json!({ "folders": folders }).to_string());
    let flags = if folders.is_empty() { d.flags & !FLAG_RETIRED } else { d.flags | FLAG_RETIRED };
    let done = (|| -> rusqlite::Result<()> {
        let tx = d.conn.transaction()?;
        sql::put_state(&tx, NAME, KEY, value.as_deref(), d.now_ms)?;
        schema::set_flags(&tx, flags)?;
        tx.commit()
    })();
    match done {
        Ok(()) => d.flags = flags,
        Err(e) => d.summary.reports.push(format!("couldn't record the retired folders: {e}")),
    }
}

/// `path`, or `path` with `-2`, `-3`, … before its extension when that name is taken.
fn free(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| path.with_file_name(format!("{stem}-{n}{ext}"))).find(|p| !p.exists()).unwrap_or(path)
}

/// `YYYY-MM-DD` (UTC) of epoch milliseconds (Hinnant's `civil_from_days`).
fn date(ms: i64) -> String {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
