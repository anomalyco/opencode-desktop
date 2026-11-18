//! Kit's captures of a reply's screenshots letterboxed in 4:3 boxes (t1378): a Read tool returns
//! a tall and a wide screenshot as `media:` refs, then three (a bento block). `--scene reply-image`.

use opencode_kit::model::{Assistant, Attachment, Message, Part, ToolStatus, Transcript};
use serde_json::json;
use session_view::images::MediaStore;

const TALL: &str = "b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b137";
const WIDE: &str = "c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c137";
const PHONE: &str = "d1378d1378d1378d1378d1378d1378d1378d1378d1378d1378d1378d1378d137";

/// A screenshot the size of Kit's (1560×1728 tall, 1600×1000 wide): a dark page of headings and rows.
fn screenshot(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(w, h, |x, y| {
        let margin = x > 120 && x < w - 120;
        let heading = margin && y % 240 < 36 && x < 700;
        let line = margin && y % 240 > 70 && y % 48 < 18 && x + (y / 48 % 5) * 160 < w - 120;
        let button = margin && (180..240).contains(&y) && x < 420;
        image::Rgb(if button {
            [46, 160, 110]
        } else if heading {
            [225, 228, 232]
        } else if line {
            [120, 128, 138]
        } else {
            [24, 27, 31]
        })
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok();
    out.into_inner()
}

/// Appends the `reply-image` scene's user/assistant turns to `transcript` and installs its
/// synthetic screenshots in `media`.
pub fn add_reply_image(transcript: &mut Transcript, media: &MediaStore) {
    transcript.messages.push(Message::User {
        id: "msg_zzreplyimage0".into(),
        text: "Show me the Home tab".into(),
    });
    let shot = |hash: &str, name: &str| Attachment {
        mime: "image/png".into(),
        name: name.into(),
        media: Some(hash.into()),
        ..Default::default()
    };
    let parts = vec![
        Part::Tool {
            id: "call_zzreplyimage".into(),
            name: "read".into(),
            title: "home-tab.png".into(),
            status: ToolStatus::Completed,
            input: json!({}),
            files: vec![shot(TALL, "home-tab.png"), shot(WIDE, "cached-tests.png")],
            metadata: json!(null),
        },
        Part::Text { text: "The Home tab is merged. This still uses demo data.".into() },
        Part::Tool {
            id: "call_zzreplyimage2".into(),
            name: "read".into(),
            title: "phone.png".into(),
            status: ToolStatus::Completed,
            input: json!({}),
            files: vec![shot(PHONE, "phone.png"), shot(WIDE, "cached-tests.png"), shot(TALL, "home-tab.png")],
            metadata: json!(null),
        },
        Part::Text { text: "And on the phone, beside the desktop shots.".into() },
    ];
    transcript.messages.push(Message::Assistant(Assistant {
        id: "msg_zzreplyimage1".into(),
        completed: Some(1),
        parts,
        ..Default::default()
    }));
    media.fixture_media(TALL, Some(screenshot(1560, 1728)));
    media.fixture_media(WIDE, Some(screenshot(1600, 1000)));
    media.fixture_media(PHONE, Some(screenshot(1179, 2556)));
}
