use super::*;

fn user(id: &str, text: &str) -> Message {
    Message::User { id: id.into(), text: text.into() }
}

#[test]
fn opening_does_not_fetch_images_from_older_history() {
    let mut attach = Attach::default();
    attach.set_fetch(Arc::new(|_| panic!("older image fetched before its row mounted")));
    let file = Attachment { mime:"image/png".into(), media:Some("b".repeat(64)), ..Default::default() };
    opencode_kit::model::set_user_files("older_open_image", vec![file]);
    let mut messages = vec![user("older_open_image", "Old screenshot")];
    messages.extend((0..crate::session_pages::LATEST).map(|i| user(&format!("newer_open_{i}"), "Recent text")));
    assert!(attach.transcript_ready(&Transcript { session:"history_open".into(), messages }));
    assert!(attach.image_handles().is_empty());
}

#[test]
fn the_servers_copy_replaces_the_echo() {
    let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "older"), user("msg_5", "hi")] };
    let mut echoes = vec![("s".to_owned(), "msg_5".to_owned(), "hi".to_owned())];
    let fresh = vec![user("msg_1", "older"), user("msg_5", "hi")];
    reconcile(&mut echoes, &mut t, &fresh);
    t.merge(fresh);
    assert!(echoes.is_empty());
    assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_5"]);
}

#[test]
fn an_echo_under_another_id_drops_when_the_servers_lands() {
    let mut t = Transcript { session: "s".into(), messages: vec![user("msg_1", "hi"), user("msg_9", "hi")] };
    let mut echoes = vec![("s".to_owned(), "msg_9".to_owned(), "hi".to_owned())];
    // An unrelated page first: the echo stays.
    reconcile(&mut echoes, &mut t, &[user("msg_1", "hi")]);
    assert_eq!(echoes.len(), 1);
    let fresh = vec![user("msg_1", "hi"), user("msg_7", "hi")];
    reconcile(&mut echoes, &mut t, &fresh);
    t.merge(fresh);
    assert!(echoes.is_empty());
    assert_eq!(t.messages.iter().map(Message::id).collect::<Vec<_>>(), ["msg_1", "msg_7"]);
}

#[test]
fn a_live_send_shows_one_message_under_the_id_it_sends() {
    use opencode_kit::client::Write;
    let mut app = App::empty();
    let writes = app.fake_writes();
    app.route = crate::Route::Session("ses_a".into());
    app.composer = rune::TextEdit::new("look at this");
    app.attach.add(Attachment { mime: "image/png".into(), name: "a.png".into(), bytes: Arc::from(&b"ab"[..]), media: None });
    app.send(&mut Tx::default());
    let Ok(Write::Send { session, prompt }) = writes.try_recv() else { panic!("no prompt") };
    let id = prompt.id.clone().unwrap();
    assert_eq!((session.as_str(), prompt.text.as_str(), prompt.files.len()), ("ses_a", "look at this", 1));
    assert!(app.attach.files.is_empty());
    assert_eq!(opencode_kit::model::user_files(&id).len(), 1, "the echo shows the image");
    // The server's copy comes back under the same id: still one message.
    let t = app.transcripts.get_mut("ses_a").unwrap();
    let fresh = vec![user(&id, "look at this")];
    reconcile(&mut app.attach.echoes, t, &fresh);
    t.merge(fresh);
    assert_eq!(t.messages.len(), 1);
    assert!(app.attach.echoes.is_empty());
}

