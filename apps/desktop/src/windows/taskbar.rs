//! What a window's taskbar button says about the sessions in its tabs (`rune::Taskbar`): one
//! machine per window, stepped by each poll from the marks its tabs already show
//! (`session_status`), so it costs nothing while nothing changes and a window that never runs
//! anything never touches the taskbar. Each window speaks for its own tabs: with several
//! windows, the button that lights is the one with the session that needs you.
//!
//! The rules:
//! - **Bar:** amber while a session waits on you (a permission or a question); else red while a
//!   run failed and you haven't looked at it; else Windows' moving bar while a session works;
//!   else none.
//! - **Badge:** how many tabs need you: a permission or a question, or a run that ended since
//!   you last looked at it. Looking at the session, answering it here, in another window or
//!   from its toast, takes it off at once.
//! - **Flash:** only for a new permission or question while the app isn't in front and no
//!   toast announced it (Settings turned that kind's notifications off, or the system has
//!   none): twice, then the button stays lit until the window comes forward. Never for a run
//!   that ends, never during Focus Assist (Rune), and it stops when the request is answered
//!   anywhere.

use rune::{TaskbarProgress, WindowHandle};

use crate::App;

/// What the window's tabs need.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Tabs whose session waits on a permission or a question.
    pub waiting: u32,
    /// Tabs whose run ended since they were last looked at.
    pub unread: u32,
    /// One of those ended failed.
    pub failed: bool,
    /// A tab's session works.
    pub busy: bool,
}

impl Summary {
    fn progress(self) -> TaskbarProgress {
        if self.waiting > 0 {
            return TaskbarProgress::Paused;
        }
        if self.failed {
            return TaskbarProgress::Error;
        }
        if self.busy {
            return TaskbarProgress::Busy;
        }
        TaskbarProgress::None
    }

    fn count(self) -> u32 {
        self.waiting + self.unread
    }

    fn shown(self) -> rune::Taskbar {
        let n = self.count();
        let description = if n == 1 { "1 session needs you".to_owned() } else { format!("{n} sessions need you") };
        rune::Taskbar::new().progress(self.progress()).badge(n, description)
    }
}

/// A window's taskbar button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Taskbar {
    /// Shows nothing, and never showed anything: the taskbar's COM object isn't made.
    #[default]
    Blank,
    /// Shows `summary`; `lit` while a flash for one of its requests stays on the button.
    Showing { summary: Summary, lit: bool },
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// A poll read the tabs. `focused`: one of the app's windows is in front. `announced`: a toast tells of a
    /// new permission or question (Settings has both on and the system shows notifications).
    Polled { summary: Summary, focused: bool, announced: bool },
    /// The window came to the front: Windows put out the button's light itself.
    Focused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Show(rune::Taskbar),
    Flash(bool),
}

impl Taskbar {
    pub fn step(self, event: Event) -> (Taskbar, Vec<Effect>) {
        match (self, event) {
            (Taskbar::Blank, Event::Polled { summary, .. }) if summary == Summary::default() => (Taskbar::Blank, Vec::new()),
            (Taskbar::Blank, Event::Focused) => (Taskbar::Blank, Vec::new()),
            (Taskbar::Showing { summary, .. }, Event::Focused) => (Taskbar::Showing { summary, lit: false }, Vec::new()),
            (state, Event::Polled { summary, focused, announced }) => {
                let (before, lit) = match state {
                    Taskbar::Blank => (Summary::default(), false),
                    Taskbar::Showing { summary, lit } => (summary, lit),
                };
                let mut effects = Vec::new();
                if (before.progress(), before.count()) != (summary.progress(), summary.count()) {
                    effects.push(Effect::Show(summary.shown()));
                }
                let asked = summary.waiting > before.waiting && !focused && !announced;
                let lit = match (lit, asked, summary.waiting) {
                    (_, true, _) => {
                        effects.push(Effect::Flash(true));
                        true
                    }
                    (true, false, 0) => {
                        effects.push(Effect::Flash(false));
                        false
                    }
                    (lit, false, _) => lit,
                };
                (Taskbar::Showing { summary, lit }, effects)
            }
        }
    }
}

impl App {
    /// The window's tabs on the server it's checked out on, as their avatars mark them.
    fn taskbar_summary(&self) -> Summary {
        let server = crate::servers::runtime::active(self);
        self.tabs.iter().filter(|t| !t.is_draft() && t.server == *server).fold(Summary::default(), |mut sum, t| {
            let marks = self.marks(&t.id);
            let ended = marks.unread && !marks.attention;
            sum.waiting += u32::from(marks.attention);
            sum.unread += u32::from(ended);
            sum.failed |= ended && self.session(&t.id).is_some_and(|s| s.outcome.as_deref() == Some("failed"));
            sum.busy |= marks.busy;
            sum
        })
    }

    /// `poll`: steps the window's taskbar machine and shows what it says.
    pub(crate) fn poll_taskbar(&mut self) {
        // Nothing before the first frame: a launch costs the taskbar nothing.
        if !matches!(self.checkout.boot, crate::servers::runtime::Boot::Ready) {
            return;
        }
        let Some(handle) = super::handle(&self.window.id) else { return };
        let k = self.settings.notify;
        let announced = k.permissions && k.questions && self.notify.shows();
        let event = Event::Polled { summary: self.taskbar_summary(), focused: rune_notify::app_focused(), announced };
        self.step_taskbar(&handle, event);
    }

    /// The window came to the front.
    pub(crate) fn taskbar_focused(&mut self) {
        if let Some(handle) = super::handle(&self.window.id) {
            self.step_taskbar(&handle, Event::Focused);
        }
    }

    fn step_taskbar(&mut self, handle: &WindowHandle, event: Event) {
        let (next, effects) = self.window.taskbar.step(event);
        self.window.taskbar = next;
        for effect in effects {
            match effect {
                Effect::Show(state) => handle.set_taskbar(state),
                Effect::Flash(on) => handle.flash(on),
            }
        }
    }
}
