#[cfg(target_os = "macos")]
use std::io::{Read as _, Write as _};
#[cfg(target_os = "macos")]
use std::net::TcpListener;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

use rune::Tx;
use serde_json::json;
#[cfg(target_os = "macos")]
use sha2::{Digest as _, Sha256};

#[cfg(target_os = "macos")]
use super::mac::{self, BundleInfo, LOCAL_DEV_BUNDLE_ID, TEAM_BUNDLE_ID};
use super::protocol::{self, Decision, TARGET_MACOS_ARM64};
#[cfg(target_os = "macos")]
use super::engine::Facts;
#[cfg(target_os = "macos")]
use super::platform::Platform as _;
use crate::App;

#[test]
fn parses_manifest_and_resolves_relative_target_urls() {
    let sha = "a".repeat(64);
    let body = json!({
        "schema": 1,
        "app": "opencode-rune",
        "artifact": "desktop",
        "channel": "dev",
        "sequence": 7,
        "published_at": "2026-11-03T18:00:00Z",
        "release": {
            "version": "0.0.0-dev-42",
            "notes_url": "https://github.com/anomalyco/rune-desktop/commit/abc",
            "rollout_percent": 100,
            "targets": {
                "macos-arm64": {
                    "url": format!("/release/opencode-rune/files/{sha}/OpenCode-Rune-0.0.0-dev-42-macos-arm64.zip"),
                    "size": 12345,
                    "sha256": sha,
                }
            }
        }
    })
    .to_string();

    let url = "https://anoma.ly/release/opencode-rune/desktop/dev.json";
    let m = protocol::parse_manifest(&body, url, TARGET_MACOS_ARM64).unwrap();
    assert_eq!(m.sequence, 7);
    let target = m.release.target.as_ref().unwrap();
    assert_eq!(
        target.url,
        format!("https://anoma.ly/release/opencode-rune/files/{sha}/OpenCode-Rune-0.0.0-dev-42-macos-arm64.zip")
    );
    assert_eq!(
        protocol::decide(&m, "dev", "0.0.0-dev-42", None, 0).unwrap(),
        Decision::UpToDate
    );
    // Versions are never ordered: a rollback on the channel still installs.
    assert!(matches!(
        protocol::decide(&m, "dev", "0.0.0-dev-99", None, 0).unwrap(),
        Decision::Install { version, .. } if version == "0.0.0-dev-42"
    ));
}