#[test]
fn a_drop_anywhere_on_the_page_attaches_under_a_fading_overlay() {
    let mut app = App::fixture_session();
    app.zoom = 1.4;
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let dir = std::env::temp_dir().join(format!("oc-desktop-drop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shot = dir.join("CleanShot 2026-10-26 at 8.00.42 PM@2x.png");
    std::fs::write(&shot, png(4, 4)).unwrap();
    let notes = dir.join("notes.txt");
    std::fs::write(&notes, "hello").unwrap();
    let panel = rect(&rt, "panel").unwrap();
    // Over the timeline, well above the composer.
    let at = (panel[0] + panel[2] / 2.0, panel[1] + panel[3] * 0.3);
    rt.drag_files(vec![shot.clone(), notes.clone()], Some(at));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.drop_hovered().as_deref(), Some("panel"));
    let overlay = rect(&rt, "drop-overlay").expect("the overlay shows");
    assert!(overlay.iter().zip(panel).all(|(a, b)| (a - b).abs() < 0.5), "it covers the page: {overlay:?} vs {panel:?}");
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Drop images, PDFs, or files to add")), "Desktop's label");
    rt.drop_files(vec![shot.clone(), notes.clone()], Some(at));
    rt.frame(1.0 / 60.0);
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!(rt.state.attach.paths.len(), 1, "the files land at once");
    // Then it fades out over Desktop's 200 ms.
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    let layer = |rt: &rune::Runtime<App>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("drop-layer")).unwrap().opacity;
    assert!(layer(&rt) > 0.05 && layer(&rt) < 0.95, "fading: {}", layer(&rt));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(layer(&rt), 0.0);
    assert!(rect(&rt, "drop-overlay").is_none(), "gone");
    // The image travels inline; the text file reaches the model by path, as a file chip.
    assert_eq!(rt.state.attach.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["CleanShot 2026-10-26 at 8.00.42 PM@2x.png"]);
    let paths: Vec<_> = rt.state.attach.paths.iter().map(|f| (f.name.as_str(), f.mime.as_str())).collect();
    assert_eq!(paths, [("notes.txt", "text/plain")]);
    assert!(rect(&rt, &format!("path-{}", notes.display())).is_some(), "the file shows as a chip");
}

#[test]
fn dropped_images_become_data_urls() {
    let dir = std::env::temp_dir().join(format!("oc-desktop-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Shot.PNG");
    std::fs::write(&path, b"ab").unwrap();
    let a = from_path(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert_eq!((a.mime.as_str(), a.name.as_str()), ("image/png", "Shot.PNG"));
    assert_eq!(inputs(&[a]), [FileInput { uri: "data:image/png;base64,YWI=".into(), name: "Shot.PNG".into(), ..Default::default() }]);
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&vec![200; (w * h * 4) as usize]).unwrap();
    out
}

fn rect(rt: &rune::Runtime<App>, path: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.path.ends_with(path) && !e.exiting).map(|e| e.rect)
}

/// Every edge heads one way from `from` to `to`, never more than a fifth of the way in one
/// frame, and lands.
fn flies(frames: &[[f32; 4]], from: [f32; 4], to: [f32; 4]) {
    let mut last = from;
    for (n, f) in frames.iter().enumerate() {
        for k in 0..4 {
            let (travel, step) = (to[k] - from[k], f[k] - last[k]);
            // Once landed, separately rounded layout edges can differ by 1/32 pt.
            let settled = (last[k] - to[k]).abs() <= 1.0 / 16.0 && (f[k] - to[k]).abs() <= 1.0 / 16.0;
            assert!(settled || step * travel.signum() >= -0.01, "frame {n} edge {k} turns back: {last:?} -> {f:?}");
            assert!(step.abs() <= travel.abs() * 0.2 + 0.5, "frame {n} edge {k} jumps {step} of {travel}");
        }
        last = *f;
    }
    for k in 0..4 {
        assert!((last[k] - to[k]).abs() < 0.5, "lands: {last:?} vs {to:?}");
    }
}

#[test]
fn a_thumbnail_grows_into_the_viewer_and_escape_shrinks_it_back() {
    use rune::{Key, KeyEvent, Mods};
    let mut rt = crate::fixture_runtime();
    let a = Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: png(1600, 1000).into(), media: None };
    let added = a.clone();
    rt.update(move |s, _| s.attach.add(added));
    rt.state.attach.image(&a).unwrap().wait().unwrap();
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let chip = &format!("chip-{}/attachment-thumb", identity(&a));
    let thumb = rect(&rt, chip).unwrap();
    let (x, y) = rt.find(chip).unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    let mut open = Vec::new();
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
        open.push(rect(&rt, "/image-viewer").expect("the viewer is open"));
    }
    assert!(rect(&rt, chip).is_none(), "the thumbnail flew out of the chip");
    let window = rect(&rt, "/window").unwrap();
    let large = shown(Some([1600, 1000]), window[2], window[3]);
    assert!(large[2] > 1000.0, "fitted to the window: {large:?}");
    flies(&open, thumb, large);
    // 0.25 s: most of the way by 15 frames.
    assert!((open[14][2] - large[2]).abs() < (large[2] - thumb[2]) * 0.1, "still far at 0.25 s: {:?}", open[14]);
    assert!(rect(&rt, "/image-viewer-scrim").is_some());

    rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() });
    let mut close = Vec::new();
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
        close.push(rect(&rt, chip).expect("the thumbnail is back"));
    }
    assert!(rt.state.attach.viewing.is_none());
    assert!(rect(&rt, "/image-viewer").is_none());
    assert!(matches!(rt.state.route, crate::Route::Session(_)), "Escape only closes the viewer");
    flies(&close, large, thumb);
}

