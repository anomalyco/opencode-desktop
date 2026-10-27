//! The actual session page and opening path, with deterministic page and image delivery.
use std::sync::{Arc, mpsc::Sender};
use opencode_kit::model::{self, Message, Transcript};
use rune::{El, KeyEvent, Tx, col, row, text};
use rune_playground::{Action, Look, Move, Play, Story};
use serde_json::json;
use crate::{App, Route, palette::Pal, session_pages::{Key, Page, Pages}};

const TARGET: &str = "ses_open_story";
const MEDIA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const FIRST: usize = 10;

pub struct Demo {
    play: Play<App>,
    page: Arc<Page>,
    send: Sender<Result<Arc<Page>, (Key, String)>>,
    png: Vec<u8>,
    at: usize,
    // Only used by the explicit fixture scene for a faithful replay of the old route switch.
    before: bool,
}

struct Case(&'static str);
impl Story<App> for Case {
    fn group(&self) -> &'static str { "Session opening" }
    fn name(&self) -> &'static str { self.0 }
    fn keys(&self) -> &'static [Action<App>] {
        &[Action { cap: "R", label: "Replay", run: replay }, Action { cap: "O", label: "Open now", run: click }]
    }
}
const CASES: &[Case] = &[Case("Cold · ready at 100 ms"), Case("Prefetched · ready now"), Case("Slow · 150 ms fallback")];

pub fn setup(s: &mut App, at: usize) {
    let png = picture();
    let mut messages: Vec<_> = (0..24).map(|i| Message::User { id:format!("story_note_{i:02}"), text:format!("Layout review {i}: keep the chart aligned with the session header.") }).collect();
    messages.extend(model::parse_messages(&json!({"data":[
        {"id":"story_image", "type":"user", "text":"Here's the revised dashboard. Does the chart have enough room?", "files":[{"mime":"image/png", "name":"dashboard.png", "uri":format!("media:{MEDIA}")}]},
        {"id":"story_answer", "type":"assistant", "time":{"created":100,"completed":101}, "tokens":{"input":42000}, "content":[{"type":"text","text":"Yes. The chart now fills the available width, and the controls stay aligned with the header.\n\nThe final image size and scroll position should be ready on the first frame."}]}
    ]})));
    let page = Arc::new(Page { key:Key { id:TARGET.into(), updated:0 }, messages, cursor:None, cached:false, history:false, images:vec![], bytes:0 });
    let (pages, send) = Pages::fixture((at == 1).then(|| page.clone()));
    s.pages = Some(pages);
    s.transcripts.remove(TARGET);
    s.opening = None;
    s.attach = Default::default();
    s.attach.fixture_media(MEDIA, (at == 1).then(|| png.clone()));
    s.route = Route::Session("ses_open_previous".into());
    s.transcripts.insert("ses_open_previous".into(), Transcript { session:"ses_open_previous".into(), messages:vec![Message::User { id:"story_previous".into(), text:"Keep this session visible while the dashboard review gets ready.".into() }] });
    if !s.sessions.iter().any(|i| i.id == TARGET) {
        let mut info = s.sessions.first().cloned().unwrap_or_default();
        info.id = TARGET.into(); info.title = "Dashboard review".into(); info.parent = None;
        s.sessions.push(info.clone());
        info.id = "ses_open_previous".into(); info.title = "Current session".into();
        s.sessions.push(info);
    }
    s.composer.focused = false;
    s.open_demo = Some(Demo { play:Play::default(), page, send, png, at, before:false });
}

pub fn scene(s: &mut App, before: bool) {
    crate::stories::open(s, FIRST, &mut Tx::default());
    s.open_demo.as_mut().unwrap().before = before;
    replay(s, &mut Tx::default());
}

fn replay(s: &mut App, tx: &mut Tx) {
    let d = s.open_demo.as_ref().unwrap();
    let (at, before) = (d.at, d.before);
    setup(s, at);
    s.open_demo.as_mut().unwrap().before = before;
    // Sync the shared replay clock before scheduling relative steps.
    rune_playground::poll(s, |s| &mut s.open_demo.as_mut().unwrap().play, s.clock, tx);
    s.open_demo.as_mut().unwrap().play.after(0.75, click, tx);
    tx.settle();
}

fn click(s: &mut App, tx: &mut Tx) {
    let d = s.open_demo.as_mut().unwrap();
    d.play.clear();
    let (at, before) = (d.at, d.before);
    if before { s.open_now(TARGET, tx); s.open_page(TARGET, tx); } else { s.open(TARGET, tx); }
    let d = s.open_demo.as_mut().unwrap();
    if at != 1 {
        d.play.after(if at == 2 { 0.25 } else { 0.05 }, deliver, tx);
        d.play.after(if at == 2 { 0.4 } else { 0.1 }, pixels, tx);
    }
}

fn deliver(s: &mut App, _: &mut Tx) {
    let d = s.open_demo.as_ref().unwrap();
    d.send.send(Ok(d.page.clone())).unwrap();
}
fn pixels(s: &mut App, _: &mut Tx) {
    s.attach.fixture_media(MEDIA, Some(s.open_demo.as_ref().unwrap().png.clone()));
}
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    if s.open_demo.is_none() { return false; }
    rune_playground::poll(s, |s| &mut s.open_demo.as_mut().unwrap().play, now, tx)
}
pub fn key(s: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(d) = &mut s.open_demo else { return false };
    match rune_playground::key(CASES, Some(d.at), &mut d.play, &mut s.knobs, ev) {
        Move::Pass => false,
        Move::Done => true,
        Move::Leave => { crate::stories::close(s, tx); true },
        Move::Open(i) => { crate::stories::open(s, FIRST + i, tx); true },
        Move::Run(step) => { step(s, tx); true },
    }
}
fn look(p: &Pal) -> Look { Look { text:p.text, muted:p.muted, faint:p.faint, selected:p.pressed, hover:p.hover, panel:p.bg_deep, line:p.border, top:44.0 } }
pub fn view(s: &App, p: &Pal) -> El {
    let d = s.open_demo.as_ref().unwrap();
    let Route::Session(id) = &s.route else { return col() };
    let label = if d.before { "Before · immediate route switch" } else { "After · wait for complete content" };
    let stage = col().key("opening-stage").grow(1.0).shrink().h_full().fill(p.bg_base)
        .child(text(label).font(crate::view::UI).color(p.muted).px(16.0).py(16.0))
        .child(crate::view::session_page(s, p, id));
    row().w_full().h_full().fill(p.bg_deep)
        .child(col().w(210.0).h_full().child(rune_playground::list(CASES, Some(d.at), &d.play, look(p), |s| &mut s.open_demo.as_mut().unwrap().play, |s, i, tx| crate::stories::open(s, FIRST+i, tx))))
        .child(rune_playground::page(stage, rune_playground::side(&CASES[d.at], &s.knobs, &d.play, rune_knobs::Style::dark(), |s| &mut s.knobs, look(p))).shrink())
}

fn picture() -> Vec<u8> {
    let image = image::RgbImage::from_fn(640, 360, |x,y| {
        let bar = x > 45 && x < 600 && x % 70 < 38 && y > 310 - (x / 70) * 25 && y < 310;
        image::Rgb(if bar { [92, 171, 190] } else if y % 60 == 10 { [58, 65, 72] } else { [30, 35, 42] })
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}
