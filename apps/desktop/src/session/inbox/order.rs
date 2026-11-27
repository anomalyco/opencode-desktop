//! The inbox's rows in order, owned and kept while what they're read from holds: Dax's model
//! (`inbox_model`: `App::inbox_sections`, Recent ranked by the session list's times) read once
//! per change, not on every poll and every view. The view builds only the rows in sight
//! (`view::inbox`, a virtual list): each row is its server and session, and its card reads the
//! rest live (`App::inbox_entry_of`).

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use opencode_kit::servers::ServerKey;
use rune::Tracked;

use crate::App;
use crate::inbox_model::{Entry, RECENT_LIMIT, Sections};

/// Which of the inbox's groups a row is in, in the order they show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// The open session when no group shows it: unheaded, first.
    Current,
    Needs,
    /// The pinned sessions, then the settled ones: unheaded.
    Recent,
    /// The shelf below the list, or a group in it (`workingPlacement`).
    Working,
    /// Under a header that folds, shut at first.
    Archived,
}

impl Group {
    pub const ALL: [Group; 5] = [Group::Current, Group::Needs, Group::Recent, Group::Working, Group::Archived];
}

/// Which of the list's folds are open (`recentExpanded`, `archivedExpanded`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Folds {
    pub recent: bool,
    pub archived: bool,
}

impl From<crate::restore::InboxUi> for Folds {
    fn from(ui: crate::restore::InboxUi) -> Folds {
        Folds { recent: ui.recent_expanded, archived: ui.archived_expanded }
    }
}

/// One row: its session, on its server.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub server: ServerKey,
    pub id: String,
    pub directory: String,
    pub working: bool,
    /// Its card's key (`ib-` and `Entry::key`), made once per reading.
    pub key: String,
}

/// The rows, grouped in order, and what shows around them.
#[derive(Debug, Default, PartialEq)]
pub struct Order {
    pub rows: Vec<Shown>,
    /// Where each group's rows are in `rows`, in [`Group::ALL`]'s order.
    pub groups: [Range<usize>; 5],
    /// How many of Recent's rows are pinned: they lead it, whatever Show more says.
    pub pinned: usize,
    /// Recent has more than it shows shut.
    pub more: bool,
    /// The open session's place in `rows`, in Recent or Archived: a fold that hides it (Recent
    /// shut shows its pins and its first [`RECENT_LIMIT`], Archived shut none) shows it as the
    /// current row.
    pub open: Option<usize>,
    /// Which reading this is: a new one has a new number.
    pub generation: u64,
}

impl Order {
    pub fn group(&self, g: Group) -> &[Shown] {
        &self.rows[self.groups[g as usize].clone()]
    }

    /// The places in `rows` group `g` shows under `folds`.
    pub fn shows(&self, g: Group, folds: Folds) -> Vec<usize> {
        let range = self.groups[g as usize].clone();
        match g {
            Group::Recent if !folds.recent => range.take(self.pinned + RECENT_LIMIT).collect(),
            Group::Archived if !folds.archived => Vec::new(),
            Group::Current => {
                let folded = |at: &usize| !self.shows(Group::Recent, folds).contains(at) && !self.shows(Group::Archived, folds).contains(at);
                range.chain(self.open.filter(folded)).collect()
            }
            _ => range.collect(),
        }
    }

    /// Nothing to list, Working included.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn of(app: &App, sections: &Sections<'_>) -> Order {
        let mut rows = Vec::new();
        let mut groups: [Range<usize>; 5] = Default::default();
        for (g, entries) in [&sections.current, &sections.needs, &sections.recent, &sections.working, &sections.archived].into_iter().enumerate() {
            let start = rows.len();
            rows.extend(entries.iter().map(shown));
            groups[g] = start..rows.len();
        }
        // Read with Recent and Archived open: shut, the open session they hide stands first, unheaded.
        let open = [(Group::Recent, &sections.recent), (Group::Archived, &sections.archived)]
            .into_iter()
            .find_map(|(g, entries)| entries.iter().position(|e| app.inbox_selected(e)).map(|k| groups[g as usize].start + k));
        GENERATION.set(GENERATION.get() + 1);
        Order { rows, groups, pinned: sections.pinned, more: sections.more, open, generation: GENERATION.get() }
    }
}

fn shown(e: &Entry<'_>) -> Shown {
    Shown { server: e.server.clone(), id: e.session.id.clone(), directory: e.session.directory.clone(), working: e.status == crate::inbox_model::InboxStatus::Working, key: format!("ib-{}", e.key()) }
}

thread_local! {
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The order last read, and what it was read from.

#[derive(Default)]
pub struct Kept(RefCell<Option<(u64, Rc<Order>)>>);

impl App {
    /// The inbox's rows in order (`inbox_sections`), read again only when something they're read
    /// from changed.
    pub fn inbox_order(&self) -> Rc<Order> {
        let key = self.inbox_order_key();
        if let Some((k, order)) = &*self.sidebar_inbox.order.0.borrow()
            && *k == key
        {
            return order.clone();
        }
        let order = Rc::new(Order::of(self, &self.inbox_sections_with(true, true)));
        *self.sidebar_inbox.order.0.borrow_mut() = Some((key, order.clone()));
        order
    }

    /// Everything `inbox_sections` reads: the app's versions of what's tracked, and of the
    /// inbox's own state what a row's place depends on (another server's rows, the requests'
    /// ages, the archived list). The tick and the laid-out height move it every second while
    /// something works, so the inbox's state isn't read by version.
    fn inbox_order_key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        (
            Tracked::version(&self.sessions),
            Tracked::version(&self.recent_page),
            Tracked::version(&self.running),
            Tracked::version(&self.requests),
            Tracked::version(&self.pending),
            self.kernel.slots.version(&crate::permission::PERMISSION_SENT),
            Tracked::version(&self.inboxes),
            Tracked::version(&self.projects),
            Tracked::version(&self.settings),
            self.kernel.slots.version(&crate::settings::SETTINGS_SCREEN),
            Tracked::version(&self.route),
            self.kernel.slots.version(&crate::session_menu::MENUS),
        )
            .hash(&mut h);
        self.kernel.slots.version(&crate::tabs::pins::PINS).hash(&mut h);
        crate::servers::runtime::active(self).hash(&mut h);
        crate::servers::runtime::version().hash(&mut h);
        let inbox = &*self.sidebar_inbox;
        // The requests' ages, whatever order the map keeps them in.
        inbox.asked.len().hash(&mut h);
        inbox.asked.iter().fold(0u64, |acc, kv| {
            let mut one = DefaultHasher::new();
            kv.hash(&mut one);
            acc ^ one.finish()
        }).hash(&mut h);
        self.inbox_archived().iter().map(|a| (&a.id, &a.title, a.archived)).for_each(|a| a.hash(&mut h));
        inbox.other_servers_hash(&mut h);
        h.finish()
    }
}
