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
//! `permission.*` and `form.*` events keep them current (`events.rs`).

use std::collections::{HashMap, HashSet};
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

/// Which sessions' pending requests this connection has read (`sync.run`'s done keys), and the
/// tabs waiting out their delay.
#[derive(Default)]
pub struct RequestReads {
    read: HashSet<String>,
    due: HashMap<String, f64>,
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
            if !form && (self.permission_sent.busy(id) || self.auto_accepts(session)) {
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
        let Source::Live { requests: Some(r), .. } = &*self.source else { return };
        if self.request_reads.read.insert(id.to_owned()) {
            let _ = r.send(Request::Pending(id.to_owned()));
        }
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
        *self.request_reads = RequestReads::default();
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
            .filter(|(_, t)| !t.is_draft() && t.server == *crate::servers::runtime::active(s) && shown.as_deref() != Some(t.id.as_str()) && !s.request_reads.read.contains(&t.id))
            .filter(|(_, t)| s.running.iter().any(|r| s.root_of(r) == t.id))
            .map(|(i, t)| (i, t.id.clone()))
            .collect()
    };
    let reads = &mut *s.request_reads;
    reads.due.retain(|id, _| tabs.iter().any(|(_, t)| t == id));
    for (index, id) in &tabs {
        reads.due.entry(id.clone()).or_insert(now + TAB_DELAY + TAB_STEP * *index as f64);
    }
    let ready: Vec<String> = reads.due.iter().filter(|(_, at)| **at <= now + 1e-9).map(|(id, _)| id.clone()).collect();
    if let Some(next) = reads.due.values().copied().filter(|at| *at > now + 1e-9).reduce(f64::min) {
        tx.wake_after(Duration::from_secs_f64(next - now));
    }
    for id in &ready {
        s.request_reads.due.remove(id);
        s.read_requests(id);
    }
    // A live run may be waiting on a person. A saved busy timestamp is not a live run:
    // asking for its permissions/forms can boot a cold location just to draw the Inbox.
    // (`inbox-model.ts:234-275` syncs their permissions and forms). Home doesn't read them, as
    // Desktop's doesn't: a working row whose requests nobody read shows its activity ring, and
    // the unread dot once the Inbox, its tab or the stream brings them. Settings, where the
    // layout switches to the Inbox, reads them ahead, so the Inbox's first load has settled when
    // it shows and the active tab flies into its card rather than the Inbox showing its loader.
    if layout == TabLayout::Inbox || s.settings.open {
        let working: Vec<String> = s
            .sessions
            .iter()
            .filter(|i| i.parent.is_none() && !s.request_reads.read.contains(&i.id) && s.has_live_family(&i.id))
            .map(|i| i.id.clone())
            .collect();
        for id in working {
            s.read_requests(&id);
        }
    }
    false
}
