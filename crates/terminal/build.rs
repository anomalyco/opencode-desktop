//! Builds or locates `libghostty-vt.a` (`v1.4.0`, commit `83fb95624b`).
//!
//! Lookup order:
//! 1. `GHOSTTY_VT_LIB=/path/to/libghostty-vt.a`: offline override, zero Zig or network use.
//! 2. Warm machine cache (`$GHOSTTY_VT_CACHE_DIR`, else `~/.cache/seer/ghostty-vt/<key>/libghostty-vt.a`,
//!    or `%LOCALAPPDATA%\seer\ghostty-vt\<key>\ghostty-vt-static.lib` on Windows):
//!    links immediately with zero downloads and zero Zig invocations.
//! 3. Cold miss under exclusive `<cache>/.build.lock`: resolves Zig `0.17.x` (`ZIG`, `PATH`, or
//!    SHA256-verified official Zig `0.17.0` archive), fetches SHA256-verified Ghostty `v1.4.0`
//!    source tarball (or `GHOSTTY_SOURCE_DIR`), builds static `ReleaseFast` archive, and publishes
//!    it atomically into the cache.

use std::time::{SystemTime, UNIX_EPOCH};

include!("build_support.rs");

const ZIG_VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const AROCC_FETCH_TIMEOUT: Duration = Duration::from_secs(600);
const TAR_EXTRACT_TIMEOUT: Duration = Duration::from_secs(120);
const ZIG_BUILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Ends the build script with the whole message: a failure names its cause (zig's stderr), not one line of it.
fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn cargo_env(name: &str) -> String {
    std::env::var_os(name)
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| fail(&format!("terminal build: {name} is not set or not valid UTF-8")))
}

fn cargo_out_dir() -> PathBuf {
    std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("terminal build: OUT_DIR is not set"))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    for var in [
        "GHOSTTY_VT_LIB",
        "GHOSTTY_VT_CACHE_DIR",
        "GHOSTTY_SOURCE_DIR",
        "GHOSTTY_ZIG_SYSTEM_DIR",
        "ZIG",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let target = cargo_env("TARGET");
    let host = cargo_env("HOST");

    if let Some(raw) = std::env::var_os("GHOSTTY_VT_LIB").filter(|s| !s.is_empty()) {
        let lib = resolve_lib_override(Path::new(&raw)).unwrap_or_else(|e| fail(&e));
        emit_link(&lib, &target);
        return;
    }

    let cache_root = default_cache_root();
    let archive_name = static_lib_filename(&target);
    if let Some(src_dir) = std::env::var_os("GHOSTTY_SOURCE_DIR").filter(|s| !s.is_empty()) {
        let out_dir = cargo_out_dir();
        let dest = out_dir.join("override-build").join(archive_name);
        let _lock = CacheLock::acquire(&cache_root.join(".build.lock"))
            .unwrap_or_else(|e| fail(&e));
        build_from_source(Path::new(&src_dir), &cache_root, &host, &target, &dest)
            .unwrap_or_else(|e| fail(&e));
        emit_link(&dest, &target);
        return;
    }

    let key = format!("ghostty-{}-{target}-releasefast", &GHOSTTY_COMMIT[..12]);
    let cached_archive = cache_root.join(&key).join(archive_name);

    let archive = ensure_cached_archive(&cache_root, &cached_archive, |dest| {
        build_cold(&cache_root, &host, &target, dest)
    })
    .unwrap_or_else(|e| fail(&e));

    emit_link(&archive, &target);
}

fn default_cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("GHOSTTY_VT_CACHE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(dir);
    }
    // Keep the legacy `seer/ghostty-vt` cache directory name for CI cache compatibility.
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty())
    {
        return PathBuf::from(local).join("seer").join("ghostty-vt");
    }
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("seer").join("ghostty-vt");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home).join(".cache").join("seer").join("ghostty-vt");
    }
    cargo_out_dir().join("ghostty-vt-cache")
}

fn static_lib_filename(target: &str) -> &'static str {
    if target.contains("windows") {
        "ghostty-vt-static.lib"
    } else {
        "libghostty-vt.a"
    }
}

fn emit_link(archive: &Path, target: &str) {
    if target.contains("windows") {
        let dir = archive
            .parent()
            .unwrap_or_else(|| fail("terminal build: archive path has no parent directory"));
        let name = archive
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| fail("terminal build: invalid archive file name"));
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static:+verbatim={name}");
        return;
    }

    let name = archive.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let link_dir = if let Some(stem) = name.strip_prefix("lib").and_then(|s| s.strip_suffix(".a")) {
        let dir = archive
            .parent()
            .unwrap_or_else(|| fail("terminal build: archive path has no parent directory"));
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static={stem}");
        dir.to_path_buf()
    } else {
        // Custom filename via GHOSTTY_VT_LIB: stage as `$OUT_DIR/libghostty-vt.a` for Apple/Unix `ld`.
        let out_dir = cargo_out_dir();
        let staged = out_dir.join("libghostty-vt.a");
        fs::copy(archive, &staged)
            .unwrap_or_else(|e| fail(&format!("terminal build: copy override failed ({e})")));
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=ghostty-vt");
        out_dir
    };
    let _ = link_dir;
    if target.contains("apple") {
        println!("cargo:rustc-link-lib=c++");
    }
}

