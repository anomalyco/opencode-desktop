//! Following one shell's output, as Desktop's `shell-output-machine.ts` does: read from the
//! cursor, wait a second and read again while the shell runs (backing off after failures), and
//! read once more after it stops so its last output lands. A shell the server no longer knows
//! is missing. The caller runs the effects and feeds the results back.

/// Running output is read again this often (`SHELL_POLL_MS`), backing off to at most
/// `SHELL_MAX_BACKOFF_MS` after failures.
const POLL_MS: i64 = 1_000;
const MAX_BACKOFF_MS: i64 = 16_000;
/// Reads of a stopped shell that may fail before it counts as failed (`SHELL_EXITED_MAX_RETRIES`).
const EXITED_MAX_RETRIES: u32 = 3;
/// The output kept (`SHELL_OUTPUT_TAIL_BYTES`).
const TAIL_BYTES: usize = 64 * 1024;

/// What has been read: the cursor the next read starts from and the output's last 64 KiB.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text {
    pub cursor: u64,
    pub output: String,
}

/// `ShellFollowerState`. `last`: the read in flight started after the shell stopped (`final`).
#[derive(Clone, Debug, PartialEq)]
pub enum Tail {
    Reading { attempt: u64, text: Text, running: bool, last: bool, failures: u32 },
    Waiting { attempt: u64, text: Text, running: bool, failures: u32 },
    Complete { text: Text },
    Failed { text: Text },
    Missing { text: Text },
}

/// One page of output (`ShellOutputOutput.data`).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub output: String,
    pub cursor: u64,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TailEvent {
    Running(bool),
    WaitElapsed { attempt: u64 },
    Read { attempt: u64, page: Page },
    ReadFailed { attempt: u64 },
    ReadMissing { attempt: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum TailEffect {
    Read { attempt: u64, cursor: u64 },
    Wait { attempt: u64, ms: i64 },
}

impl Tail {
    pub fn text(&self) -> &Text {
        match self {
            Tail::Reading { text, .. } | Tail::Waiting { text, .. } | Tail::Complete { text } | Tail::Failed { text } | Tail::Missing { text } => text,
        }
    }
}

/// The follower of a shell just shown (`createShellFollowerState` with nothing cached).
pub fn start(running: bool) -> (Tail, Vec<TailEffect>) {
    (Tail::Reading { attempt: 1, text: Text::default(), running, last: !running, failures: 0 }, vec![TailEffect::Read { attempt: 1, cursor: 0 }])
}

/// `transitionShellFollower`.
pub fn step(state: Tail, event: TailEvent) -> (Tail, Vec<TailEffect>) {
    if matches!(state, Tail::Missing { .. }) {
        return (state, Vec::new());
    }
    match event {
        TailEvent::Running(running) => running_changed(state, running),
        TailEvent::WaitElapsed { attempt: a } => match state {
            Tail::Waiting { attempt, text, running, failures } if a == attempt => read(attempt, text, running, failures),
            other => (other, Vec::new()),
        },
        TailEvent::ReadMissing { attempt: a } => match state {
            Tail::Reading { attempt, text, .. } if a == attempt => (Tail::Missing { text }, Vec::new()),
            other => (other, Vec::new()),
        },
        TailEvent::ReadFailed { attempt: a } => match state {
            Tail::Reading { attempt, text, running, failures, .. } if a == attempt => {
                let failures = failures + 1;
                if !running && failures > EXITED_MAX_RETRIES {
                    return (Tail::Failed { text }, Vec::new());
                }
                backoff(attempt, text, running, failures)
            }
            other => (other, Vec::new()),
        },
        TailEvent::Read { attempt: a, page } => match state {
            Tail::Reading { attempt, text, running, last, .. } if a == attempt => {
                let advanced = page.cursor > text.cursor;
                let text = Text { cursor: text.cursor.max(page.cursor), output: keep_tail(text.output + &page.output) };
                let attempt = attempt + 1;
                if running {
                    return (Tail::Waiting { attempt, text, running: true, failures: 0 }, vec![TailEffect::Wait { attempt, ms: POLL_MS }]);
                }
                // A read that started while the shell ran may have missed its last output, so it's read once more.
                if !last || (text.cursor < page.size && advanced) {
                    let cursor = text.cursor;
                    return (Tail::Reading { attempt, text, running: false, last: true, failures: 0 }, vec![TailEffect::Read { attempt, cursor }]);
                }
                (Tail::Complete { text }, Vec::new())
            }
            other => (other, Vec::new()),
        },
    }
}

fn running_changed(state: Tail, running: bool) -> (Tail, Vec<TailEffect>) {
    match state {
        Tail::Complete { text } | Tail::Failed { text } if running => read(0, text, true, 0),
        // The read in flight still counts; the next one decides whether the output is final.
        Tail::Reading { attempt, text, running: was, last, failures } if was != running => {
            (Tail::Reading { attempt, text, running, last: last && !running, failures: if running { failures } else { 0 } }, Vec::new())
        }
        Tail::Waiting { attempt, text, running: was, failures } if was != running => {
            if running {
                return (Tail::Waiting { attempt, text, running, failures }, Vec::new());
            }
            // Stopped between polls or during a backoff: read its final output now.
            read(attempt, text, false, 0)
        }
        other => (other, Vec::new()),
    }
}

fn read(attempt: u64, text: Text, running: bool, failures: u32) -> (Tail, Vec<TailEffect>) {
    let attempt = attempt + 1;
    let cursor = text.cursor;
    (Tail::Reading { attempt, text, running, last: !running, failures }, vec![TailEffect::Read { attempt, cursor }])
}

fn backoff(attempt: u64, text: Text, running: bool, failures: u32) -> (Tail, Vec<TailEffect>) {
    let attempt = attempt + 1;
    let ms = MAX_BACKOFF_MS.min(POLL_MS << (failures.saturating_sub(1)).min(8));
    (Tail::Waiting { attempt, text, running, failures }, vec![TailEffect::Wait { attempt, ms }])
}

fn keep_tail(mut s: String) -> String {
    if s.len() > TAIL_BYTES {
        let cut = (s.len() - TAIL_BYTES..s.len()).find(|&i| s.is_char_boundary(i)).unwrap_or(s.len());
        s.drain(..cut);
    }
    s
}
