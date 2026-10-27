use crate::{App, Route, open_story, view};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

#[test]
fn slow_image_opening_keeps_loading_until_pixels_arrive() {
    let mut app = App::empty();
    open_story::setup(&mut app, 2);
    let mut rt = Runtime::new(app, view::app, (1280.0,820.0), Env::new(true,1.0));
    rt.poll = Some(crate::poll);
    rt.on_key = Some(crate::on_key);
    rt.frame(0.0);
    rt.key(KeyEvent { key:Key::Text("o".into()), mods:Mods::default() });
    rt.frame(0.0); // Dispatch the queued click at t=0, before measuring its deadlines.
    for frame in 1..=30 {
        rt.frame(1.0/60.0);
        let elements = rt.elements();
        let loading = elements.iter().any(|e| e.text.as_deref() == Some("Loading messages…"));
        let image = elements.iter().any(|e| e.path.ends_with("/img-0"));
        if frame < 9 {
            assert_eq!(rt.state.route, Route::Session("ses_open_previous".into()));
            assert!(!loading && !image);
        } else if frame >= 10 && frame < 24 {
            assert!(loading && !image, "frame {frame}: loading={loading}, image={image}");
        } else if frame >= 25 {
            assert!(!loading && image, "frame {frame}: loading={loading}, image={image}, opening={}, handles={}, image paths={:?}", rt.state.opening.is_some(), rt.state.attach.image_handles().len(), elements.iter().filter(|e| e.path.contains("img-")).map(|e| &e.path).collect::<Vec<_>>());
            assert!(rt.state.attach.image_handles().iter().all(|i| i.is_ready()));
            assert_eq!(rt.scroll_at_bottom("scroll-ses_open_story"), Some(true));
        }
    }
}
