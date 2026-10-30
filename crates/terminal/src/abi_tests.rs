use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ffi::*;

#[allow(dead_code)]
mod build_support {
    include!("../build_support.rs");
}

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "seer-terminal-abi-test-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn rust_repr_c_structs_match_linked_libghostty_vt_type_manifest() {
    let raw = unsafe { ghostty_type_json() };
    assert!(!raw.is_null(), "ghostty_type_json must return a non-null manifest");
    let json_str = unsafe { CStr::from_ptr(raw) }.to_str().expect("valid UTF-8 manifest");
    let manifest: serde_json::Value = serde_json::from_str(json_str).expect("valid JSON manifest");
    assert_eq!(manifest["schema"].as_u64(), Some(1));
    let types = &manifest["types"];

    let check_struct = |name: &str, rust_size: usize, rust_align: usize| {
        let entry = &types[name];
        assert!(
            entry.is_object(),
            "type `{name}` missing from ghostty_type_json"
        );
        assert_eq!(
            entry["size"].as_u64(),
            Some(rust_size as u64),
            "size mismatch for {name}"
        );
        assert_eq!(
            entry["align"].as_u64(),
            Some(rust_align as u64),
            "align mismatch for {name}"
        );
    };

    check_struct(
        "GhosttyColorRgb",
        std::mem::size_of::<GhosttyColorRgb>(),
        std::mem::align_of::<GhosttyColorRgb>(),
    );
    check_struct(
        "GhosttyStyleColor",
        std::mem::size_of::<GhosttyStyleColor>(),
        std::mem::align_of::<GhosttyStyleColor>(),
    );
    check_struct(
        "GhosttyStyle",
        std::mem::size_of::<GhosttyStyle>(),
        std::mem::align_of::<GhosttyStyle>(),
    );
    check_struct(
        "GhosttyPoint",
        std::mem::size_of::<GhosttyPoint>(),
        std::mem::align_of::<GhosttyPoint>(),
    );
    check_struct(
        "GhosttyGridRef",
        std::mem::size_of::<GhosttyGridRef>(),
        std::mem::align_of::<GhosttyGridRef>(),
    );
    check_struct(
        "GhosttySelection",
        std::mem::size_of::<GhosttySelection>(),
        std::mem::align_of::<GhosttySelection>(),
    );
    check_struct(
        "GhosttyRenderStateCursor",
        std::mem::size_of::<GhosttyRenderStateCursor>(),
        std::mem::align_of::<GhosttyRenderStateCursor>(),
    );
    check_struct(
        "GhosttyRenderStateColors",
        std::mem::size_of::<GhosttyRenderStateColors>(),
        std::mem::align_of::<GhosttyRenderStateColors>(),
    );
    check_struct(
        "GhosttyFormatterTerminalOptions",
        std::mem::size_of::<GhosttyFormatterTerminalOptions>(),
        std::mem::align_of::<GhosttyFormatterTerminalOptions>(),
    );
    check_struct(
        "GhosttyTerminalScrollbar",
        std::mem::size_of::<GhosttyTerminalScrollbar>(),
        std::mem::align_of::<GhosttyTerminalScrollbar>(),
    );
    check_struct(
        "GhosttyMouseEncoderSize",
        std::mem::size_of::<GhosttyMouseEncoderSize>(),
        std::mem::align_of::<GhosttyMouseEncoderSize>(),
    );

    let check_enum_val = |enum_name: &str, variant: &str, expected: i64| {
        let got = types[enum_name]["values"][variant]
            .as_i64()
            .unwrap_or_else(|| panic!("missing {enum_name}::{variant}"));
        assert_eq!(got, expected, "enum mismatch for {enum_name}::{variant}");
    };
    check_enum_val("GhosttyTerminalOption", "SCROLLBACK_MAX_LINES", i64::from(GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES));
    check_enum_val("GhosttyTerminalOption", "RENDER_HOLD", i64::from(GHOSTTY_TERMINAL_OPT_RENDER_HOLD));
    check_enum_val("GhosttyTerminalData", "MODE", i64::from(GHOSTTY_TERMINAL_DATA_MODE));
    check_enum_val("GhosttyRenderStateData", "CURSOR", i64::from(GHOSTTY_RENDER_STATE_DATA_CURSOR));
    check_enum_val("GhosttyRenderStateData", "COLORS", i64::from(GHOSTTY_RENDER_STATE_DATA_COLORS));
    check_enum_val("GhosttyRenderStateRowData", "ID", i64::from(GHOSTTY_RENDER_STATE_ROW_DATA_ID));
    check_enum_val("GhosttyKey", "ESCAPE", GhosttyKey::Escape as i64);
    check_enum_val("GhosttyKey", "F12", GhosttyKey::F12 as i64);
}

#[test]
fn build_sha256_verification_fails_with_one_clear_line() {
    let payload = b"ghostty-vt test archive bytes";
    let digest = build_support::sha256_hex(payload);
    assert!(build_support::verify_sha256("test.tar.gz", "https://example.invalid/test.tar.gz", &digest, payload).is_ok());

    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let err = build_support::verify_sha256(
        "test.tar.gz",
        "https://example.invalid/test.tar.gz",
        wrong,
        payload,
    )
    .unwrap_err();
    assert_eq!(err.lines().count(), 1, "mismatch error must be a single clear line: {err}");
    assert!(err.contains("SHA256 mismatch") && err.contains(wrong) && err.contains(&digest));
}

#[test]
fn build_cache_skips_producer_on_hit_and_serializes_concurrent_cold_misses() {
    let dir = scratch_dir("cache");
    let archive = dir.join("entry").join("libghostty-vt.a");
    let runs = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..4 {
        let dir = dir.clone();
        let archive = archive.clone();
        let runs = runs.clone();
        handles.push(std::thread::spawn(move || {
            build_support::ensure_cached_archive(&dir, &archive, |dest| {
                runs.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(40));
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                std::fs::write(dest, b"fake-static-lib").unwrap();
                Ok(())
            })
            .unwrap()
        }));
    }
    for h in handles {
        assert_eq!(h.join().unwrap(), archive);
    }
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "exclusive cache lock must prevent duplicate cold builds across parallel workers"
    );

    // Subsequent warm cache hit never invokes producer.
    let warm = build_support::ensure_cached_archive(&dir, &archive, |_| {
        panic!("warm cache hit must never download or rebuild");
    })
    .unwrap();
    assert_eq!(warm, archive);

    // Offline override validation.
    assert_eq!(build_support::resolve_lib_override(&archive).unwrap(), archive);
    let missing = build_support::resolve_lib_override(&dir.join("missing.a")).unwrap_err();
    assert_eq!(missing.lines().count(), 1);

    let _ = std::fs::remove_dir_all(&dir);
}