#[test]
fn a_sent_image_opens_from_the_transcript_and_a_click_outside_closes_it() {
    let mut app = App::empty();
    let _writes = app.fake_writes();
    app.route = crate::Route::Session("ses_a".into());
    app.composer = rune::TextEdit::new("look");
    let a = Attachment { mime: "image/png".into(), name: "sent.png".into(), bytes: png(800, 400).into(), media: None };
    app.attach.add(a.clone());
    app.send(&mut Tx::default());
    let mut rt = app.into_runtime();
    rt.state.attach.image(&a).unwrap().wait().unwrap();
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    let thumb = rect(&rt, "/img-0").expect("the sent image shows in the transcript");
    let (x, y) = rt.find("img-0").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    let mut open = Vec::new();
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
        open.push(rect(&rt, "/image-viewer").unwrap());
    }
    let window = rect(&rt, "/window").unwrap();
    let large = shown(Some([800, 400]), window[2], window[3]);
    assert_eq!(large[2..], [800.0, 400.0], "a pixel a point when it fits");
    flies(&open, thumb, large);
    // A click on the image keeps it open; one on the scrim closes it.
    rt.pointer_down(window[2] / 2.0, window[3] / 2.0);
    rt.pointer_up(window[2] / 2.0, window[3] / 2.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.attach.viewing.is_some());
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    let mut close = Vec::new();
    for _ in 0..45 {
        rt.frame(1.0 / 60.0);
        close.push(rect(&rt, "/img-0").unwrap());
    }
    assert!(rt.state.attach.viewing.is_none());
    flies(&close, large, thumb);
}

fn media(hash: &str, name: &str) -> Attachment {
    Attachment { mime: "image/png".into(), name: name.into(), media: Some(hash.into()), ..Default::default() }
}

/// A fetcher that counts its calls and answers from `bytes` (Err when None).
fn stub(bytes: Option<Vec<u8>>) -> (Fetch, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let fetch: Fetch = Arc::new(move |hash: &str| {
        seen.lock().unwrap().push(hash.to_owned());
        bytes.clone().ok_or_else(|| format!("GET /api/experimental/media/{hash}: 404"))
    });
    (fetch, calls)
}

/// [`stub`] whose fetch holds its answer until the sender sends, so a test sees `Loading` for
/// as long as it needs, however fast the fetch thread runs.
fn gated(bytes: Option<Vec<u8>>) -> (Fetch, Arc<Mutex<Vec<String>>>, std::sync::mpsc::Sender<()>) {
    let (fetch, calls) = stub(bytes);
    let (open, gate) = std::sync::mpsc::channel();
    let gate = Mutex::new(gate);
    let held: Fetch = Arc::new(move |hash: &str| {
        let _ = gate.lock().unwrap().recv();
        fetch(hash)
    });
    (held, calls, open)
}

fn settle(a: &Attach, f: &Attachment) -> Fetched {
    for _ in 0..500 {
        match a.fetched(f) {
            Some(Fetched::Loading) | None => std::thread::sleep(std::time::Duration::from_millis(2)),
            Some(done) => return done,
        }
    }
    panic!("the fetch never finished")
}

