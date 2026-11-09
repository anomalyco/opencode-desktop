//! Files copying to a remote server before a send can name them, Desktop's `transitionUploads`
//! (`packages/app/src/composer/attachments/upload-machine.ts`): each upload has its own id (the
//! file's path, so the same file twice starts nothing) and the attempt it started with, so two
//! files of one name no longer share an entry, and an answer for an upload that was cancelled
//! changes nothing. While any is listed, sending waits.
//!
//! Desktop's stall timer runs only while a body is `sending`, between progress events; the
//! server client here reports none, so an upload stays `queued` until it answers, as Desktop's
//! does before its first progress event.

use rune::machine::{Attempt, Effects, Seq};

#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    /// The local path.
    pub id: String,
    pub attempt: Attempt,
    pub name: String,
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
    Started { id: &'a str, name: &'a str },
    Succeeded { attempt: Attempt },
    Failed { attempt: Attempt },
    Cancelled { id: &'a str },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Starts the copy for this attempt (`upload.run`).
    Run(Attempt),
    /// Its answer lands in the composer (`upload.resolve`).
    Resolve(Attempt),
    /// Its failure shows (`upload.reject`).
    Reject(Attempt),
    /// It's dropped: whatever the copy answers is ignored (`upload.abort`).
    Abort(Attempt),
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
}

pub fn step(seq: &mut Seq, state: &mut Uploads, event: Event<'_>) -> Effects<Effect> {
    let mut list = match std::mem::take(state) {
        Uploads::Idle => Vec::new(),
        Uploads::Uploading(list) => list,
    };
    let effects = match event {
        Event::Started { id, name } if !list.iter().any(|u| u.id == id) => {
            let attempt = seq.next();
            list.push(Upload { id: id.to_owned(), attempt, name: name.to_owned() });
            Effects::one(Effect::Run(attempt))
        }
        Event::Started { .. } => Effects::none(),
        Event::Succeeded { attempt } => finish(&mut list, |u| u.attempt == attempt, Effect::Resolve),
        Event::Failed { attempt } => finish(&mut list, |u| u.attempt == attempt, Effect::Reject),
        Event::Cancelled { id } => finish(&mut list, |u| u.id == id, Effect::Abort),
    };
    *state = if list.is_empty() { Uploads::Idle } else { Uploads::Uploading(list) };
    effects
}

fn finish(list: &mut Vec<Upload>, which: impl Fn(&Upload) -> bool, effect: fn(Attempt) -> Effect) -> Effects<Effect> {
    let Some(i) = list.iter().position(which) else { return Effects::none() };
    Effects::one(effect(list.remove(i).attempt))
}
