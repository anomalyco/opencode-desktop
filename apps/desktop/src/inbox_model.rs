//! Desktop's Inbox layout (`shell/titlebar/inbox-model.ts`, `inbox.tsx`, `inbox-working.ts`,
//! `inbox-branch-machine.ts`): every root session with the state a person acts on, sorted into
//! the sidebar's groups. A plain projection of the one session store (`App::sessions`, and the
//! family's requests and work from `session_status`); the inbox keeps only its own view state here.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opencode_kit::model::SessionInfo;
use rune::Tx;

use crate::layout_state::TabLayout;
use crate::{App, Route, Source};

/// Recent shows this many before Show more (`RECENT_LIMIT`).
pub const RECENT_LIMIT: usize = 10;

/// What a root session needs from a person, across its subagent family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InboxStatus {
    /// A permission waits on you.
    Approval,
    /// A question or form waits on you.
    Input,
    /// Something in the family runs.
    Working,
    /// It finished, failed, and nobody has looked yet.
    Failed,
    /// It finished and nobody has looked yet.
    Done,
    /// Read and idle.
    Ready,
}

/// One root session in the inbox.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub session: &'a SessionInfo,
    pub status: InboxStatus,
    /// Wall-clock ms the row's age counts from.
    pub since: Option<i64>,
}

/// The sidebar's groups, in their order.
#[derive(Default)]
pub struct Sections<'a> {
    /// The open session when no group shows it (archived, or past Recent's cap): unheaded, first.
    pub current: Vec<Entry<'a>>,
    pub needs: Vec<Entry<'a>>,
    pub unread: Vec<Entry<'a>>,
    pub recent: Vec<Entry<'a>>,
    /// Recent has more than `RECENT_LIMIT`.
    pub more: bool,
    /// Some session is archived: the Archived button shows.
    pub archived: bool,
    /// The pinned shelf below the list.
    pub working: Vec<Entry<'a>>,
}

impl Sections<'_> {
    pub fn is_empty(&self) -> bool {
        self.current.is_empty() && self.needs.is_empty() && self.unread.is_empty() && self.recent.is_empty() && self.working.is_empty()
    }

    /// Every row in order, the Working shelf last: the order the arrow keys rove in.
    pub fn rows(&self) -> impl Iterator<Item = &Entry<'_>> {
        self.current.iter().chain(&self.needs).chain(&self.unread).chain(&self.recent).chain(&self.working)
    }
}

/// When the session's current run started (`workingSince`): none while the run it holds ended
/// (`time.idle` after `time.busy`) or the server never recorded one.
pub fn working_since(s: &SessionInfo) -> Option<i64> {
    let busy = s.busy?;
    if s.idle.is_some_and(|idle| idle > busy) {
        return None;
    }
    Some(busy)
}

