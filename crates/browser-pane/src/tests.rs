use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rune::NativeSurface;
use serde_json::json;

use crate::address::resolve_address;
use crate::client::BrowserHost;
use crate::protocol::{CursorKind, EngineMsg, HostMsg, PickedElement, TabSnapshot};

#[test]
fn each_app_gets_its_own_engine_socket() {
    use crate::installer::socket_name_for;
    let seer = std::path::Path::new("/Users/a/Applications/Seer.app/Contents/MacOS/seer");
    let desktop = std::path::Path::new("/Users/a/Applications/OpenCode (Rune).app/Contents/MacOS/opencode-desktop");
    // The engine takes one app at a time: a second app on its socket takes its frames and tabs.
    assert_ne!(socket_name_for(seer, 501), socket_name_for(desktop, 501));
    // The same app finds its engine again after a relaunch or hot reload.
    assert_eq!(socket_name_for(seer, 501), socket_name_for(seer, 501));
    assert!(socket_name_for(desktop, 501).starts_with("browser-501-"));
}

#[test]
fn each_app_keeps_its_own_profile_paired_with_its_socket() {
    use crate::installer::socket_name_for;
    use crate::profile::profile_dir_for;
    let root = std::path::Path::new("/Users/a/Library/Application Support/co.anomaly.opencode");
    let seer = std::path::Path::new("/Users/a/Applications/Seer.app/Contents/MacOS/seer");
    let desktop = std::path::Path::new("/Users/a/Applications/OpenCode (Rune).app/Contents/MacOS/opencode-desktop");
    let tag_of = |exe| socket_name_for(exe, 501).trim_start_matches("browser-501-").trim_end_matches(".sock").to_owned();
    for exe in [seer, desktop] {
        assert_eq!(profile_dir_for(root, exe), root.join(format!("browser-profile-{}", tag_of(exe))));
    }
    assert_eq!(profile_dir_for(root, seer), profile_dir_for(root, seer));
    assert_ne!(profile_dir_for(root, seer), profile_dir_for(root, desktop));
}

#[test]
fn the_profile_defaults_per_app_and_the_environment_wins() {
    use crate::profile::{profile_dir_for, resolve, shared_profile_dir};
    let exe = std::path::Path::new("/Users/a/Applications/Seer Dev.app/Contents/MacOS/seer");
    let own = resolve(None, None, exe);
    assert_eq!(own, profile_dir_for(own.parent().unwrap(), exe));
    assert_eq!(own.parent(), shared_profile_dir().parent(), "sits next to the shared profile");
    let shared = shared_profile_dir();
    assert!(shared.ends_with("co.anomaly.opencode/browser-profile") || shared.ends_with("opencode/browser-profile"));
    assert_eq!(resolve(None, Some(&shared), exe), shared);
    let env = std::ffi::OsString::from("/tmp/scratch-profile");
    assert_eq!(resolve(Some(env.clone()), Some(&shared), exe), std::path::PathBuf::from(&env));
    assert_eq!(resolve(Some(env.clone()), None, exe), std::path::PathBuf::from(&env));
    assert_eq!(resolve(Some("".into()), None, exe), own, "an empty variable doesn't count");
}

#[test]
fn the_engine_is_spawned_with_its_profile() {
    use crate::transport::engine_command;
    let cmd = engine_command("/x/engine".as_ref(), "/x/e.sock".as_ref(), "/x/browser-profile-0a1b2c3d".as_ref());
    let envs: Vec<_> = cmd.get_envs().collect();
    assert_eq!(
        envs,
        vec![(std::ffi::OsStr::new("OPENCODE_BROWSER_PROFILE"), Some(std::ffi::OsStr::new("/x/browser-profile-0a1b2c3d")))]
    );
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, ["--socket", "/x/e.sock"]);
}

