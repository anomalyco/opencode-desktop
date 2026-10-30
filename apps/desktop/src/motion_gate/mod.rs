//! The motion gate: every message and timeline animation, run headless on the virtual clock
//! across a matrix of circumstances, traced frame by frame and checked by [`check`].
//!
//! A [`Run`] builds the real app from the fixture in one [`Circ`]umstance, settles it, then
//! records every frame while a scenario drives it through real input (clicks, keys, wheel) and
//! server events (through the fixture's playback, as a live stream would arrive).
//! `scenarios.rs` lists the matrix and its allowlist; Rune's `rune::motion_gate` checks and
//! reports it (`MOTION_GATE=<filter>` narrows it, `MOTION_GATE_OUT` names the report).

mod flights;
mod odometers;
mod scenarios;

use rune::{Env, Key, KeyEvent, Mods, Runtime};
use serde_json::{Value, json};

use crate::App;
use rune::motion_gate::Recording;

pub const DT: f64 = 1.0 / 120.0;
pub const SID: &str = crate::fixture_ids::MAIN;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Long,
    Short,
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dock {
    None,
    Question,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prompt {
    Line,
    Lines,
    Image,
    /// A pasted transcript, long enough to land folded (`fold.rs`).
    Long,
}

/// What's true before the action: one cell's column of the matrix.
#[derive(Clone, Copy, Debug)]
pub struct Circ {
    pub length: Length,
    pub scrolled: bool,
    pub narrow: bool,
    pub dock: Dock,
    pub prompt: Prompt,
    /// The fixture's streaming turn is still to play (else it's applied, finished).
    pub unplayed: bool,
    /// The app zoomed in (⌘+), as a whole window scaled up.
    pub zoomed: bool,
    /// The session ends with a turn that used 40 tools (`tool_group_stories::add_long_group`).
    pub long_group: bool,
}

impl Default for Circ {
    fn default() -> Self {
        Circ { length: Length::Long, scrolled: false, narrow: false, dock: Dock::None, prompt: Prompt::Line, unplayed: false, zoomed: false, long_group: false }
    }
}

impl Circ {
    pub fn name(&self) -> String {
        let mut parts = vec![if self.scrolled { "scrolled-up" } else { "pinned" }];
        parts.push(match self.length { Length::Long => "long", Length::Short => "short", Length::Empty => "empty" });
        parts.push(if self.narrow { "narrow" } else { "wide" });
        match self.dock {
            Dock::None => {}
            Dock::Question => parts.push("question-open"),
            Dock::Pending => parts.push("pending-open"),
        }
        match self.prompt {
            Prompt::Line => {}
            Prompt::Lines => parts.push("multi-line"),
            Prompt::Image => parts.push("image"),
            Prompt::Long => parts.push("long-prompt"),
        }
        if self.zoomed {
            parts.push("zoomed");
        }
        if self.long_group {
            parts.push("long-group");
        }
        parts.join("/")
    }

    pub fn size(&self) -> (f32, f32) {
        if self.narrow { (720.0, 820.0) } else { (1280.0, 820.0) }
    }

    pub fn body(&self) -> &'static str {
        match self.prompt {
            Prompt::Long => crate::fold_stories::LONG,
            Prompt::Lines => "Three things before you merge:\n- keep the backoff capped at 30 s\n- log every retry with its attempt number\n- and add a test that drops the socket twice in a row",
            _ => "Can you also check the reconnect jitter?",
        }
    }
}

fn fixture(c: &Circ) -> String {
    let mut f: Value = serde_json::from_str(crate::FIXTURE).unwrap();
    match c.length {
        Length::Long => {}
        Length::Short => f["messages"][SID].as_array_mut().unwrap().truncate(2),
        Length::Empty => f["messages"][SID] = json!([]),
    }
    if c.length != Length::Long && !c.unplayed {
        // The streamed turn belongs to the long session.
        f["stream"]["played"] = true.into();
    }
    f.to_string()
}

pub struct Run {
    pub rt: Runtime<App>,
    pub rec: Recording,
    pub circ: Circ,
}