fn build_cold(cache_root: &Path, host: &str, target: &str, dest: &Path) -> Result<(), String> {
    let stamp = unique_suffix();
    let work_dir = cache_root.join(format!(".work-{stamp}"));
    let result = (|| {
        fs::create_dir_all(&work_dir).map_err(|e| {
            format!(
                "terminal build: failed to create scratch dir `{}` ({e})",
                work_dir.display()
            )
        })?;
        let tarball = work_dir.join("libghostty-vt-source.tar.gz");
        let policy = FetchPolicy::for_archive(GHOSTTY_TARBALL_MAX_BYTES);
        fetch_verified_archive(
            "libghostty-vt-source.tar.gz",
            &[GHOSTTY_TARBALL_URL.to_owned()],
            GHOSTTY_TARBALL_SHA256,
            &tarball,
            &policy,
        )?;
        let src_dir = work_dir.join("src");
        fs::create_dir_all(&src_dir).map_err(|e| format!("terminal build: mkdir src failed ({e})"))?;
        extract_tar(&tarball, &src_dir, true)?;
        build_from_source(&src_dir, cache_root, host, target, dest)
    })();
    fs::remove_dir_all(&work_dir).ok();
    result
}

fn build_from_source(
    source_dir: &Path,
    cache_root: &Path,
    host: &str,
    target: &str,
    dest: &Path,
) -> Result<(), String> {
    if !source_dir.join("build.zig").is_file() {
        return Err(format!(
            "terminal build: source directory `{}` does not contain build.zig",
            source_dir.display()
        ));
    }
    let zig = resolve_or_bootstrap_zig(cache_root, host)?;
    let stamp = unique_suffix();
    let scratch = cache_root.join(format!(".zig-build-{stamp}"));
    let result = (|| {
        let build_src = scratch.join("tree");
        copy_source_tree(source_dir, &build_src)?;
        let prefix = scratch.join("out");
        let local_cache = scratch.join("zig-cache");
        let global_cache = cache_root.join("zig-pkg-cache");
        fs::create_dir_all(&global_cache)
            .map_err(|e| format!("terminal build: mkdir zig-pkg-cache failed ({e})"))?;
        if std::env::var_os("GHOSTTY_ZIG_SYSTEM_DIR").filter(|s| !s.is_empty()).is_none() {
            prefetch_arocc(&zig, &global_cache);
        }

        let mut cmd = Command::new(&zig);
        cmd.arg("build")
            .arg("-Demit-lib-vt=true")
            .arg("-Doptimize=ReleaseFast")
            // Its SIMD paths are C++ (highway), which a page's module has no runtime for.
            .arg(format!("-Dsimd={}", !target.starts_with("wasm32")))
            .arg(format!("-Dtarget={}", zig_target(target)?))
            .arg("-Dcpu=baseline")
            .arg(format!("-Dversion-string={GHOSTTY_VERSION}"))
            .arg("-Dapp-runtime=none")
            .arg("-Demit-xcframework=false")
            .arg("-Demit-themes=false")
            .arg("--prefix")
            .arg(&prefix)
            .arg("--cache-dir")
            .arg(&local_cache)
            .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
            .current_dir(&build_src);

        if let Some(sys_dir) = std::env::var_os("GHOSTTY_ZIG_SYSTEM_DIR").filter(|s| !s.is_empty()) {
            cmd.arg("--system").arg(sys_dir);
        }

        let output = run_command_with_timeout(
            &mut cmd,
            ZIG_BUILD_TIMEOUT,
            "`zig build -Demit-lib-vt=true`",
        )?;
        if !output.status.success() {
            // zig's last lines, not its last one: on 18 Nov the last one was the caret under the error.
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lines: Vec<&str> = stderr.lines().collect();
            let tail = lines[lines.len().saturating_sub(60)..].join("\n");
            return Err(format!(
                "terminal build: `zig build -Demit-lib-vt=true` failed ({}); zig's stderr, last {} of {} lines:\n{tail}",
                output.status,
                lines.len().min(60),
                lines.len()
            ));
        }

        let built = prefix.join("lib").join(static_lib_filename(target));
        if !archive_present(&built) {
            return Err(format!(
                "terminal build: zig build succeeded without producing `{}`",
                built.display()
            ));
        }
        publish_atomic(&built, dest)
    })();
    fs::remove_dir_all(&scratch).ok();
    result
}

