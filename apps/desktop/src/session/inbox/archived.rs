//! The inbox's Archived group: archiving a session and restoring it, and the one read of the
//! server's newest archived root sessions (the session list leaves them out). Desktop's first
//! inbox listed its archive under Recent (`packages/app/src/shell/titlebar/inbox.tsx` before
//! #61445 put it behind the Archived button's search); the group is that list again, behind a
//! header that opens and shuts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use opencode_kit::model::SessionInfo;
use rune::Tx;

use super::model::{BRANCH_RETRY_DELAY, BRANCH_RETRY_LIMIT, BRANCH_RETRY_MAX_DELAY};
use crate::{App, Source};

impl App {
    /// Archive: the session leaves the list and its tabs at once (`inbox.archive`), for the
    /// Archived group.
    pub fn archive_session(&mut self, id: &str, tx: &mut Tx) {
        let now = *self.now_ms;
        let Some(info) = self.sessions.iter_mut().find(|s| s.id == id) else { return };
        info.archived = Some(now);
        let kept = info.clone();
        let here = crate::servers::runtime::active(self).clone();
        // The archive read holds it too: a session list read again leaves it out.
        if self.sidebar_inbox.archived_on.as_ref() == Some(&here) {
            self.sidebar_inbox.archived.retain(|a| a.id != id);
            self.sidebar_inbox.archived.insert(0, kept);
        }
        if let Some(uid) = self.tabs.iter().find(|t| t.is(&here, id) && !t.is_draft()).map(crate::tabs::Tab::uid) {
            self.close_tab(&uid, tx);
        }
        self.write(opencode_kit::client::Write::Archive(id.to_owned()));
        // Beyond Desktop: the toast offers it back, tab and all.
        crate::undo::archived(self, here, id.to_owned());
    }

    /// Unarchive: it comes back to its group (`inbox.unarchive`).
    pub fn unarchive_session(&mut self, id: &str) {
        self.unarchive_session_locally(id);
        self.write(opencode_kit::client::Write::Unarchive(id.to_owned()));
    }

    /// Shows the session as restored: off the archive read and not archived. One only the
    /// archive read held joins the session list.
    pub(crate) fn unarchive_session_locally(&mut self, id: &str) {
        let read = self.sidebar_inbox.archived.iter().position(|a| a.id == id).map(|at| self.sidebar_inbox.archived.remove(at));
        match self.sessions.iter_mut().find(|s| s.id == id) {
            Some(info) => info.archived = None,
            None => self.sessions.extend(read.map(|info| SessionInfo { archived: None, ..info })),
        }
    }

    /// The archive read's root sessions the session list doesn't hold, while the window works
    /// against the server they were read from: the rest of the Archived group's rows.
    pub(crate) fn archived_read(&self) -> impl Iterator<Item = &SessionInfo> {
        let here = self.sidebar_inbox.archived_on.as_ref() == Some(crate::servers::runtime::active(self));
        self.sidebar_inbox.archived.iter().filter(move |a| here && a.archived.is_some() && a.parent.is_none() && self.session(&a.id).is_none())
    }

    /// A row the archive read brought joins the session list as it is, so it opens, renames,
    /// restores and deletes as the list's own rows do.
    pub(crate) fn adopt_archived(&mut self, id: &str) {
        if self.session(id).is_none()
            && let Some(info) = self.sidebar_inbox.archived.iter().find(|a| a.id == id).cloned()
        {
            self.sessions.push(info);
        }
    }
}

type Slot = Arc<Mutex<Option<Result<Vec<SessionInfo>, ()>>>>;

/// One-shot fetch of the newest archived root sessions (`ARCHIVED_LIMIT`): mirrors
/// [`super::model::Branches`]'s bounded backoff retries (`BRANCH_RETRY_LIMIT`) on failure before
/// resting as `Failed`.
#[derive(Default)]
pub enum Fetch { #[default] Idle, Loading { failures: u32, landed: Slot }, Retrying { failures: u32, due: f64 }, Loaded, Failed }
enum Event { Poll { now: f64 }, Landed { result: Result<Vec<SessionInfo>, ()>, now: f64 }, SpawnFailed }
enum Effect { Spawn(Slot), WakeAfter(Duration), Loaded(Vec<SessionInfo>) }

impl Fetch {
    fn begin(&mut self, failures: u32) -> Option<Effect> {
        let slot: Slot = Arc::new(Mutex::new(None));
        *self = Self::Loading { failures, landed: Arc::clone(&slot) };
        Some(Effect::Spawn(slot))
    }

    fn step(&mut self, event: Event) -> Option<Effect> {
        match (std::mem::take(self), event) {
            (Self::Idle, Event::Poll { .. }) => self.begin(0),
            (Self::Retrying { failures, due }, Event::Poll { now }) if due <= now + 1e-9 => self.begin(failures),
            (Self::Retrying { failures, due }, Event::Poll { now }) => {
                *self = Self::Retrying { failures, due };
                Some(Effect::WakeAfter(Duration::from_secs_f64((due - now).max(0.0))))
            }
            (Self::Loading { failures, .. }, Event::Landed { result, now }) => match result {
                Ok(list) => { *self = Self::Loaded; Some(Effect::Loaded(list)) }
                Err(()) if failures >= BRANCH_RETRY_LIMIT => { *self = Self::Failed; None }
                Err(()) => {
                    let delay = (BRANCH_RETRY_DELAY * f64::from(1u32 << failures)).min(BRANCH_RETRY_MAX_DELAY);
                    *self = Self::Retrying { failures: failures + 1, due: now + delay };
                    Some(Effect::WakeAfter(Duration::from_secs_f64(delay)))
                }
            },
            (Self::Loading { failures: 0, .. }, Event::SpawnFailed) => { *self = Self::Idle; None }
            (Self::Loading { failures, .. }, Event::SpawnFailed) => { *self = Self::Retrying { failures, due: 0.0 }; None }
            (state, _) => { *self = state; None }
        }
    }
}

/// Reads the newest archived root sessions once (`ARCHIVED_LIMIT`), retrying on failure with
/// bounded backoff, and takes them in. True when they land.
pub fn load(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    let landed = match &s.sidebar_inbox.archived_fetch {
        Fetch::Loading { landed, .. } => landed.lock().unwrap_or_else(|e| e.into_inner()).take(),
        _ => None,
    };
    if let Some(result) = landed {
        return match s.sidebar_inbox.archived_fetch.step(Event::Landed { result, now }) {
            Some(Effect::Loaded(list)) => { s.sidebar_inbox.archived = list; true }
            Some(Effect::WakeAfter(after)) => { tx.wake_after(after); false }
            _ => false,
        };
    }
    if !matches!(s.sidebar_inbox.archived_fetch, Fetch::Idle | Fetch::Retrying { .. }) { return false; }
    let Source::Live { server, .. } = &*s.source else { return false };
    let server = server.clone();
    match s.sidebar_inbox.archived_fetch.step(Event::Poll { now }) {
        Some(Effect::Spawn(slot)) => {
            s.sidebar_inbox.archived_on = Some(crate::servers::runtime::active(s).clone());
            let wake = (*s.waker).clone();
            if rune::task::spawn("inbox-archived", move || {
                let result = server.get("/api/session?parentID=null&archived=true&limit=50").map(|v| opencode_kit::model::parse_sessions(&v)).map_err(|_| ());
                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                if let Some(w) = wake { w(); }
            }).is_err() { s.sidebar_inbox.archived_fetch.step(Event::SpawnFailed); }
        }
        Some(Effect::WakeAfter(after)) => tx.wake_after(after),
        _ => {}
    }
    false
}
