//! Live widgets (`specs/v2/widgets-live.md`) in this window: each running program's thread
//! ([`opencode_widget::live::Live`]), what it posts taken every frame, display frames delivered
//! from the window's frame clock when it asks for one and shows, and its pause while it doesn't:
//! scrolled out of view, its session not shown, or its window not on screen (hidden, minimized or
//! covered: [`Tx::window_visible`], whose every change wakes `poll`).

use opencode_widget::{Event, Response, WidgetEvent};
use rune::Tx;

use crate::{WidgetTemplate, WidgetsHost, advance, engine};

/// The window draws at most this often; a live program gets no more frames.
const FRAME_S: f64 = 1.0 / 60.0;

/// One widget's running program, as this window drives it.
#[derive(Default)]
pub struct Run {
    #[cfg(not(target_arch = "wasm32"))]
    live: Option<engine::Live>,
    /// It asked for a frame.
    frame_wanted: bool,
    /// The clock of the last frame it got.
    last_frame: Option<f64>,
    /// The GPU canvases' sizes last sent, by id: physical pixels, clamped (`widgets-gpu.md`).
    gpu_sizes: std::collections::HashMap<String, (u32, u32)>,
    /// The run's GPU canvases' revision and publications last seen.
    gpu_seen: (u64, u64),
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
    let allow_local_network = host.widget_dev_mode();
    let w = host.widgets_mut();
    let Some(e) = w.entries.get(id) else { return };
    let Some(WidgetTemplate::Ready(c)) = w.widget_templates.get(&e.widget_template) else { return };
    let engine::Kind::Live { program, grants } = &c.kind else { return };
    let transport: Option<std::sync::Arc<dyn opencode_widget::live::Transport>> = grants.https.as_ref().map(|_| std::sync::Arc::new(crate::transport::Ureq) as _);
    let live = engine::Live::start(program.clone(), e.machine.data.clone(), engine::Setup { grants: grants.clone(), transport, notify, allow_local_network });
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
        e.local.set_gpu(None);
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
        // Full screen, it shows whatever the timeline does.
        let visible = host.widgets().screen.is(&id) || (e.local.shown() && shown.as_deref() == Some(e.session.as_str()));
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
    let on = visible && tx.window_visible();
    if on && live.paused() {
        live.resume();
    } else if !on && !live.paused() {
        live.pause();
    }
    // How it ended first: the posts it made before its end (those that waited for the post rate
    // too) are all there by then.
    let ended = live.ended();
    let failed_at = live.failed_at();
    let posts = if live.news() || ended.is_some() { live.take() } else { Vec::new() };
    let mut changed = false;
    for post in posts {
        let event = match post {
            Post::Render(node) => {
                // Only GPU canvas values moved (a shader game's frame): the slots take them, and
                // the widget doesn't build again.
                // A still GPU canvas (`fps: 0`) draws only on a build, so it builds.
                let mut still = false;
                node.walk(&mut |n| still |= matches!(n, opencode_widget::Node::Canvas { wgsl: Some(_), .. }) && n.fps() <= 0.0);
                let quiet = if still { None } else { host.widgets().entries.get(id).and_then(|e| e.machine.view()).and_then(|v| v.values_only(&node)) };
                match quiet {
                    Some(moved) => {
                        for (path, values) in &moved {
                            opencode_widget_view::shader::set_values(&format!("{id}/{path}"), values);
                        }
                        advance(host, id, Event::Rendered(node));
                        continue;
                    }
                    None => Event::Rendered(node),
                }
            }
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
            Post::Capture(on) => {
                if let Some(e) = host.widgets_mut().entries.get_mut(id) {
                    e.capture_wanted = on;
                }
                continue;
            }
            // A fetch that failed is the program's to handle (it gets the error): not a failure.
            Post::Failed { phase, message } => {
                crate::client_error(host, id, phase, &message);
                continue;
            }
            // A GPU batch's error: the program's (phase `gpu`), or this client's. The canvases it
            // stopped show their placeholder with it (their status, below).
            Post::Gpu(e) => {
                match e.kind {
                    opencode_widget::gpu::ErrorKind::Program => crate::report(host, id, "gpu", &e.message, None, 0),
                    opencode_widget::gpu::ErrorKind::Client => crate::client_error(host, id, "gpu", &e.message),
                }
                continue;
            }
        };
        advance(host, id, event);
        changed = true;
    }
    if let Some(end) = ended {
        // Its line in the widget template's source: the program's, less the prelude's lines.
        let offset = host.widgets().entries.get(id).and_then(|e| match host.widgets().widget_templates.get(&e.widget_template) {
            Some(WidgetTemplate::Ready(c)) => Some(c.offset),
            _ => None,
        });
        let offset = offset.unwrap_or(0);
        let error = match end {
            Ended::Failed(m) => {
                crate::report(host, id, "main", &m, failed_at, offset);
                Some(crate::in_source(&m, failed_at, offset))
            }
            Ended::Broke(m) => {
                crate::client_error(host, id, "main", &m);
                Some(m)
            }
            Ended::View(m) => {
                crate::report(host, id, "view", &m, None, 0);
                Some(m)
            }
            Ended::Returned | Ended::Stopped => None,
        };
        stop(host, id);
        advance(host, id, Event::Ended(error));
        return true;
    }
    let Some(e) = host.widgets_mut().entries.get_mut(id) else { return changed };
    let Some(live) = e.run.live.as_ref() else { return changed };
    if let Some(canvases) = live.gpu() {
        // A canvas's placeholder shows or goes: the widget builds again. A new version needs a
        // frame, unless the window draws them already.
        let seen = (canvases.revision(), canvases.published());
        if seen.0 != e.run.gpu_seen.0 || (seen.1 != e.run.gpu_seen.1 && !opencode_widget_view::shader::gpu::live_animating()) {
            changed = true;
        }
        e.run.gpu_seen = seen;
        // Before the frame after a GPU canvas's size changed (and the first), the program hears
        // it: physical pixels, clamped to the texture limit.
        if let Some(view) = e.machine.view() {
            let (scale, _) = opencode_widget_view::appearance();
            for (canvas, [w, h]) in e.local.gpu_canvases(view) {
                let size = opencode_widget::gpu::clamp_size((w * scale).round().max(0.0) as u32, (h * scale).round().max(0.0) as u32);
                if e.run.gpu_sizes.get(&canvas) != Some(&size)
                    && let Some(key) = canvases.key(&canvas)
                {
                    live.canvas(&key, size.0, size.1);
                    e.run.gpu_sizes.insert(canvas, size);
                }
            }
        }
        e.local.set_gpu(Some(canvases));
    }
    if !e.run.frame_wanted || !on {
        return changed;
    }
    if let Some(last) = e.run.last_frame
        && now - last < FRAME_S - 0.002
    {
        tx.wake_at(last + FRAME_S);
        return changed;
    }
    // Before its first render it has no canvas yet: its first frame says so (no size).
    let (canvas, size, pointer, close) = match e.machine.view().and_then(|v| e.local.live_canvas(v)) {
        Some(c) => match c.size {
            Some(size) if size[0] > 0.0 && size[1] > 0.0 => (c.id, size, c.pointer, c.close),
            // Its canvas isn't laid out yet (or has no area): the frame waits for its size.
            _ => return changed,
        },
        None => (None, [0.0, 0.0], None, None),
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
        fullscreen: e.local.fullscreen(),
        close,
    };
    live.frame(&frame);
    e.run.frame_wanted = false;
    e.run.last_frame = Some(now);
    changed
}