// A second source, not a replacement, for the one package Ghostty's build fetches from codeberg.org: `arocc`, named
// by `translate_c`'s build.zig.zon. On 18 Nov codeberg.org left DNS (registrar hold) and every cold build failed.
// `zig fetch` stores the GitHub copy at the same commit under the hash zig computes from its contents; `zig build`
// takes it only if that is the pinned hash, so a changed copy is never used and zig tries Codeberg as before.
// Remove once Ghostty's build no longer names Codeberg, or once every package comes through GHOSTTY_ZIG_SYSTEM_DIR.
const AROCC_GITHUB: &str = "git+https://github.com/Vexu/arocc#d0c8c4d9c55daa7ef6e40cf0f630a5b5e900989b";
const AROCC_HASH: &str = "aro-0.0.0-JSD1QtuBNwCASyBtNF3pqTl_W3oAJQGEVyFAtrBSE_Pa";

fn prefetch_arocc(zig: &Path, global_cache: &Path) {
    let packages = global_cache.join("p");
    if packages.join(AROCC_HASH).exists() || packages.join(format!("{AROCC_HASH}.tar.gz")).exists() {
        return;
    }
    let mut cmd = Command::new(zig);
    cmd.arg("fetch")
        .arg(AROCC_GITHUB)
        .env("ZIG_GLOBAL_CACHE_DIR", global_cache)
        .current_dir(global_cache);
    let note = match run_command_with_timeout(&mut cmd, AROCC_FETCH_TIMEOUT, "`zig fetch`") {
        Err(e) => e,
        Ok(out) if !out.status.success() => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            format!("`zig fetch` failed ({}): {last}", out.status)
        }
        Ok(out) => {
            let hash = String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").trim().to_string();
            if hash == AROCC_HASH {
                return;
            }
            format!("its hash is `{hash}`, not the pinned `{AROCC_HASH}`")
        }
    };
    println!("cargo:warning=terminal build: arocc from GitHub ({AROCC_GITHUB}): {note}; zig build fetches it from codeberg.org as before");
}

fn publish_atomic(built: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest
        .parent()
        .ok_or_else(|| "terminal build: destination has no parent".to_owned())?;
    fs::create_dir_all(parent).map_err(|e| {
        format!(
            "terminal build: failed to create `{}` ({e})",
            parent.display()
        )
    })?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("lib"),
        unique_suffix()
    ));
    fs::copy(built, &tmp).map_err(|e| {
        format!(
            "terminal build: failed to copy archive to `{}` ({e})",
            tmp.display()
        )
    })?;
    if cfg!(windows) && dest.exists() {
        fs::remove_file(dest).ok();
    }
    fs::rename(&tmp, dest).map_err(|e| {
        fs::remove_file(&tmp).ok();
        format!(
            "terminal build: failed to move archive into `{}` ({e})",
            dest.display()
        )
    })
}

fn resolve_or_bootstrap_zig(cache_root: &Path, host: &str) -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("ZIG").filter(|s| !s.is_empty()) {
        let path = PathBuf::from(raw);
        let ver = zig_version_of(&path)?;
        if !ver.starts_with("0.17.") {
            return Err(format!(
                "terminal build: ZIG at `{}` is version `{ver}`, expected 0.17.x",
                path.display()
            ));
        }
        return Ok(path);
    }

    let path_zig = PathBuf::from("zig");
    if let Ok(ver) = zig_version_of(&path_zig)
        && ver.starts_with("0.17.")
    {
        return Ok(path_zig);
    }

    let dist = zig_dist_for_host(host)?;
    let install_dir = cache_root.join(dist.folder);
    let zig_exe = format!("zig{}", std::env::consts::EXE_SUFFIX);
    let zig_bin = install_dir.join(&zig_exe);
    if zig_bin.is_file()
        && let Ok(ver) = zig_version_of(&zig_bin)
        && ver.starts_with("0.17.")
    {
        return Ok(zig_bin);
    }

    let stamp = unique_suffix();
    let stage = cache_root.join(format!(".zig-stage-{stamp}"));
    let res = (|| {
        fs::create_dir_all(&stage)
            .map_err(|e| format!("terminal build: mkdir zig stage failed ({e})"))?;
        // `.tar.xz` on macOS and Linux, `.zip` on Windows; `tar` (bsdtar on Windows) reads both.
        let archive = stage.join(dist.url.rsplit('/').next().unwrap_or("zig-archive"));
        let policy = FetchPolicy::for_zig(ZIG_ARCHIVE_MAX_BYTES);
        fetch_verified_archive("zig-0.17.0", &zig_sources(&dist), dist.sha256, &archive, &policy)?;
        let unpacked = stage.join("unpacked");
        fs::create_dir_all(&unpacked)
            .map_err(|e| format!("terminal build: mkdir zig unpacked failed ({e})"))?;
        extract_tar(&archive, &unpacked, true)?;
        let staged_zig = unpacked.join(&zig_exe);
        let ver = zig_version_of(&staged_zig)?;
        if !ver.starts_with("0.17.") {
            return Err(format!(
                "terminal build: downloaded Zig reported unexpected version `{ver}`"
            ));
        }
        // A rename can't replace a directory on Windows, so a stale install has to go first.
        if install_dir.exists() {
            fs::remove_dir_all(&install_dir).map_err(|e| {
                format!(
                    "terminal build: failed to remove the old Zig in `{}` ({e})",
                    install_dir.display()
                )
            })?;
        }
        fs::rename(&unpacked, &install_dir).map_err(|e| {
            format!(
                "terminal build: failed to install Zig into `{}` ({e})",
                install_dir.display()
            )
        })
    })();
    fs::remove_dir_all(&stage).ok();
    res?;
    Ok(zig_bin)
}

