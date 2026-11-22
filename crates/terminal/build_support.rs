// Shared deterministic cache, file-lock, override, SHA256, and resilient download helpers for `build.rs`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

pub const GHOSTTY_COMMIT: &str = "83fb95624b0439ee660c2679b43b38c8b63769cd";
pub const GHOSTTY_VERSION: &str = "1.4.0-main+83fb956";
pub const GHOSTTY_TARBALL_URL: &str =
    "https://tip.files.ghostty.org/83fb95624b0439ee660c2679b43b38c8b63769cd/libghostty-vt-source.tar.gz";
pub const GHOSTTY_TARBALL_SHA256: &str =
    "ed57ae9b95581c88f2614e976c1d3f146c02d443a3c26139200eb9de6e645eb4";
pub const GHOSTTY_TARBALL_MAX_BYTES: u64 = 32 << 20;

pub const AROCC_TARBALL_URL: &str =
    "https://github.com/Vexu/arocc/archive/d0c8c4d9c55daa7ef6e40cf0f630a5b5e900989b.tar.gz";
pub const AROCC_GITHUB: &str =
    "git+https://github.com/Vexu/arocc#d0c8c4d9c55daa7ef6e40cf0f630a5b5e900989b";
pub const AROCC_HASH: &str = "aro-0.0.0-JSD1QtuBNwCASyBtNF3pqTl_W3oAJQGEVyFAtrBSE_Pa";
pub const AROCC_TARBALL_MAX_BYTES: u64 = 16 << 20;

pub const ZIG_VERSION: &str = "0.17.0";
pub const ZIG_PRIMARY_BASE: &str = "https://ziglang.org/download/0.17.0";
pub const ZIG_COMMUNITY_MIRRORS_URL: &str = "https://ziglang.org/download/community-mirrors.txt";
pub const ZIG_ARCHIVE_MAX_BYTES: u64 = 128 << 20;

// Compiled-in snapshot of https://ziglang.org/download/community-mirrors.txt (2026-11-22), used when
// ziglang.org itself cannot serve the live list within the fast mirror-list timeout. Every byte
// downloaded from any mirror is checked against the pinned SHA-256 in `ZIG_DISTS`.
pub const ZIG_COMMUNITY_MIRRORS_SNAPSHOT: &[&str] = &[
    "https://pkg.hexops.org/zig",
    "https://zigmirror.hryx.net/zig",
    "https://zig.linus.dev/zig",
    "https://zig.squirl.dev",
    "https://zig.mirror.mschae23.de/zig",
    "https://ziglang.freetls.fastly.net",
    "https://zig.tilok.dev",
    "https://zig-mirror.tsimnet.eu/zig",
    "https://zig.karearl.com/zig",
    "https://pkg.earth/zig",
    "https://fs.liujiacai.net/zigbuilds",
    "https://zigmirror.com",
    "https://zig.chainsafe.dev",
    "https://zig.savalione.com",
    "https://zig.bcr.ist",
    "https://zig.vortan.dev/zig",
    "https://pkg.alexrp.com/zig",
];

pub struct ZigDist {
    pub hosts: &'static [&'static str],
    pub filename: &'static str,
    pub sha256: &'static str,
    pub folder: &'static str,
}

/// Pinned Zig 0.17.0 archives and SHA-256 digests from https://ziglang.org/download/index.json
/// covering all six release targets and all three PR-build hosts.
pub const ZIG_DISTS: &[ZigDist] = &[
    ZigDist {
        hosts: &["aarch64-apple-darwin"],
        filename: "zig-aarch64-macos-0.17.0.tar.xz",
        sha256: "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a",
        folder: "zig-aarch64-macos-0.17.0",
    },
    ZigDist {
        hosts: &["x86_64-apple-darwin"],
        filename: "zig-x86_64-macos-0.17.0.tar.xz",
        sha256: "4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6",
        folder: "zig-x86_64-macos-0.17.0",
    },
    ZigDist {
        hosts: &["x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl"],
        filename: "zig-x86_64-linux-0.17.0.tar.xz",
        sha256: "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026",
        folder: "zig-x86_64-linux-0.17.0",
    },
    ZigDist {
        hosts: &["aarch64-unknown-linux-gnu", "aarch64-unknown-linux-musl"],
        filename: "zig-aarch64-linux-0.17.0.tar.xz",
        sha256: "9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8",
        folder: "zig-aarch64-linux-0.17.0",
    },
    ZigDist {
        hosts: &[
            "x86_64-pc-windows-msvc",
            "x86_64-pc-windows-gnu",
            "x86_64-pc-windows-gnullvm",
        ],
        filename: "zig-x86_64-windows-0.17.0.zip",
        sha256: "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428",
        folder: "zig-x86_64-windows-0.17.0",
    },
    ZigDist {
        hosts: &["aarch64-pc-windows-msvc", "aarch64-pc-windows-gnullvm"],
        filename: "zig-aarch64-windows-0.17.0.zip",
        sha256: "0a59d91fa1cb40cf068e9b0954434ce973500c7a2ea749f1e01af62cdab52d26",
        folder: "zig-aarch64-windows-0.17.0",
    },
];

