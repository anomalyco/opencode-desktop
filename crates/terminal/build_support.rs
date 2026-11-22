// Shared deterministic cache, file-lock, override, SHA256, child-timeout, and resilient HTTPS download helpers for `build.rs`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

pub const GHOSTTY_COMMIT: &str = "83fb95624b0439ee660c2679b43b38c8b63769cd";
pub const GHOSTTY_VERSION: &str = "1.4.0-main+83fb956";
pub const GHOSTTY_TARBALL_URL: &str =
    "https://tip.files.ghostty.org/83fb95624b0439ee660c2679b43b38c8b63769cd/libghostty-vt-source.tar.gz";
pub const GHOSTTY_TARBALL_SHA256: &str =
    "ed57ae9b95581c88f2614e976c1d3f146c02d443a3c26139200eb9de6e645eb4";
pub const GHOSTTY_TARBALL_MAX_BYTES: u64 = 32 << 20;

pub const ZIG_VERSION: &str = "0.17.0";
pub const ZIG_ARCHIVE_MAX_BYTES: u64 = 128 << 20;

pub struct ZigDist {
    pub url: &'static str,
    pub sha256: &'static str,
    pub folder: &'static str,
}

pub fn zig_dist_for_host(host: &str) -> Result<ZigDist, String> {
    match host {
        "aarch64-apple-darwin" => Ok(ZigDist {
            url: "https://ziglang.org/download/0.17.0/zig-aarch64-macos-0.17.0.tar.xz",
            sha256: "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a",
            folder: "zig-aarch64-macos-0.17.0",
        }),
        "x86_64-apple-darwin" => Ok(ZigDist {
            url: "https://ziglang.org/download/0.17.0/zig-x86_64-macos-0.17.0.tar.xz",
            sha256: "4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6",
            folder: "zig-x86_64-macos-0.17.0",
        }),
        "x86_64-unknown-linux-gnu" | "x86_64-unknown-linux-musl" => Ok(ZigDist {
            url: "https://ziglang.org/download/0.17.0/zig-x86_64-linux-0.17.0.tar.xz",
            sha256: "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026",
            folder: "zig-x86_64-linux-0.17.0",
        }),
        "aarch64-unknown-linux-gnu" | "aarch64-unknown-linux-musl" => Ok(ZigDist {
            url: "https://ziglang.org/download/0.17.0/zig-aarch64-linux-0.17.0.tar.xz",
            sha256: "9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8",
            folder: "zig-aarch64-linux-0.17.0",
        }),
        "x86_64-pc-windows-msvc" | "x86_64-pc-windows-gnu" | "x86_64-pc-windows-gnullvm" => {
            Ok(ZigDist {
                url: "https://ziglang.org/download/0.17.0/zig-x86_64-windows-0.17.0.zip",
                sha256: "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428",
                folder: "zig-x86_64-windows-0.17.0",
            })
        }
        "aarch64-pc-windows-msvc" | "aarch64-pc-windows-gnullvm" => Ok(ZigDist {
            url: "https://ziglang.org/download/0.17.0/zig-aarch64-windows-0.17.0.zip",
            sha256: "0a59d91fa1cb40cf068e9b0954434ce973500c7a2ea749f1e01af62cdab52d26",
            folder: "zig-aarch64-windows-0.17.0",
        }),
        other => Err(format!(
            "terminal build: no pinned Zig {ZIG_VERSION} archive for host `{other}`; set ZIG or GHOSTTY_VT_LIB"
        )),
    }
}

pub struct FetchPolicy {
    pub max_attempts: usize,
    pub total_budget: Duration,
    pub connect_timeout: Duration,
    pub recv_response_timeout: Duration,
    pub per_read_timeout: Duration,
    pub per_attempt_timeout: Duration,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    pub max_bytes: u64,
}

