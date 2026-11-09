use super::*;
use crate::{App, Route, fixture_ids::{MAIN, CHILD}};
use rune::Tx;

fn runtime(menu: bool) -> rune::Runtime<App> {
    let mut app = App::fixture_session();
    app.zoom = 1.6;
    // Keep only the call, so it is on screen even at the user's larger zoom.
    app.transcripts.get_mut(MAIN).unwrap().messages.retain(|m| matches!(m,
        Message::Assistant(a) if a.parts.iter().any(|p| matches!(p, model::Part::Tool { name, .. } if name == "subagent"))));
    app.transcripts.remove(CHILD);
    if menu {
        app.transcripts.get_mut(MAIN).unwrap().messages.clear();
        app.running.insert(CHILD.into());
        crate::subagents::shells::set_open(&mut app, true);
    }
    app.pages.set(Pages::fixture(None).0);
    let mut rt = app.into_runtime();
    for _ in 0..4 { rt.frame(1.0 / 60.0); }
    rt
}

fn ready(rt: &mut rune::Runtime<App>) {
    let child = App::fixture_session().transcripts.remove(CHILD).unwrap();
    let page = Arc::new(Page { key: rt.state.page_key(CHILD), messages: child.messages, cursor: None,
        cached: false, history: false, images: vec![], bytes: 0, read: Default::default() });
    let pool = rt.state.pages.pool.as_ref().unwrap();
    let mut q = pool.shared.0.lock().unwrap();
    assert_eq!(q.jobs.iter().filter(|j| j.key.id == CHILD).count(), 1, "a visible child is requested once");
    assert!(q.jobs.iter().all(|j| !j.foreground && j.limit == LATEST && j.cursor.is_none()));
    q.jobs.clear();
    q.ready.push_back(page);
}

#[test]
fn card_and_menu_prefetch_before_a_press_and_open_on_its_first_frame() {
    for menu in [false, true] {
        let mut rt = runtime(menu);
        ready(&mut rt);
        let key = if menu { format!("sub-{CHILD}") } else { "agent-line".into() };
        let (x, y) = rt.find(&key).unwrap();
        rt.pointer_move(x, y);
        rt.frame(1.0 / 60.0);
        assert!(rt.state.pages.pool.as_ref().unwrap().shared.0.lock().unwrap().jobs.is_empty(), "hover reuses the same updated page");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.route, Route::Session(CHILD.into()));
        assert!(rt.state.opening.is_none());
        assert!(!rt.state.transcripts[CHILD].messages.is_empty());
        assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Loading messages…")));
        assert_eq!(rt.state.pages.pool.as_ref().unwrap().shared.0.lock().unwrap().jobs.iter().filter(|j| j.foreground).count(), 1, "opening still refreshes from the server");
    }
}

#[test]
fn updated_child_prefetches_again_without_rewinding_a_live_transcript() {
    let mut rt = runtime(false);
    ready(&mut rt);
    rt.update(|s, _| {
        s.sessions.iter_mut().find(|s| s.id == CHILD).unwrap().updated += 1;
        s.prefetch_session(CHILD);
    });
    let q = rt.state.pages.pool.as_ref().unwrap().shared.0.lock().unwrap();
    assert_eq!(q.jobs.len(), 1);
    assert_eq!(q.jobs[0].key, rt.state.page_key(CHILD));
    drop(q);
    rt.state.transcripts.insert(CHILD.into(), model::Transcript { session: CHILD.into(), messages: vec![Message::User { id: "live_prompt".into(), text: "New work".into() }] });
    rt.state.open_child(MAIN, CHILD, "Review", "general", &mut Tx::default());
    assert!(matches!(&rt.state.transcripts[CHILD].messages[0], Message::User { text, .. } if text == "New work"));
}

/// Two fixture frames at 200 ms after the press, with a 300 ms server response:
/// before, the read begins on press; after, the mounted card already fetched it.
#[test]
#[ignore = "headless GPU stills; run through a render lease with SESSION_OPEN_STILLS set"]
fn session_open_stills() {
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
    let out = PathBuf::from(std::env::var("SESSION_OPEN_STILLS").unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let gpu = Gpu::headless();
    let format = OFFSCREEN_FORMAT.remove_srgb_suffix();
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(true);
    let target = Offscreen::with_format(&gpu.device, 1280, 820, format);
    for warm in [false, true] {
        let mut rt = runtime(false);
        if warm { ready(&mut rt); }
        rt.update(|s, tx| s.open_child(MAIN, CHILD, "Review", "general", tx));
        rt.frame(0.2);
        rt.frame(0.0);
        assert_eq!(rt.state.loading_session(CHILD), !warm);
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (1280, 820));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, rt.clear_color());
        target.copy(&mut enc);
        renderer.submit(&gpu.queue, enc);
        let image = image::RgbaImage::from_raw(1280, 820, target.read(&gpu.device)).unwrap();
        let crop = image::imageops::crop_imm(&image, 430, 14, 836, 550).to_image();
        write_png(&out.join(if warm { "after.png" } else { "before.png" }), 836, 550, crop.as_raw()).unwrap();
    }
}
