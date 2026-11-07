//! Hex's capture API: Hex records its own microphone, the app keeps the capture alive until the
//! stop or the cancel, then waits for the whole transcript (no partials). A pause never ends it,
//! and Hex pastes nothing.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::{Cmd, Heard, Problem, TAIL};
use crate::Hex;

/// `_warming`: a capture records before Hex transcribes, so a Hex that just started needs no wait.
pub(super) fn run(hex: &Hex, cmds: &Receiver<Cmd>, _warming: bool, heard: &mut dyn FnMut(Heard)) {
    let cap = match hex.start() {
        Ok(c) => c,
        Err(why) => return heard(Heard::Failed(Problem::of(why))),
    };
    heard(Heard::Ready);
    let mut beat = Instant::now();
    let mut missed = 0;
    loop {
        match cmds.recv_timeout(Duration::from_millis(250)) {
            Ok(Cmd::Stop) => break,
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return hex.cancel(&cap),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if beat.elapsed() >= Duration::from_secs(3) {
            beat = Instant::now();
            match hex.heartbeat(&cap) {
                Ok(()) => missed = 0,
                // Hex ended it (Escape in Hex, or it restarted): there's nothing to finish.
                Err(code) if code.starts_with("dictation-") => return heard(Heard::Failed(Problem::of("Hex stopped listening"))),
                Err(_) => {
                    missed += 1;
                    if missed >= 3 {
                        return heard(Heard::Failed(Problem::of("Lost Hex while listening")));
                    }
                }
            }
        }
    }
    std::thread::sleep(TAIL);
    heard(match hex.finish(&cap) {
        Ok(words) if words.trim().is_empty() => Heard::Failed(Problem::of("Hex heard nothing")),
        Ok(words) => Heard::Words(words),
        Err(why) => Heard::Failed(Problem::of(why)),
    });
}
