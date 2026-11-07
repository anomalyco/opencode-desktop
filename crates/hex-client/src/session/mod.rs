//! One dictation, from the press to the words, on its own thread: finds Hex (starting an
//! installed Hex that isn't running), records through the transport this OS's Hex offers, and
//! reports what it heard.
//!
//! - Hex's capture API (`captured.rs`), wherever Hex has it: Hex records the microphone the user
//!   chose in Hex, sends the words so far where it offers them, and answers the whole transcript
//!   when the app finishes.
//! - Windows Hex from before it had one: the app records the default microphone itself and
//!   pushes it to a live stream (`streamed.rs`), which also sends partial text while it listens.

use std::path::Path;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::install::{Fix, Presence};
use crate::{Hex, NOT_RUNNING, OUTDATED};

mod captured;
#[cfg(windows)]
mod streamed;

/// How long the microphone stays open after the stop, so a last word said as the stop is
/// pressed lands. Hex's own shortcut keeps 200 ms after release.
pub const TAIL: Duration = Duration::from_millis(350);
/// How long a Hex this started has to publish its endpoint.
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

/// What the app tells a dictation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Finish: the words come back.
    Stop,
    /// Throw the recording away.
    Cancel,
}

/// What a dictation heard, in order: `Ready`, any `Partial`s, then `Words` or `Failed`. A
/// cancelled one ends with nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Heard {
    /// Hex is listening.
    Ready,
    /// The words so far while it listens: `stable` won't change, `tentative` may.
    Partial { stable: String, tentative: String },
    /// The finished transcript.
    Words(String),
    Failed(Problem),
}

/// Why a dictation couldn't happen, as the app says it, and what it offers to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub says: String,
    pub fix: Option<Fix>,
}

pub const NOT_INSTALLED: &str = "Hex isn't installed";
pub const MIC_BLOCKED: &str = "Windows is blocking microphone access";
pub const NO_MICROPHONE: &str = "No microphone found";

impl Problem {
    pub fn not_installed() -> Problem {
        Problem { says: NOT_INSTALLED.into(), fix: Some(Fix::Install) }
    }

    /// Hex's or the microphone's words, with the fix they call for: open Hex when it isn't
    /// running or isn't ready, update an old one.
    pub fn of(says: impl Into<String>) -> Problem {
        let says = says.into();
        let fix = match says.as_str() {
            NOT_RUNNING | crate::NO_MODEL | crate::NO_ENGINE => Some(Fix::OpenHex),
            NOT_INSTALLED | OUTDATED => Some(Fix::Install),
            MIC_BLOCKED => Some(Fix::MicSettings),
            _ => None,
        };
        Problem { says, fix }
    }
}

/// Runs one dictation with Hex behind `file` (its data folder `support`), recording for
/// `source`, until `Cmd::Stop` gets the words or `Cmd::Cancel` (or the sender going away)
/// throws them away.
pub fn dictate(file: &Path, support: &Path, source: &str, cmds: &Receiver<Cmd>, heard: &mut dyn FnMut(Heard)) {
    let warming = match crate::install::presence(file, support) {
        Presence::Missing if crate::install::MIC_WITHOUT_HEX => return heard(Heard::Failed(Problem::not_installed())),
        Presence::Missing => return heard(Heard::Failed(Problem::of(NOT_RUNNING))),
        Presence::Stopped => match start_hex(file, support, cmds) {
            Some(Ok(())) => true,
            Some(Err(p)) => return heard(Heard::Failed(p)),
            None => return,
        },
        Presence::Running => false,
    };
    match Hex::connect_at(file, source) {
        Ok(hex) => run(&hex, cmds, warming, heard),
        Err(why) => heard(Heard::Failed(Problem::of(why))),
    }
}

/// Hex's own capture: every Hex off Windows has one.
#[cfg(not(windows))]
fn run(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool, heard: &mut dyn FnMut(Heard)) {
    captured::run(hex, cmds, warming, false, heard);
}

/// Hex's own capture when this Windows Hex says it has one, else a stream the app records for.
#[cfg(windows)]
fn run(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool, heard: &mut dyn FnMut(Heard)) {
    match hex.capabilities() {
        Some(c) if c.capture => captured::run(hex, cmds, warming, true, heard),
        _ => streamed::run(hex, cmds, warming, heard),
    }
}

/// Starts the installed Hex in the background and waits for its endpoint. None when the app
/// cancelled meanwhile.
fn start_hex(file: &Path, support: &Path, cmds: &Receiver<Cmd>) -> Option<Result<(), Problem>> {
    if let Err(why) = crate::install::launch(support, false) {
        return Some(Err(Problem { says: why, fix: Some(Fix::OpenHex) }));
    }
    let since = Instant::now();
    while !file.exists() {
        if since.elapsed() > LAUNCH_WAIT {
            return Some(Err(Problem::of(NOT_RUNNING)));
        }
        match cmds.recv_timeout(Duration::from_millis(100)) {
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return None,
            // A stop before Hex is up has nothing to finish.
            Ok(Cmd::Stop) => return Some(Err(Problem::of("Hex is still starting"))),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    // Hex writes its endpoint as it starts listening; give it a moment to answer.
    std::thread::sleep(Duration::from_millis(300));
    Some(Ok(()))
}
