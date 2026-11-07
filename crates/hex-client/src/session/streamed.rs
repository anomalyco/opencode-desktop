//! A Windows Hex from before its capture API: the app records the default microphone (`mic.rs`)
//! and pushes it to one of Hex's live streams every 100 ms. Hex's events give the words so far
//! while it listens; the stop keeps the microphone open for `TAIL`, pushes the rest, and waits
//! for Hex's final text.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::{Cmd, Heard, Problem, TAIL};
use crate::mic::Mic;
use crate::{BACKLOG_FULL, Hex, NO_ENGINE, NO_MODEL, Stream, StreamEvent};

/// How often the recording goes to Hex.
const PUSH_EVERY: Duration = Duration::from_millis(100);
/// How long a Hex that's just started has to load its model.
const WARM_UP: Duration = Duration::from_secs(30);
/// A quiet stream's heartbeat; Hex ends one that hears nothing for 10 s.
const BEAT: Duration = Duration::from_secs(3);

pub(super) fn run(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool, heard: &mut dyn FnMut(Heard)) {
    let mic = match Mic::open() {
        Ok(m) => m,
        Err(p) => return heard(Heard::Failed(p)),
    };
    let stream = match create(hex, cmds, warming) {
        Some(Ok(s)) => s,
        Some(Err(why)) => return heard(Heard::Failed(Problem::of(why))),
        None => return,
    };
    let events = follow(hex, &stream);
    heard(Heard::Ready);
    let mut feed = Feed { pending: Vec::new(), beat: Instant::now() };
    let mut live = Live::default();
    loop {
        match cmds.recv_timeout(PUSH_EVERY) {
            Ok(Cmd::Stop) => break,
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return hex.cancel_stream(&stream),
            Err(RecvTimeoutError::Timeout) => {}
        }
        let step = mic.failure().map_or(Ok(()), |why| Err(Problem::of(why))).and_then(|()| {
            feed.pending.extend(mic.take());
            feed.send(hex, &stream)?;
            events.try_iter().try_for_each(|e| live.hear(e).map(|h| h.into_iter().for_each(&mut *heard)))
        });
        if let Err(p) = step {
            hex.cancel_stream(&stream);
            return heard(Heard::Failed(p));
        }
    }
    std::thread::sleep(TAIL);
    feed.pending.extend(mic.take());
    drop(mic);
    heard(match finish(hex, &stream, &mut feed) {
        Ok(words) if words.trim().is_empty() => Heard::Failed(Problem::of("Hex heard nothing")),
        Ok(words) => Heard::Words(words),
        Err(p) => Heard::Failed(p),
    });
}

/// Starts the stream. A Hex this dictation started (`warming`) serves its API before its
/// listener has loaded the model, so it's asked again until it's ready. None when the app
/// cancelled meanwhile; the recording so far stays in the microphone.
fn create(hex: &Hex, cmds: &Receiver<Cmd>, warming: bool) -> Option<Result<Stream, String>> {
    let since = Instant::now();
    loop {
        match hex.stream() {
            Err(why) if warming && (why == NO_ENGINE || why == NO_MODEL) && since.elapsed() < WARM_UP => {}
            other => return Some(other),
        }
        match cmds.recv_timeout(Duration::from_millis(500)) {
            Ok(Cmd::Cancel) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(Cmd::Stop) => return Some(Err("Hex is still starting".into())),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// The stream's events on their own thread, as they come.
fn follow(hex: &Hex, stream: &Stream) -> Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel();
    // Without events the words still come from the finish; only the partials are missed.
    if let Ok(events) = hex.stream_events(stream) {
        std::thread::spawn(move || events.take_while(|e| tx.send(e.clone()).is_ok()).for_each(drop));
    }
    rx
}

/// Recorded samples not yet with Hex, and when Hex last heard from the app.
struct Feed {
    pending: Vec<f32>,
    beat: Instant,
}

impl Feed {
    /// Pushes what's pending, or a heartbeat when there's nothing. A full backlog keeps the
    /// samples for the next try.
    fn send(&mut self, hex: &Hex, stream: &Stream) -> Result<(), Problem> {
        let sent = if self.pending.is_empty() {
            if self.beat.elapsed() < BEAT {
                return Ok(());
            }
            hex.stream_heartbeat(stream)
        } else {
            hex.push(stream, &self.pending)
        };
        match sent {
            Ok(()) => {
                self.pending.clear();
                self.beat = Instant::now();
                Ok(())
            }
            Err(code) if code == BACKLOG_FULL => Ok(()),
            Err(code) => Err(Problem::of(ended(&code, ""))),
        }
    }
}

/// Pushes the rest and waits for the whole text, asking again while Hex is still busy.
fn finish(hex: &Hex, stream: &Stream, feed: &mut Feed) -> Result<String, Problem> {
    for _ in 0..20 {
        if feed.pending.is_empty() {
            break;
        }
        feed.send(hex, stream)?;
        if !feed.pending.is_empty() {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    for _ in 0..3 {
        match hex.finish_stream(stream) {
            Ok(fin) => return Ok(fin.text),
            Err(code) if code == "stream-finish-timeout" => {}
            Err(code) => return Err(Problem::of(ended(&code, ""))),
        }
    }
    Err(Problem::of("Hex took too long to transcribe"))
}

/// The words so far: the finished windows, then the one Hex is hearing.
#[derive(Default)]
struct Live {
    segments: Vec<String>,
    shown: (String, String),
}

impl Live {
    /// What an event changes, if anything; Hex's error ends the dictation.
    fn hear(&mut self, e: StreamEvent) -> Result<Option<Heard>, Problem> {
        let (stable, tentative) = match e {
            StreamEvent::Partial { stable, tentative, .. } => (self.joined(&stable), tentative),
            StreamEvent::Segment(s) => {
                self.segments.push(s.text);
                (self.joined(""), String::new())
            }
            StreamEvent::Error { code, message } => return Err(Problem::of(ended(&code, &message))),
            StreamEvent::Final(_) | StreamEvent::Cancelled => return Ok(None),
        };
        if (&stable, &tentative) == (&self.shown.0, &self.shown.1) {
            return Ok(None);
        }
        self.shown = (stable.clone(), tentative.clone());
        Ok(Some(Heard::Partial { stable, tentative }))
    }

    fn joined(&self, current: &str) -> String {
        let mut parts: Vec<&str> = self.segments.iter().map(|s| s.trim()).collect();
        parts.push(current.trim_start());
        parts.retain(|p| !p.is_empty());
        parts.join(" ")
    }
}

/// The app's words for the code a stream ended with (Hex's `docs/streams.md`).
fn ended(code: &str, message: &str) -> String {
    match code {
        "stream-ended" | "stream-not-found" | "lease-expired" => "Hex stopped listening".into(),
        "model-changed" => "Hex's model changed: dictate again".into(),
        "transcription-failed" => "Hex couldn't transcribe this".into(),
        "hex-quitting" | "shutting-down" => "Hex quit".into(),
        "stream-cancelled" => "Hex cancelled the recording".into(),
        _ if code.contains("running") => code.into(),
        _ if !message.is_empty() => format!("Hex: {message}"),
        _ => format!("Hex couldn't finish: {code}"),
    }
}