impl FetchPolicy {
    pub fn for_archive(max_bytes: u64) -> Self {
        Self {
            max_attempts: 5,
            total_budget: Duration::from_secs(180),
            connect_timeout: Duration::from_secs(10),
            recv_response_timeout: Duration::from_secs(20),
            per_read_timeout: Duration::from_secs(20),
            per_attempt_timeout: Duration::from_secs(45),
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(8),
            max_bytes,
        }
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| {
        format!(
            "terminal build: failed to open `{}` for SHA256 ({e})",
            path.display()
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            format!(
                "terminal build: failed reading `{}` for SHA256 ({e})",
                path.display()
            )
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        write!(&mut hex, "{b:02x}").ok();
    }
    Ok(hex)
}

/// Verifies the whole file at `path` against `expected_hex`, returning a single-line error on mismatch.
pub fn verify_file_sha256(
    label: &str,
    url: &str,
    expected_hex: &str,
    path: &Path,
) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "terminal build: SHA256 mismatch for {label} ({url}): expected {expected_hex}, got {actual}"
        ))
    }
}

fn is_retryable_status(code: u16) -> bool {
    matches!(code, 408 | 429 | 500..=599)
}

fn parse_content_range_start(header: &str) -> Option<u64> {
    let rest = header.trim().strip_prefix("bytes ")?;
    let (start, _) = rest.split_once('-')?;
    start.trim().parse::<u64>().ok()
}

enum AttemptOutcome {
    Done,
    Retry(String),
    Fail(String),
}

fn run_single_attempt(
    label: &str,
    url: &str,
    part_path: &Path,
    expected_sha256: &str,
    policy: &FetchPolicy,
    remaining: Duration,
) -> AttemptOutcome {
    let mut file = match OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(part_path)
    {
        Ok(f) => f,
        Err(e) => return AttemptOutcome::Fail(format!("open part file failed ({e})")),
    };
    let mut have = file.metadata().map_or(0, |m| m.len());
    if have > policy.max_bytes {
        file.set_len(0).ok();
        have = 0;
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(policy.connect_timeout.min(remaining)))
        .timeout_recv_response(Some(policy.recv_response_timeout.min(remaining)))
        .timeout_per_read(Some(policy.per_read_timeout.min(remaining)))
        .timeout_global(Some(policy.per_attempt_timeout.min(remaining)))
        .http_status_as_error(true)
        .build()
        .into();

    let mut req = agent.get(url);
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }

    let mut resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::RequireHttpsOnly(uri)) => {
            return AttemptOutcome::Fail(format!("refused non-HTTPS URL or redirect ({uri})"));
        }
        Err(ureq::Error::StatusCode(416)) if have > 0 => {
            file.set_len(0).ok();
            return AttemptOutcome::Retry(
                "HTTP 416 on Range resume; restarting from byte 0".to_owned(),
            );
        }
        Err(ureq::Error::StatusCode(code)) if !is_retryable_status(code) => {
            return AttemptOutcome::Fail(format!("HTTP {code}"));
        }
        Err(e) => return AttemptOutcome::Retry(e.to_string()),
    };

    let status = resp.status().as_u16();
    let resumed_from = if have > 0 && status == 206 {
        let range_ok = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range_start)
            == Some(have);
        if !range_ok {
            file.set_len(0).ok();
            return AttemptOutcome::Retry(
                "unexpected Content-Range on HTTP 206; restarting from byte 0".to_owned(),
            );
        }
        if let Err(e) = file.seek(SeekFrom::Start(have)) {
            return AttemptOutcome::Fail(format!("seek part file failed ({e})"));
        }
        have
    } else {
        if have > 0 {
            if let Err(e) = file.set_len(0) {
                return AttemptOutcome::Fail(format!("truncate part file failed ({e})"));
            }
            if let Err(e) = file.seek(SeekFrom::Start(0)) {
                return AttemptOutcome::Fail(format!("seek part file failed ({e})"));
            }
        }
        0
    };

    let expected_body_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());

    let mut written_this_attempt: u64 = 0;
    let mut total_written = resumed_from;
    let mut reader = resp
        .body_mut()
        .with_config()
        .limit(policy.max_bytes.saturating_add(1))
        .reader();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                written_this_attempt = written_this_attempt.saturating_add(n as u64);
                total_written = total_written.saturating_add(n as u64);
                if total_written > policy.max_bytes {
                    file.set_len(0).ok();
                    return AttemptOutcome::Fail(format!(
                        "body exceeded {} byte limit",
                        policy.max_bytes
                    ));
                }
                if let Err(e) = file.write_all(&buf[..n]) {
                    return AttemptOutcome::Fail(format!("write part file failed ({e})"));
                }
            }
            Err(e) => {
                file.flush().ok();
                return AttemptOutcome::Retry(format!(
                    "read body failed after {total_written} bytes ({e})"
                ));
            }
        }
    }
    if let Err(e) = file.flush() {
        return AttemptOutcome::Fail(format!("flush part file failed ({e})"));
    }
    if let Some(expected_len) = expected_body_len
        && written_this_attempt < expected_len
    {
        return AttemptOutcome::Retry(format!(
            "truncated response: received {written_this_attempt} of {expected_len} bytes (total {total_written})"
        ));
    }
    drop(file);

    if let Err(err) = verify_file_sha256(label, url, expected_sha256, part_path) {
        fs::remove_file(part_path).ok();
        if resumed_from > 0 {
            return AttemptOutcome::Retry(format!(
                "{err} after Range resume; restarting from byte 0"
            ));
        }
        return AttemptOutcome::Fail(err);
    }
    AttemptOutcome::Done
}