impl App {
    /// The inbox's rows, newest first (`createSessionInbox().entries`).
    pub fn inbox_entries(&self) -> Vec<Entry<'_>> {
        let mut list: Vec<Entry<'_>> = self.sessions.iter().filter(|s| s.parent.is_none()).map(|s| self.inbox_entry(s)).collect();
        list.sort_by_key(|e| std::cmp::Reverse(e.session.updated));
        list
    }

    fn inbox_entry<'a>(&self, session: &'a SessionInfo) -> Entry<'a> {
        let root = session.id.as_str();
        let (permission, form) = self.family_requests(root);
        let busy = self.family_busy(root);
        let status = if permission.is_some() {
            InboxStatus::Approval
        } else if form.is_some() {
            InboxStatus::Input
        } else if busy {
            InboxStatus::Working
        } else if !session.unread() {
            InboxStatus::Ready
        } else if session.outcome.as_deref() == Some("failed") {
            InboxStatus::Failed
        } else {
            InboxStatus::Done
        };
        // A waiting session ages from the request it shows, not from one auto-accept answers.
        let since = match status {
            InboxStatus::Working => working_since(session),
            InboxStatus::Approval | InboxStatus::Input => {
                let request = if status == InboxStatus::Approval { permission } else { form };
                request
                    .and_then(|id| self.sidebar_inbox.asked.get(id).copied())
                    .or_else(|| working_since(session))
                    .or(Some(session.updated))
            }
            _ => session.idle.or(Some(session.updated)),
        };
        Entry { session, status, since }
    }

    /// Whether `e` is the open session, or the root of the open subagent.
    pub fn inbox_selected(&self, e: &Entry<'_>) -> bool {
        matches!(&self.route, Route::Session(id) if !self.settings.open && self.root_of(id) == e.session.id)
    }

    /// The groups the sidebar shows (`SidebarInbox`'s `sections`).
    pub fn inbox_sections(&self) -> Sections<'_> {
        let list = self.inbox_entries();
        // An automation's sessions (Flow's) stay out of it; the open one still shows, as `current`.
        let active = || list.iter().filter(|e| e.session.archived.is_none() && !crate::view::is_automation(&e.session.title)).copied();
        let mut needs: Vec<_> = active().filter(|e| matches!(e.status, InboxStatus::Approval | InboxStatus::Input)).collect();
        needs.sort_by_key(|e| e.since.unwrap_or(i64::MAX));
        let mut unread: Vec<_> = active().filter(|e| matches!(e.status, InboxStatus::Done | InboxStatus::Failed)).collect();
        unread.sort_by_key(|e| std::cmp::Reverse(e.since.unwrap_or(0)));
        let mut read: Vec<_> = active().filter(|e| e.status == InboxStatus::Ready).collect();
        read.sort_by_key(|e| std::cmp::Reverse(e.since.unwrap_or(0)));
        let more = read.len() > RECENT_LIMIT;
        if !self.restore.inbox.recent_expanded {
            read.truncate(RECENT_LIMIT);
        }
        // `time.updated` moves with every message of a run, so Working keeps run-start order.
        let mut working: Vec<_> = active().filter(|e| e.status == InboxStatus::Working).collect();
        working.sort_by(|a, b| {
            let start = |e: &Entry<'_>| working_since(e.session).unwrap_or(i64::MAX);
            start(a).cmp(&start(b)).then_with(|| a.session.id.cmp(&b.session.id))
        });
        let shown = |e: &Entry<'_>| [&needs, &unread, &read, &working].iter().any(|g| g.iter().any(|x| x.session.id == e.session.id));
        let current = list.iter().find(|e| self.inbox_selected(e)).filter(|e| !shown(e)).copied().into_iter().collect();
        let archived = list.iter().any(|e| e.session.archived.is_some())
            || self.sidebar_inbox.archived.iter().any(|a| self.session(&a.id).is_none_or(|i| i.archived.is_some()));
        Sections { current, needs, unread, recent: read, more, archived, working }
    }

    /// The Working shelf opens or shuts (`workingExpanded`).
    pub fn toggle_inbox_working(&mut self) {
        self.restore.inbox.working_expanded = !self.restore.inbox.working_expanded;
    }

    /// Show more / Show less under Recent (`recentExpanded`).
    pub fn toggle_inbox_recent(&mut self) {
        self.restore.inbox.recent_expanded = !self.restore.inbox.recent_expanded;
    }

    /// An inbox row opens its session, as a tab (`inbox.open`).
    pub fn open_inbox_session(&mut self, id: &str, tx: &mut Tx) {
        if !self.tabs.iter().any(|t| t.id == id) {
            let (title, dir) = self.session(id).map(|s| (s.title.clone(), s.directory.clone())).unwrap_or_default();
            self.tabs.push(crate::tabs::Tab::session(id, title, dir));
            self.save_tabs();
        }
        self.open(id, tx);
    }

    /// Archive: the session leaves the list and its tabs at once (`inbox.archive`).
    pub fn archive_session(&mut self, id: &str, tx: &mut Tx) {
        let now = self.now_ms;
        let Some(info) = self.sessions.iter_mut().find(|s| s.id == id) else { return };
        info.archived = Some(now);
        if self.tabs.iter().any(|t| t.id == id) {
            self.close_tab(id, tx);
        }
        self.write(opencode_kit::client::Write::Archive(id.to_owned()));
    }

    /// Unarchive: it comes back to its group (`inbox.unarchive`).
    pub fn unarchive_session(&mut self, id: &str) {
        self.unarchive_session_locally(id);
        self.write(opencode_kit::client::Write::Unarchive(id.to_owned()));
    }

    /// Shows the session as restored: off the archived list and not archived.
    pub(crate) fn unarchive_session_locally(&mut self, id: &str) {
        self.sidebar_inbox.archived.retain(|a| a.id != id);
        if let Some(info) = self.sessions.iter_mut().find(|s| s.id == id) {
            info.archived = None;
        }
    }
}

