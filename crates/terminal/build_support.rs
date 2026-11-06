// Shared deterministic cache, file-lock, override, and SHA256 helpers for `build.rs`
// (also included by `abi_tests.rs` so every build invariant is unit-tested).

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};

pub const GHOSTTY_COMMIT: &str = "83fb95624b0439ee660c2679b43b38c8b63769cd";
pub const GHOSTTY_VERSION: &str = "1.4.0-main+83fb956";
pub const GHOSTTY_TARBALL_URL: &str =
    "https://tip.files.ghostty.org/83fb95624b0439ee660c2679b43b38c8b63769cd/libghostty-vt-source.tar.gz";
pub const GHOSTTY_TARBALL_SHA256: &str =
    "ed57ae9b95581c88f2614e976c1d3f146c02d443a3c26139200eb9de6e645eb4";

pub const ZIG_VERSION: &str = "0.17.0";

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

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    hex
}

/// Verifies `bytes` against `expected_hex`, returning a single-line error message on mismatch.
pub fn verify_sha256(label: &str, url: &str, expected_hex: &str, bytes: &[u8]) -> Result<(), String> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "terminal build: SHA256 mismatch for {label} ({url}): expected {expected_hex}, got {actual}"
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
        let _ = self.file.unlock();
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
