//! Kit's captures of a reply's screenshots letterboxed in 4:3 boxes (t1378): a Read tool returns
//! a tall and a wide screenshot as `media:` refs, under the reply. `--scene reply-image`.

use opencode_kit::model::{Assistant, Attachment, Message, Part, ToolStatus};
use serde_json::json;

use crate::App;

const TALL: &str = "b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b1378b137";
const WIDE: &str = "c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c1378c137";

/// A screenshot the size of Kit's (1560×1728 tall, 1600×1000 wide): a dark page of headings and rows.
fn screenshot(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(w, h, |x, y| {
        let margin = x > 120 && x < w - 120;
        let heading = margin && y % 240 < 36 && x < 700;
        let line = margin && y % 240 > 70 && y % 48 < 18 && x + (y / 48 % 5) * 160 < w - 120;
        let button = margin && (180..240).contains(&y) && x < 420;
        image::Rgb(if button { [46, 160, 110] } else if heading { [225, 228, 232] } else if line { [120, 128, 138] } else { [24, 27, 31] })
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("encode the screenshot");
    out.into_inner()
}

pub fn add_reply_image(app: &mut App, sid: &str) {
    let t = app.transcripts.get_mut(sid).expect("session");
    t.messages.push(Message::User { id: "msg_zzreplyimage0".into(), text: "Show me the Home tab".into() });
    let shot = |hash: &str, name: &str| Attachment { mime: "image/png".into(), name: name.into(), media: Some(hash.into()), ..Default::default() };
    let parts = vec![
        Part::Tool { id: "call_zzreplyimage".into(), name: "read".into(), title: "home-tab.png".into(), status: ToolStatus::Completed, input: json!({}), files: vec![shot(TALL, "home-tab.png"), shot(WIDE, "cached-tests.png")], metadata: json!(null) },
        Part::Text { text: "The Home tab is merged. This still uses demo data.".into() },
    ];
    t.messages.push(Message::Assistant(Assistant { id: "msg_zzreplyimage1".into(), completed: Some(1), parts, ..Default::default() }));
    app.attach.fixture_media(TALL, Some(screenshot(1560, 1728)));
    app.attach.fixture_media(WIDE, Some(screenshot(1600, 1000)));
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("reply-image", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session(id) = s.route.clone() {
            add_reply_image(s, &id);
        }
    }),
]);