/// The inbox's own state: when requests were asked, the list's height for the shelf's cap, and
/// the rows' branches.
#[derive(Default)]
pub struct SidebarInbox {
    /// Wall-clock ms each open request arrived, by request id: a snapshot's requests have no
    /// time, live ones age from their event.
    pub asked: HashMap<String, i64>,
    /// The inbox's laid-out height: the Working shelf takes at most 40% of it.
    pub height: f32,
    /// The list's and the shelf's viewport and content heights, for their scroll bars' gutter.
    pub list: Fit,
    pub shelf: Fit,
    /// The age labels' last tick: seconds while something works, minutes otherwise.
    tick: i64,
    /// The newest archived root sessions (the list leaves them out): the Archived button shows
    /// while there are any.
    pub archived: Vec<SessionInfo>,
    archived_asked: bool,
    archived_landed: Arc<Mutex<Option<Vec<SessionInfo>>>>,
    pub branches: Branches,
    /// The first load, which shows Desktop's loader in the inbox's place.
    settle: Settle,
    heard: Heard,
}

/// The longest the inbox hides behind its loader on first open, so a stalled server can't hide
/// it for good (`INBOX_SETTLE_TIMEOUT_MS`).
const SETTLE_TIMEOUT_MS: i64 = 10_000;

/// The inbox's first load (`inbox-settle-machine.ts`): it settles once the session list is in and
/// every working root's requests are read, or when the timeout passes; once settled it stays
/// settled, so a session starting later never blanks it.
#[derive(Clone, Debug, Default)]
enum Settle {
    /// Before the inbox first shows; what's heard counts already.
    #[default]
    Idle,
    Loading { since: i64 },
    Settled,
}

/// What the first load waits on, as it arrives: the session list, and which sessions' pending
/// requests are read.
#[derive(Clone, Debug, Default)]
struct Heard {
    sessions: bool,
    read: HashSet<String>,
}

/// A scroll area's viewport and content heights, as laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fit {
    pub view: f32,
    pub content: f32,
}

impl Fit {
    pub fn overflows(self) -> bool {
        self.view > 0.0 && self.content > self.view + 0.5
    }
}

/// Branch lookups each run git on the server, so a few at a time (`BRANCH_BATCH`).
const BRANCH_BATCH: usize = 4;
const BRANCH_RETRY_DELAY: f64 = 1.0;
const BRANCH_RETRY_MAX_DELAY: f64 = 4.0;
/// Retries after the first failed lookup before the folder rests as failed until a reconnect.
const BRANCH_RETRY_LIMIT: u32 = 2;

/// One folder's git branch load (`InboxBranchEntry`).
#[derive(Clone, Debug, PartialEq)]
enum Branch {
    Queued { failures: u32 },
    Loading { failures: u32 },
    Retrying { failures: u32, due: f64 },
    Ok(Option<String>),
    Failed,
}

/// The rows' branches by folder: git info isn't on the session, so each shown folder loads it
/// from `/api/vcs`, and `vcs.branch.updated` keeps it current.
#[derive(Default)]
pub struct Branches {
    by_dir: HashMap<String, Branch>,
    queue: Vec<String>,
    landed: Arc<Mutex<Vec<(String, Result<Option<String>, ()>)>>>,
}

impl Branches {
    pub fn get(&self, dir: &str) -> Option<&str> {
        match self.by_dir.get(dir) {
            Some(Branch::Ok(b)) => b.as_deref(),
            _ => None,
        }
    }

