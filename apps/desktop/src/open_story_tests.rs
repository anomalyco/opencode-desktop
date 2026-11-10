use crate::stories::{story, world};
use crate::{Route, open_story};

#[test]
fn slow_image_opening_keeps_loading_until_pixels_arrive() {
    let mut rt = story(open_story::PATHS[2], None);
    rt.playground_trigger('o');
    for frame in 1..=30 {
        rt.frame(1.0/60.0);
        let elements = rt.elements();
        let loading = elements.iter().any(|e| e.text.as_deref() == Some("Loading messages…"));
        // The picture's tile, keyed by its place among the message's files: the placeholder square
        // while it loads, the thumbnail once its pixels arrive.
        let image = elements.iter().any(|e| e.path.ends_with("/attachments/file-0") && e.rect[2] < 96.0);
        if frame < 9 {
            assert_eq!(world(&rt).route, Route::here("ses_open_story".into()));
            assert!(!loading && !image);
        } else if frame >= 10 && frame < 24 {
            assert!(loading && !image, "frame {frame}: loading={loading}, image={image}");
        } else if frame >= 25 {
            assert!(!loading && image, "frame {frame}: loading={loading}, image={image}, opening={}, handles={}, image paths={:?}", world(&rt).opening.is_some(), world(&rt).attach.image_handles().len(), elements.iter().filter(|e| e.path.contains("/attachments/")).map(|e| (&e.path, e.rect)).collect::<Vec<_>>());
            assert!(world(&rt).attach.image_handles().iter().all(|i| i.is_ready()));
            assert_eq!(rt.scroll_at_bottom("scroll-ses_open_story"), Some(true));
        }
    }
}