/// Downloads `url` over HTTPS-only with per-stage timeouts, retries with exponential backoff,
/// Range 206 resume on truncated/dropped transfers, whole-file SHA-256 verification before
/// atomic rename into `dest_path`, and a strict total wall-clock budget.
pub fn fetch_verified_archive(
    label: &str,
    url: &str,
    expected_sha256: &str,
    dest_path: &Path,
    policy: &FetchPolicy,
) -> Result<(), String> {
    let started = Instant::now();
    let deadline = started + policy.total_budget;
    let part_name = format!(
        "{}.part",
        dest_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("archive")
    );
    let part_path = dest_path.with_file_name(part_name);
    fs::remove_file(&part_path).ok();

    let mut failures: Vec<String> = Vec::new();
    let mut budget_exhausted = false;
    let max_attempts = policy.max_attempts.max(1);

    for attempt in 1..=max_attempts {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            budget_exhausted = true;
            break;
        }
        match run_single_attempt(
            label,
            url,
            &part_path,
            expected_sha256,
            policy,
            remaining,
        ) {
            AttemptOutcome::Done => {
                if dest_path.exists() {
                    fs::remove_file(dest_path).ok();
                }
                fs::rename(&part_path, dest_path).map_err(|e| {
                    fs::remove_file(&part_path).ok();
                    format!(
                        "terminal build: failed to move verified {label} into `{}` ({e})",
                        dest_path.display()
                    )
                })?;
                return Ok(());
            }
            AttemptOutcome::Fail(reason) => {
                failures.push(format!("- {url} (attempt {attempt}/{max_attempts}): {reason}"));
                break;
            }
            AttemptOutcome::Retry(reason) => {
                failures.push(format!("- {url} (attempt {attempt}/{max_attempts}): {reason}"));
                if attempt < max_attempts {
                    let backoff = policy
                        .base_backoff
                        .saturating_mul(1u32 << (attempt - 1).min(4))
                        .min(policy.max_backoff);
                    let rem = deadline.saturating_duration_since(Instant::now());
                    if rem.is_zero() {
                        budget_exhausted = true;
                        break;
                    }
                    std::thread::sleep(backoff.min(rem));
                }
            }
        }
    }

    fs::remove_file(&part_path).ok();
    let elapsed = started.elapsed().as_secs_f64();
    let budget_s = policy.total_budget.as_secs();
    let budget_note = if budget_exhausted {
        "; total fetch budget exhausted"
    } else {
        ""
    };
    Err(format!(
        "terminal build: failed to download {label} (expected sha256: {expected_sha256}; elapsed {elapsed:.1}s, budget <= {budget_s}s{budget_note}) after {} attempt(s):\n{}",
        failures.len(),
        failures.join("\n")
    ))
}