    /// Queues the folders a view shows that haven't loaded.
    fn enqueue(&mut self, dirs: impl IntoIterator<Item = String>) {
        for dir in dirs {
            if dir.is_empty() || self.by_dir.contains_key(&dir) {
                continue;
            }
            self.by_dir.insert(dir.clone(), Branch::Queued { failures: 0 });
            self.queue.push(dir);
        }
    }

    /// A folder's branch changed on the server.
    pub fn updated(&mut self, dir: &str, branch: Option<String>) {
        if self.by_dir.contains_key(dir) {
            self.by_dir.insert(dir.to_owned(), Branch::Ok(branch));
        }
    }

    /// A reconnect: failed folders try again.
    pub fn reconnected(&mut self) {
        for (dir, b) in &mut self.by_dir {
            if *b == Branch::Failed {
                *b = Branch::Queued { failures: 0 };
                self.queue.push(dir.clone());
            }
        }
    }

    fn loading(&self) -> usize {
        self.by_dir.values().filter(|b| matches!(b, Branch::Loading { .. })).count()
    }

    /// Takes in finished loads, starts due retries and fills the free slots. True on a change.
    fn poll(&mut self, s: &Source, waker: Option<&rune::Waker>, now: f64, tx: &mut Tx) -> bool {
        let landed = std::mem::take(&mut *self.landed.lock().unwrap_or_else(|e| e.into_inner()));
        let mut changed = !landed.is_empty();
        for (dir, result) in landed {
            let Some(Branch::Loading { failures }) = self.by_dir.get(&dir).cloned() else { continue };
            let next = match result {
                Ok(branch) => Branch::Ok(branch),
                Err(()) if failures >= BRANCH_RETRY_LIMIT => Branch::Failed,
                Err(()) => {
                    let delay = (BRANCH_RETRY_DELAY * f64::from(1u32 << failures)).min(BRANCH_RETRY_MAX_DELAY);
                    Branch::Retrying { failures: failures + 1, due: now + delay }
                }
            };
            self.by_dir.insert(dir, next);
        }
        let mut next_due = None::<f64>;
        for (dir, b) in &mut self.by_dir {
            if let Branch::Retrying { failures, due } = *b {
                if due <= now + 1e-9 {
                    *b = Branch::Queued { failures };
                    self.queue.push(dir.clone());
                } else {
                    next_due = Some(next_due.map_or(due, |d| d.min(due)));
                }
            }
        }
        if let Some(due) = next_due {
            tx.wake_after(Duration::from_secs_f64((due - now).max(0.0)));
        }
        let Source::Live { server, .. } = s else {
            // A fixture has no git: its rows show none.
            for dir in self.queue.drain(..) {
                self.by_dir.insert(dir, Branch::Ok(None));
                changed = true;
            }
            return changed;
        };
        while self.loading() < BRANCH_BATCH && !self.queue.is_empty() {
            let dir = self.queue.remove(0);
            let Some(Branch::Queued { failures }) = self.by_dir.get(&dir).cloned() else { continue };
            self.by_dir.insert(dir.clone(), Branch::Loading { failures });
            let (server, landed, wake) = (server.clone(), Arc::clone(&self.landed), waker.cloned());
            let spawned = std::thread::Builder::new().name("inbox-branch".into()).spawn(move || {
                let path = format!("/api/vcs?location%5Bdirectory%5D={}", crate::switcher::encode(&dir));
                // A folder the server hasn't loaded yet (a worktree just picked, say) answers before
                // its VCS provider has, with neither provider nor branch: that read is retried, as
                // Desktop reads the folder again once `vcs.branch.updated` says it loaded.
                let result = server.get(&path).map_err(|_| ()).and_then(|v| {
                    let data = v.get("data").unwrap_or(&v);
                    let branch = data["branch"]["current"].as_str().map(str::to_owned);
                    if branch.is_none() && data["provider"].is_null() { Err(()) } else { Ok(branch) }
                });
                landed.lock().unwrap_or_else(|e| e.into_inner()).push((dir, result));
                if let Some(w) = wake {
                    w();
                }
            });
            if spawned.is_err() {
                break;
            }
        }
        changed
    }
}

