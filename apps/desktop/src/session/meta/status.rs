//! What every list shows for a session, from the one session store (`App::running`, `inboxes`,
//! `requests` and `pending`, and the sessions' read marks): Desktop's `sessionRequests`,
//! `sessionBusy` and `useSessionTabAvatarState` (`shell/layout/project-avatar-state.ts`). Tabs,
//! the palette, Home's rows and the Inbox all read it here.
//!
//! Pending permissions and forms load as Desktop's data store syncs them
//! (`session.permission.sync`, `session.form.sync`): each session's once per connection, for the
//! open session and what runs below it (`session/requests/model.ts`), an inactive tab whose family
//! runs (`tab-strip.tsx`) and a root that works, for the Inbox (`inbox-settle-machine.ts`) and
//! for Home's rows, so a row waiting on a person shows its unread dot as Desktop's does. The
//! `permission.*` and `form.*` events keep them current (`session::events`).

use std::collections::HashMap;
use std::time::Duration;

use opencode_kit::client::Request;
use opencode_kit::model::{Pending, SessionInfo};
use rune::Tx;

use crate::layout_state::TabLayout;
use crate::{App, Route, Source};

/// A tab's requests load this long after it shows, the next tab's 50 ms later (`tab-strip.tsx`).
const TAB_DELAY: f64 = 0.3;
const TAB_STEP: f64 = 0.05;

/// A session's marks wherever it's listed (`useSessionTabAvatarState` for a root).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    /// A permission or a form waits on a person somewhere in the family.
    pub attention: bool,
    /// Something in the family works, and nothing waits on a person.
    pub busy: bool,
    /// Waiting on a person, or finished since it was last read.
    pub unread: bool,
}

/// Per-session pending-request fetch state on the current connection (absence means unread).
#[derive(Clone, Copy, Debug, PartialEq)]
enum SessionRead {
    /// Inactive busy tab waiting out its stagger delay until `at` (`App::clock`).
    Scheduled { at: f64 },
    /// Pending permissions and forms have been requested on this connection.
    Read,
}

enum ReadEvent {
    /// Request a session's pending permissions and forms immediately.
    ReadNow(String),
    /// Reconcile the inactive busy tabs eligible for staggered fetching at `now`.
    SyncTabs { tabs: Vec<(usize, String)>, now: f64 },
    /// Connection dropped: forget all read marks and pending schedules.
    Reset,
}

enum ReadEffect {
    Fetch(String),
    WakeAfter(Duration),
}

/// Which sessions' pending requests this connection has read (`sync.run`'s done keys), and the
/// tabs waiting out their delay, in one per-session map so a session cannot be both scheduled
/// and already read.
#[derive(Default)]
pub struct RequestReads {
    sessions: HashMap<String, SessionRead>,
}

impl RequestReads {
    fn is_read(&self, id: &str) -> bool {
        matches!(self.sessions.get(id), Some(SessionRead::Read))
    }

    fn has_scheduled(&self) -> bool {
        self.sessions.values().any(|s| matches!(s, SessionRead::Scheduled { .. }))
    }

    fn step(&mut self, event: ReadEvent) -> Vec<ReadEffect> {
        match event {
            ReadEvent::ReadNow(id) => {
                if self.is_read(&id) {
                    return Vec::new();
                }
                self.sessions.insert(id.clone(), SessionRead::Read);
                vec![ReadEffect::Fetch(id)]
            }
            ReadEvent::SyncTabs { tabs, now } => {
                self.sessions.retain(|id, state| {
                    matches!(state, SessionRead::Read) || tabs.iter().any(|(_, t)| t == id)
                });
                for (index, id) in tabs {
                    self.sessions
                        .entry(id)
                        .or_insert(SessionRead::Scheduled { at: now + TAB_DELAY + TAB_STEP * index as f64 });
                }
                let mut effects = Vec::new();
                let mut next: Option<f64> = None;
                for (id, state) in &mut self.sessions {
                    let SessionRead::Scheduled { at } = *state else { continue };
                    if at <= now + 1e-9 {
                        *state = SessionRead::Read;
                        effects.push(ReadEffect::Fetch(id.clone()));
                    } else {
                        next = Some(next.map_or(at, |m| m.min(at)));
                    }
                }
                if let Some(at) = next {
                    effects.push(ReadEffect::WakeAfter(Duration::from_secs_f64(at - now)));
                }
                effects
            }
            ReadEvent::Reset => {
                self.sessions.clear();
                Vec::new()
            }
        }
    }
}

