use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::{App, FIXTURE, open_scene, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(FIXTURE);
        open_scene(&mut a);
        a.finish_stream();
        a
    });
    let mut rt = Runtime::new((s.init)(), s.view, s.size, Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

#[test]
fn alt_cmd_b_toggles_browser_split_pane_and_cmd_l_focuses_address() {
    let mut rt = test_runtime();
    assert!(!rt.state.browser.open);

    // ⌥⌘B opens the pane with an initial tab.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, alt: true, logo: true, ..Default::default() },
    });
    for _ in 0..20 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.browser.open);
    assert_eq!(rt.state.browser.host.tabs().len(), 1);
    assert!(rt.find("browser-address").is_some(), "URL input mounted in split pane");

    // ⌘L focuses the URL input; typing `:5173` + Enter resolves to http://localhost:5173.
    rt.key(KeyEvent {
        key: Key::Command('l'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.browser.address.focused);

    rt.update(|s, _| s.browser.address = rune::TextEdit::new(":5173/app"));
    rt.update(|s, _| s.browser.address.focused = true);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);

    assert!(!rt.state.browser.address.focused);
    assert_eq!(
        rt.state.browser.host.focused_tab().map(|t| t.url.as_str()),
        Some("http://localhost:5173/app")
    );

    // ⌥⌘B again hides the pane.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, alt: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.browser.open);
}

#[test]
fn multi_tab_close_hides_pane_when_last_tab_closes() {
    let mut rt = test_runtime();
    rt.update(|s, tx| s.toggle_browser(tx));
    let t1 = rt.state.browser.host.focused_tab_id().unwrap();
    let t2 = rt.state.open_new_browser_tab("github.com/anomalyco/rune");
    assert_eq!(rt.state.browser.host.tabs().len(), 2);
    assert_eq!(rt.state.browser.host.focused_tab_id(), Some(t2));

    rt.update(|s, _| s.browser.host.close_tab(t2));
    assert_eq!(rt.state.browser.host.focused_tab_id(), Some(t1));

    let close_key = format!("btab-close-{t1}");
    for _ in 0..24 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(&close_key).expect("close tab button");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    assert!(rt.state.browser.host.tabs().is_empty());
    assert!(!rt.state.browser.open);
}