fn zig_version_of(bin: &Path) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("version");
    let label = format!("`{} version`", bin.display());
    let out = run_command_with_timeout(&mut cmd, ZIG_VERSION_TIMEOUT, &label)?;
    if !out.status.success() {
        return Err(format!(
            "terminal build: `{} version` exited with {}",
            bin.display(),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn extract_tar(archive: &Path, dest: &Path, strip_first: bool) -> Result<(), String> {
    let mut cmd = Command::new(tar_program());
    cmd.arg("-xf").arg(archive).arg("-C").arg(dest);
    if strip_first {
        cmd.arg("--strip-components=1");
    }
    let label = format!("`tar` on `{}`", archive.display());
    let out = run_command_with_timeout(&mut cmd, TAR_EXTRACT_TIMEOUT, &label)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let line = stderr.lines().next().unwrap_or("tar extraction failed").trim();
        return Err(format!(
            "terminal build: tar extraction failed for `{}`: {line}",
            archive.display()
        ));
    }
    Ok(())
}

/// Windows ships bsdtar as `System32\tar.exe`, which reads `.zip` and takes drive-letter paths.
/// A GNU tar earlier on `PATH` (Git for Windows, MSYS2) reads neither, so prefer the system one.
fn tar_program() -> PathBuf {
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot").filter(|s| !s.is_empty())
    {
        let tar = PathBuf::from(root).join("System32").join("tar.exe");
        if tar.is_file() {
            return tar;
        }
    }
    PathBuf::from("tar")
}

fn copy_source_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| {
        format!(
            "terminal build: failed to create `{}` ({e})",
            to.display()
        )
    })?;
    let entries = fs::read_dir(from).map_err(|e| {
        format!(
            "terminal build: failed to read `{}` ({e})",
            from.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("terminal build: dir entry error ({e})"))?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(".git" | ".zig-cache" | "zig-cache" | "zig-out" | "zig-pkg")
        ) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        let ft = entry
            .file_type()
            .map_err(|e| format!("terminal build: stat `{}` failed ({e})", src.display()))?;
        if ft.is_dir() {
            copy_source_tree(&src, &dst)?;
        } else if ft.is_file() {
            fs::copy(&src, &dst).map_err(|e| {
                format!(
                    "terminal build: copy `{}` -> `{}` failed ({e})",
                    src.display(),
                    dst.display()
                )
            })?;
        }
    }
    Ok(())
}

fn zig_target(target: &str) -> Result<&'static str, String> {
    match target {
        "aarch64-apple-darwin" => Ok("aarch64-macos"),
        "x86_64-apple-darwin" => Ok("x86_64-macos"),
        "x86_64-unknown-linux-gnu" => Ok("x86_64-linux-gnu"),
        "x86_64-unknown-linux-musl" => Ok("x86_64-linux-musl"),
        "aarch64-unknown-linux-gnu" => Ok("aarch64-linux-gnu"),
        "aarch64-unknown-linux-musl" => Ok("aarch64-linux-musl"),
        "x86_64-pc-windows-msvc" => Ok("x86_64-windows-msvc"),
        "aarch64-pc-windows-msvc" => Ok("aarch64-windows-msvc"),
        // The browser build: the engine as wasm objects, linked into the page's module.
        "wasm32-unknown-unknown" => Ok("wasm32-freestanding"),
        "aarch64-apple-ios" => Ok("aarch64-ios"),
        "aarch64-apple-ios-sim" => Ok("aarch64-ios-simulator"),
        other => Err(format!(
            "terminal build: unsupported target `{other}` for libghostty-vt; set GHOSTTY_VT_LIB"
        )),
    }
}

// Build script has no config::now() or injected clock; wall time disambiguates scratch paths across runs.
#[allow(clippy::disallowed_methods)]
fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}
