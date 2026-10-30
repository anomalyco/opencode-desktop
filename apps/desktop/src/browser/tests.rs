use rune::{Env, Key, KeyEvent, Mods, Runtime};

use crate::panels::panel_key;
use crate::{App, spec};

fn test_runtime() -> Runtime<App> {
    let s = spec(App::fixture_session);
    let mut rt = Runtime::new((s.init)(), s.view, s.size, Env::new(true, 2.0));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    rt
}

#[test]
fn alt_cmd_b_toggles_side_region_and_cmd_l_navigates_browser() {
    let mut rt = test_runtime();
    assert!(!rt.state.side_open());

    // ⌥⌘B opens the side region.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, alt: true, logo: true, ..Default::default() },
    });
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.state.side_open());

    // ⌘L opens a browser tab if on New tab and focuses the URL input; `:5173/app` resolves to localhost.
    rt.key(KeyEvent {
        key: Key::Command('l'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.state.browser.address.focused);
    assert!(rt.find("browser-address").is_some(), "URL input mounted in side region");

    rt.update(|s, _| s.browser.address = rune::TextEdit::new(":5173/app"));
    rt.update(|s, _| s.browser.address.focused = true);
    rt.key(KeyEvent { key: Key::Enter, mods: Mods::default() });
    rt.frame(1.0 / 60.0);

    assert!(!rt.state.browser.address.focused);
    let active_url = rt
        .state
        .active_browser_tab_id()
        .and_then(|id| rt.state.browser.host.tab(id))
        .map(|t| t.url.as_str());
    assert_eq!(active_url, Some("http://localhost:5173/app"));

    // ⌥⌘B again hides the side region while keeping the tab stored for next open.
    rt.key(KeyEvent {
        key: Key::Command('b'),
        mods: Mods { cmd: true, alt: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.side_open());
}

#[test]
fn multi_tab_close_hides_region_when_last_tab_closes() {
    let mut rt = test_runtime();
    let t1 = rt.state.open_new_browser_tab("http://localhost:3000/");
    let t2 = rt.state.open_new_browser_tab("github.com/anomalyco/rune");
    assert_eq!(rt.state.browser.host.tabs().len(), 2);
    assert_eq!(rt.state.active_browser_tab_id(), Some(t2));

    let k2 = panel_key("browser", &t2.to_string());
    rt.update(|s, tx| s.close_side_tab(&k2, tx));
    assert_eq!(rt.state.active_browser_tab_id(), Some(t1));

    let close_key = format!("side-tab-close-browser-{t1}");
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find(&close_key).expect("close tab button in side strip");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    assert!(rt.state.browser.host.tabs().is_empty());
    assert!(!rt.state.side_open());
}

#[test]
fn border_runs_all_the_way_around_over_opaque_native_surface() {
    use rune::native_surface::{ByteFrame, ColorInfo, Format};
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

    let mut rt = test_runtime();
    let tab_id = rt.state.browser.host.insert_mock_tab(
        "http://localhost:3000/",
        "Local Preview",
        browser_pane::EngineStatus::Ready,
    );
    let mut red = Vec::with_capacity(4 * 4 * 4);
    for _ in 0..16 {
        red.extend_from_slice(&[0, 0, 255, 255]); // BGRA -> pure red
    }
    rt.state
        .browser
        .host
        .tab(tab_id)
        .unwrap()
        .surface
        .push_bytes(
            ByteFrame {
                width: 4,
                height: 4,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [red, Vec::new()],
            },
            2,
        )
        .unwrap();

    rt.update(|s, tx| {
        s.side.width = 480.0;
        s.register_existing_browser_tab(tab_id);
        tx.settle();
    });
    rt.frame(1.0 / 60.0);

    let ([bx, by, bw, bh], _) = rt.presented("side-panel").expect("side-panel laid out");
    let gpu = Gpu::headless();
    let (wp, hp) = (2560u32, 1640u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, wp, hp);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (wp, hp));
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (wp, hp));
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, rt.clear_color());
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    let rgba = target.read(&gpu.device);
    let px = |x: u32, y: u32| -> [u8; 4] {
        let idx = ((y * wp + x) * 4) as usize;
        [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]]
    };

    let x_left = (bx * 2.0).round() as u32;
    let x_right = ((bx + bw) * 2.0).round() as u32 - 1;
    let x_mid = ((bx + bw * 0.5) * 2.0).round() as u32;
    let y_mid = ((by + bh * 0.6) * 2.0).round() as u32;
    let y_low = ((by + bh - 30.0) * 2.0).round() as u32;
    let y_bot = ((by + bh) * 2.0).round() as u32 - 1;

    let inside = px(x_left + 20, y_mid);
    assert!(inside[0] > 240 && inside[1] < 10 && inside[2] < 10, "expected red NativeSurface interior, got {inside:?}");

    for (name, sample) in [
        ("left-mid", px(x_left, y_mid)),
        ("left-low", px(x_left, y_low)),
        ("right-mid", px(x_right, y_mid)),
        ("bottom-mid", px(x_mid, y_bot)),
    ] {
        assert!(
            sample[1] > 30 && sample[2] > 30,
            "{name} border pixel was overwritten by NativeSurface: {sample:?}"
        );
    }

    let outer_corner = px(x_left, y_bot);
    assert!(
        outer_corner[0] < 40,
        "bottom-left rounded corner leaked unclipped NativeSurface: {outer_corner:?}"
    );
}