impl Run {
    pub fn new(c: Circ) -> Run {
        let mut app = App::empty();
        app.load_fixture(&fixture(&c));
        // Sent prompts' files are kept by message id process-wide; each run sends its own ids.
        static RUNS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        app.sends = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) * 100;
        crate::open_scene(&mut app);
        if c.zoomed {
            app.zoom = 1.6;
        }
        if !c.unplayed && c.length == Length::Long {
            app.finish_stream();
        }
        if c.long_group {
            crate::tool_group_stories::add_long_group(&mut app, SID);
        }
        match c.dock {
            Dock::None => {}
            Dock::Question => {
                let q = app.fixture["question"].clone();
                app.apply_event("form.created", &q);
            }
            Dock::Pending => {
                app.inboxes.entry(SID.into()).or_default().upsert(opencode_kit::model::InboxItem {
                    id: "msg_zzpending".into(),
                    text: "After this, bump the version and tag it".into(),
                    delivery: opencode_kit::model::Delivery::Queue,
                    files: vec![],
                });
            }
        }
        let size = c.size();
        let mut rt = Runtime::new(app, crate::view::app, size, Env::new(true, 1.0));
        rt.poll = Some(crate::poll);
        rt.on_key = Some(crate::on_key);
        rt.frame(0.0);
        let mut run = Run { rt, rec: recording(size), circ: c };
        run.idle(1.0);
        if c.scrolled {
            run.scroll_up(320.0);
        }
        // Frame 0 is the settled start the first action changes.
        run.step();
        run
    }

    /// Steps `secs` unrecorded (setting up, before the run).
    pub fn idle(&mut self, secs: f64) {
        for _ in 0..(secs / DT).round() as usize {
            self.rt.frame(DT);
        }
    }

    fn step(&mut self) {
        let queued = self.rt.state.playback.len();
        let awake = self.rt.frame(DT);
        self.rec.awake.push(awake);
        if self.rt.state.playback.len() < queued {
            let what = self.rt.state.playback.len();
            self.rec.input(format!("event ({what} left)"));
        }
        self.rec.sample(&self.rt.elements());
    }

    /// Records `secs` of frames.
    pub fn play(&mut self, secs: f64) {
        for _ in 0..(secs / DT).round() as usize {
            self.step();
        }
    }

    /// Records until the fixture's playback runs out and the frame settles, then `tail` more.
    pub fn play_out(&mut self, tail: f64) {
        let mut guard = 0;
        while !self.rt.state.playback.is_empty() && guard < 2400 {
            self.step();
            guard += 1;
        }
        self.play(tail);
    }

    /// An input on the next frame: `f` runs, then the frame it lands on is recorded.
    pub fn act(&mut self, what: &str, f: impl FnOnce(&mut Runtime<App>)) {
        self.rec.input(what);
        f(&mut self.rt);
        self.step();
    }

    /// Marks the cell's motion as over from the next frame: frames asked for well after it
    /// are `wakes-after-end`.
    pub fn quiet(&mut self) {
        self.rec.quiet_from = Some(self.rec.frames);
    }

    /// A server event, arriving through playback as a live one would, after the playing turn's
    /// own.
    pub fn event(&mut self, ty: &str, data: Value) {
        let at = self.rt.state.clock + 0.001;
        self.rt.state.playback.push_back((at, ty.into(), data));
        self.step();
    }

    /// [`Run::event`] now, ahead of the playing turn's later events, as a steer lands mid-turn.
    pub fn event_now(&mut self, ty: &str, data: Value) {
        let at = self.rt.state.clock + 0.001;
        self.rt.state.schedule(at, ty, data);
        self.step();
    }

    pub fn tap(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no {key}"));
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
    }

    pub fn scroll_up(&mut self, dy: f32) {
        let v = self.timeline_rect();
        self.rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, dy);
        self.idle(1.0);
    }

    /// A recorded wheel scroll up the timeline, then `secs` of frames.
    pub fn swipe_up(&mut self, dy: f32, secs: f64) {
        let v = self.timeline_rect();
        self.act("scroll up", |rt| rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, dy));
        self.play(secs);
    }

    /// A recorded wheel scroll down the timeline, then `secs` of frames.
    pub fn swipe_down(&mut self, dy: f32, secs: f64) {
        let v = self.timeline_rect();
        self.act("scroll down", |rt| rt.wheel(v[0] + v[2] / 2.0, v[1] + v[3] / 2.0, -dy));
        self.play(secs);
    }

    fn timeline_rect(&self) -> [f32; 4] {
        let key = format!("scroll-{SID}");
        self.rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str())).map(|e| e.rect).expect("a timeline")
    }

    /// Focuses the composer and types the circumstance's prompt (and attaches its image).
    pub fn type_prompt(&mut self) {
        // After a send the composer keeps focus, so a second prompt is typed straight in.
        if !self.rt.state.composer.focused {
            self.act("tap the composer", |rt| Run::tap(rt, "prompt"));
        }
        let body = self.circ.body().to_owned();
        let image = self.circ.prompt == Prompt::Image;
        self.act("type", |rt| {
            rt.key(KeyEvent { key: Key::Text(body), mods: Mods::default() });
            if image {
                rt.update(|s, _| {
                    let a = opencode_kit::model::Attachment { mime: "image/png".into(), name: "shot.png".into(), bytes: png().into(), media: None };
                    s.attach.add(a.clone());
                    s.attach.image(&a).unwrap().wait().unwrap();
                });
            }
        });
        self.play(0.6);
    }

    /// Enter (⌘⏎ when `alternate`) in the composer: the prompt sends, steers or queues.
    pub fn enter(&mut self, alternate: bool) {
        self.act(if alternate { "⌘⏎" } else { "⏎" }, |rt| rt.key(KeyEvent { key: Key::Enter, mods: Mods { cmd: alternate, ..Default::default() } }));
    }

    /// Starts the fixture's streaming turn now.
    pub fn start_stream(&mut self) {
        self.act("stream starts", |rt| rt.update(|s, _| s.start_stream_if_due(SID)));
    }

}

/// A recording on the gate's clock. Live numbers roll here (`odometer.rs`), so one that snaps
/// in place is a `text-swap`.
pub fn recording(size: (f32, f32)) -> Recording {
    let mut rec = Recording::new(size, 1.0 / DT);
    rec.counts_roll = true;
    rec
}

fn png() -> Vec<u8> {
    let image = image::RgbImage::from_fn(480, 300, |x, y| image::Rgb(if (x / 40 + y / 40) % 2 == 0 { [92, 171, 190] } else { [30, 35, 42] }));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}
