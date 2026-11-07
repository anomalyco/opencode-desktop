//! Fetches the engine archive into a partial file that survives an interruption: the next fetch
//! asks for the rest with an HTTP `Range` and hashes the bytes already there, so the SHA-256
//! covers the whole archive however many tries it took.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::github_download::{self, ERR_RANGE, GithubAuth};
use crate::installer::{ENGINE_DEFAULT_URL, ENGINE_GITHUB_FALLBACK_URL, ERR_DOWNLOAD, ERR_INSTALL};

/// How fast the next bytes may come, asked before each read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pace {
    Full,
    /// At most this many bytes a second.
    Limit(u64),
    /// Stop now and keep the partial file for later.
    Stop,
}

pub(crate) enum Fetched {
    /// The whole archive is in `partial`, with this SHA-256 (lowercase hex).
    Done { sha256: String },
    /// `pace` said stop; `partial` holds what came so far.
    Stopped,
}

/// Fetches `url` into `partial`, resuming after the bytes already there. `progress` hears
/// `(downloaded, total)` as bytes arrive (every 256 KB, and at the start and end). A failed read
/// keeps the partial file, so the next fetch resumes; the caller deletes it once it is used.
pub(crate) fn fetch(
    url: &str,
    partial: &Path,
    github: &GithubAuth,
    mut progress: impl FnMut(u64, Option<u64>),
    pace: &mut dyn FnMut() -> Pace,
) -> Result<Fetched, String> {
    let mut have = std::fs::metadata(partial).map_or(0, |m| m.len());
    let mut resp = match open(url, github, have) {
        // The partial file is already whole, or the server lost track of it: start over.
        Err(err) if err == ERR_RANGE => {
            have = 0;
            open(url, github, 0)?
        }
        other => other?,
    };
    let resumed = have > 0 && resp.status().as_u16() == 206;
    if !resumed {
        have = 0;
    }
    // `bytes <start>-<end>/<total>`: the rest must start where the partial file ends.
    let range = header(&resp, "content-range");
    let start = range.as_deref().and_then(|r| r.strip_prefix("bytes ")?.split('-').next()?.parse::<u64>().ok());
    if resumed && start != Some(have) {
        let _ = std::fs::remove_file(partial);
        return Err(ERR_DOWNLOAD.into());
    }
    let total_bytes = if resumed {
        range.and_then(|r| r.rsplit('/').next()?.parse::<u64>().ok())
    } else {
        header(&resp, "content-length").and_then(|v| v.parse::<u64>().ok())
    };

    let mut hasher = Sha256::new();
    let mut file = open_partial(partial, resumed, &mut hasher).map_err(|_| ERR_INSTALL.to_owned())?;
    progress(have, total_bytes);

    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut downloaded = have;
    let mut last_notify = have;
    let mut window = (Instant::now(), 0u64, Pace::Full);
    loop {
        let limit = match pace() {
            Pace::Stop => {
                file.flush().map_err(|_| ERR_INSTALL.to_owned())?;
                return Ok(Fetched::Stopped);
            }
            p => p,
        };
        if limit != window.2 {
            window = (Instant::now(), 0, limit);
        }
        if let Pace::Limit(rate) = limit {
            // Hold the window's average at `rate`: sleep until its bytes are due.
            let due = Duration::from_secs_f64(window.1 as f64 / rate.max(1) as f64);
            if let Some(wait) = due.checked_sub(window.0.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        let chunk = match limit {
            Pace::Limit(rate) => buf.len().min((rate / 8).max(4096) as usize),
            _ => buf.len(),
        };
        let n = reader.read(&mut buf[..chunk]).map_err(|_| ERR_DOWNLOAD.to_owned())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|_| ERR_INSTALL.to_owned())?;
        hasher.update(&buf[..n]);
        downloaded += n as u64;
        window.1 += n as u64;
        if downloaded - last_notify >= 256 * 1024 {
            last_notify = downloaded;
            progress(downloaded, total_bytes);
        }
    }
    file.flush().map_err(|_| ERR_INSTALL.to_owned())?;
    if total_bytes.is_some_and(|t| t != downloaded) {
        return Err(ERR_DOWNLOAD.into());
    }
    progress(downloaded, total_bytes.or(Some(downloaded)));
    Ok(Fetched::Done { sha256: format!("{:x}", hasher.finalize()) })
}

/// Opens the archive, falling back to the GitHub release mirror when the release CDN fails.
fn open(url: &str, github: &GithubAuth, from: u64) -> Result<ureq::http::Response<ureq::Body>, String> {
    github_download::open(url, github, from).or_else(|primary_err| {
        match ENGINE_GITHUB_FALLBACK_URL.filter(|_| Some(url) == ENGINE_DEFAULT_URL && primary_err != ERR_RANGE) {
            Some(fallback) => github_download::open(fallback, github, from).map_err(|_| primary_err),
            None => Err(primary_err),
        }
    })
}

fn header(resp: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    resp.headers().get(name)?.to_str().ok().map(str::to_owned)
}

/// Opens `partial` to append to after hashing what it holds, or emptied when not `resuming`. It
/// stays locked while open, so two apps sharing the engine folder never write it at once.
fn open_partial(partial: &Path, resuming: bool, hasher: &mut Sha256) -> std::io::Result<File> {
    if let Some(dir) = partial.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(partial)?;
    file.try_lock().map_err(std::io::Error::other)?;
    if resuming {
        hash_into(&mut file, hasher)?;
    } else {
        file.set_len(0)?;
    }
    file.seek(SeekFrom::End(0))?;
    Ok(file)
}

/// The SHA-256 of a whole file (lowercase hex).
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    hash_into(&mut File::open(path)?, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_into(file: &mut File, hasher: &mut Sha256) -> std::io::Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        hasher.update(&buf[..n]);
    }
}