#[test]
fn captures_live_chromium_sidecar_split_still_when_requested() {
    let Ok(out_png) = std::env::var("OPENCODE_DESKTOP_LIVE_STILL") else {
        return;
    };
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
    use std::io::{Read, Write};
    use std::net::TcpListener;
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
  body { font-family: -apple-system, BlinkMacSystemFont, "Inter", sans-serif; background: #0e1015; color: #eef2f8; padding: 20px; }
  .top { display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px; gap: 8px; }
  .badge { font: 500 11px ui-monospace, "SF Mono", monospace; padding: 3px 8px; border-radius: 999px; background: rgba(52,211,153,0.15); color: #6ee7b7; border: 1px solid rgba(52,211,153,0.3); white-space: nowrap; }
  h1 { font-size: 16px; font-weight: 620; letter-spacing: -0.02em; }
  .card { padding: 14px; border-radius: 10px; background: #151922; border: 1px solid rgba(255,255,255,0.08); margin-bottom: 10px; }
  .card h2 { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: #8b95ab; margin-bottom: 6px; }
  .metric { font: 600 18px ui-monospace, "SF Mono", monospace; color: #f8fafc; }
  .sub { font-size: 12px; color: #94a3b8; margin-top: 4px; }
  .bars { display: flex; gap: 6px; align-items: flex-end; height: 54px; margin-top: 10px; }
  .bar { flex: 1; border-radius: 4px 4px 0 0; background: linear-gradient(180deg, #38bdf8, #2563eb); }
</style></head><body>
  <div class="top"><h1>Backoff Reconnect</h1><span class="badge">LIVE · 2x</span></div>
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
    <h2>Side Region · Panel Registry</h2>
    <div class="sub">Modular side tabs · On-demand Chromium 156 sidecar</div>
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

    let sock_dir = std::env::temp_dir().join(format!("ocd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sock_dir);
    std::fs::create_dir_all(&sock_dir).unwrap();

    let zoom: f32 = std::env::var("OPENCODE_DESKTOP_ZOOM")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.6);
    let scale = 2.0f32;
    let size = (1600.0f32, 960.0f32);
    let s = rune::AppSpec { setup: None, ..spec(App::fixture_session) };
    let mut rt = rune::App::new(s)
        .chrome(rune::Chrome {
            full_size_content: true,
            traffic_lights: Some((25.0, 25.0)),
            drag_regions: true,
        })
        .headless_runtime(size, Env::new(true, scale));
    rt.set_srgb_blend(true);
    rt.state.zoom = zoom;
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
    rt.frame(1.0 / 60.0);
    if let Ok(before_png) = std::env::var("OPENCODE_DESKTOP_BEFORE_STILL") {
        snap_to(&mut rt, &mut renderer, &before_png);
    }

    // Open New tab view first if requested, to capture the modular launcher + Recently closed still!
    if let Ok(newtab_png) = std::env::var("OPENCODE_DESKTOP_NEWTAB_STILL") {
        rt.update(|app, tx| {
            let sid = app.active_session_id().unwrap().to_owned();
            let now = app.now_ms;
            crate::panels::tabs::push_closed_tab(
                &mut app.session_side_mut(&sid).closed,
                crate::panels::ClosedSideTab {
                    key: "browser:77".into(),
                    title: "localhost:5173/dashboard".into(),
                    icon_name: Some("globe".into()),
                    file: None,
                    detail: Some("localhost:5173".into()),
                    data: Some("http://localhost:5173/dashboard".into()),
                    closed_ms: now,
                },
            );
            app.toggle_side_region(tx);
            tx.settle();
        });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
        snap_to(&mut rt, &mut renderer, &newtab_png);
    }

    let url = format!("http://127.0.0.1:{port}/");
    rt.update(|app, tx| {
        let docs_id = app.browser.host.insert_mock_tab(
            "https://github.com/anomalyco/rune",
            "anomalyco/rune",
            browser_pane::EngineStatus::NotInstalled,
        );
        app.register_existing_browser_tab(docs_id);
        // Open the live tab second so it stays `focused: true` in CEF and streams IOSurface frames.
        let id = app.browser.host.open_tab(&url);
        app.register_existing_browser_tab(id);
        app.select_side_tab(&panel_key("browser", &id.to_string()), tx);
        tx.settle();
        app.sync_browser_address_now();
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut loaded_at: Option<Instant> = None;
    while Instant::now() < deadline {
        rt.frame(1.0 / 50.0);
        let ready = rt
            .state
            .active_browser_tab_id()
            .and_then(|id| rt.state.browser.host.tab(id))
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
        "live sidecar did not finish loading preview page: status={:?}",
        rt.state.browser.host.status(),
    );

    rt.frame(1.0 / 60.0);
    snap_to(&mut rt, &mut renderer, &out_png);

    rt.state.browser.host.shutdown_sidecar();
    let _ = std::fs::remove_dir_all(&sock_dir);
}
