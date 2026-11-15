//! A subagent call's run, as the host tracks it for the background panel above the composer.
//! The timeline draws subagents as events (`activity::event_row`), as Desktop's New timeline
//! does: their titles and progress stay in that panel.

use rune::{Color, El, TextStyle, text};

use opencode_kit::model::ToolStatus;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed,
    /// A later call to the same agent took over while this one's work was under way.
    Steered,
}

/// One call's run, as the host tracks it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub state: State,
    /// The latest steps, newest last ("Read backoff.ts"), and how many came before them.
    pub steps: Vec<String>,
    pub taken: usize,
    /// How long it has run, or ran, in ms.
    pub ms: Option<i64>,
    /// Its answer's first line, once done.
    pub result: Option<String>,
    /// What went wrong, once failed.
    pub error: Option<String>,
}

impl Run {
    pub fn from_status(status: &ToolStatus) -> Self {
        let state = match status {
            ToolStatus::Running | ToolStatus::Pending => State::Running,
            ToolStatus::Error => State::Failed,
            ToolStatus::Completed => State::Done,
        };
        Run { state, steps: Vec::new(), taken: 0, ms: None, result: None, error: None }
    }

    /// The one line that says where it is: its step while running, else its result or error.
    pub fn line(&self) -> String {
        match self.state {
            State::Running => self.steps.last().cloned().unwrap_or_else(|| opencode_i18n::t!("rune.subagent.starting").to_owned()),
            State::Done => self.result.clone().unwrap_or_else(|| opencode_i18n::t!("session.child.status.succeeded").to_owned()),
            State::Failed => self.error.clone().unwrap_or_else(|| opencode_i18n::t!("session.child.status.failed").to_owned()),
            State::Steered => opencode_i18n::t!("rune.subagent.steered").to_owned(),
        }
    }
}

/// "12s", "1m 05s", "1h 02m" (`formatElapsed`).
pub fn elapsed(ms: i64) -> String {
    let t = (ms / 1000).max(0);
    match t {
        0..60 => format!("{t}s"),
        60..3600 => format!("{}m {:02}s", t / 60, t % 60),
        _ => format!("{}h {:02}m", t / 3600, t / 60 % 60),
    }
}

/// A run time, "12s", in tabular digits. It changes at once, as Desktop's run times do: a digit
/// that rolls each second smears the number while it's read.
pub fn run_time(key: String, ms: i64, font: TextStyle, color: Color) -> El {
    text(elapsed(ms)).key(key).font(font.features("\"tnum\"")).color(color)
}
