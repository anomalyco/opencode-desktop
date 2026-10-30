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