/// Runs `cmd` while draining stdout/stderr on helper threads so pipes never stall, killing the
/// child process if it exceeds `timeout`.
pub fn run_command_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    label: &str,
) -> Result<Output, String> {
    let started = Instant::now();
    let deadline = started + timeout;
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("terminal build: failed to spawn {label} ({e})"))?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stdout_pipe {
            pipe.read_to_end(&mut buf).ok();
        }
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            pipe.read_to_end(&mut buf).ok();
        }
        buf
    });

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    child.kill().ok();
                    child.wait().ok();
                    return Err(format!(
                        "terminal build: {label} timed out after {:.1}s (limit {}s)",
                        started.elapsed().as_secs_f64(),
                        timeout.as_secs()
                    ));
                }
                let rem = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(Duration::from_millis(25).min(rem));
            }
            Err(e) => {
                child.kill().ok();
                child.wait().ok();
                return Err(format!("terminal build: failed waiting on {label} ({e})"));
            }
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Validates an explicit `GHOSTTY_VT_LIB=/path/libghostty-vt.a` offline override.
pub fn resolve_lib_override(raw: &Path) -> Result<PathBuf, String> {
    let meta = fs::metadata(raw).map_err(|e| {
        format!(
            "terminal build: GHOSTTY_VT_LIB does not exist at `{}` ({e})",
            raw.display()
        )
    })?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(format!(
            "terminal build: GHOSTTY_VT_LIB at `{}` is not a non-empty archive file",
            raw.display()
        ));
    }
    if raw.parent().is_none_or(|p| p.as_os_str().is_empty()) {
        fs::canonicalize(raw).map_err(|e| {
            format!(
                "terminal build: failed to resolve GHOSTTY_VT_LIB `{}` ({e})",
                raw.display()
            )
        })
    } else {
        Ok(raw.to_path_buf())
    }
}

pub fn archive_present(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Exclusive cross-process/cross-thread lock on `<cache_root>/.build.lock` (`flock` on Unix,
/// `LockFileEx` on Windows). Both lock per open file, so two threads or processes opening the same
/// lock file serialize cleanly, and closing the file releases the lock.
pub struct CacheLock {
    file: File,
}

impl CacheLock {
    pub fn acquire(lock_path: &Path) -> Result<Self, String> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "terminal build: failed to create cache directory `{}` ({e})",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| {
                format!(
                    "terminal build: failed to open lock file `{}` ({e})",
                    lock_path.display()
                )
            })?;
        file.lock().map_err(|e| {
            format!(
                "terminal build: failed to lock `{}` ({e})",
                lock_path.display()
            )
        })?;
        Ok(Self { file })
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        self.file.unlock().ok();
    }
}

/// Returns `archive_path` immediately on a warm cache hit without locking or downloading;
/// on a miss, acquires `<cache_root>/.build.lock`, re-checks `archive_path`, and calls `produce`
/// at most once to write `archive_path`.
pub fn ensure_cached_archive(
    cache_root: &Path,
    archive_path: &Path,
    produce: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<PathBuf, String> {
    if archive_present(archive_path) {
        return Ok(archive_path.to_path_buf());
    }
    let _guard = CacheLock::acquire(&cache_root.join(".build.lock"))?;
    if archive_present(archive_path) {
        return Ok(archive_path.to_path_buf());
    }
    produce(archive_path)?;
    if !archive_present(archive_path) {
        return Err(format!(
            "terminal build: producer completed without writing `{}`",
            archive_path.display()
        ));
    }
    Ok(archive_path.to_path_buf())
}