#[test]
fn a_spawn_waits_for_an_engine_on_its_way_out_to_free_the_profile() {
    use crate::profile::{is_free, wait_until_free};
    use std::os::fd::AsRawFd;
    let dir = std::env::temp_dir().join(format!("browser-pane-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(is_free(&dir), "no profile yet");
    std::fs::create_dir_all(&dir).unwrap();
    let held = std::fs::File::create(dir.join("engine.lock")).unwrap();
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
    assert!(!is_free(&dir));
    assert!(!wait_until_free(&dir, Duration::from_millis(150)));
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(held);
    });
    let start = Instant::now();
    assert!(wait_until_free(&dir, Duration::from_secs(5)));
    assert!(start.elapsed() >= Duration::from_millis(150));
    releaser.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolves_addresses_like_desktop() {
    assert_eq!(resolve_address(""), "about:blank");
    assert_eq!(resolve_address(":3000"), "http://localhost:3000");
    assert_eq!(resolve_address(":5173/dashboard"), "http://localhost:5173/dashboard");
    assert_eq!(resolve_address("localhost:4096/api"), "http://localhost:4096/api");
    assert_eq!(resolve_address("127.0.0.1:8080"), "http://127.0.0.1:8080");
    assert_eq!(resolve_address("preview.localhost:1355"), "http://preview.localhost:1355");
    assert_eq!(resolve_address("github.com/anomalyco/rune"), "https://github.com/anomalyco/rune");
    assert_eq!(resolve_address("https://anoma.ly/team"), "https://anoma.ly/team");
    assert_eq!(
        resolve_address("rust wgpu iosurface"),
        "https://www.google.com/search?q=rust+wgpu+iosurface"
    );
}

#[test]
fn sidecar_socket_handshake_reconnect_and_cdp_roundtrip() {
    let sock_dir = std::env::temp_dir().join(format!("bp-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock = sock_dir.join("engine.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server_handle = std::thread::spawn(move || {
        // Connection 1: initial open + CDP + inspect pick
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello: HostMsg = serde_json::from_str(&line).unwrap();
        assert!(matches!(hello, HostMsg::Hello { .. }));

        let ready = serde_json::to_string(&EngineMsg::Ready { chromium_major: 156, pid: 999 }).unwrap();
        writeln!(stream, "{ready}").unwrap();

        line.clear();
        reader.read_line(&mut line).unwrap();
        let open: HostMsg = serde_json::from_str(&line).unwrap();
        let tab_id = match open {
            HostMsg::OpenTab { tab_id, url, .. } => {
                assert_eq!(url, "http://localhost:3000");
                tab_id
            }
            other => panic!("expected OpenTab, got {other:?}"),
        };

        let update = serde_json::to_string(&EngineMsg::TabUpdate {
            tab: TabSnapshot {
                tab_id,
                url: "http://localhost:3000/".into(),
                title: "Local Dev".into(),
                loading: false,
                can_go_back: true,
                can_go_forward: false,
                zoom: 1.0,
                cursor: CursorKind::Pointer,
            },
        })
        .unwrap();
        writeln!(stream, "{update}").unwrap();

        let pick = serde_json::to_string(&EngineMsg::InspectState {
            tab_id,
            active: false,
            picked: Some(PickedElement {
                tab_id,
                eref: "@e1".into(),
                backend_node_id: 42,
                selector: "button#ship".into(),
                label: "button#ship".into(),
                role: Some("button".into()),
                name: Some("Ship".into()),
                text: Some("Ship".into()),
                rect: [10.0, 20.0, 100.0, 36.0],
            }),
        })
        .unwrap();
        writeln!(stream, "{pick}").unwrap();

        line.clear();
        reader.read_line(&mut line).unwrap();
        let cdp: HostMsg = serde_json::from_str(&line).unwrap();
        if let HostMsg::CdpCall { tab_id, request_id, method, .. } = cdp {
            assert_eq!(method, "Runtime.evaluate");
            let resp = serde_json::to_string(&EngineMsg::CdpResponse {
                tab_id,
                request_id,
                ok: true,
                result: json!({ "result": { "value": 7 } }),
            })
            .unwrap();
            writeln!(stream, "{resp}").unwrap();
        } else {
            panic!("expected CdpCall");
        }
    });

    let mut host = BrowserHost::with_socket(sock.clone());
    let tab_id = host.open_tab(":3000");

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        host.poll();
        if host.tab(tab_id).is_some_and(|t| t.title == "Local Dev") {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host.tab(tab_id).unwrap().title, "Local Dev");
    assert_eq!(host.tab(tab_id).unwrap().cursor, CursorKind::Pointer);

    let picks: Vec<_> = host.drain_picked_elements().collect();
    assert_eq!(picks.len(), 1);
    assert_eq!(picks[0].eref, "@e1");
    assert_eq!(picks[0].selector, "button#ship");

    let req = host.send_cdp(tab_id, "Runtime.evaluate", json!({ "expression": "3+4" }), None);
    let mut got = None;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        host.poll();
        if let Some(r) = host.take_cdp_response(req) {
            got = Some(r);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(got.unwrap().unwrap()["result"]["value"], json!(7));

    server_handle.join().unwrap();
    let _ = std::fs::remove_dir_all(&sock_dir);
}

#[cfg(target_os = "macos")]
#[test]
fn mach_pipe_transfers_iosurface_into_native_surface_zero_copy() {
    use std::ffi::c_void;

    use crate::mach_pipe::{MachReceiver, lookup_service, send_iosurface_frame};

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVPixelBufferGetIOSurface(pixel_buffer: *const c_void) -> *mut c_void;
    }

    let surface = NativeSurface::new();
    let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let woke_flag = woke.clone();
    surface.set_waker(Arc::new(move || {
        woke_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let mut map = HashMap::new();
    map.insert(7u64, surface.clone());
    let surfaces = Arc::new(Mutex::new(map));

    let receiver = MachReceiver::start(surfaces, 9999).expect("start MachReceiver");
    let remote = lookup_service(&receiver.service_name).expect("lookup_service");

    // Create an IOSurface-backed PixelBuffer using Rune's helper and extract its raw IOSurfaceRef.
    let synthetic = rune::native_surface::synthetic(64, 64, 1);
    let pb = rune::native_surface::macos::PixelBuffer::from_bytes(&synthetic).unwrap();
    // `PixelBuffer` is `#[repr(transparent)]`-shaped around `CFRetained<CVPixelBuffer>` (pointer-sized).
    let cv_ptr: *const c_void = unsafe { *(&pb as *const _ as *const *const c_void) };
    let io_surface = unsafe { CVPixelBufferGetIOSurface(cv_ptr) };
    assert!(!io_surface.is_null(), "expected IOSurface-backed CVPixelBuffer");

    let ok = unsafe { send_iosurface_frame(remote, 7, 42, 64, 64, io_surface) };
    assert!(ok, "send_iosurface_frame failed");

    let deadline = Instant::now() + Duration::from_secs(2);
    while !woke.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(woke.load(std::sync::atomic::Ordering::SeqCst), "NativeSurface waker was not fired by MachReceiver");
    assert_eq!(surface.last_error(), None);
}

#[test]
fn live_sidecar_engine_smoke_when_installed() {
    if std::env::var("OPENCODE_BROWSER_ENGINE_SMOKE").as_deref() != Ok("1") {
        return;
    }
    let sock_dir = std::env::temp_dir().join(format!("bp-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock = sock_dir.join("live.sock");

    let mut host = BrowserHost::with_socket(sock);
    let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let wf = woke.clone();
    host.set_waker(Arc::new(move || {
        wf.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let tab_id = host.open_tab("data:text/html,<title>SidecarOK</title><h1>Live</h1>");
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        host.poll();
        if host.tab(tab_id).is_some_and(|t| t.title == "SidecarOK") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(host.tab(tab_id).map(|t| t.title.as_str()), Some("SidecarOK"));

    let req = host.send_cdp(
        tab_id,
        "Runtime.evaluate",
        json!({ "expression": "document.querySelector('h1').textContent", "returnByValue": true }),
        None,
    );
    let mut text = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        host.poll();
        if let Some(res) = host.take_cdp_response(req) {
            text = res.ok().and_then(|v| v["result"]["value"].as_str().map(str::to_owned));
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(text.as_deref(), Some("Live"));

    host.close_tab(tab_id);
    std::thread::sleep(Duration::from_millis(150));
    let _ = std::fs::remove_dir_all(&sock_dir);
}

#[cfg(target_os = "macos")]
fn serve_bytes_once(payload: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut req_buf = [0u8; 2048];
        let _ = std::io::Read::read(&mut stream, &mut req_buf);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        stream.write_all(header.as_bytes()).unwrap();
        for chunk in payload.chunks(128 * 1024) {
            stream.write_all(chunk).unwrap();
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/OpenCode-Browser-Engine.zip"), handle)
}

#[cfg(target_os = "macos")]
fn make_synthetic_engine_zip(work: &std::path::Path) -> (Vec<u8>, String) {
    use sha2::{Digest, Sha256};

    let bundle_dir = work.join("src").join(crate::installer::ENGINE_BUNDLE_NAME);
    let macos_dir = bundle_dir.join("Contents/MacOS");
    std::fs::create_dir_all(&macos_dir).unwrap();
    // Make the payload > 600 KiB incompressible pseudo-random bytes so multi-chunk progress ticks fire.
    let mut bin_bytes = b"#!/bin/sh\necho ok\n".to_vec();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..(650 * 1024) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        bin_bytes.push((seed & 0xff) as u8);
    }
    let bin_path = macos_dir.join("opencode-browser-engine");
    std::fs::write(&bin_path, &bin_bytes).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let zip_path = work.join("test-engine.zip");
    let status = std::process::Command::new("/usr/bin/ditto")
        .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
        .arg(&bundle_dir)
        .arg(&zip_path)
        .status()
        .unwrap();
    assert!(status.success());

    let bytes = std::fs::read(&zip_path).unwrap();
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let sha = format!("{:x}", hasher.finalize());
    (bytes, sha)
}

#[cfg(target_os = "macos")]
#[test]
fn sha256_mismatch_rejects_archive_and_preserves_existing_cache() {
    use crate::installer::{
        ENGINE_DEFAULT_SHA256, ENGINE_DEFAULT_URL, EngineStatus, download_and_install_to,
    };

    assert!(ENGINE_DEFAULT_URL.is_some());
    assert_eq!(ENGINE_DEFAULT_SHA256.unwrap().len(), 64);

    let root = std::env::temp_dir().join(format!("bp-sha-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let (zip_bytes, _real_sha) = make_synthetic_engine_zip(&root);
    let dest = root.join("cache").join("156.0.14");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("sentinel.txt"), b"keep-me").unwrap();

    let (url, srv) = serve_bytes_once(zip_bytes);
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let wrong_sha = "0000000000000000000000000000000000000000000000000000000000000000";
    let res = download_and_install_to(&url, Some(wrong_sha), &dest, &state, None);
    srv.join().unwrap();

    assert_eq!(res.unwrap_err(), "Download couldn't be verified. Try again.");
    assert_eq!(
        std::fs::read_to_string(dest.join("sentinel.txt")).unwrap(),
        "keep-me",
        "existing cache directory must stay intact on SHA mismatch"
    );
    let leftovers: Vec<_> = std::fs::read_dir(root.join("cache"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec!["156.0.14"], "staging directories must be cleaned up");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(target_os = "macos")]
#[test]
fn valid_archive_verifies_sha256_reports_progress_and_swaps_atomically() {
    use crate::installer::{EngineStatus, download_and_install_to};

    let root = std::env::temp_dir().join(format!("bp-sha-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let (zip_bytes, real_sha) = make_synthetic_engine_zip(&root);
    let total_len = zip_bytes.len() as u64;
    let dest = root.join("cache").join("156.0.14");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("stale.txt"), b"old").unwrap();

    let (url, srv) = serve_bytes_once(zip_bytes);
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let wakes_cb = wakes.clone();
    let waker: rune::Waker = Arc::new(move || {
        wakes_cb.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });

    let installed_bin =
        download_and_install_to(&url, Some(&real_sha), &dest, &state, Some(&waker)).unwrap();
    srv.join().unwrap();

    assert!(installed_bin.is_file(), "installed binary exists at {}", installed_bin.display());
    assert!(!dest.join("stale.txt").exists(), "old cache contents atomically replaced");
    assert!(
        wakes.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "waker fired for start, mid-stream progress, and completion"
    );
    assert_eq!(
        *state.lock().unwrap(),
        EngineStatus::Downloading {
            downloaded_bytes: total_len,
            total_bytes: Some(total_len),
        }
    );
    let leftovers: Vec<_> = std::fs::read_dir(root.join("cache"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec!["156.0.14"], "no staging or backup leftovers remain");
    let _ = std::fs::remove_dir_all(&root);
}


fn snapshot(tab_id: u64, url: &str, zoom: f64) -> TabSnapshot {
    TabSnapshot {
        tab_id,
        url: url.into(),
        title: String::new(),
        loading: false,
        can_go_back: false,
        can_go_forward: false,
        zoom,
        cursor: CursorKind::Pointer,
    }
}

#[test]
fn site_of_is_host_and_port() {
    use crate::site_zoom::site_of;
    assert_eq!(site_of("https://GitHub.com/anomalyco?tab=1").as_deref(), Some("github.com"));
    assert_eq!(site_of("http://localhost:3000/a#b").as_deref(), Some("localhost:3000"));
    assert_eq!(site_of("http://user@example.com/").as_deref(), Some("example.com"));
    assert_eq!(site_of("about:blank"), None);
    assert_eq!(site_of(""), None);
}

#[test]
fn page_zoom_is_remembered_per_site() {
    let dir = std::env::temp_dir().join(format!("browser-pane-zoom-{}", std::process::id()));
    let file = dir.join("zoom.json");
    let _ = std::fs::remove_dir_all(&dir);

    let mut host = BrowserHost::new();
    host.remember_site_zoom(file.clone());
    host.upsert_tab(snapshot(1, "https://example.com/a", 1.0));
    host.step_zoom(1, 1);
    host.step_zoom(1, 1);
    assert_eq!(host.tab(1).unwrap().zoom, 1.2);

    // Another page on the site opens at its zoom; another site starts at 100%.
    host.upsert_tab(snapshot(2, "https://example.com/b", 1.0));
    assert_eq!(host.tab(2).unwrap().zoom, 1.2);
    host.upsert_tab(snapshot(1, "https://other.dev/", 1.2));
    assert_eq!(host.tab(1).unwrap().zoom, 1.0);

    // It survives a relaunch, and resetting to 100% forgets the site.
    let mut again = BrowserHost::new();
    again.remember_site_zoom(file.clone());
    again.upsert_tab(snapshot(7, "https://example.com/", 1.0));
    assert_eq!(again.tab(7).unwrap().zoom, 1.2);
    again.step_zoom(7, 0);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A cookie set before the engine quits is sent again after it relaunches (the profile persists).
/// Run with `OPENCODE_BROWSER_ENGINE_SMOKE=1` and `OPENCODE_BROWSER_PROFILE` set to a scratch dir.
#[test]
fn live_engine_keeps_cookies_across_relaunch() {
    if std::env::var("OPENCODE_BROWSER_ENGINE_SMOKE").as_deref() != Ok("1") {
        return;
    }
    use std::io::Read;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let Ok(n) = s.read(&mut buf) else { continue };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let cookie = req
                .lines()
                .find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: ")))
                .unwrap_or("none")
                .to_owned();
            let (extra, title) = if req.starts_with("GET /set") {
                ("Set-Cookie: sid=signed-in; Max-Age=86400; Path=/\r\n", "set".to_owned())
            } else {
                ("", format!("cookie={cookie}"))
            };
            let body = format!("<title>{title}</title>");
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });

    let sock_dir = std::env::temp_dir().join(format!("bp-cookie-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock = sock_dir.join("live.sock");

    let load = |url: String, want: &dyn Fn(&str) -> bool| -> (BrowserHost, String) {
        let mut host = BrowserHost::with_socket(sock.clone());
        host.set_waker(Arc::new(|| {}));
        let id = host.open_tab(&url);
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            host.poll();
            if host.tab(id).is_some_and(|t| want(&t.title) && !t.loading) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let title = host.tab(id).map(|t| t.title.clone()).unwrap_or_default();
        (host, title)
    };
    let quit = |mut host: BrowserHost| {
        host.shutdown_sidecar();
        let deadline = Instant::now() + Duration::from_secs(15);
        while sock.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!sock.exists(), "engine exited");
        std::thread::sleep(Duration::from_millis(300));
    };

    let (host, title) = load(format!("http://127.0.0.1:{port}/set"), &|t| t == "set");
    assert_eq!(title, "set");
    quit(host);

    let (host, title) = load(format!("http://127.0.0.1:{port}/check"), &|t| t.starts_with("cookie="));
    assert_eq!(title, "cookie=sid=signed-in", "the cookie survived the relaunch");
    quit(host);
    let _ = std::fs::remove_dir_all(&sock_dir);
}

#[derive(Clone, Debug)]
struct SeenRequest {
    path: String,
    auth: Option<String>,
    accept: Option<String>,
}

struct MockHttp {
    base: String,
    seen: Arc<Mutex<Vec<SeenRequest>>>,
}

/// A local HTTP server answering every request with `route(base, request)`: (status, extra headers, body).
fn mock_http(
    route: impl Fn(&str, &SeenRequest) -> (u16, String, Vec<u8>) + Send + 'static,
) -> MockHttp {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (base_srv, seen_srv) = (base.clone(), seen.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let mut req = SeenRequest { path, auth: None, accept: None };
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap_or((&line, ""));
                let value = Some(value.trim().to_owned());
                match name.trim().to_ascii_lowercase().as_str() {
                    "authorization" => req.auth = value,
                    "accept" => req.accept = value,
                    _ => {}
                }
            }
            let (status, headers, body) = route(&base_srv, &req);
            seen_srv.lock().unwrap().push(req);
            let mut stream = stream;
            let head = format!(
                "HTTP/1.1 {status} X\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    MockHttp { base, seen }
}

const PUBLIC_PATH: &str = "/anomalyco/rune-desktop/releases/download/browser-engine-1/engine.zip";

/// A private release: the public URL 404s, the API lists the asset, and the asset redirects to `storage`.
fn private_github(storage: String) -> MockHttp {
    mock_http(move |base, req| match req.path.as_str() {
        "/repos/anomalyco/rune-desktop/releases/tags/browser-engine-1" => {
            let assets = json!({ "assets": [
                { "name": "SHA256SUMS", "url": format!("{base}/assets/1") },
                { "name": "engine.zip", "url": format!("{base}/assets/2") },
            ]});
            (200, String::new(), assets.to_string().into_bytes())
        }
        "/assets/2" => (302, format!("Location: {storage}/signed?sig=abc\r\n"), Vec::new()),
        _ => (404, String::new(), b"Not Found".to_vec()),
    })
}

fn github_auth(api: &MockHttp, token: Option<&str>) -> crate::github_download::GithubAuth {
    let token = token.map(str::to_owned);
    crate::github_download::GithubAuth { api_base: api.base.clone(), token: Box::new(move || token.clone()) }
}

fn install_scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("bp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn release_asset_comes_from_the_default_url() {
    use crate::github_download::ReleaseAsset;
    let public = crate::installer::ENGINE_DEFAULT_URL.unwrap();
    let sha = crate::installer::ENGINE_DEFAULT_SHA256.unwrap();
    let ver = crate::installer::ENGINE_VERSION;
    assert_eq!(
        public,
        format!("https://anoma.ly/release/opencode-rune/files/{sha}/OpenCode-Browser-Engine-{ver}-macos-arm64.zip")
    );
    assert_eq!(ReleaseAsset::parse(public), None, "default URL is public on anoma.ly/release");
    let asset = ReleaseAsset::parse(crate::installer::ENGINE_GITHUB_FALLBACK_URL.unwrap()).unwrap();
    assert_eq!((asset.owner.as_str(), asset.repo.as_str()), ("anomalyco", "rune-desktop"));
    assert_eq!(asset.tag, format!("browser-engine-{ver}"));
    assert!(asset.file.ends_with("-macos-arm64.zip"));
    assert_eq!(ReleaseAsset::parse("https://example.com/engine.zip"), None);
}

#[cfg(target_os = "macos")]
#[test]
fn public_release_downloads_without_a_github_login() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("public");
    let (zip, sha) = make_synthetic_engine_zip(&root);
    let github = mock_http(move |_, _| (200, String::new(), zip.clone()));
    let auth = crate::github_download::GithubAuth {
        api_base: github.base.clone(),
        token: Box::new(|| panic!("a public download never asks for a GitHub login")),
    };
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("{}{PUBLIC_PATH}", github.base);
    let bin = download_and_install_with(&url, Some(&sha), &root.join("cache/1"), &state, None, &auth);
    assert!(bin.unwrap().is_file());
    let seen = github.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].auth, None);
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(target_os = "macos")]
#[test]
fn private_release_downloads_with_the_github_login_and_keeps_it_off_the_storage_host() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("private");
    let (zip, sha) = make_synthetic_engine_zip(&root);
    let storage = mock_http(move |_, _| (200, String::new(), zip.clone()));
    let github = private_github(storage.base.clone());
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("{}{PUBLIC_PATH}", github.base);
    let auth = github_auth(&github, Some("t0k"));
    let bin = download_and_install_with(&url, Some(&sha), &root.join("cache/1"), &state, None, &auth);
    assert!(bin.unwrap().is_file());

    let seen = github.seen.lock().unwrap().clone();
    let paths: Vec<_> = seen.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [PUBLIC_PATH, "/repos/anomalyco/rune-desktop/releases/tags/browser-engine-1", "/assets/2"]);
    assert_eq!(seen[0].auth, None, "the public URL is tried without a login");
    assert_eq!(seen[1].auth.as_deref(), Some("Bearer t0k"));
    assert_eq!(seen[2].auth.as_deref(), Some("Bearer t0k"));
    assert_eq!(seen[2].accept.as_deref(), Some("application/octet-stream"));
    let stored = storage.seen.lock().unwrap().clone();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].path, "/signed?sig=abc");
    assert_eq!(stored[0].auth, None, "the token never reaches the storage host");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn private_release_without_a_github_login_asks_to_sign_in() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("no-login");
    let github = private_github("http://127.0.0.1:9".into());
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("{}{PUBLIC_PATH}", github.base);
    let err = download_and_install_with(&url, None, &root.join("cache/1"), &state, None, &github_auth(&github, None))
        .unwrap_err();
    assert_eq!(err, "Sign in to GitHub to download the browser.\nRun gh auth login in Terminal, then retry.");
    assert_eq!(github.seen.lock().unwrap().len(), 1, "no API call without a login");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn private_release_the_login_cant_see_says_so() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("no-access");
    let github = mock_http(|_, _| (404, String::new(), Vec::new()));
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("{}{PUBLIC_PATH}", github.base);
    let auth = github_auth(&github, Some("t0k"));
    let err = download_and_install_with(&url, None, &root.join("cache/1"), &state, None, &auth).unwrap_err();
    assert_eq!(
        err,
        "Your GitHub account can't see the browser download.\nAsk for access to anomalyco/rune-desktop."
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(target_os = "macos")]
#[test]
fn private_release_with_a_bad_sha_is_rejected() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("private-bad-sha");
    let (zip, _) = make_synthetic_engine_zip(&root);
    let storage = mock_http(move |_, _| (200, String::new(), zip.clone()));
    let github = private_github(storage.base.clone());
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("{}{PUBLIC_PATH}", github.base);
    let dest = root.join("cache/1");
    let auth = github_auth(&github, Some("t0k"));
    let err = download_and_install_with(&url, Some(&"0".repeat(64)), &dest, &state, None, &auth).unwrap_err();
    assert_eq!(err, "Download couldn't be verified. Try again.");
    assert!(!dest.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unreachable_server_is_a_connection_error() {
    use crate::installer::{EngineStatus, download_and_install_with};
    let root = install_scratch("offline");
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let auth = crate::github_download::GithubAuth {
        api_base: format!("http://127.0.0.1:{port}"),
        token: Box::new(|| Some("t0k".into())),
    };
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let url = format!("http://127.0.0.1:{port}{PUBLIC_PATH}");
    let err = download_and_install_with(&url, None, &root.join("cache/1"), &state, None, &auth).unwrap_err();
    assert_eq!(err, "Couldn't download the browser. Check your connection and try again.");
    let _ = std::fs::remove_dir_all(&root);
}

/// Downloads the real release into a scratch folder. Run with `OPENCODE_BROWSER_ENGINE_LIVE_DOWNLOAD=1`.
#[test]
fn live_release_downloads_and_verifies() {
    if std::env::var("OPENCODE_BROWSER_ENGINE_LIVE_DOWNLOAD").as_deref() != Ok("1") {
        return;
    }
    use crate::installer::{EngineStatus, download_and_install_to, resolve_download_source};
    let root = install_scratch("live");
    let (url, sha) = resolve_download_source().unwrap();
    let state = Arc::new(Mutex::new(EngineStatus::NotInstalled));
    let res = download_and_install_to(&url, sha.as_deref(), &root.join("cache/1"), &state, None);
    let _ = std::fs::remove_dir_all(&root);
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn a_tab_opened_during_a_background_download_starts_when_it_lands() {
    let sock_dir = std::env::temp_dir().join(format!("bp-prefetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();
    let sock = sock_dir.join("engine.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let engine = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(matches!(serde_json::from_str(&line).unwrap(), HostMsg::Hello { .. }));
        let ready = serde_json::to_string(&EngineMsg::Ready { chromium_major: 156, pid: 1 }).unwrap();
        writeln!(stream, "{ready}").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str::<HostMsg>(&line).unwrap()
    });

    let mut host = BrowserHost::with_socket(sock);
    host.set_mock_status(crate::EngineStatus::Downloading { downloaded_bytes: 0, total_bytes: None });
    host.open_tab(":3000");
    host.poll();
    assert!(!host.is_connected(), "the tab waits while the engine downloads");

    // What `prefetch`'s download does when it lands.
    *host.status.lock().unwrap() = crate::EngineStatus::Ready;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !engine.is_finished() && Instant::now() < deadline {
        host.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    match engine.join().unwrap() {
        HostMsg::OpenTab { url, .. } => assert_eq!(url, "http://localhost:3000"),
        other => panic!("expected OpenTab, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&sock_dir);
}