fn apply_effects(s: &mut App, effects: Vec<ReadEffect>, mut tx: Option<&mut Tx>) {
    for effect in effects {
        match effect {
            ReadEffect::Fetch(id) => {
                if let Source::Live { requests: Some(r), .. } = &*s.source {
                    r.send(Request::Pending(id)).ok();
                }
            }
            ReadEffect::WakeAfter(delay) => {
                if let Some(tx) = tx.as_deref_mut() {
                    tx.wake_after(delay);
                }
            }
        }
    }
}

impl App {
    /// `id`'s marks, across its family.
    pub fn marks(&self, id: &str) -> Marks {
        let (permission, form) = self.family_requests(id);
        let attention = permission.is_some() || form.is_some();
        Marks {
            attention,
            busy: !attention && self.family_busy(id),
            unread: attention || self.session(id).is_some_and(SessionInfo::unread) || crate::session_menu::unread::manual(self, crate::servers::runtime::active(self), id),
        }
    }

    /// Whether anything in `root`'s family runs or has a prompt waiting in its inbox (`sessionBusy`).
    pub fn family_busy(&self, root: &str) -> bool {
        self.has_live_family(root)
            || self.inboxes.iter().any(|(id, inbox)| (!inbox.items.is_empty() || inbox.compaction.is_some()) && self.root_of(id) == root)
    }

    /// Current server activity, excluding historical busy times and queued but inactive work.
    pub(crate) fn has_live_family(&self, root: &str) -> bool {
        self.running.iter().any(|id| self.root_of(id) == root)
    }

    /// The permission and the form waiting on a person anywhere in `root`'s family, by request
    /// id (`sessionPermissionRequest`, `sessionFormRequest`). A permission with a reply on its way,
    /// or one auto-accept answers, doesn't wait (`needsUser`).
    pub fn family_requests(&self, root: &str) -> (Option<&str>, Option<&str>) {
        let mut found: (Option<&str>, Option<&str>) = (None, None);
        let asked = |id: &str| self.sidebar_inbox.asked.get(id).copied().unwrap_or(i64::MAX);
        for (session, id) in &self.requests {
            if self.root_of(session) != root {
                continue;
            }
            let form = match self.pending.iter().find(|p| p.id() == id) {
                Some(Pending::Form { .. }) => true,
                Some(Pending::Permission { .. }) => false,
                None => !id.starts_with("per"),
            };
            if !form && (self.kernel.slots.get(&crate::permission::PERMISSION_SENT).busy(id) || self.auto_accepts(session)) {
                continue;
            }
            let slot = if form { &mut found.1 } else { &mut found.0 };
            // The oldest, so a row's age doesn't depend on the set's order.
            if slot.is_none_or(|have| (asked(id), id.as_str()) < (asked(have), have)) {
                *slot = Some(id.as_str());
            }
        }
        found
    }

    /// Only a running session can hold a pending permission or form, and events keep loaded
    /// requests current, so idle subagents are never read. One that still has cached requests is
    /// read too, so a reconnect clears requests answered while offline (`sessionMayHoldRequests`,
    /// `session-request-tree.ts`).
    pub(crate) fn session_may_hold_requests(&self, id: &str) -> bool {
        self.running.contains(id) || self.requests.iter().any(|(s, _)| s == id)
    }