#[test]
fn rollout_and_channel_rules_match_release_protocol() {
    let sha = "b".repeat(64);
    let make = |rollout: u64, required: Option<serde_json::Value>| {
        let mut rel = json!({
            "version": "0.0.0-dev-10",
            "rollout_percent": rollout,
            "targets": {
                "macos-arm64": {
                    "url": format!("/release/opencode-rune/files/{sha}/app.zip"),
                    "size": 99,
                    "sha256": sha,
                }
            }
        });
        if let Some(req) = required {
            rel["required"] = req;
        }
        json!({
            "schema": 1,
            "app": "opencode-rune",
            "artifact": "desktop",
            "channel": "dev",
            "sequence": 1,
            "release": rel,
        })
        .to_string()
    };

    let m = protocol::parse_manifest(&make(25, None), "https://anoma.ly/release/opencode-rune/desktop/dev.json", TARGET_MACOS_ARM64).unwrap();
    assert!(matches!(protocol::decide(&m, "dev", "0.0.0-dev-9", None, 24).unwrap(), Decision::Install { .. }));
    assert_eq!(
        protocol::decide(&m, "dev", "0.0.0-dev-9", None, 25).unwrap(),
        Decision::RolloutDeferred { version: "0.0.0-dev-10".into() }
    );
    assert!(protocol::decide(&m, "stable", "0.0.0-dev-9", None, 0).is_err());

    let halted = protocol::parse_manifest(&make(0, Some(json!(true))), "https://anoma.ly/release/opencode-rune/desktop/dev.json", TARGET_MACOS_ARM64).unwrap();
    assert!(matches!(
        protocol::decide(&halted, "dev", "0.0.0-dev-9", None, 99).unwrap(),
        Decision::Install { .. }
    ));

    assert!(protocol::parse_manifest(&make(100, Some(json!(false))), "https://anoma.ly/release/opencode-rune/desktop/dev.json", TARGET_MACOS_ARM64).is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn kit_local_bundle_is_never_touched_by_team_updater() {
    assert!(!mac::is_updatable_bundle_id(LOCAL_DEV_BUNDLE_ID));
    assert!(mac::is_updatable_bundle_id(TEAM_BUNDLE_ID));
    assert_eq!(
        mac::bundle_root_from_exe(Path::new("/Applications/OpenCode (Rune).app/Contents/MacOS/opencode-desktop")),
        Some(PathBuf::from("/Applications/OpenCode (Rune).app"))
    );
    assert_eq!(mac::bundle_root_from_exe(Path::new("/tmp/target/release/opencode-desktop")), None);
}

#[test]
fn idle_updater_poll_allocates_nothing() {
    let mut app = App::empty();
    let before = procwatch::alloc::thread_allocations();
    assert!(!super::poll(&mut app, &mut Tx::default()));
    let after = procwatch::alloc::thread_allocations();
    assert_eq!(after - before, 0);
}

#[cfg(target_os = "macos")]
fn make_signed_test_bundle(dir: &Path, version: &str) -> PathBuf {
    let app = dir.join("OpenCode (Rune).app");
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let exe = macos.join("opencode-desktop");
    std::fs::copy("/usr/bin/true", &exe).unwrap();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>OpenCode (Rune)</string>
  <key>CFBundleIdentifier</key><string>{TEAM_BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>opencode-desktop</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>1</string>
</dict></plist>
"#
    );
    std::fs::write(app.join("Contents/Info.plist"), plist).unwrap();
    let st = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&app).status().unwrap();
    assert!(st.success());
    app
}

#[cfg(target_os = "macos")]
#[test]
fn downloads_verifies_and_swaps_signed_bundle_end_to_end() {
    let root = std::env::temp_dir().join(format!("oc-rune-upd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let install_dir = root.join("install");
    let build_dir = root.join("build");
    let cache_dir = root.join("cache");
    std::fs::create_dir_all(&install_dir).unwrap();
    std::fs::create_dir_all(&build_dir).unwrap();

    let installed_app = make_signed_test_bundle(&install_dir, "0.0.0-dev-1");
    let next_app = make_signed_test_bundle(&build_dir, "0.0.0-dev-2");
    let zip_path = root.join("OpenCode-Rune-0.0.0-dev-2-macos-arm64.zip");
    let st = Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
        .arg(&next_app)
        .arg(&zip_path)
        .status()
        .unwrap();
    assert!(st.success());
    let zip_bytes = std::fs::read(&zip_path).unwrap();
    let sha: String = Sha256::digest(&zip_bytes).iter().map(|b| format!("{b:02x}")).collect();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let manifest_body = json!({
        "schema": 1,
        "app": "opencode-rune",
        "artifact": "desktop",
        "channel": "dev",
        "sequence": 2,
        "release": {
            "version": "0.0.0-dev-2",
            "rollout_percent": 100,
            "targets": {
                "macos-arm64": {
                    "url": format!("/release/opencode-rune/files/{sha}/OpenCode-Rune-0.0.0-dev-2-macos-arm64.zip"),
                    "size": zip_bytes.len(),
                    "sha256": sha,
                }
            }
        }
    })
    .to_string();

    let server_thread = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut req_buf = [0u8; 2048];
            let n = stream.read(&mut req_buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&req_buf[..n]);
            if req.starts_with("GET /release/opencode-rune/desktop/dev.json ") {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nETag: \"seq-2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    manifest_body.len()
                );
                stream.write_all(hdr.as_bytes()).unwrap();
                stream.write_all(manifest_body.as_bytes()).unwrap();
            } else {
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    zip_bytes.len()
                );
                stream.write_all(hdr.as_bytes()).unwrap();
                stream.write_all(&zip_bytes).unwrap();
            }
        }
    });

    let facts = Facts { version: "0.0.0-dev-1".into(), channel: "dev".into(), service: format!("http://{addr}/release"), updates: cache_dir };
    let bundle = BundleInfo { path: installed_app.clone(), id: TEAM_BUNDLE_ID.into(), version: "0.0.0-dev-1".into(), team: None };
    let platform = mac::Mac::new(facts, bundle, None, false);
    assert_eq!(platform.check().unwrap(), Some("0.0.0-dev-2".into()));
    platform.stage(&|_, _| {}).unwrap();
    platform.apply().unwrap();
    let updated = mac::bundle_info(&installed_app).unwrap();
    assert_eq!(updated.version, "0.0.0-dev-2");
    server_thread.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