pub fn zig_dist_for_host(host: &str) -> Result<&'static ZigDist, String> {
    ZIG_DISTS
        .iter()
        .find(|d| d.hosts.contains(&host))
        .ok_or_else(|| {
            format!(
                "terminal build: no pinned Zig {ZIG_VERSION} archive for host `{host}`; set ZIG, ZIG_ARCHIVE, or GHOSTTY_VT_LIB"
            )
        })
}

pub fn is_debug_profile() -> bool {
    std::env::var_os("PROFILE")
        .and_then(|v| v.into_string().ok())
        .is_some_and(|p| p == "debug")
}

pub fn debug_env_str(name: &str) -> Option<String> {
    if !is_debug_profile() {
        return None;
    }
    std::env::var_os(name).and_then(|v| v.into_string().ok())
}

pub struct FetchSource {
    pub url: String,
    pub max_attempts: usize,
}

pub struct FetchPolicy {
    pub total_budget: Duration,
    pub primary_budget: Option<Duration>,
    pub connect_timeout: Duration,
    pub recv_response_timeout: Duration,
    pub per_read_timeout: Duration,
    pub per_attempt_timeout: Duration,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    pub max_bytes: u64,
}

impl FetchPolicy {
    pub fn for_archive(max_bytes: u64, total_budget: Duration, reserve_for_mirrors: bool) -> Self {
        let mut policy = Self {
            total_budget,
            primary_budget: None,
            connect_timeout: Duration::from_secs(10),
            recv_response_timeout: Duration::from_secs(20),
            per_read_timeout: Duration::from_secs(20),
            per_attempt_timeout: Duration::from_secs(45),
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(8),
            max_bytes,
        };
        if let Some(ms) = debug_env_str("ZIG_TEST_TIMEOUT_MS")
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&ms| ms > 0)
        {
            let t = Duration::from_millis(ms);
            policy.connect_timeout = policy.connect_timeout.min(t);
            policy.recv_response_timeout = policy.recv_response_timeout.min(t);
            policy.per_read_timeout = policy.per_read_timeout.min(t);
            policy.per_attempt_timeout = policy.per_attempt_timeout.min(t.saturating_mul(2));
            policy.base_backoff = Duration::from_millis(50);
            policy.max_backoff = Duration::from_millis(200);
            policy.total_budget = policy.total_budget.min(t.saturating_mul(10));
        }
        if reserve_for_mirrors {
            policy.primary_budget = Some(policy.total_budget / 2);
        }
        policy
    }
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|e| format!("terminal build: failed to open `{}` for SHA256 ({e})", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("terminal build: failed reading `{}` for SHA256 ({e})", path.display()))?;
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

pub fn verify_file_sha256(label: &str, source: &str, expected_hex: &str, path: &Path) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "terminal build: SHA256 mismatch for {label} ({source}): expected {expected_hex}, got {actual}"
        ))
    }
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

pub fn allowed_source_url(url: &str, allow_debug_http: bool) -> bool {
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    if url.starts_with("https://") {
        return true;
    }
    if !allow_debug_http {
        return false;
    }
    url.starts_with("http://127.0.0.1:")
        || url.starts_with("http://[::1]:")
        || url.starts_with("http://localhost:")
        || url.starts_with("http://192.0.2.")
}

fn join_base_url(base: &str, filename: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), filename)
}

