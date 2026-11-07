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

fn cmd_t() -> KeyEvent {
    KeyEvent { key: Key::Command('t'), mods: Mods { cmd: true, logo: true, ..Default::default() } }
}

#[test]
fn cmd_t_in_the_browser_pane_opens_a_browser_tab_and_elsewhere_the_chooser() {
    let mut rt = test_runtime();
    let first = rt.state.open_new_browser_tab("http://localhost:3000/");
    if !rt.state.side_open() {
        rt.update(|s, tx| s.toggle_side_region(tx));
    }
    rt.frame(1.0 / 60.0);
    assert!(rt.state.side_open());

    // A click in the page, then ⌘T, opens another browser tab with its address bar focused.
    rt.update(|s, _| s.composer.focused = true);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("browser-surface").expect("page surface mounted");
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert!(!rt.state.composer.focused && rt.state.side.focused, "a click in the page focuses its pane");
    let pane = rt.state.side.active_pane_focus;
    rt.key(cmd_t());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.browser.host.tabs().len(), 2);
    let second = rt.state.active_browser_tab_id().expect("new browser tab is active");
    assert_ne!(second, first);
    assert!(rt.state.browser.address.focused, "address bar focused for typing");
    assert_eq!(rt.state.side.active_pane_focus, pane);

    // From the address bar it opens a third.
    rt.key(cmd_t());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.browser.host.tabs().len(), 3);

    // Focus in the composer: ⌘T keeps doing what it did (a new session), no browser tab.
    rt.update(|s, _| {
        s.composer.focused = true;
        s.side.focused = false;
        s.browser.address.focused = false;
    });
    rt.key(cmd_t());
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.browser.host.tabs().len(), 3, "no browser tab from outside the pane");
}

#[test]
fn scroll_and_click_on_the_side_panel_page_reach_the_engine_at_zoom_1_6() {
    use browser_pane::protocol::{HostMsg, PointerBtn};
    use rune::WheelEvent;

    let mut rt = test_runtime();
    rt.update(|s, _| s.zoom = 1.6);
    let tab = rt.state.open_new_browser_tab("https://example.com/");
    if !rt.state.side_open() {
        rt.update(|s, tx| s.toggle_side_region(tx));
    }
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let sent = rt.state.browser.host.capture_sent();
    let ([sx, sy, sw, sh], _) = rt.presented("browser-surface").expect("page surface drawn");
    // A point a quarter of the way into the page, in window points.
    let (x, y) = (sx + sw * 0.25, sy + sh * 0.25);

    rt.wheel_event(WheelEvent::new(0.0, -40.0).at(x, y).precise(true));
    rt.frame(1.0 / 60.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);

    let msgs: Vec<HostMsg> = sent.try_iter().collect();
    let near = |a: f32, b: f32| (a - b).abs() < 1.0;
    let (px, py) = (sw * 0.25, sh * 0.25);
    assert!(
        msgs.iter().any(|m| matches!(m, HostMsg::Wheel { tab_id, x, y, dy, .. }
            if *tab_id == tab && near(*x, px) && near(*y, py) && *dy < 0.0)),
        "the scroll reaches the page at ({px}, {py}): {msgs:?}"
    );
    for up in [false, true] {
        assert!(
            msgs.iter().any(|m| matches!(m, HostMsg::MouseButton { tab_id, x, y, button: PointerBtn::Primary, up: u, .. }
                if *tab_id == tab && *u == up && near(*x, px) && near(*y, py))),
            "the click's {} reaches the page at ({px}, {py}): {msgs:?}",
            if up { "release" } else { "press" }
        );
    }
}