#[test]
fn a_ref_fetches_once_by_hash_and_decodes() {
    let (fetch, calls, land) = gated(Some(png(6, 4)));
    let mut a = Attach::default();
    a.set_fetch(fetch);
    let hash = "c".repeat(64);
    let (first, again) = (media(&hash, "shot.png"), media(&hash, "same.png"));
    assert!(a.image(&first).is_none(), "nothing until the bytes land");
    assert_eq!(a.fetched(&first), Some(Fetched::Loading));
    land.send(()).unwrap();
    assert!(matches!(settle(&a, &first), Fetched::Ready(_)));
    assert_eq!(a.image(&first).unwrap().wait().unwrap(), [6, 4]);
    // The pixels are the cache's now; the same hash elsewhere is a hit.
    assert_eq!(a.fetched(&first), Some(Fetched::Decoded));
    assert_eq!(a.image(&again).unwrap().id(), a.image(&first).unwrap().id());
    assert_eq!(*calls.lock().unwrap(), [hash]);
    // An inline file never fetches.
    let inline = Attachment { mime: "image/png".into(), name: "inline.png".into(), bytes: png(2, 2).into(), media: None };
    assert_eq!(a.image(&inline).unwrap().wait().unwrap(), [2, 2]);
    assert_eq!((a.fetched(&inline), calls.lock().unwrap().len()), (None, 1));
}

#[test]
fn a_failed_fetch_shows_the_name_and_isnt_retried_each_frame() {
    let (fetch, calls) = stub(None);
    let mut a = Attach::default();
    a.set_fetch(fetch);
    let f = media(&"d".repeat(64), "gone.png");
    assert!(a.image(&f).is_none());
    assert!(matches!(settle(&a, &f), Fetched::Failed(e) if e.contains("404")));
    assert!(a.image(&f).is_none());
    assert_eq!(calls.lock().unwrap().len(), 1);
    // Without a server (fixtures), a ref is a named chip at once.
    assert!(matches!(Attach::default().media.get("e", None), Fetched::Failed(_)));
}

const GALLERY: &str = "msg_gallery_test";

fn gallery_view(s: &App, _: &mut Cx<App>) -> El {
    let p = crate::palette::pal(true);
    col().key("root").children(gallery(s, p, "images", &opencode_kit::model::user_files(GALLERY), true))
}

#[test]
fn the_transcript_holds_a_box_for_a_ref_then_the_image_and_shows_the_unavailable_placeholder() {
    let (fetch, _, land) = gated(Some(png(400, 200)));
    let mut app = App::empty();
    app.attach.set_fetch(fetch);
    let hash = "f".repeat(64);
    let placeholder = Attachment {
        mime: "text/plain".into(),
        name: "gone.png".into(),
        bytes: Arc::from(&b"[media unavailable: gone.png (image/png)]"[..]),
        media: None,
    };
    opencode_kit::model::set_user_files(GALLERY, vec![media(&hash, "shot.png"), placeholder]);
    let mut rt = rune::Runtime::new(app, gallery_view, (800.0, 600.0), rune::Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rect(&rt, "img-0-loading").map(|r| [r[2], r[3]]), Some([160.0, 120.0]));
    let unavailable = rt.elements().into_iter().filter_map(|e| e.text).find(|t| t.starts_with("[media unavailable"));
    assert_eq!(unavailable.as_deref(), Some("[media unavailable: gone.png (image/png)]"));
    let first = media(&hash, "shot.png");
    let _ = crate::poll(&mut rt.state, 0.0, &mut Tx::default());
    assert!(!crate::poll(&mut rt.state, 0.0, &mut Tx::default()), "idle while the fetch runs");
    land.send(()).unwrap();
    settle(&rt.state.attach, &first);
    // The worker's wake runs the app's poll, which must report the bytes (t1378: the loading
    // box stayed until a click).
    assert!(crate::poll(&mut rt.state, 1.0 / 60.0, &mut Tx::default()), "a landed ref changes the view");
    rt.update(|_, _| {});
    rt.frame(1.0 / 60.0);
    rt.state.attach.image(&first).unwrap().wait().unwrap();
    rt.update(|_, _| {});
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert!(rect(&rt, "img-0-loading").is_none());
    assert_eq!(rect(&rt, "/img-0").map(|r| [r[2], r[3]]), Some([200.0, 100.0]), "a ref takes its image's own shape");
    opencode_kit::model::set_user_files(GALLERY, Vec::new());
}

