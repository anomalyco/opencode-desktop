//! Drive real key dispatch while the page reader is deliberately withheld.
use super::*;
use crate::{App, Route};
use rune::{Key as RuneKey, KeyEvent, Mods};

fn long_page(id: &str, history: bool) -> Arc<Page> {
    Arc::new(Page {
        key: Key { id: id.into(), updated: 0 },
        messages: (0..80).map(|i| Message::User {
            id: format!("msg_{history}_{i:03}"),
            text: format!("Discussion {i}: {}", "Keep the editor responsive while history loads. ".repeat(20)),
        }).collect(),
        cursor: None, cached: false, history, images: vec![], bytes: 0, read: Default::default(),
    })
}

fn runtime(home: bool) -> (rune::Runtime<App>, Sender<Result<Arc<Page>, (Key, String)>>) {
    let mut app = App::fixture_session();
    app.zoom = 1.6;
    if home { app.route = Route::Home; }
    app.composer.replace_text(String::new());
    app.composer.focused = false;
    let (pages, send) = Pages::fixture(None);
    app.pages.set(pages);
    let mut rt = app.into_runtime();
    rt.frame(0.0);
    rt.update(|s, tx| s.open("ses_typing", tx));
    rt.frame(1.0 / 60.0);
    (rt, send)
}

#[test]
fn typing_during_load_keeps_every_key_from_the_first_frame() {
    for home in [false, true] {
        let (mut rt, send) = runtime(home);
        let mut accepted = 0;
        let mut first = None;
        let mut times = Vec::new();
        // Two seconds of loading, then a long latest page and an older page.
        for frame in 0..132 {
            if frame == 120 { send.send(Ok(long_page("ses_typing", false))).unwrap(); }
            if frame == 126 { send.send(Ok(long_page("ses_typing", true))).unwrap(); }
            let before = rt.state.composer.text.len();
            let start = Instant::now();
            rt.key(KeyEvent { key: RuneKey::Text("x".into()), mods: Mods::default() });
            rt.frame(1.0 / 60.0);
            if rt.state.composer.text.len() == before + 1 {
                accepted += 1;
                first.get_or_insert(frame);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!("METRIC typing home={home} accepted={accepted}/132 first_key_frame={first:?} key_to_scene_p50_ms={:.3} key_to_scene_p95_ms={:.3}", times[times.len()/2], times[times.len()*95/100]);
        assert_eq!(accepted, 132, "keys must work before the page arrives, including the first frame");
        assert_eq!(rt.state.composer.text, "x".repeat(132), "page delivery must not reset the draft");
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("prompt") && e.caret.is_some()), "the typed editor is in the frame");
    }
}

#[test]
fn typing_during_load_follows_the_latest_click_without_exposing_a_partial_page() {
    let (mut rt, _) = runtime(false);
    let previous = rt.state.opening.as_ref().unwrap().from.clone();
    rt.update(|s, tx| s.open("ses_typing", tx));
    assert!(rt.state.opening.is_some(), "reopening the loading session keeps its image gate");
    rt.update(|s, tx| s.open("ses_next", tx));
    assert_eq!(rt.state.opening.as_ref().unwrap().from, previous);
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent { key: RuneKey::Text("new draft".into()), mods: Mods::default() });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.route, Route::Session("ses_next".into()));
    assert_eq!(rt.state.composer.text, "new draft");
}

/// Actual offscreen GPU pixels, not a display/compositor latency claim. Run base and head
/// in one worker lease. Virtual time withholds the long page for two seconds in both runs.
#[test]
#[ignore = "headless GPU comparison; TYPING_PAINT_OUT and TYPING_PAINT_VARIANT required"]
fn typing_during_load_paint() {
    use std::io::Write as _;
    use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer, write_png};
    let out = PathBuf::from(std::env::var("TYPING_PAINT_OUT").unwrap());
    let variant = std::env::var("TYPING_PAINT_VARIANT").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let gpu = Gpu::headless();
    let format = OFFSCREEN_FORMAT.remove_srgb_suffix();
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(true);
    let target = Offscreen::with_format(&gpu.device, 1280, 820, format);
    let mut samples = Vec::new();
    let mut lost = 0;
    for run in 0..8 {
        let (mut rt, send) = runtime(false);
        let mut video = (run == 1).then(|| std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "rawvideo", "-pixel_format", "rgba", "-video_size", "1280x820", "-framerate", "60", "-i", "pipe:0", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(out.join(format!("{variant}.mp4"))).stdin(std::process::Stdio::piped()).spawn().unwrap());
        for frame in 0..132 {
            if frame == 120 { send.send(Ok(long_page("ses_typing", false))).unwrap(); }
            if frame == 126 { send.send(Ok(long_page("ses_typing", true))).unwrap(); }
            let before = rt.state.composer.text.len();
            let start = Instant::now();
            rt.key(KeyEvent { key: RuneKey::Text("x".into()), mods: Mods::default() });
            rt.frame(1.0 / 60.0);
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (1280, 820));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, rt.clear_color());
            target.copy(&mut enc);
            renderer.submit(&gpu.queue, enc);
            let pixels = target.read(&gpu.device);
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if let Some(video) = &mut video { video.stdin.as_mut().unwrap().write_all(&pixels).unwrap(); }
            if run > 0 && frame < 120 {
                if rt.state.composer.text.len() == before + 1 { samples.push(elapsed); } else { lost += 1; }
            }
            if run == 1 && frame == 12 {
                write_png(&out.join(format!("{variant}.png")), 1280, 820, &pixels).unwrap();
            }
        }
        if let Some(mut video) = video { drop(video.stdin.take()); assert!(video.wait().unwrap().success()); }
    }
    samples.sort_by(f64::total_cmp);
    let pct = |p: usize| samples.get(samples.len() * p / 100).copied();
    eprintln!("METRIC paint variant={variant} loading_keys_lost={lost}/840 loading_key_to_pixels_p50_ms={:?} loading_key_to_pixels_p95_ms={:?} warmup=1 samples=7 size=1280x820 zoom=1.6", pct(50), pct(95));
}