impl SidebarInbox {
    /// A request arrived live at `at`.
    pub fn asked(&mut self, id: &str, at: i64) {
        self.asked.entry(id.to_owned()).or_insert(at);
    }

    /// Whether the inbox still shows its loader.
    pub fn loading(&self) -> bool {
        matches!(self.settle, Settle::Idle | Settle::Loading { .. })
    }

    /// The server's session list arrived.
    pub fn heard_sessions(&mut self) {
        self.heard.sessions = true;
    }

    /// A session's pending requests arrived.
    pub fn heard_pending(&mut self, session: &str) {
        if !matches!(self.settle, Settle::Settled) {
            self.heard.read.insert(session.to_owned());
        }
    }

    /// Starts or finishes the first load at `now` (wall ms), waiting on the requests of the
    /// `working` roots: a source with nothing to wait for (a fixture) settles at once. True when
    /// the loader goes.
    fn settle(&mut self, live: bool, working: &[String], now: i64, tx: &mut Tx) -> bool {
        let since = match self.settle {
            Settle::Settled => return false,
            Settle::Idle if !live => {
                self.settle = Settle::Settled;
                return true;
            }
            Settle::Idle => {
                self.settle = Settle::Loading { since: now };
                now
            }
            Settle::Loading { since } => since,
        };
        let h = &self.heard;
        let ready = h.sessions && working.iter().all(|id| h.read.contains(id));
        if !ready && now - since < SETTLE_TIMEOUT_MS {
            tx.wake_after(Duration::from_millis((since + SETTLE_TIMEOUT_MS - now).max(1) as u64));
            return false;
        }
        self.heard = Heard::default();
        self.settle = Settle::Settled;
        true
    }
}

/// The inbox's clock: ages tick each second while something works and each minute otherwise,
/// and the shown rows' branches load. True when the sidebar should draw again.
/// The first load's settling, now rather than at the next poll.
pub fn settle_now(s: &mut App, tx: &mut Tx) -> bool {
    let live = s.live();
    let working: Vec<String> = s.sessions.iter().filter(|i| i.parent.is_none() && (working_since(i).is_some() || s.family_busy(&i.id))).map(|i| i.id.clone()).collect();
    s.sidebar_inbox.settle(live, &working, s.now_ms, tx)
}

pub fn poll(s: &mut App, now: f64, tx: &mut Tx) -> bool {
    s.sidebar_inbox.asked.retain(|id, _| s.requests.iter().any(|(_, r)| r == id));
    // The new-session page names its folder's branch the same way (`NewSessionBranch`).
    let draft = matches!(s.route, crate::Route::Draft(_)).then(|| s.draft_dir().to_owned());
    // So does a Home row under the pointer, as Desktop syncs a row's location on hover.
    let hovered = (s.route == crate::Route::Home)
        .then(|| s.home_search.hovered.as_deref().and_then(|id| s.session(id).or_else(|| s.home_search.loaded.iter().find(|x| x.id == id))))
        .flatten()
        .map(|i| i.directory.clone());
    let draft: Option<String> = draft.or(hovered);
    // Alt held on Home shows every row's location, so their branches load (`alt-hold.ts`).
    let revealed: Vec<String> = if s.route == crate::Route::Home && s.home_search.alt_held {
        s.home_sessions(None).iter().map(|i| i.directory.clone()).collect()
    } else {
        Vec::new()
    };
    if s.tab_layout() != TabLayout::Inbox {
        if draft.is_none() && revealed.is_empty() {
            return false;
        }
        s.sidebar_inbox.branches.enqueue(draft.into_iter().chain(revealed));
        let waker = s.waker.clone();
        return s.sidebar_inbox.branches.poll(&s.source, waker.as_ref(), now, tx);
    }
    // A working root's pending requests load in `session_status::poll`, so a run waiting on a
    // person shows in Needs attention rather than Working (`inbox-settle-machine`).
    let mut changed = settle_now(s, tx);
    changed |= load_archived(s);
    let sections = s.inbox_sections();
    let working = sections.rows().any(|e| e.status == InboxStatus::Working);
    let dirs: Vec<String> = sections.rows().map(|e| e.session.directory.clone()).collect();
    let step = if working { 1000 } else { 60_000 };
    let tick = s.now_ms.div_euclid(step);
    changed |= std::mem::replace(&mut s.sidebar_inbox.tick, tick) != tick;
    tx.wake_after(Duration::from_millis((step - s.now_ms.rem_euclid(step)).max(1) as u64));
    s.sidebar_inbox.branches.enqueue(dirs.into_iter().chain(draft).chain(revealed));
    let waker = s.waker.clone();
    changed |= s.sidebar_inbox.branches.poll(&s.source, waker.as_ref(), now, tx);
    changed
}

