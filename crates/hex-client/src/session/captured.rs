//! Hex's capture API: Hex records the microphone the user chose in Hex, the app keeps the
//! capture alive until the stop or the cancel, then waits for the whole transcript. Asked for,
//! Hex also sends the words so far while its model streams. A pause never ends it, and Hex
//! pastes nothing.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

// The page's clock in a browser, where std's panics; std's own natively.
use web_time::Instant;

use super::{Cmd, Heard, Problem, TAIL};
use crate::{Capture, Hex, NO_ENGINE};

/// How long a Hex that's just started has to load its model before its listener records.
const WARM_UP: Duration = Duration::from_secs(30);

/// `records`: this Hex said it records captures (`Hex::start`). `warming`: this dictation
/// started that Hex, whose listener records only once its model has loaded, so the start is
/// asked again until then.
pub(super) fn run(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool, records: bool, heard: &mut dyn FnMut(Heard)) {
    let cap = match start(hex, cmds, warming, records) {
        Some(Ok(c)) => c,
        Some(Err(why)) => return heard(Heard::Failed(Problem::of(why))),
        None => return,
    };
    let words = follow(hex, &cap);
    heard(Heard::Ready);
    let mut beat = Instant::now();
    let mut missed = 0;
    let mut shown = (String::new(), String::new());
    loop {
        match cmds.recv_timeout(Duration::from_millis(250)) {
            Ok(Cmd::Stop) => break,
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return hex.cancel(&cap),
            Err(RecvTimeoutError::Timeout) => {}
        }
        // Only the newest words matter; each carries the whole text so far.
        if let Some(latest) = words.try_iter().last()
            && latest != shown
        {
            shown = latest.clone();
            heard(Heard::Partial { stable: latest.0, tentative: latest.1 });
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

/// Starts the capture, asking again while a Hex this dictation started loads its model. None
/// when the app cancelled meanwhile.
fn start(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool, records: bool) -> Option<Result<Capture, String>> {
    let since = Instant::now();
    loop {
        match hex.start(records) {
            Err(why) if warming && why == NO_ENGINE && since.elapsed() < WARM_UP => {}
            other => return Some(other),
        }
        match cmds.recv_timeout(Duration::from_millis(500)) {
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(Cmd::Stop) => return Some(Err("Hex is still starting".into())),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// The capture's words so far on their own thread, as they come.
fn follow(hex: &Hex, cap: &Capture) -> Receiver<(String, String)> {
    let (tx, rx) = mpsc::channel();
    // A Hex without them (macOS, or older) sends none, and the words come from the finish.
    if let Ok(words) = hex.partials(cap) {
        std::thread::spawn(move || words.take_while(|w| tx.send(w.clone()).is_ok()).for_each(drop));
    }
    rx
}