// Build script has no config::now() or injected clock; wall time seeds mirror shuffling and scratch suffixes.
#[allow(clippy::disallowed_methods)]
pub fn wall_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn shuffle_strings(items: &mut [String]) {
    let mut state = (wall_nanos() as u64) ^ ((std::process::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    if state == 0 {
        state = 0xD1B5_4A32_D192_ED03;
    }
    let mut i = items.len();
    while i > 1 {
        i -= 1;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        items.swap(i, j);
    }
}

/// Fetches Zig's official community mirror list with a fast timeout (<= 6 s, clamped to
/// `remaining`) so an outage on ziglang.org never consumes the archive download budget; falls back
/// to the compiled-in snapshot.
pub fn zig_mirror_bases(limit: usize, remaining: Duration) -> Vec<String> {
    if let Some(override_list) = debug_env_str("ZIG_TEST_MIRRORS") {
        let mut bases: Vec<String> = override_list
            .split(',')
            .map(str::trim)
            .filter(|s| allowed_source_url(s, true))
            .map(|s| s.trim_end_matches('/').to_owned())
            .collect();
        shuffle_strings(&mut bases);
        bases.truncate(limit);
        return bases;
    }

    let live = fetch_live_mirror_list(remaining);
    let mut bases = if live.is_empty() {
        ZIG_COMMUNITY_MIRRORS_SNAPSHOT
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        live
    };
    shuffle_strings(&mut bases);
    bases.truncate(limit);
    bases
}

fn fetch_live_mirror_list(remaining: Duration) -> Vec<String> {
    if remaining.is_zero() {
        return Vec::new();
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3).min(remaining)))
        .timeout_recv_response(Some(Duration::from_secs(5).min(remaining)))
        .timeout_global(Some(Duration::from_secs(6).min(remaining)))
        .http_status_as_error(true)
        .build()
        .into();
    let Ok(mut resp) = agent.get(ZIG_COMMUNITY_MIRRORS_URL).call() else {
        return Vec::new();
    };
    let mut body = Vec::new();
    if resp
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .reader()
        .read_to_end(&mut body)
        .is_err()
    {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&body);
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_end_matches('/');
        if allowed_source_url(trimmed, false) && !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_owned());
        }
    }
    out
}

pub fn build_zig_sources(dist: &ZigDist) -> Result<Vec<FetchSource>, String> {
    let debug = is_debug_profile();
    let primary_base = match debug_env_str("ZIG_TEST_PRIMARY_URL") {
        Some(raw) => {
            let trimmed = raw.trim().trim_end_matches('/').to_owned();
            if !allowed_source_url(&trimmed, debug) {
                return Err(format!(
                    "terminal build: invalid ZIG_TEST_PRIMARY_URL `{trimmed}`"
                ));
            }
            trimmed
        }
        None => ZIG_PRIMARY_BASE.to_owned(),
    };
    let sources = vec![FetchSource {
        url: join_base_url(&primary_base, dist.filename),
        max_attempts: 4,
    }];
    Ok(sources)
}

pub fn build_zig_mirror_sources(dist: &ZigDist, limit: usize, remaining: Duration) -> Vec<FetchSource> {
    zig_mirror_bases(limit, remaining)
        .into_iter()
        .map(|base| FetchSource {
            url: join_base_url(&base, dist.filename),
            max_attempts: 2,
        })
        .collect()
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
    NextSource(String),
}

fn run_single_attempt(
    url: &str,
    part_path: &Path,
    expected_sha256: Option<&str>,
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
        Err(e) => return AttemptOutcome::NextSource(format!("open part file failed ({e})")),
    };
    let mut have = file.metadata().map_or(0, |m| m.len());
    if have > policy.max_bytes {
        file.set_len(0).ok();
        have = 0;
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
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
        Err(ureq::Error::StatusCode(416)) if have > 0 => {
            file.set_len(0).ok();
            return AttemptOutcome::Retry("HTTP 416 on Range resume; restarting from byte 0".to_owned());
        }
        Err(ureq::Error::StatusCode(code)) if !is_retryable_status(code) => {
            return AttemptOutcome::NextSource(format!("HTTP {code}"));
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
            return AttemptOutcome::Retry("unexpected Content-Range on HTTP 206; restarting from byte 0".to_owned());
        }
        if let Err(e) = file.seek(SeekFrom::Start(have)) {
            return AttemptOutcome::NextSource(format!("seek part file failed ({e})"));
        }
        have
    } else {
        if have > 0 {
            if let Err(e) = file.set_len(0) {
                return AttemptOutcome::NextSource(format!("truncate part file failed ({e})"));
            }
            if let Err(e) = file.seek(SeekFrom::Start(0)) {
                return AttemptOutcome::NextSource(format!("seek part file failed ({e})"));
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
    let mut reader = resp.body_mut().with_config().limit(policy.max_bytes).reader();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                written_this_attempt = written_this_attempt.saturating_add(n as u64);
                total_written = total_written.saturating_add(n as u64);
                if total_written > policy.max_bytes {
                    file.set_len(0).ok();
                    return AttemptOutcome::NextSource(format!(
                        "body exceeded {} byte limit",
                        policy.max_bytes
                    ));
                }
                if let Err(e) = file.write_all(&buf[..n]) {
                    return AttemptOutcome::NextSource(format!("write part file failed ({e})"));
                }
            }
            Err(e) => {
                file.flush().ok();
                return AttemptOutcome::Retry(format!("read body failed after {total_written} bytes ({e})"));
            }
        }
    }
    if let Err(e) = file.flush() {
        return AttemptOutcome::NextSource(format!("flush part file failed ({e})"));
    }
    if let Some(expected_len) = expected_body_len
        && written_this_attempt < expected_len
    {
        return AttemptOutcome::Retry(format!(
            "truncated response: received {written_this_attempt} of {expected_len} bytes (total {total_written})"
        ));
    }
    drop(file);

    if let Some(expected) = expected_sha256
        && let Err(err) = verify_file_sha256("download", url, expected, part_path)
    {
        fs::remove_file(part_path).ok();
        if resumed_from > 0 {
            return AttemptOutcome::Retry(format!("{err} after Range resume; restarting from byte 0"));
        }
        return AttemptOutcome::NextSource(err);
    }
    AttemptOutcome::Done
}

