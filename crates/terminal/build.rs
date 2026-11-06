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

use std::io::Read;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

include!("build_support.rs");

fn fail_one_line(msg: &str) -> ! {
    let single = msg.lines().next().unwrap_or(msg).trim();
    eprintln!("{single}");
    std::process::exit(1);
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

    let target = std::env::var("TARGET").expect("TARGET");
    let host = std::env::var("HOST").expect("HOST");

    if let Some(raw) = std::env::var_os("GHOSTTY_VT_LIB").filter(|s| !s.is_empty()) {
        let lib = resolve_lib_override(Path::new(&raw)).unwrap_or_else(|e| fail_one_line(&e));
        emit_link(&lib, &target);
        return;
    }

    let cache_root = default_cache_root();
    let archive_name = static_lib_filename(&target);
    if let Some(src_dir) = std::env::var_os("GHOSTTY_SOURCE_DIR").filter(|s| !s.is_empty()) {
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let dest = out_dir.join("override-build").join(archive_name);
        let _lock = CacheLock::acquire(&cache_root.join(".build.lock"))
            .unwrap_or_else(|e| fail_one_line(&e));
        build_from_source(Path::new(&src_dir), &cache_root, &host, &target, &dest)
            .unwrap_or_else(|e| fail_one_line(&e));
        emit_link(&dest, &target);
        return;
    }

    let key = format!("ghostty-{}-{target}-releasefast", &GHOSTTY_COMMIT[..12]);
    let cached_archive = cache_root.join(&key).join(archive_name);

    let archive = ensure_cached_archive(&cache_root, &cached_archive, |dest| {
        build_cold(&cache_root, &host, &target, dest)
    })
    .unwrap_or_else(|e| fail_one_line(&e));

    emit_link(&archive, &target);
}

fn default_cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("GHOSTTY_VT_CACHE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(dir);
    }
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
    PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("ghostty-vt-cache")
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
            .unwrap_or_else(|| fail_one_line("terminal build: archive path has no parent directory"));
        let name = archive
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_else(|| fail_one_line("terminal build: invalid archive file name"));
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static:+verbatim={name}");
        return;
    }

    let name = archive.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let link_dir = if let Some(stem) = name.strip_prefix("lib").and_then(|s| s.strip_suffix(".a")) {
        let dir = archive
            .parent()
            .unwrap_or_else(|| fail_one_line("terminal build: archive path has no parent directory"));
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static={stem}");
        dir.to_path_buf()
    } else {
        // Custom filename via GHOSTTY_VT_LIB: stage as `$OUT_DIR/libghostty-vt.a` for Apple/Unix `ld`.
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let staged = out_dir.join("libghostty-vt.a");
        fs::copy(archive, &staged)
            .unwrap_or_else(|e| fail_one_line(&format!("terminal build: copy override failed ({e})")));
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
        let bytes = download_bytes(GHOSTTY_TARBALL_URL)?;
        verify_sha256("libghostty-vt-source.tar.gz", GHOSTTY_TARBALL_URL, GHOSTTY_TARBALL_SHA256, &bytes)?;
        fs::write(&tarball, &bytes).map_err(|e| {
            format!(
                "terminal build: failed to write `{}` ({e})",
                tarball.display()
            )
        })?;
        let src_dir = work_dir.join("src");
        fs::create_dir_all(&src_dir).map_err(|e| format!("terminal build: mkdir src failed ({e})"))?;
        extract_tar(&tarball, &src_dir, true)?;
        build_from_source(&src_dir, cache_root, host, target, dest)
    })();
    let _ = fs::remove_dir_all(&work_dir);
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

        let mut cmd = Command::new(&zig);
        cmd.arg("build")
            .arg("-Demit-lib-vt=true")
            .arg("-Doptimize=ReleaseFast")
            .arg("-Dsimd=true")
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

        let output = cmd.output().map_err(|e| {
            format!(
                "terminal build: failed to spawn `{}` ({e})",
                zig.display()
            )
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("unknown zig build error");
            return Err(format!(
                "terminal build: `zig build -Demit-lib-vt=true` failed ({}) — {detail}",
                output.status
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
    let _ = fs::remove_dir_all(&scratch);
    result
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
    fs::rename(&tmp, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
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
        let bytes = download_bytes(dist.url)?;
        verify_sha256("zig-0.17.0", dist.url, dist.sha256, &bytes)?;
        // `.tar.xz` on macOS and Linux, `.zip` on Windows; `tar` (bsdtar on Windows) reads both.
        let archive = stage.join(dist.url.rsplit('/').next().unwrap_or("zig-archive"));
        fs::write(&archive, &bytes)
            .map_err(|e| format!("terminal build: write zig archive failed ({e})"))?;
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
        if install_dir.exists() {
            let _ = fs::remove_dir_all(&install_dir);
        }
        fs::rename(&unpacked, &install_dir).map_err(|e| {
            format!(
                "terminal build: failed to install Zig into `{}` ({e})",
                install_dir.display()
            )
        })
    })();
    let _ = fs::remove_dir_all(&stage);
    res?;
    Ok(zig_bin)
}

fn zig_version_of(bin: &Path) -> Result<String, String> {
    let out = Command::new(bin)
        .arg("version")
        .output()
        .map_err(|e| format!("terminal build: failed to run `{} version` ({e})", bin.display()))?;
    if !out.status.success() {
        return Err(format!(
            "terminal build: `{} version` exited with {}",
            bin.display(),
            out.status
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn download_bytes(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .http_status_as_error(true)
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("terminal build: failed to download {url} ({e})"))?;
    let mut bytes = Vec::new();
    resp.body_mut()
        .with_config()
        .limit(128 << 20)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("terminal build: failed reading {url} ({e})"))?;
    Ok(bytes)
}

fn extract_tar(archive: &Path, dest: &Path, strip_first: bool) -> Result<(), String> {
    let mut cmd = Command::new(tar_program());
    cmd.arg("-xf").arg(archive).arg("-C").arg(dest);
    if strip_first {
        cmd.arg("--strip-components=1");
    }
    let out = cmd.output().map_err(|e| {
        format!(
            "terminal build: failed to invoke `tar` on `{}` ({e})",
            archive.display()
        )
    })?;
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
        "aarch64-apple-ios" => Ok("aarch64-ios"),
        "aarch64-apple-ios-sim" => Ok("aarch64-ios-simulator"),
        other => Err(format!(
            "terminal build: unsupported target `{other}` for libghostty-vt; set GHOSTTY_VT_LIB"
        )),
    }
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{nanos}", std::process::id())
}
