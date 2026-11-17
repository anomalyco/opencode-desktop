//! Each file's reads, as Desktop's `packages/app/src/workspaces/files/content-machine.ts`: one
//! read at a time per file, on a worker. A change reported while a read runs (`force`) asks for
//! one more when it lands, so a file an agent writes fifty times costs a read in flight and one
//! waiting, never fifty. Files read side by side: two opened together both land.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use opencode_kit::client::Server;
use rune::Waker;
use rune::time::Instant;

use super::data::{FilePayload, fetch_live_file_payload};
use super::{delimited, html};
use crate::code_view::file_state::Prepared;

/// One file's reads; a file with none running whose text is current has no entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Read {
    /// A read runs on a worker. `rerun`: the file changed since it started, so one more follows.
    Reading { attempt: u64, rerun: bool },
    /// What shows may be outdated (a refresh that failed, a change nobody was looking at): the
    /// next ask reads.
    Stale,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ReadEvent {
    /// The file is wanted. `force`: it may have changed (on disk, or while the server was away),
    /// so it reads even when loaded. `loaded`: its text (or state) is there already.
    Asked { force: bool, loaded: bool },
    /// The read `attempt` came back.
    Landed { attempt: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadEffect {
    Nothing,
    /// Start a read numbered `attempt`.
    Start { attempt: u64 },
    /// Show what came back.
    Show,
    /// Show what came back if it's the file's content (a failure is outdated already), then read
    /// again as `attempt`.
    ShowThenStart { attempt: u64 },
    /// An answer to a read since superseded: dropped.
    Drop,
}

/// The transition; `next` numbers a read it starts.
pub(crate) fn step(state: Option<Read>, event: ReadEvent, next: u64) -> (Option<Read>, ReadEffect) {
    match (state, event) {
        (Some(Read::Reading { attempt, rerun }), ReadEvent::Asked { force, .. }) => {
            (Some(Read::Reading { attempt, rerun: rerun || force }), ReadEffect::Nothing)
        }
        (None, ReadEvent::Asked { force: false, loaded: true }) => (None, ReadEffect::Nothing),
        (Some(Read::Stale) | None, ReadEvent::Asked { .. }) => (Some(Read::Reading { attempt: next, rerun: false }), ReadEffect::Start { attempt: next }),
        (Some(Read::Reading { attempt, rerun }), ReadEvent::Landed { attempt: landed }) if attempt == landed => {
            if rerun {
                (Some(Read::Reading { attempt: next, rerun: false }), ReadEffect::ShowThenStart { attempt: next })
            } else {
                (None, ReadEffect::Show)
            }
        }
        (state, ReadEvent::Landed { .. }) => (state, ReadEffect::Drop),
    }
}

/// A read's answer, made ready on its worker.
pub(crate) enum Body {
    Text(Prepared),
    /// An SVG: its source, and its bytes for the picture.
    Svg(Prepared, Vec<u8>),
    Html(Prepared, Arc<html::Page>),
    Table(Prepared, Arc<delimited::Table>),
    Binary(usize),
    Image(Vec<u8>),
    Missing,
    Failed(opencode_kit::wire_error::WireError),
}

impl Body {
    /// Everything proportional to the file's size happens here, off the UI thread.
    fn prepare(path: &str, payload: FilePayload, find: Option<String>) -> Body {
        match payload {
            FilePayload::Text(text) if super::image::is_svg(path) => {
                let bytes = text.as_bytes().to_vec();
                Body::Svg(Prepared::new(text, find), bytes)
            }
            FilePayload::Text(text) => Body::Text(Prepared::new(text, find)),
            FilePayload::Html { source, document } => Body::Html(Prepared::new(source, find), document),
            FilePayload::Table { source, document } => Body::Table(Prepared::new(source, find), document),
            FilePayload::Binary(size) => Body::Binary(size),
            FilePayload::Image(bytes) => Body::Image(bytes),
            FilePayload::Missing => Body::Missing,
            FilePayload::Failed(e) => Body::Failed(e),
        }
    }

    /// The file's own content (text, binary or a picture), not a missing file or a failure.
    pub(crate) fn content(&self) -> bool {
        !matches!(self, Body::Missing | Body::Failed(_))
    }
}

/// One read that came back.
pub(crate) struct Landed {
    pub sid: String,
    pub path: String,
    pub attempt: u64,
    pub body: Body,
    /// When the worker started reading, had the bytes, and had them ready.
    pub times: [Instant; 3],
}

/// Every file's reads in this window.
pub(crate) struct Reads {
    states: HashMap<(String, String), Read>,
    next: u64,
    sender: Sender<Landed>,
    landed: Receiver<Landed>,
    waker: Option<Waker>,
}

impl Default for Reads {
    fn default() -> Self {
        let (sender, landed) = channel();
        Self { states: HashMap::new(), next: 0, sender, landed, waker: None }
    }
}

impl Reads {
    pub(crate) fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    fn step(&mut self, sid: &str, path: &str, event: ReadEvent) -> ReadEffect {
        let key = (sid.to_owned(), path.to_owned());
        let (state, effect) = step(self.states.get(&key).copied(), event, self.next + 1);
        if matches!(effect, ReadEffect::Start { .. } | ReadEffect::ShowThenStart { .. }) {
            self.next += 1;
        }
        match state {
            Some(s) => self.states.insert(key, s),
            None => self.states.remove(&key),
        };
        effect
    }

    /// `path` is wanted: the attempt to start, if a read should start now.
    pub(crate) fn ask(&mut self, sid: &str, path: &str, force: bool, loaded: bool) -> Option<u64> {
        match self.step(sid, path, ReadEvent::Asked { force, loaded }) {
            ReadEffect::Start { attempt } => Some(attempt),
            _ => None,
        }
    }

    /// What to do with a read that came back.
    pub(crate) fn land(&mut self, landed: &Landed) -> ReadEffect {
        self.step(&landed.sid, &landed.path, ReadEvent::Landed { attempt: landed.attempt })
    }

    /// What shows is outdated; the next ask reads (a refresh that failed).
    pub(crate) fn outdate(&mut self, sid: &str, path: &str) {
        let key = (sid.to_owned(), path.to_owned());
        if !self.states.contains_key(&key) {
            self.states.insert(key, Read::Stale);
        }
    }

    pub(crate) fn stale(&self, sid: &str, path: &str) -> bool {
        self.states.get(&(sid.to_owned(), path.to_owned())) == Some(&Read::Stale)
    }

    /// A closed session's reads: their answers are dropped when they come back.
    pub(crate) fn forget_session(&mut self, sid: &str) {
        self.states.retain(|(s, _), _| s != sid);
    }

    pub(crate) fn take(&self) -> Option<Landed> {
        self.landed.try_recv().ok()
    }

    /// Reads `path` of `directory` on a worker as `attempt`; `find` is Find's query while it's
    /// open, so its matches are found there too. With no thread to read on, the read is
    /// forgotten, so the next ask tries again.
    pub(crate) fn start(&mut self, server: &Server, directory: &str, sid: &str, path: &str, attempt: u64, find: Option<String>) {
        let key = (sid.to_owned(), path.to_owned());
        let (server, directory, sid, path) = (server.clone(), directory.to_owned(), sid.to_owned(), path.to_owned());
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        let spawned = rune::task::spawn("file-read", move || {
            let started = Instant::now();
            let payload = fetch_live_file_payload(&server, &directory, &path);
            let fetched = Instant::now();
            let body = Body::prepare(&path, payload, find);
            let times = [started, fetched, Instant::now()];
            if sender.send(Landed { sid, path, attempt, body, times }).is_ok()
                && let Some(w) = waker
            {
                w();
            }
        });
        if let Err(e) = spawned {
            eprintln!("file read didn't start: {e}");
            self.states.remove(&key);
        }
    }
}