/// Downloads into `dest_path` across `primary` (then lazily-computed `fallbacks` if `primary`
/// fails), with per-stage timeouts, retries with exponential backoff, per-URL Range resume,
/// SHA-256 verification before moving into `dest_path`, and a strict total wall-clock budget.
pub fn fetch_verified_archive(
    label: &str,
    expected_sha256: Option<&str>,
    dest_path: &Path,
    primary: &[FetchSource],
    build_fallbacks: impl FnOnce(Duration) -> Vec<FetchSource>,
    policy: &FetchPolicy,
) -> Result<String, String> {
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
    let mut failures: Vec<String> = Vec::new();
    let mut budget_exhausted = false;

    let try_group = |group: &[FetchSource],
                     group_deadline: Instant,
                     failures: &mut Vec<String>,
                     budget_exhausted: &mut bool|
     -> Option<String> {
        for source in group {
            fs::remove_file(&part_path).ok();
            for attempt in 1..=source.max_attempts {
                let remaining = group_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    if Instant::now() >= deadline {
                        *budget_exhausted = true;
                    }
                    fs::remove_file(&part_path).ok();
                    return None;
                }
                match run_single_attempt(&source.url, &part_path, expected_sha256, policy, remaining) {
                    AttemptOutcome::Done => {
                        if dest_path.exists() {
                            fs::remove_file(dest_path).ok();
                        }
                        if let Err(e) = fs::rename(&part_path, dest_path) {
                            failures.push(format!(
                                "- {} (attempt {}/{}): rename to `{}` failed ({e})",
                                source.url,
                                attempt,
                                source.max_attempts,
                                dest_path.display()
                            ));
                            fs::remove_file(&part_path).ok();
                            return None;
                        }
                        return Some(source.url.clone());
                    }
                    AttemptOutcome::NextSource(reason) => {
                        failures.push(format!(
                            "- {} (attempt {}/{}): {reason}",
                            source.url, attempt, source.max_attempts
                        ));
                        fs::remove_file(&part_path).ok();
                        break;
                    }
                    AttemptOutcome::Retry(reason) => {
                        failures.push(format!(
                            "- {} (attempt {}/{}): {reason}",
                            source.url, attempt, source.max_attempts
                        ));
                        if attempt < source.max_attempts {
                            let backoff = policy
                                .base_backoff
                                .saturating_mul(1u32 << (attempt - 1).min(4))
                                .min(policy.max_backoff);
                            let rem = group_deadline.saturating_duration_since(Instant::now());
                            if rem.is_zero() {
                                if Instant::now() >= deadline {
                                    *budget_exhausted = true;
                                }
                                fs::remove_file(&part_path).ok();
                                return None;
                            }
                            std::thread::sleep(backoff.min(rem));
                        }
                    }
                }
            }
            fs::remove_file(&part_path).ok();
        }
        None
    };

    let primary_deadline = policy
        .primary_budget
        .map_or(deadline, |cap| (started + cap).min(deadline));
    if let Some(url) = try_group(primary, primary_deadline, &mut failures, &mut budget_exhausted) {
        return Ok(url);
    }

    if !budget_exhausted {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let fallbacks = build_fallbacks(remaining);
        if !fallbacks.is_empty()
            && let Some(url) = try_group(&fallbacks, deadline, &mut failures, &mut budget_exhausted)
        {
            println!(
                "cargo:warning=terminal build: primary failed for {label}; fetched from mirror {url} (verified)"
            );
            return Ok(url);
        }
    }

    fs::remove_file(&part_path).ok();
    let elapsed = started.elapsed().as_secs_f64();
    let budget_s = policy.total_budget.as_secs();
    let sha_note = expected_sha256.unwrap_or("verified by zig fetch content hash");
    let budget_note = if budget_exhausted {
        "; total fetch budget exhausted"
    } else {
        ""
    };
    Err(format!(
        "terminal build: failed to download {label} (expected sha256: {sha_note}; elapsed {elapsed:.1}s, budget <= {budget_s}s{budget_note}) after {} attempt(s):\n{}",
        failures.len(),
        failures.join("\n")
    ))
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