#[test]
fn a_page_shown_again_after_another_session_opened_one_is_shown_in_the_engine() {
    use browser_pane::protocol::HostMsg;

    let mut rt = test_runtime();
    let sent = rt.state.browser.host.capture_sent();
    let s1 = rt.state.active_session_id().unwrap().to_owned();
    let s2 = rt.state.tabs.iter().map(|t| t.id.clone()).find(|id| id != &s1).unwrap();
    let first = rt.state.open_new_browser_tab("https://example.com/");
    rt.update(|s, tx| s.open(&s2, tx));
    rt.frame(1.0 / 60.0);
    // The engine paints one tab at a time: opening this one hides the first.
    let second = rt.state.open_new_browser_tab("https://example.org/");
    assert_ne!(first, second);
    rt.update(|s, tx| s.open(&s1, tx));
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.active_browser_tab_id(), Some(first));
    assert!(rt.find("browser-surface").is_some(), "first page drawn");

    let msgs: Vec<HostMsg> = sent.try_iter().collect();
    let shown = msgs.iter().rev().find_map(|m| match m {
        HostMsg::FocusTab { tab_id } => Some(*tab_id),
        HostMsg::OpenTab { tab_id, focused: true, .. } => Some(*tab_id),
        _ => None,
    });
    assert_eq!(shown, Some(first), "the page on screen is the one the engine paints: {msgs:?}");
}