#[test]
fn captures_live_chromium_sidecar_split_still_when_requested() {
    let Ok(out_png) = std::env::var("OPENCODE_DESKTOP_LIVE_STILL") else {
        return;
    };
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = stream;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 2048];
                let Ok(n) = s.read(&mut buf) else { return };
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("GET /favicon") {
                    let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
                    return;
                }
                let html = r##"<!doctype html><html><head><meta charset="utf-8"><title>lumen · localhost:3000</title>
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { font-family: -apple-system, BlinkMacSystemFont, "Inter", sans-serif; background: #0e1015; color: #eef2f8; padding: 24px; }
  .top { display: flex; justify-content: space-between; align-items: center; margin-bottom: 18px; }
  .badge { font: 500 11.5px ui-monospace, "SF Mono", monospace; padding: 4px 10px; border-radius: 999px; background: rgba(52,211,153,0.15); color: #6ee7b7; border: 1px solid rgba(52,211,153,0.3); }
  h1 { font-size: 18px; font-weight: 620; letter-spacing: -0.02em; }
  .card { padding: 16px; border-radius: 10px; background: #151922; border: 1px solid rgba(255,255,255,0.08); margin-bottom: 12px; }
  .card h2 { font-size: 11.5px; text-transform: uppercase; letter-spacing: 0.06em; color: #8b95ab; margin-bottom: 8px; }
  .metric { font: 600 22px ui-monospace, "SF Mono", monospace; color: #f8fafc; }
  .sub { font-size: 12.5px; color: #94a3b8; margin-top: 4px; }
  .bars { display: flex; gap: 6px; align-items: flex-end; height: 64px; margin-top: 12px; }
  .bar { flex: 1; border-radius: 4px 4px 0 0; background: linear-gradient(180deg, #38bdf8, #2563eb); }
</style></head><body>
  <div class="top"><h1>Backoff Reconnect Monitor</h1><span class="badge">LIVE · 2x IOSurface</span></div>
  <div class="card">
    <h2>Reconnect Delays (ms)</h2>
    <div class="metric">1,000 → 2,000 → 4,000 → 8,000</div>
    <div class="sub">Cap held at 30,000 ms across flapping stream trials</div>
    <div class="bars">
      <div class="bar" style="height:18%"></div>
      <div class="bar" style="height:32%"></div>
      <div class="bar" style="height:56%"></div>
      <div class="bar" style="height:88%"></div>
      <div class="bar" style="height:100%"></div>
    </div>
  </div>
  <div class="card">
    <h2>Browser Sidecar</h2>
    <div class="sub">Chromium 156 · On-demand sidecar · 0 bytes CEF in OpenCode (Rune).app</div>
  </div>
</body></html>"##;
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    html.len()
                );
                let _ = s.write_all(hdr.as_bytes());
                let _ = s.write_all(html.as_bytes());
            });
        }
    });

    let sock_dir = PathBuf::from("/tmp").join(format!("ocd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();

    let scale = 2.0f32;
    let size = (1440.0f32, 840.0f32);
    let s = rune::AppSpec {
        setup: None,
        ..spec(|| {
            let mut a = App::empty();
            a.load_fixture(FIXTURE);
            open_scene(&mut a);
            a.finish_stream();
            a
        })
    };
    let mut rt = rune::App::new(s)
        .chrome(rune::Chrome {
            full_size_content: true,
            traffic_lights: Some((25.0, 25.0)),
            drag_regions: true,
        })
        .headless_runtime(size, Env::new(true, scale));
    rt.state.browser.host = browser_pane::BrowserHost::with_socket(sock_dir.join("live.sock"));
    let woke = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let wf = woke.clone();
    rt.state.browser.host.set_waker(std::sync::Arc::new(move || {
        wf.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let gpu = Gpu::headless();
    let (wp, hp) = ((size.0 * scale) as u32, (size.1 * scale) as u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, wp, hp);
    let snap_to = |rt: &mut Runtime<App>, renderer: &mut Renderer, out: &str| {
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (wp, hp));
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (wp, hp));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, rt.clear_color());
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let rgba = target.read(&gpu.device);
        write_png(std::path::Path::new(out), wp, hp, &rgba).unwrap();
    };

    rt.frame(0.0);
    if let Ok(before_png) = std::env::var("OPENCODE_DESKTOP_BEFORE_STILL") {
        snap_to(&mut rt, &mut renderer, &before_png);
    }

    let url = format!("http://127.0.0.1:{port}/");
    rt.update(|app, _| {
        app.browser.open = true;
        app.browser.width = 500.0;
        app.browser.host.open_tab(&url);
        app.sync_browser_address_now();
    });

    // Step frames so `on_layout` records the exact 500pt x ~748pt viewport size for the sidecar,
    // and wait until Chromium loads the title AND delivers a matching IOSurface frame.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut loaded_at: Option<Instant> = None;
    while Instant::now() < deadline {
        rt.frame(1.0 / 50.0);
        let ready = rt
            .state
            .browser
            .host
            .focused_tab()
            .is_some_and(|t| t.title.contains("lumen") && !t.loading);
        if ready {
            let t0 = *loaded_at.get_or_insert_with(Instant::now);
            if t0.elapsed() > Duration::from_millis(400) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(
        loaded_at.is_some(),
        "live sidecar did not finish loading preview page: status={:?} tab={:?} wakes={}",
        rt.state.browser.host.status(),
        rt.state.browser.host.focused_tab(),
        woke.load(std::sync::atomic::Ordering::SeqCst),
    );

    rt.frame(1.0 / 60.0);
    snap_to(&mut rt, &mut renderer, &out_png);

    rt.state.browser.host.shutdown_sidecar();
    let _ = std::fs::remove_dir_all(&sock_dir);
}