/// Reads the newest archived root sessions once (`ARCHIVED_LIMIT`), and takes them in. True
/// when they land.
fn load_archived(s: &mut App) -> bool {
    let inbox = &mut s.sidebar_inbox;
    if let Some(list) = inbox.archived_landed.lock().unwrap_or_else(|e| e.into_inner()).take() {
        inbox.archived = list;
        return true;
    }
    let Source::Live { server, .. } = &s.source else { return false };
    if std::mem::replace(&mut inbox.archived_asked, true) {
        return false;
    }
    let (server, landed, wake) = (server.clone(), Arc::clone(&inbox.archived_landed), s.waker.clone());
    let _ = std::thread::Builder::new().name("inbox-archived".into()).spawn(move || {
        let Ok(v) = server.get("/api/session?parentID=null&archived=true&limit=50") else { return };
        *landed.lock().unwrap_or_else(|e| e.into_inner()) = Some(opencode_kit::model::parse_sessions(&v));
        if let Some(w) = wake {
            w();
        }
    });
    false
}

/// The age beside a row: "now" under a minute ("12s" while working), then m, h, d.
pub fn age(status: InboxStatus, since: Option<i64>, now_ms: i64) -> Option<String> {
    let seconds = ((now_ms - since?) / 1000).max(0);
    Some(match seconds {
        s if s < 60 && status == InboxStatus::Working => opencode_i18n::plural!("sidebar.inbox.elapsed.seconds", s),
        s if s < 60 => opencode_i18n::t!("sidebar.inbox.time.now").into(),
        s if s < 3600 => opencode_i18n::plural!("sidebar.inbox.elapsed.minutes", s / 60),
        s if s < 86_400 => opencode_i18n::plural!("sidebar.inbox.elapsed.hours", s / 3600),
        s => opencode_i18n::plural!("sidebar.inbox.elapsed.days", s / 86_400),
    })
}

/// Whether `dir` is a worktree of its project: a folder other than the project's own.
pub fn in_worktree(s: &App, info: &SessionInfo) -> bool {
    let root = s.project_for_session(info).map(|p| p.directory.as_str());
    root.is_some_and(|root| !same_path(root, &info.directory))
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim_end_matches(['/', '\\']).replace('\\', "/").to_lowercase();
    norm(a) == norm(b)
}

/// The layout's test scene: Home with the fixture's sessions in the Inbox, one in each state. The
/// fixture already has a permission waiting, two runs and two unread; this adds a question
/// waiting, a failed run and when the runs started.
fn scene(s: &mut App) {
    s.layout = crate::layout_state::Shell::at_rest(TabLayout::Inbox);
    s.settings.tab_layout = TabLayout::Inbox;
    let now = s.now_ms;
    s.requests.insert(("ses_fx0003aaaaaaaaaaaaaaaaaa".into(), "que_fx0001".into()));
    for (id, busy) in [("ses_fx0006aaaaaaaaaaaaaaaaaa", 95_000), ("ses_fx0018aaaaaaaaaaaaaaaaaa", 12_000)] {
        if let Some(info) = s.sessions.iter_mut().find(|i| i.id == id) {
            info.busy = Some(now - busy);
        }
    }
    if let Some(info) = s.sessions.iter_mut().find(|i| i.id == "ses_fx0007aaaaaaaaaaaaaaaaaa") {
        info.outcome = Some("failed".into());
    }
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [("layout-inbox", scene)]);