    /// Every session in `root`'s family that may hold a pending permission or form (sorted, deduped).
    fn family_may_hold_requests(&self, root: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .running
            .iter()
            .filter(|id| self.root_of(id) == root)
            .chain(self.requests.iter().map(|(id, _)| id).filter(|id| self.root_of(id) == root))
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Reads `id`'s open permissions and forms, once per connection.
    pub(crate) fn read_requests(&mut self, id: &str) {
        if !matches!(&*self.source, Source::Live { requests: Some(_), .. }) || self.request_reads.is_read(id) {
            return;
        }
        let effects = self.request_reads.step(ReadEvent::ReadNow(id.to_owned()));
        apply_effects(self, effects, None);
    }

    /// The open session's requests and those of every descendant that runs or still holds cached
    /// requests (`session/requests/model.ts`, `requestSessions`).
    pub(crate) fn read_open_requests(&mut self, id: &str) {
        let mut ids: Vec<String> = self
            .running
            .iter()
            .chain(self.requests.iter().map(|(s, _)| s))
            .filter(|r| r.as_str() != id && self.descends(r, id))
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        ids.insert(0, id.to_owned());
        for id in ids {
            self.read_requests(&id);
        }
    }

    /// The connection dropped: every session's requests read again when next shown.
    pub(crate) fn forget_request_reads(&mut self) {
        if !self.request_reads.sessions.is_empty() {
            let _ = self.request_reads.step(ReadEvent::Reset);
        }
    }
}

/// Starts the reads the shown tabs and the Inbox's working roots are due. True on a change.
pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    if !matches!(*s.source, Source::Live { requests: Some(_), .. }) {
        return false;
    }
    let layout = s.tab_layout();
    // The tab strip (top or side tabs): a tab other than the shown one, whose family runs.
    // The shown tab is its session on its server (one server listed twice has the same ids).
    let shown = match &*s.route {
        Route::Session { id, server } if server == crate::servers::runtime::active(s) => Some(s.root_of(id).to_owned()),
        _ => None,
    };
    let tabs: Vec<(usize, String)> = if layout == TabLayout::Inbox {
        Vec::new()
    } else {
        // Inactive tabs: read permissions and forms for every session in the family that is
        // running or still has cached requests (`tab-strip.tsx`, `syncInactiveSession`), never
        // booting cold idle subagent locations.
        s.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_draft() && t.server == *crate::servers::runtime::active(s) && shown.as_deref() != Some(t.id.as_str()))
            .flat_map(|(i, t)| {
                s.family_may_hold_requests(&t.id)
                    .into_iter()
                    .filter(|sid| !s.request_reads.is_read(sid))
                    .map(move |sid| (i, sid))
            })
            .collect()
    };
    if !tabs.is_empty() || s.request_reads.has_scheduled() {
        let effects = s.request_reads.step(ReadEvent::SyncTabs { tabs, now });
        apply_effects(s, effects, Some(tx));
    }
    // A live run or a cached request in the family may be waiting on a person or need clearing
    // after reconnect (`inbox-model.ts:275-287`). A saved busy timestamp alone is not a live run:
    // asking for its permissions/forms can boot a cold location just to draw the Inbox.
    if layout == TabLayout::Inbox || s.settings_open() {
        let mut to_read: Vec<String> = Vec::new();
        for root in s.sessions.iter().filter(|i| i.parent.is_none()) {
            let family = s.family_may_hold_requests(&root.id);
            if family.is_empty() {
                continue;
            }
            if !s.request_reads.is_read(&root.id) {
                to_read.push(root.id.clone());
            }
            for sid in family {
                if sid != root.id && !s.request_reads.is_read(&sid) {
                    to_read.push(sid);
                }
            }
        }
        to_read.sort();
        to_read.dedup();
        for id in to_read {
            s.read_requests(&id);
        }
    }
    false
}
