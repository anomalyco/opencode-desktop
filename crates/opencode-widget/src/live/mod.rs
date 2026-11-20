//! Live widgets (`specs/v2/widgets-live.md`): one long-running PsychoScript program per open
//! widget, `<program>\nreturn await __widget_live(__input())`, on a thread of its own, talking to
//! the client through the host functions under `__host` ([`host`]): `post` (to the client),
//! `receive` (from it), `sleep`, `now` and `fetch`.
//!
//! The client ([`Live`]) delivers events and display frames, takes what the program posts, and
//! pauses it while the widget is out of view: no frame, event or timer reaches it then, its
//! timers keep their remaining time, `now()` stops, a `fetch` that comes back is held, and it costs
//! no CPU. Resumed, it hears `{ type: "resumed", pausedMs }` first. Paused 30 s, it is saved to a
//! snapshot and its thread let go ([`link`]); resumed, it continues from the snapshot, or starts
//! again from `main` when it couldn't be saved.
//!
//! Budgets: a program that runs 50 ms without awaiting is stopped with an error (watched from
//! outside the run, [`link`]); a `draw` over 2,000 commands throws; posts over 240 a second are
//! dropped (submit and dismiss never are).

pub mod fetch;
mod host;
mod link;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use psychoscript::{Compiled, Runtime};
use serde_json::{Value, json};

use crate::draw::{Command, Frame};
use crate::node::Node;
use crate::runner::Error;
use crate::step::{SendMessage, WidgetEvent};

pub use fetch::{Grants, Transport};
use link::Link;

/// The longest a program may run without awaiting.
pub const SLICE_MS: u64 = 50;
/// Posts a second past which they are dropped.
pub const MAX_POSTS: u32 = 240;
/// How long a paused program keeps its thread before it is saved to a snapshot.
pub const SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// What runs a live program after the template's.
const TAIL: &str = "\nreturn await __widget_live(__input())";

/// A live template's program, compiled once per `name@version`.
pub struct LiveProgram {
    compiled: Compiled,
}

impl fmt::Debug for LiveProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveProgram").field("bytes", &self.compiled.code().len()).finish()
    }
}

impl LiveProgram {
    /// Compiles `program` (the server's `prelude + "\n" + source`) with the live entry.
    pub fn compile(program: &str) -> Result<LiveProgram, Error> {
        let compiled = Runtime::make().compile(&format!("{program}{TAIL}")).map_err(|d| Error::Compile(d.message_lossy()))?;
        Ok(LiveProgram { compiled })
    }
}

/// What a live program posted, read and checked on its thread.
#[derive(Clone, Debug, PartialEq)]
pub enum Post {
    /// Replace the view.
    Render(Node),
    /// Replace canvas `id`'s commands.
    Draw { id: String, commands: Vec<Command> },
    /// One display frame wanted.
    Frame,
    Submit(Value),
    Dismiss,
    Send(SendMessage),
    /// Posts were dropped over the rate (`count` since the last warning).
    Dropped(u32),
    /// Pointer capture asked for (`widget.capturePointer()`) or given back.
    Capture(bool),
    /// Something the program asked of the host failed (`phase`: `fetch`), for the agent to hear.
    Failed { phase: &'static str, message: String },
}

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// `main` returned.
    Returned,
    /// It threw, or broke a budget: the message to show.
    Failed(String),
    /// The client stopped it.
    Stopped,
}

/// The host's side of one running live widget. Dropping it stops the program.
pub struct Live {
    link: Arc<Link>,
}

/// What a live program needs from its host besides its program and data.
#[derive(Clone)]
pub struct Setup {
    /// The capabilities granted to it.
    pub grants: Grants,
    /// Sends `fetch`'s requests; none, and `fetch` fails as `Network`.
    pub transport: Option<Arc<dyn Transport>>,
    /// Dev mode: `fetch` may also reach loopback, private and link-local hosts the template
    /// declares, over `http:` too (`allowLocalNetwork`). Off by default.
    pub allow_local_network: bool,
    /// Wakes the client when the program posted, ended or a frame is due.
    pub notify: Arc<dyn Fn() + Send + Sync>,
}

impl Live {
    /// Starts `program` on `data` on a thread of its own.
    pub fn start(program: Arc<LiveProgram>, data: Value, setup: Setup) -> Live {
        let link = Link::new(program, data, setup);
        link.start();
        Live { link }
    }

    /// Delivers what the user did.
    pub fn event(&self, event: &WidgetEvent) {
        self.link.deliver(json!({ "type": "event", "event": event }));
    }

    /// Delivers the display frame the program asked for.
    pub fn frame(&self, frame: &Frame) {
        self.link.deliver(json!({ "type": "frame", "frame": frame }));
    }

    /// Whether anything waits in [`Live::take`] or the program ended since the last look.
    pub fn news(&self) -> bool {
        self.link.news.load(Ordering::Acquire)
    }

    /// What the program posted since the last call, in order; a later render or draw of the same
    /// canvas replaces one not yet taken.
    pub fn take(&self) -> Vec<Post> {
        self.link.take()
    }

    /// How the program ended, once it has.
    pub fn ended(&self) -> Option<Ended> {
        self.link.ended()
    }

    /// Where in the program it threw (1-based line and column), when it failed so.
    pub fn failed_at(&self) -> Option<(u32, u32)> {
        self.link.failed_at()
    }

    /// The widget left view: the program pauses.
    pub fn pause(&self) {
        self.link.pause();
    }

    /// The widget is in view again: the program resumes, hearing `resumed` first.
    pub fn resume(&self) {
        self.link.resume();
    }

    pub fn paused(&self) -> bool {
        self.link.is_paused()
    }

    /// Milliseconds it has run, not counting pauses (`__host.now()`).
    pub fn now_ms(&self) -> f64 {
        self.link.now_ms()
    }

    /// Stops the program; it ends as [`Ended::Stopped`].
    pub fn stop(&self) {
        self.link.stop();
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.link.stop();
    }
}
