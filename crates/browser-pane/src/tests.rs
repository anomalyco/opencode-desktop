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

