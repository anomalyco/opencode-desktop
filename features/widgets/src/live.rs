//! Live widgets (`specs/v2/widgets-live.md`) in this window: each running program's thread
//! ([`opencode_widget::live::Live`]), what it posts taken every frame, display frames delivered
//! from the window's frame clock when it asks for one and shows, and its pause while it doesn't:
//! scrolled out of view, its session not shown, or its window not drawing (covered or minimized:
//! a frame asked for and nothing drawn for 250 ms).

use opencode_widget::{Event, Response, WidgetEvent};
use rune::Tx;

use crate::{Template, WidgetsHost, advance, engine};

/// The window draws at most this often; a live program gets no more frames.
const FRAME_S: f64 = 1.0 / 60.0;
/// A frame asked for and nothing drawn this long: the window isn't drawing.
const STALL_S: f64 = 0.25;

/// One widget's running program, as this window drives it.
#[derive(Default)]
pub struct Run {
    #[cfg(not(target_arch = "wasm32"))]
    live: Option<engine::Live>,
    /// It asked for a frame.
    frame_wanted: bool,
    /// The clock of the last frame it got.
    last_frame: Option<f64>,
    /// The widget's builds when it got that frame.
    built_at: u64,
}

impl Run {
    #[cfg(not(target_arch = "wasm32"))]
    fn running(&self) -> bool {
        self.live.is_some()
    }

    #[cfg(target_arch = "wasm32")]
    fn running(&self) -> bool {
        false
    }
}

/// Starts widget `id`'s live program.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn start(host: &mut dyn WidgetsHost, id: &str) {
    let notify = host.widget_notify();
    let w = host.widgets_mut();
    let Some(e) = w.entries.get(id) else { return };
    let Some(Template::Ready(c)) = w.templates.get(&e.template) else { return };
    let engine::Kind::Live { program, grants } = &c.kind else { return };
    let transport: Option<std::sync::Arc<dyn opencode_widget::live::Transport>> = grants.https.as_ref().map(|_| std::sync::Arc::new(crate::transport::Ureq) as _);
    let live = engine::Live::start(program.clone(), e.machine.data.clone(), engine::Setup { grants: grants.clone(), transport, notify });
    if let Some(e) = w.entries.get_mut(id) {
        e.run = Run { live: Some(live), ..Run::default() };
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn start(_host: &mut dyn WidgetsHost, _id: &str) {}

/// Delivers what the user did to widget `id`'s program.
pub(crate) fn deliver(host: &mut dyn WidgetsHost, id: &str, event: &WidgetEvent) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(live) = host.widgets().entries.get(id).and_then(|e| e.run.live.as_ref()) {
        live.event(event);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (host, id, event);
}

/// Stops widget `id`'s program.
pub(crate) fn stop(host: &mut dyn WidgetsHost, id: &str) {
    if let Some(e) = host.widgets_mut().entries.get_mut(id) {
        e.run = Run::default();
    }
}

/// Starts the live widgets that came into view, pauses and resumes the running ones, takes what
/// they posted and delivers the frames they asked for. Whether a widget changed.
pub(crate) fn poll(host: &mut dyn WidgetsHost, now: f64, tx: &mut Tx) -> bool {
    let shown = host.widget_session().map(|s| s.0);
    let ids: Vec<String> = host.widgets().entries.iter().filter(|(_, e)| e.machine.waiting() || e.run.running()).map(|(id, _)| id.clone()).collect();
    let mut changed = false;
    for id in ids {
        let Some(e) = host.widgets().entries.get(&id) else { continue };
        let visible = e.local.shown() && shown.as_deref() == Some(e.session.as_str());
        if e.machine.waiting() {
            if visible {
                advance(host, &id, Event::Shown);
                changed = true;
            }
            continue;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            changed |= drive(host, &id, visible, now, tx);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (now, tx);
    changed
}

#[cfg(not(target_arch = "wasm32"))]
fn drive(host: &mut dyn WidgetsHost, id: &str, visible: bool, now: f64, tx: &mut Tx) -> bool {
    use engine::{Ended, Post};
    let Some(e) = host.widgets_mut().entries.get_mut(id) else { return false };
    let Some(live) = e.run.live.as_ref() else { return false };
    let stalled = e.run.frame_wanted && e.run.last_frame.is_some_and(|t| now - t > STALL_S) && e.local.builds() == e.run.built_at;
    let on = visible && !stalled;
    if on && live.paused() {
        live.resume();
    } else if !on && !live.paused() {
        live.pause();
    }
    let posts = if live.news() { live.take() } else { Vec::new() };
    let ended = live.ended();
    let mut changed = false;
    for post in posts {
        let event = match post {
            Post::Render(node) => Event::Rendered(node),
            Post::Draw { id: canvas, commands } => {
                if let Some(e) = host.widgets_mut().entries.get_mut(id)
                    && let Some(view) = e.machine.view()
                {
                    e.local.canvas_posted(view, &canvas, commands);
                    changed = true;
                }
                continue;
            }
            Post::Frame => {
                if let Some(e) = host.widgets_mut().entries.get_mut(id) {
                    e.run.frame_wanted = true;
                }
                continue;
            }
            Post::Submit(value) => Event::Answered(Response::Submit(value)),
            Post::Dismiss => Event::Answered(Response::Dismiss),
            Post::Send(message) => Event::Sending(message),
            Post::Dropped(n) => {
                eprintln!("widget {id}: dropped {n} posts over 240 a second");
                continue;
            }
            Post::Failed { phase, message } => {
                crate::report(host, id, phase, &message, None, 0);
                continue;
            }
        };
        advance(host, id, event);
        changed = true;
    }
    if let Some(end) = ended {
        let error = match end {
            Ended::Failed(m) => Some(m),
            Ended::Returned | Ended::Stopped => None,
        };
        if let Some(m) = &error {
            crate::report(host, id, "main", m, None, 0);
        }
        stop(host, id);
        advance(host, id, Event::Ended(error));
        return true;
    }
    let Some(e) = host.widgets_mut().entries.get_mut(id) else { return changed };
    let Some(live) = e.run.live.as_ref() else { return changed };
    if !e.run.frame_wanted || !on {
        return changed;
    }
    if let Some(last) = e.run.last_frame
        && now - last < FRAME_S - 0.002
    {
        tx.wake_at(last + FRAME_S);
        return changed;
    }
    let Some(view) = e.machine.view() else { return changed };
    let (canvas, size, pointer) = match e.local.live_canvas(view) {
        Some(c) => match c.size {
            Some(size) => (c.id, size, c.pointer),
            // Its canvas isn't laid out yet: the frame waits for its size.
            None => return changed,
        },
        None => (None, [0.0, 0.0], None),
    };
    let (scale, dark) = opencode_widget_view::appearance();
    let frame = opencode_widget::Frame {
        id: canvas,
        t: live.now_ms() / 1000.0,
        dt: e.run.last_frame.map_or(0.0, |t| now - t),
        width: f64::from(size[0]),
        height: f64::from(size[1]),
        scale: f64::from(scale),
        pointer,
        dark,
    };
    live.frame(&frame);
    e.run.frame_wanted = false;
    e.run.last_frame = Some(now);
    e.run.built_at = e.local.builds();
    // Looks again after a stall's time, to pause a window that stopped drawing; builds the
    // widget again, which a window that draws does at once.
    tx.wake_at(now + STALL_S + 0.01);
    true
}