#[test]
fn cmd_r_in_the_side_panel_page_reloads_it() {
    use browser_pane::protocol::HostMsg;

    let mut rt = test_runtime();
    rt.update(|s, _| s.zoom = 1.6);
    let tab = rt.state.open_new_browser_tab("https://example.com/");
    if !rt.state.side_open() {
        rt.update(|s, tx| s.toggle_side_region(tx));
    }
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let sent = rt.state.browser.host.capture_sent();
    let ([sx, sy, sw, sh], _) = rt.presented("browser-surface").expect("page surface drawn");
    let (x, y) = (sx + sw * 0.5, sy + sh * 0.5);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent {
        key: Key::Command('r'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);

    let msgs: Vec<HostMsg> = sent.try_iter().collect();
    let reloads = msgs.iter().filter(|m| matches!(m, HostMsg::Reload { tab_id } if *tab_id == tab)).count();
    assert_eq!(reloads, 1, "⌘R after clicking the page reloads it once: {msgs:?}");
}

#[test]
fn after_dragging_the_divider_at_zoom_1_6_the_page_is_sized_to_its_surface_below_the_toolbar() {
    use browser_pane::protocol::HostMsg;

    let mut rt = test_runtime();
    // Wide enough at 1.6 for the panel to grow (the session keeps its minimum width).
    rt.set_viewport((2000.0, 1100.0), 2.0);
    rt.update(|s, _| s.zoom = 1.6);
    let tab = rt.state.open_new_browser_tab("https://example.com/");
    if !rt.state.side_open() {
        rt.update(|s, tx| s.toggle_side_region(tx));
    }
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let sent = rt.state.browser.host.capture_sent();
    let ([rx, ry, rw, rh], _) = rt.presented("side-resize").expect("divider drawn");
    let (x, y) = (rx + rw * 0.5, ry + rh * 0.5);
    let before = rt.presented("browser-surface").expect("page drawn").0;
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    for i in 1..=12 {
        rt.pointer_move(x - 10.0 * i as f32, y);
        rt.frame(1.0 / 60.0);
    }
    rt.pointer_up(x - 120.0, y);
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let ([_, sy, sw, sh], _) = rt.presented("browser-surface").expect("page drawn");
    assert!((sw - before[2]).abs() > 20.0, "the drag resized the page: {before:?} -> {sw}");
    let (bar, _) = rt.presented("browser-navbar").expect("toolbar drawn");
    assert!(sy >= bar[1] + bar[3] - 0.5, "the page starts below the toolbar: page y {sy}, toolbar {bar:?}");

    let msgs: Vec<HostMsg> = sent.try_iter().collect();
    let last = msgs.iter().rev().find_map(|m| match m {
        HostMsg::Resize { tab_id, width_pt, height_pt, .. } if *tab_id == tab => Some((*width_pt, *height_pt)),
        _ => None,
    });
    let (w, h) = last.unwrap_or_else(|| panic!("the drag resized the page in the engine: {msgs:?}"));
    assert!((w - sw).abs() < 1.0 && (h - sh).abs() < 1.0, "the engine draws the page at its surface's size {sw}x{sh}, got {w}x{h}");
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
    assert_eq!(
        rt.state.resolve_side_region().and_then(|r| r.active).as_deref(),
        Some("review:changes")
    );
    rt.key(KeyEvent {
        key: Key::Command('w'),
        mods: Mods { cmd: true, logo: true, ..Default::default() },
    });
    rt.frame(1.0 / 60.0);
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
    // The frame draws one texel per device pixel, so it covers the whole window to fill the page.
    let (fw, fh) = (2560u32, 1640u32);
    let mut red = Vec::with_capacity((fw * fh * 4) as usize);
    for _ in 0..fw * fh {
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
                width: fw,
                height: fh,
                format: Format::Bgra,
                color: ColorInfo::default(),
                planes: [red, Vec::new()],
            },
            2,
        )
        .unwrap();

    rt.update(|s, tx| {
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

/// A red frame with a green 20-texel square 40 texels in from its top-left corner.
fn marked_frame((w, h): (u32, u32)) -> rune::native_surface::ByteFrame {
    use rune::native_surface::{ByteFrame, ColorInfo, Format};
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let mark = (40..60).contains(&x) && (40..60).contains(&y);
            bgra.extend_from_slice(if mark { &[0, 255, 0, 255] } else { &[0, 0, 255, 255] });
        }
    }
    ByteFrame { width: w, height: h, format: Format::Bgra, color: ColorInfo::default(), planes: [bgra, Vec::new()] }
}

/// Where a `marked_frame` drew on the page at device rect `page`, measured from the page's
/// top-left corner: the green mark's first column and row, and how far the red reaches across
/// and down.
fn landed(rgba: &[u8], wp: u32, page: [u32; 4]) -> ([u32; 2], [u32; 2]) {
    let [x0, y0, w, h] = page;
    let px = |x: u32, y: u32| {
        let i = ((y * wp + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    let red = |[r, g, b]: [u8; 3]| r > 200 && g < 60 && b < 60;
    let green = |[r, g, b]: [u8; 3]| g > 200 && r < 60 && b < 60;
    let mark_x = (x0..x0 + w).find(|&x| green(px(x, y0 + 50))).expect("mark's row has the mark");
    let mark_y = (y0..y0 + h).find(|&y| green(px(x0 + 50, y))).expect("mark's column has the mark");
    let reach_x = (x0..x0 + w).rfind(|&x| red(px(x, y0 + 100))).expect("red across");
    let reach_y = (y0..y0 + h).rfind(|&y| red(px(x0 + 100, y))).expect("red down");
    ([mark_x - x0, mark_y - y0], [reach_x + 1 - x0, reach_y + 1 - y0])
}

#[test]
fn a_resized_panel_holds_the_last_frame_at_its_top_left_until_one_at_the_new_size_lands() {
    for zoom in [1.0, 1.6] {
        holds_the_last_frame_at(zoom);
    }
}

fn holds_the_last_frame_at(zoom: f32) {
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

    let (scale, size) = (2.0f32, (2000.0f32, 1200.0f32));
    let s = spec(App::fixture_session);
    let mut app = (s.init)();
    app.zoom = zoom;
    app.sidebar_open = false;
    let mut rt = Runtime::new(app, s.view, size, Env::new(true, scale));
    rt.poll = s.poll;
    rt.on_key = s.on_key;
    rt.frame(0.0);
    let tab_id = rt.state.browser.host.insert_mock_tab(
        "http://localhost:3000/",
        "Local Preview",
        browser_pane::EngineStatus::Ready,
    );
    let resize = |rt: &mut Runtime<App>, width: f32| {
        rt.update(|s, tx| {
            // The side region takes what the chat panel leaves.
            let chat = s.session_row_available(size.0 / s.zoom) - width;
            let sid = s.active_session_id().unwrap_or_default().to_owned();
            s.session_side_mut(&sid).session_width = Some(chat);
            tx.settle();
        });
        rt.frame(1.0 / 60.0);
        rt.frame(1.0 / 60.0);
    };
    rt.update(|s, tx| {
        s.register_existing_browser_tab(tab_id);
        tx.settle();
    });
    resize(&mut rt, 480.0);

    let gpu = Gpu::headless();
    let (wp, hp) = ((size.0 * scale) as u32, (size.1 * scale) as u32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, wp, hp);
    let mut shoot = |rt: &mut Runtime<App>| {
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (wp, hp));
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (wp, hp));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, rt.clear_color());
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        target.read(&gpu.device)
    };
    // The page in device pixels, and the frame size the engine is asked for: the same, give or
    // take the engine's whole-point view rect.
    let page = |rt: &Runtime<App>| {
        let ([x, y, w, h], _) = rt.presented("browser-surface").expect("page laid out");
        let d = [x, y, w, h].map(|v| (v * scale).round() as u32);
        let f = rt.state.browser.host.tab(tab_id).unwrap().frame_px();
        assert!(
            f.0.abs_diff(d[2]) <= 1 && f.1.abs_diff(d[3]) <= 1,
            "zoom {zoom}: frames of {f:?} px for a page of {:?} device px",
            [d[2], d[3]]
        );
        (d, f)
    };
    let near = |a: [u32; 2], b: [u32; 2], tol: u32| a[0].abs_diff(b[0]) <= tol && a[1].abs_diff(b[1]) <= tol;
    let push = |rt: &mut Runtime<App>, f: (u32, u32), id: u64| {
        rt.state.browser.host.tab(tab_id).unwrap().surface.push_bytes(marked_frame(f), id).unwrap();
        rt.frame(1.0 / 60.0);
    };

    // The mock's 640×360 first frame covers only its own pixels; below it is the page's fill.
    let (d, f) = page(&rt);
    assert!(d[3] > 420, "zoom {zoom}: page {d:?} is tall enough to show the fill under 360 px");
    let rgba = shoot(&mut rt);
    let fill_at = |rgba: &[u8], x: u32, y: u32| {
        let i = ((y * wp + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    };
    let fill = fill_at(&rgba, d[0] + 100, d[1] + 400);

    // A frame at this size fills the page from its corner.
    push(&mut rt, f, 10);
    let (mark, reach) = landed(&shoot(&mut rt), wp, d);
    assert!(near(mark, [40, 40], 1), "zoom {zoom}: mark at {mark:?}");
    assert!(near(reach, [d[2].min(f.0), d[3].min(f.1)], 2), "zoom {zoom}: reach {reach:?} on {d:?}");

    // Wider, with no new frame: the frame keeps its corner and size, and the fill shows beside it.
    resize(&mut rt, 600.0);
    let (wide, _) = page(&rt);
    assert!(wide[2] > f.0 + 40, "zoom {zoom}: page {wide:?} grew past the {f:?} frame");
    let rgba = shoot(&mut rt);
    let (mark, reach) = landed(&rgba, wp, wide);
    assert!(near(mark, [40, 40], 1), "zoom {zoom}: wider mark at {mark:?}");
    assert!(near(reach, [f.0, wide[3].min(f.1)], 2), "zoom {zoom}: wider reach {reach:?}, frame {f:?}");
    let gap = fill_at(&rgba, wide[0] + f.0 + 20, wide[1] + 100);
    assert!(gap.iter().zip(fill).all(|(a, b)| a.abs_diff(b) <= 2), "zoom {zoom}: gap {gap:?}, page fill {fill:?}");

    // Narrower, with no new frame: the right edge is cropped, nothing scales or moves.
    resize(&mut rt, 360.0);
    let (narrow, f2) = page(&rt);
    assert!(narrow[2] + 40 < f.0, "zoom {zoom}: page {narrow:?} shrank under the {f:?} frame");
    let (mark, reach) = landed(&shoot(&mut rt), wp, narrow);
    assert!(near(mark, [40, 40], 1), "zoom {zoom}: narrower mark at {mark:?}");
    assert!(near(reach, [narrow[2], narrow[3].min(f.1)], 2), "zoom {zoom}: narrower reach {reach:?}");

    // The frame at the new size fills the page.
    push(&mut rt, f2, 11);
    let (mark, reach) = landed(&shoot(&mut rt), wp, narrow);
    assert!(near(mark, [40, 40], 1), "zoom {zoom}: new frame's mark at {mark:?}");
    assert!(near(reach, [narrow[2], narrow[3]], 2), "zoom {zoom}: new frame reaches {reach:?} on {narrow:?}");
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

#[test]
fn download_prompt_odometer_progress_and_retry_states_render_cleanly() {
    let s = spec(|| {
        let mut a = App::empty();
        a.load_fixture(crate::FIXTURE);
        crate::open_scene(&mut a);
        a.finish_stream();
        a.sidebar_open = false;
        a
    });
    let size = (1600.0, 960.0);
    let scale = 2.0f32;
    let mut rt = rune::App::new(s)
        .chrome(rune::Chrome {
            full_size_content: true,
            traffic_lights: Some((25.0, 25.0)),
            drag_regions: true,
        })
        .headless_runtime(size, Env::new(true, scale));
    rt.state.zoom = 1.6;
    rt.frame(0.0);

    // 1. Prompt state (`NotInstalled`)
    rt.update(|app, tx| {
        let id = app.browser.host.insert_mock_tab(
            "http://localhost:3000/",
            "Local Preview",
            browser_pane::EngineStatus::NotInstalled,
        );
        app.register_existing_browser_tab(id);
        tx.settle();
        app.sync_browser_address_now();
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("browser-download-prompt").is_some());
    assert!(rt.find("browser-download-btn").is_some());

    let stills_dir = std::env::var("OPENCODE_DESKTOP_INSTALL_STILLS_DIR").ok();
    let mut gpu_ctx = stills_dir.as_ref().map(|_| {
        use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        let gpu = Gpu::headless();
        let (wp, hp) = ((size.0 * scale) as u32, (size.1 * scale) as u32);
        let renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, wp, hp);
        (gpu, renderer, target, wp, hp)
    });
    let mut maybe_snap = |rt: &mut Runtime<App>, name: &str| {
        if let (Some(dir), Some((gpu, renderer, target, wp, hp))) =
            (stills_dir.as_deref(), gpu_ctx.as_mut())
        {
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (*wp, *hp));
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (*wp, *hp));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, rt.clear_color());
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            let rgba = target.read(&gpu.device);
            let out = std::path::Path::new(dir).join(name);
            rune::render::write_png(&out, *wp, *hp, &rgba).unwrap();
        }
    };
    maybe_snap(&mut rt, "browser-install-prompt-zoom-1.6.png");

    // 2. Downloading progress state with rolling odometer (`Downloading`)
    rt.update(|app, _| {
        app.browser.host.set_mock_status(browser_pane::EngineStatus::Downloading {
            downloaded_bytes: 58_400_000,
            total_bytes: Some(128_352_035),
        });
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.find("browser-installing").is_some());
    assert!(rt.find("browser-dl-mb").is_some());
    assert!(rt.find("browser-dl-fill").is_some());
    maybe_snap(&mut rt, "browser-install-progress-zoom-1.6.png");

    // 3. Failure state (`Failed`) with plain message + Retry
    rt.update(|app, _| {
        app.browser.host.set_mock_status(browser_pane::EngineStatus::Failed(
            "Couldn't download the browser. Check your connection and try again.".into(),
        ));
    });
    rt.frame(1.0 / 60.0);
    assert!(rt.find("browser-download-failed").is_some());
    assert!(rt.find("browser-retry-btn").is_some());
    maybe_snap(&mut rt, "browser-install-failed-zoom-1.6.png");
}
