//! The storybook's Timeline › Image gallery story: a tool returns six pictures of mixed shapes
//! (wide, tall, square, a panorama), larger than the window as screenshots are, in the timeline's
//! bento block. R plays it: the first picture opens in the viewer, → steps through them all and
//! one past the last, ← steps back, and Escape closes it. O opens the first, N and P step, and the
//! variants set the column narrow or wide.

use std::sync::{Arc, OnceLock};

use opencode_kit::model::{Assistant, Attachment, Message, Part, ToolStatus};
use rune::{El, Key, Play};
use serde_json::json;

use crate::App;
use crate::palette::Pal;
use crate::stories::{Story, Trigger, frame, replay, sid, stage};

const TOOL: &str = "call_zzzzgallery";

const KEYS: &[Trigger] = &[
    replay(open),
    Trigger { key: 'o', label: "Open the first picture", run: view_first },
    Trigger { key: 'n', label: "Next picture (→)", run: |_, tx| tx.press(Key::Right) },
    Trigger { key: 'p', label: "Previous picture (←)", run: |_, tx| tx.press(Key::Left) },
    Trigger { key: 'e', label: "Escape", run: |_, tx| tx.press(Key::Escape) },
];

/// Made-up pictures: flat gradients of the sizes screenshots come in.
fn pictures() -> &'static [Attachment] {
    static PICS: OnceLock<Vec<Attachment>> = OnceLock::new();
    PICS.get_or_init(|| {
        let shot = |name: &str, size: (u32, u32), a: [u8; 3], b: [u8; 3]| Attachment {
            mime: "image/png".into(),
            name: name.into(),
            bytes: Arc::from(gradient(size, a, b)),
            media: None,
        };
        vec![
            shot("wide.png", (2400, 1500), [58, 92, 220], [158, 82, 200]),
            shot("tall.png", (1170, 2532), [242, 140, 64], [220, 60, 108]),
            shot("square.png", (1500, 1500), [40, 160, 132], [46, 76, 178]),
            shot("panorama.png", (3000, 900), [200, 180, 60], [60, 140, 90]),
            shot("dialog.png", (1400, 1000), [120, 120, 140], [40, 40, 60]),
            shot("phone.png", (900, 1950), [90, 200, 220], [40, 80, 160]),
        ]
    })
}

/// A `w`×`h` PNG shading from `a` to `b`, with a light frame so its edges show when clipped.
fn gradient((w, h): (u32, u32), a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
    let edge = (w.min(h) / 40).max(4);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        if x < edge || y < edge || x >= w - edge || y >= h - edge {
            return image::Rgb([245, 245, 245]);
        }
        let t = (x as f32 / w as f32 * 0.7 + y as f32 / h as f32 * 0.3).clamp(0.0, 1.0);
        let c = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8;
        image::Rgb([c(0), c(1), c(2)])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok();
    out.into_inner()
}

/// The session ends on a finished turn whose one tool call returned the pictures.
fn setup(s: &mut App) {
    crate::stories::scenes::open_scene(s);
    s.finish_stream();
    let input = json!({ "app": "Launcher" });
    let reply = Assistant {
        id: "msg_zzzzgallery".into(),
        completed: Some(1),
        parts: vec![Part::Tool {
            id: TOOL.into(),
            name: "screenshot".into(),
            status: ToolStatus::Completed,
            title: "Launcher".into(),
            input,
            files: pictures().to_vec(),
            metadata: json!({}),
        }],
        ..Default::default()
    };
    let session = sid(s);
    if let Some(t) = s.transcripts.get_mut(&session)
        && !t.messages.iter().any(|m| m.id() == reply.id)
    {
        t.messages.push(Message::Assistant(reply));
    }
}

fn open(s: &mut App, tx: &mut Play<App>) {
    setup(s);
    tx.after(1.2, view_first);
    for at in [2.2, 3.0, 3.8, 4.6, 5.4, 6.2] {
        tx.after(at, |_, tx| tx.press(Key::Right));
    }
    tx.after(7.0, |_, tx| tx.press(Key::Left));
    tx.after(7.8, |_, tx| tx.press(Key::Escape));
}

fn view_first(s: &mut App, tx: &mut Play<App>) {
    if let Some(f) = pictures().first() {
        crate::attachments::view_image(s, &format!("tool-images-{TOOL}"), 0, f, tx);
    }
}

/// The timeline in a frame as wide as the variant asks (the column follows it).
fn view(s: &App, p: &Pal) -> El {
    let w = match *s.fixture_run.variant { 1 => 520.0, 2 => 1100.0, _ => 820.0 };
    frame(p, "story-session").w(w).max_w(w).h_full().max_h(760.0).child(crate::view::story_session(s, p))
}

rune_register::register!(crate::stories::STORY_LIST: Story = [
    Story {
        path: "Timeline/Image gallery",
        scene: |s| {
            setup(s);
            stage(s, view);
        },
        knobs: &[],
        keys: KEYS,
        variants: &[("Comfortable", 0), ("Narrow", 1), ("Wide", 2)],
    },
]);
