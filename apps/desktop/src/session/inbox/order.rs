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
use crate::inbox_model::{ARCHIVED_SHOWN, Entry, RECENT_LIMIT, Sections};

/// Which of the inbox's groups a row is in, in the order they show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// The open session when no group shows it: unheaded, first.
    Current,
    Needs,
    Unread,
    Recent,
    /// Archived sessions, under the list behind a header that opens and shuts.
    Archived,
    /// The pinned shelf below the list.
    Working,
}

/// Which of the list's two folds are open (`InboxUi`): Recent past its first [`RECENT_LIMIT`],
/// and Archived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Open {
    pub recent: bool,
    pub archived: bool,
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
    /// Where each group's rows are in `rows`, in [`Group`]'s order.
    pub groups: [Range<usize>; 6],
    /// Recent has more than it shows shut.
    pub more: bool,
    /// The open session's place in `rows`, in Recent or Archived: while its fold doesn't show
    /// it (Recent past its first [`RECENT_LIMIT`], Archived shut or past its first
    /// [`ARCHIVED_SHOWN`]) it shows as the current row.
    pub selected: Option<usize>,
    /// Which reading this is: a new one has a new number.
    pub generation: u64,
}

impl Order {
    pub fn group(&self, g: Group) -> &[Shown] {
        &self.rows[self.groups[g as usize].clone()]
    }

    /// The places in `rows` group `g` shows, with the folds as `open` has them.
    pub fn shows(&self, g: Group, open: Open) -> Vec<usize> {
        let range = self.groups[g as usize].clone();
        match g {
            Group::Recent | Group::Archived => range.take(self.cap(g, open)).collect(),
            Group::Current => range.chain(self.selected.filter(|&at| !self.folded_in(at, open))).collect(),
            _ => range.collect(),
        }
    }

    /// How many rows of fold `g` show.
    fn cap(&self, g: Group, open: Open) -> usize {
        match g {
            Group::Recent if !open.recent => RECENT_LIMIT,
            Group::Archived if !open.archived => 0,
            Group::Archived => ARCHIVED_SHOWN,
            _ => usize::MAX,
        }
    }

    /// Whether Recent or Archived shows the row at `at`.
    fn folded_in(&self, at: usize, open: Open) -> bool {
        [Group::Recent, Group::Archived].into_iter().any(|g| {
            let range = &self.groups[g as usize];
            range.contains(&at) && at - range.start < self.cap(g, open)
        })
    }

    fn of(app: &App, sections: &Sections<'_>) -> Order {
        let mut rows = Vec::new();
        let mut groups: [Range<usize>; 6] = Default::default();
        for (g, entries) in [&sections.current, &sections.needs, &sections.unread, &sections.recent, &sections.archived, &sections.working].into_iter().enumerate() {
            let start = rows.len();
            rows.extend(entries.iter().map(shown));
            groups[g] = start..rows.len();
        }
        // Read with both folds open: where one doesn't show the open session, it stands first, unheaded.
        let held = |g: Group, entries: &[Entry<'_>]| entries.iter().position(|e| app.inbox_selected(e)).map(|k| groups[g as usize].start + k);
        let selected = held(Group::Recent, &sections.recent).or_else(|| held(Group::Archived, &sections.archived));
        GENERATION.set(GENERATION.get() + 1);
        Order { rows, groups, more: sections.more, selected, generation: GENERATION.get() }
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
    /// The folds as the window has them (`InboxUi`).
    pub fn inbox_open(&self) -> Open {
        let ui = self.kernel.slots.get(&crate::restore::RESTORE).inbox;
        Open { recent: ui.recent_expanded, archived: ui.archived_expanded }
    }

    /// The inbox's rows in order (`inbox_sections`), read again only when something they're read
    /// from changed.
    pub fn inbox_order(&self) -> Rc<Order> {
        let key = self.inbox_order_key();
        if let Some((k, order)) = &*self.sidebar_inbox.order.0.borrow()
            && *k == key
        {
            return order.clone();
        }
        let order = Rc::new(Order::of(self, &self.inbox_sections()));
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
        inbox.archived_on.hash(&mut h);
        inbox.archived.iter().map(|a| (&a.id, &a.title, a.archived)).for_each(|a| a.hash(&mut h));
        inbox.other_servers_hash(&mut h);
        h.finish()
    }
}
