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

    /// Reads `id`'s open permissions and forms, once per connection.
    pub(crate) fn read_requests(&mut self, id: &str) {
        if !matches!(&*self.source, Source::Live { requests: Some(_), .. }) || self.request_reads.is_read(id) {
            return;
        }
        let effects = self.request_reads.step(ReadEvent::ReadNow(id.to_owned()));
        apply_effects(self, effects, None);
    }

    /// The open session's requests and those of what runs below it.
    pub(crate) fn read_open_requests(&mut self, id: &str) {
        let mut ids: Vec<String> = self.running.iter().filter(|r| r.as_str() != id && self.descends(r, id)).cloned().collect();
        ids.sort();
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
        s.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_draft() && t.server == *crate::servers::runtime::active(s) && shown.as_deref() != Some(t.id.as_str()) && !s.request_reads.is_read(&t.id))
            .filter(|(_, t)| s.running.iter().any(|r| s.root_of(r) == t.id))
            .map(|(i, t)| (i, t.id.clone()))
            .collect()
    };
    if !tabs.is_empty() || s.request_reads.has_scheduled() {
        let effects = s.request_reads.step(ReadEvent::SyncTabs { tabs, now });
        apply_effects(s, effects, Some(tx));
    }
    // A live run may be waiting on a person. A saved busy timestamp is not a live run:
    // asking for its permissions/forms can boot a cold location just to draw the Inbox.
    // (`inbox-model.ts:234-275` syncs their permissions and forms). Home doesn't read them, as
    // Desktop's doesn't: a working row whose requests nobody read shows its activity ring, and
    // the unread dot once the Inbox, its tab or the stream brings them. Settings, where the
    // layout switches to the Inbox, reads them ahead, so the Inbox's first load has settled when
    // it shows and the active tab flies into its card rather than the Inbox showing its loader.
    if layout == TabLayout::Inbox || s.settings_open() {
        let working: Vec<String> = s
            .sessions
            .iter()
            .filter(|i| i.parent.is_none() && !s.request_reads.is_read(&i.id) && s.has_live_family(&i.id))
            .map(|i| i.id.clone())
            .collect();
        for id in working {
            s.read_requests(&id);
        }
    }
    false
}