#[test]
fn a_ref_withdrawn_from_the_queue_goes_back_as_its_uri() {
    use opencode_kit::client::Write;
    use opencode_kit::model::Delivery;
    let mut app = App::empty();
    let writes = app.fake_writes();
    app.route = crate::Route::Session("ses_q".into());
    let hash = "a1".repeat(32);
    // As an opted-in `session.inbox.list` returns a queued prompt with a ref.
    let listed = serde_json::json!({"sessionID": "ses_q", "items": [{"id": "msg_queued", "type": "user", "delivery": "queue",
        "payload": {"text": "again", "files": [{"uri": format!("media:{hash}"), "mime": "image/png", "name": "again.png", "bytes": 3}]}}]});
    app.apply_inbox("session.inbox.listed", &listed, "ses_q");
    app.undo_queued("ses_q", "msg_queued");
    let Ok(Write::Cancel { item, .. }) = writes.try_recv() else { panic!("no cancel") };
    assert_eq!(item, "msg_queued");
    assert_eq!(app.attach.files, [media(&hash, "again.png")]);
    app.composer = rune::TextEdit::new("again");
    app.send(&mut Tx::default());
    let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no prompt") };
    assert_eq!(
        prompt.files,
        [FileInput { uri: format!("media:{hash}"), name: "again.png".into(), mime: Some("image/png".into()), mention: None }]
    );
    // Queued again, it keeps the ref.
    app.attach.add(media(&hash, "again.png"));
    app.admit("ses_q", opencode_kit::client::Prompt { files: inputs(&app.attach.files), ..Default::default() }, Delivery::Queue);
    let Ok(Write::Send { prompt, .. }) = writes.try_recv() else { panic!("no queued prompt") };
    assert_eq!((prompt.files[0].uri.as_str(), prompt.delivery), (format!("media:{hash}").as_str(), Some(Delivery::Queue)));
}

#[test]
fn fits_in_the_box() {
    assert_eq!(fit(1280.0, 800.0, 320.0, 240.0), (320.0, 200.0));
    assert_eq!(fit(100.0, 50.0, 320.0, 240.0), (100.0, 50.0));
}

#[test]
fn prefetched_pixels_and_header_dimensions_survive_the_first_paint() {
    use opencode_kit::model::{Assistant, Part, ToolStatus};
    let f = Attachment { mime: "image/png".into(), name: "wide.png".into(), bytes: png(800, 200).into(), media: None };
    let messages = vec![Message::Assistant(Assistant { parts: vec![Part::Tool {
        id: "shot".into(), name: "read".into(), title: "image".into(), status: ToolStatus::Completed,
        input: serde_json::Value::Null, metadata: serde_json::Value::Null, files: vec![f.clone()],
    }], ..Default::default() })];
    let (wake, woke) = channel();
    let cache = ImageCache::new(Arc::new(move || { let _ = wake.send(()); }));
    let prepared = std::thread::spawn(move || prepare_images(&messages, &cache)).join().unwrap();
    assert_eq!(prepared.len(), 1);
    let attach = Attach::default();
    attach.adopt(&prepared);
    let before = attach.thumbnail_size(&f);
    assert_eq!(before, (320.0, 80.0));
    prepared[0].wait();
    woke.recv_timeout(std::time::Duration::from_secs(5)).expect("prefetched decode wakes without input");
    assert_eq!(attach.image(&f).unwrap().id(), prepared[0].image.id());
    assert!(attach.image(&f).unwrap().is_ready(), "the click adopts ready pixels rather than starting another decode");
    assert_eq!(attach.thumbnail_size(&f), before, "no size change when pixels land");
}

#[test]
fn files_classify_as_desktop_does() {
    let p = Path::new;
    assert_eq!(mime_for(p("a.PNG"), b""), "image/png");
    assert_eq!(mime_for(p("a.pdf"), b"%PDF"), "application/pdf");
    assert_eq!(mime_for(p("Makefile"), b"all:\n\tcargo build\n"), "text/plain");
    assert_eq!(mime_for(p("a.bin"), b"ab\0cd"), "application/octet-stream");
    assert_eq!(mime_for(p("a.dat"), &[1, 2, 3, b'a']), "application/octet-stream");
    assert_eq!(mime_for(p("empty"), b""), "text/plain");
    let r = |path: &str| PathAttachment { name: String::new(), mime: String::new(), path: path.into() };
    assert_eq!(path_lines("  ", &[r("/a"), r("/b")]), "Attached file: `/a`\nAttached file: `/b`");
    assert_eq!(path_lines("hi", &[]), "hi");
}
