//! Files copying to a remote server before a send can name them, Desktop's `transitionUploads`
//! (`packages/app/src/composer/attachments/upload-machine.ts`): each upload has its own id and the
//! attempt it started with, so an answer for an upload that was cancelled changes nothing. An
//! upload is `queued` until its body starts moving, `sending` while it moves, and `sent` once all
//! of it has gone and it waits on the server's answer. Only `sending` can stall: a body that
//! stops advancing for [`STALL`] fails instead of holding sends back. While any is listed,
//! sending waits.

use rune::machine::{Attempt, Effects, Seq};

/// `UPLOAD_STALL_TIMEOUT_MS`.
pub const STALL: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    pub id: String,
    pub attempt: Attempt,
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub loaded: u64,
    pub transfer: Transfer,
}

impl Upload {
    /// `uploadPercent`: how much has gone, rounded down; all of an empty file.
    pub fn percent(&self) -> u32 {
        if self.size == 0 { 100 } else { ((self.loaded as f64 / self.size as f64) * 100.0).floor() as u32 }
    }
}

/// `UploadEntry.transfer`; a moving body carries the time it stalls at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transfer {
    Queued,
    Sending { stall: f64 },
    Sent,
}

/// `UploadMachineState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Uploads {
    #[default]
    Idle,
    /// Never empty, in start order.
    Uploading(Vec<Upload>),
}

pub enum Event<'a> {
    Started { id: &'a str, name: &'a str, mime: &'a str, size: u64 },
    /// The body has gone this far by `now` (`upload.progress`).
    Progress { attempt: Attempt, loaded: u64, now: f64 },
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt },
    /// The window clock reached `now`: a body that stopped moving by then has stalled
    /// (`upload.timed-out`).
    Tick { now: f64 },
    Cancelled { id: &'a str },
}

/// Why an upload was turned down (`upload.reject`'s error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    Failed,
    /// `UPLOAD_TIMED_OUT`: it carries no words of its own.
    TimedOut,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Starts the copy for this attempt (`upload.run`).
    Run(Attempt),
    /// The stall timer: wake then (`timer.reset`).
    WakeAt(f64),
    /// Stops the copy, whatever it answers ignored (`upload.abort`).
    Abort(Attempt),
    /// Its answer lands in the composer, or nothing for a cancelled one (`upload.resolve`).
    Resolve(Attempt),
    /// Its failure shows (`upload.reject`).
    Reject(Attempt, Reject),
}

impl Uploads {
    pub fn list(&self) -> &[Upload] {
        match self {
            Uploads::Idle => &[],
            Uploads::Uploading(list) => list,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Uploads::Idle)
    }

    /// When the first moving body stalls.
    pub fn next_stall(&self) -> Option<f64> {
        self.list().iter().filter_map(|u| if let Transfer::Sending { stall } = u.transfer { Some(stall) } else { None }).min_by(f64::total_cmp)
    }
}

pub fn step(seq: &mut Seq, state: &mut Uploads, event: Event<'_>) -> Effects<Effect> {
    let mut list = match std::mem::take(state) {
        Uploads::Idle => Vec::new(),
        Uploads::Uploading(list) => list,
    };
    let effects = match event {
        Event::Started { id, name, mime, size } if !list.iter().any(|u| u.id == id) => {
            let attempt = seq.next();
            list.push(Upload { id: id.to_owned(), attempt, name: name.to_owned(), mime: mime.to_owned(), size, loaded: 0, transfer: Transfer::Queued });
            Effects::one(Effect::Run(attempt))
        }
        Event::Started { .. } => Effects::none(),
        Event::Progress { attempt, loaded, now } => progress(&mut list, attempt, loaded, now),
        Event::Succeeded { attempt } => finish(&mut list, |u| u.attempt == attempt, |a| vec![Effect::Resolve(a)]),
        Event::Failed { attempt } => finish(&mut list, |u| u.attempt == attempt, |a| vec![Effect::Reject(a, Reject::Failed)]),
        Event::Tick { now } => {
            let stalled = |u: &Upload| matches!(u.transfer, Transfer::Sending { stall } if now + 1e-9 >= stall);
            let mut effects = Effects::none();
            while list.iter().any(stalled) {
                effects.extend(finish(&mut list, stalled, |a| vec![Effect::Abort(a), Effect::Reject(a, Reject::TimedOut)]));
            }
            effects
        }
        // A cancelled upload settles without its work, which may wait where it can't see the abort.
        Event::Cancelled { id } => finish(&mut list, |u| u.id == id, |a| vec![Effect::Abort(a), Effect::Resolve(a)]),
    };
    *state = if list.is_empty() { Uploads::Idle } else { Uploads::Uploading(list) };
    effects
}

fn progress(list: &mut [Upload], attempt: Attempt, loaded: u64, now: f64) -> Effects<Effect> {
    let Some(entry) = list.iter_mut().find(|u| u.attempt == attempt) else { return Effects::none() };
    let loaded = loaded.max(entry.loaded).min(entry.size);
    let sending = loaded != entry.size;
    let was_sending = matches!(entry.transfer, Transfer::Sending { .. });
    if loaded == entry.loaded && sending == was_sending && (sending || entry.transfer == Transfer::Sent) {
        return Effects::none();
    }
    entry.loaded = loaded;
    if sending {
        let stall = now + STALL;
        entry.transfer = Transfer::Sending { stall };
        return Effects::one(Effect::WakeAt(stall));
    }
    entry.transfer = Transfer::Sent;
    Effects::none()
}

fn finish(list: &mut Vec<Upload>, which: impl Fn(&Upload) -> bool, effects: impl FnOnce(Attempt) -> Vec<Effect>) -> Effects<Effect> {
    let Some(i) = list.iter().position(which) else { return Effects::none() };
    let mut out = Effects::none();
    out.extend(effects(list.remove(i).attempt));
    out
}
