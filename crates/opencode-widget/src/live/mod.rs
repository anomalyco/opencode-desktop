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
//! Budgets: a program that runs 50 ms without awaiting is stopped with an error (the VM's slice, on
//! its thread's CPU time, [`SLICE_MS`]); a `draw` over 2,000 commands ends the run. `render` and `draw` posts
//! over the post rate wait and coalesce ([`POSTS_PER_SECOND`], `widgets-live.md`, Post rate).

pub mod fetch;
pub mod gpu;
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

/// The longest a program may run without awaiting the host (`Limits::slice_ms`): its thread's CPU
/// time, host calls left out, so a loaded machine doesn't stop it.
pub const SLICE_MS: u64 = 50;
/// The post rate (`widgets-live.md`, Post rate): a run's `render` and `draw` posts take a token of
/// a bucket of [`BURST`], refilled at this many a second; past it they wait, and coalesce.
pub const POSTS_PER_SECOND: f64 = 240.0;
/// The post rate's bucket: the most tokens it holds, and starts with.
pub const BURST: f64 = 8.0;
/// How long a paused program keeps its thread before it is saved to a snapshot.
pub const SAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// What runs a live program after the widget template's, as a native host (`widgets.md`, Native
/// hosts): this client builds its views' trees and maps its draws' keys.
const TAIL: &str = "\nreturn await __widget_live(__input(), true)";

/// A live widget template's program, compiled once per `name@version`.
pub struct LiveProgram {
    /// `Err`: refused for its protocol, never compiled; a run of it ends at once so.
    pub(super) compiled: Result<Compiled, Ended>,
}

impl fmt::Debug for LiveProgram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LiveProgram").field("bytes", &self.compiled.as_ref().map_or(0, |c| c.code().len())).finish()
    }
}

impl LiveProgram {
    /// Whether it's refused for its protocol (`widgets.md`, Protocol): no run of it starts, and no
    /// restart could.
    pub fn refused(&self) -> bool {
        self.compiled.is_err()
    }

    /// Compiles `program` (the server's `prelude + "\n" + source`) with the live entry. One built
    /// for another protocol isn't compiled: its run ends at once, an older one's as a program error
    /// (phase `main`), a newer one's as this client's.
    pub fn compile(program: &str) -> Result<LiveProgram, Error> {
        let compiled = match crate::runner::refused(program) {
            Some(Error::Newer) => Err(Ended::Broke(crate::runner::NEWER.into())),
            Some(e) => Err(Ended::Failed(e.to_string())),
            None => Ok(Runtime::make().compile(&format!("{program}{TAIL}")).map_err(|d| Error::Compile(d.message_lossy()))?),
        };
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
    /// Pointer capture asked for (`widget.capturePointer()`) or given back.
    Capture(bool),
    /// Something the program asked of the host failed (`phase`: `fetch`), for the agent to hear.
    Failed { phase: &'static str, message: String },
    /// A GPU batch's error (`widgets-gpu.md`): the program's (phase `gpu`, reported) or this
    /// client's, and the canvases it stopped.
    Gpu(widget_gpu::GpuError),
}

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// `main` returned.
    Returned,
    /// It threw, or broke a budget: the message to show. A program error.
    Failed(String),
    /// This client couldn't keep it running (no thread, a snapshot that didn't restore, a program
    /// for a newer protocol): a client error, shown but never reported.
    Broke(String),
    /// A render's view broke Keys and depth (`widgets.md`): a program error in phase `view`.
    View(String),
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
    /// Dev mode: `fetch` may also reach loopback, private and link-local hosts the widget template
    /// declares, over `http:` too (`allowLocalNetwork`). Off by default.
    pub allow_local_network: bool,
    /// Wakes the client when the program posted, ended or a frame is due.
    pub notify: Arc<dyn Fn() + Send + Sync>,
}

impl Live {
    /// Starts `program` on `data` on a thread of its own.
    pub fn start(program: Arc<LiveProgram>, data: Value, setup: Setup) -> Live {
        Live::start_rated(program, data, setup, POSTS_PER_SECOND)
    }

    /// [`Live::start`] at a post rate of `per_second` (as the TS host's `postsPerSecond`), to watch
    /// the rate at a pace that's easy to see.
    pub fn start_rated(program: Arc<LiveProgram>, data: Value, setup: Setup, per_second: f64) -> Live {
        let link = Link::new(program, data, setup, per_second);
        link.start();
        Live { link }
    }

    /// Delivers what the user did.
    pub fn event(&self, event: &WidgetEvent) {
        self.link.deliver(json!({ "type": "event", "event": event }));
    }

    /// Tells the program GPU canvas `key`'s size, in physical pixels clamped to the texture limit
    /// (`widget_gpu::clamp_size`), before the frame after it changed (`widgets-gpu.md`).
    pub fn canvas(&self, key: &str, width: u32, height: u32) {
        self.link.deliver(json!({ "type": "canvas", "key": key, "width": width, "height": height }));
    }

    /// The run's GPU canvases, as the window shows them; none before it started.
    pub fn gpu(&self) -> Option<Arc<crate::gpu::Canvases>> {
        self.link.gpu().map(|g| g.canvases.clone())
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
