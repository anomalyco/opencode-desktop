//! Pure restoration state, route/tab/notice DTOs, inbox disclosures, and organized-sidebar
//! serialization for `opencode-feature-restore`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use opencode_kit::servers::{ServerKey, TabKey};
use serde_json::{Value, json};

use crate::{Parked, copies, draft_doc, preview, undo};

/// The page a window shows (`Home`, a draft tab's new-session page, or a session on a server).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum RestoreRoute {
    #[default]
    Home,
    /// Session `id` on `server`.
    Session { server: ServerKey, id: String },
    /// "New session": an empty page with the composer, shown by the draft tab with this id.
    Draft(String),
}

impl RestoreRoute {
    pub fn session(server: ServerKey, id: impl Into<String>) -> RestoreRoute {
        RestoreRoute::Session { server, id: id.into() }
    }

    /// The draft key for this route: a session's id, or a draft tab's.
    pub fn draft_key(&self) -> Option<String> {
        match self {
            RestoreRoute::Session { id, .. } | RestoreRoute::Draft(id) => Some(id.clone()),
            RestoreRoute::Home => None,
        }
    }

    /// Session id when on a session route.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            RestoreRoute::Session { id, .. } => Some(id.as_str()),
            RestoreRoute::Home | RestoreRoute::Draft(_) => None,
        }
    }

    /// Server key when on a session route.
    pub fn server(&self) -> Option<&ServerKey> {
        match self {
            RestoreRoute::Session { server, .. } => Some(server),
            RestoreRoute::Home | RestoreRoute::Draft(_) => None,
        }
    }
}

/// The draft key for a route: a session's id, or a draft tab's.
pub fn draft_key(route: &RestoreRoute) -> Option<String> {
    route.draft_key()
}

/// Pure-data descriptor of one open window tab relevant to restoration and persistence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RestoreTab {
    /// Unique tab identifier in the window (`Tab::uid`).
    pub uid: String,
    /// Session ID or draft ID (`draft_...`).
    pub id: String,
    /// Server the session lives on, or where a draft's session will start.
    pub server: ServerKey,
    /// Saved tab title (empty for drafts or untitled sessions).
    pub title: String,
    /// `true` for a "New session" draft tab.
    pub is_draft: bool,
}

impl RestoreTab {
    /// Its identity in the window's stored rows (`TabKey`).
    pub fn key(&self) -> TabKey {
        if self.is_draft {
            TabKey::draft(self.id.clone())
        } else {
            TabKey::session(self.server.clone(), self.id.clone())
        }
    }

    /// Whether this tab is session `id` on `server`, or draft `id`.
    pub fn matches(&self, server: &ServerKey, id: &str) -> bool {
        self.id == id && (self.is_draft || self.server == *server)
    }
}

/// Persisted organized-sidebar fields (`sidebar.organize`): Projects collapsed state,
/// expanded `<server>:<project>` keys, and per-server project order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OrganizeState {
    pub collapsed: bool,
    pub expanded: HashSet<String>,
    pub order: BTreeMap<String, Vec<String>>,
}

/// What the windows are told about where their persisted state lives (`storage::Notice`), once,
/// by the first window to take it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageNotice {
    /// The database couldn't open (why): this run keeps its changes in memory.
    Memory(String),
    /// Another copy of OpenCode has this profile open: this run keeps its changes in memory.
    InUse,
    /// Another copy of OpenCode is finishing a save: windows show the last save until it has.
    Waiting,
    /// The saved state was damaged: kept at this path, and the app started a new one.
    Damaged(String),
    /// Another process holds the file: windows show its last checkpoint, and nothing is saved.
    Checkpoint,
    /// Another process holds the file and won't let it be read: first-run screen, nothing saved.
    FirstRun,
    /// The file came free and still holds what windows show, or writes succeed again.
    Saving,
    /// Writes to the database fail: changes stay in memory until a write succeeds.
    NotSaving { full: bool, why: String },
    /// Some Official Desktop drafts could not be migrated (missing chunk/attachment blob); Official's
    /// folder was not modified and still holds them at `official_dir`.
    SeedDroppedDrafts { count: usize, official_dir: String },
}

/// Where the inbox lists its working sessions (`sidebar.inbox`'s `workingPlacement`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WorkingPlacement {
    /// A shelf pinned below the scrolling list.
    #[default]
    Shelf,
    /// A group in the list, between the active and the archived sessions.
    Inline,
}

impl WorkingPlacement {
    pub fn name(self) -> &'static str {
        match self {
            WorkingPlacement::Shelf => "shelf",
            WorkingPlacement::Inline => "inline",
        }
    }
}

/// The inbox's disclosures, as Desktop's `sidebar.inbox` (`inbox.tsx`): Working open, Recent
/// showing its first ten, Archived shut, Working on its shelf; and Home's automation folds, shut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InboxUi {
    pub working_expanded: bool,
    pub recent_expanded: bool,
    pub archived_expanded: bool,
    pub working_placement: WorkingPlacement,
    pub home_automations: bool,
}

impl Default for InboxUi {
    fn default() -> Self {
        InboxUi { working_expanded: true, recent_expanded: false, archived_expanded: false, working_placement: WorkingPlacement::Shelf, home_automations: false }
    }
}

impl InboxUi {
    pub fn to_json(self) -> Value {
        json!({
            "workingExpanded": self.working_expanded,
            "recentExpanded": self.recent_expanded,
            "archivedExpanded": self.archived_expanded,
            "workingPlacement": self.working_placement.name(),
            "homeAutomationsExpanded": self.home_automations,
        })
    }

    pub fn from_json(v: &Value) -> InboxUi {
        let d = InboxUi::default();
        InboxUi {
            working_expanded: v["workingExpanded"].as_bool().unwrap_or(d.working_expanded),
            recent_expanded: v["recentExpanded"].as_bool().unwrap_or(d.recent_expanded),
            archived_expanded: v["archivedExpanded"].as_bool().unwrap_or(d.archived_expanded),
            working_placement: if v["workingPlacement"].as_str() == Some("inline") { WorkingPlacement::Inline } else { WorkingPlacement::Shelf },
            home_automations: v["homeAutomationsExpanded"].as_bool().unwrap_or(d.home_automations),
        }
    }
}

/// The rows this window last wrote (or read): its keys and each draft's document by tab key.
#[derive(Default)]
pub(crate) struct Written {
    pub(crate) keys: HashMap<&'static str, Value>,
    pub(crate) drafts: HashMap<String, String>,
}

/// The window's state beyond what the first frame needed. The boot packet holds the shown draft
/// and no closed tabs; once the storage is ready the rest comes in, and only then may the
/// window write its closed tabs or remove a draft it never saw.
#[derive(Debug, Default)]
pub enum Hydration {
    /// Built from the boot packet.
    #[default]
    Boot,
    /// Some drafts are being read from disk; `read` turns true once they're in memory.
    Loading { read: Arc<AtomicBool> },
    /// Everything is in.
    Done,
}

#[derive(Default)]
pub struct Restore {
    /// Unsent composer text per draft key; the open one lives in the host's composer.
    pub drafts: HashMap<String, String>,
    /// The rest of each draft that isn't shown: attachments, quotes, pills (`parked`).
    pub parked: HashMap<String, Parked>,
    /// Which draft the host's composer holds.
    pub shown: Option<String>,
    pub inbox: InboxUi,
    /// The saved tabs and closed tabs this build can't read, written back after its own
    /// (`WindowDoc::unread_tabs`), so a launch never loses a tab it doesn't show.
    pub unread_tabs: Vec<Value>,
    pub(crate) unread_closed: Vec<Value>,
    /// What each row was last written as, so a change writes only the rows it changed.
    pub(crate) written: Written,
    pub(crate) stamp: Option<u64>,
    /// Whether the window has what the boot didn't hold: its closed tabs and other drafts.
    pub(crate) hydration: Hydration,
    pub(crate) previews: preview::Previews,
    pub(crate) copies: copies::Copies,
    /// Which blob holds each draft file's bytes (`draft_doc`).
    pub(crate) blobs: draft_doc::Blobs,
    /// Each draft's own editor while another shows: its undo history comes back with it.
    pub(crate) edits: HashMap<String, rune::TextEdit>,
    /// What each draft's stored undo history was last written as (`undo`).
    pub(crate) undo: undo::Kept,
}

impl Restore {
    /// Whether the window has everything it saved (`Hydration::Done`).
    pub fn hydrated(&self) -> bool {
        matches!(self.hydration, Hydration::Done)
    }

    /// Current hydration phase.
    pub fn hydration(&self) -> &Hydration {
        &self.hydration
    }

    /// Saved closed-tab entries this build could not decode, preserved on write.
    pub fn unread_closed(&self) -> &[Value] {
        &self.unread_closed
    }

    /// The drafts whose blob files are still on their way from storage.
    pub(crate) fn blobs_pending(&self) -> impl Iterator<Item = &String> + '_ {
        self.blobs.pending_keys()
    }
}

/// The organized sidebar's open projects, sorted, so the file and its stamp don't depend on
/// the set's order.
pub(crate) fn organize_expanded_sorted(expanded: &HashSet<String>) -> Vec<&String> {
    let mut ids: Vec<&String> = expanded.iter().collect();
    ids.sort();
    ids
}

/// The organized sidebar's state as Desktop's `sidebar.organize` keeps it: Projects shut,
/// the projects open to their sessions and the projects' order.
pub fn organize_json(collapsed: bool, expanded: &HashSet<String>, order: &BTreeMap<String, Vec<String>>) -> Value {
    // `expanded`: `Record<"<server>:<project>", boolean>`; `order`: `Record<server, id[]>`.
    let expanded_map: serde_json::Map<String, Value> = organize_expanded_sorted(expanded)
        .into_iter()
        .map(|k| (k.clone(), Value::Bool(true)))
        .collect();
    json!({ "projectsCollapsed": collapsed, "expanded": expanded_map, "order": order })
}

/// Decodes Desktop's `sidebar.organize` JSON (including legacy pre-mixed-server plain lists).
pub fn parse_organize(v: &Value) -> OrganizeState {
    let strings = |v: &Value| v.as_array().into_iter().flatten().filter_map(|s| Some(s.as_str()?.to_owned())).collect::<Vec<_>>();
    // Saved before servers mixed: plain lists, of the app's own server's projects.
    let own = ServerKey::SIDECAR;
    let collapsed = v["projectsCollapsed"].as_bool().unwrap_or(false);
    let expanded = match &v["expanded"] {
        Value::Object(m) => m.iter().filter(|(_, on)| on.as_bool() == Some(true)).map(|(k, _)| k.clone()).collect(),
        list => strings(list).into_iter().map(|id| format!("{own}:{id}")).collect(),
    };
    let order = match &v["order"] {
        Value::Object(m) => m.iter().map(|(k, ids)| (k.clone(), strings(ids))).collect(),
        Value::Array(_) => std::iter::once((own.to_owned(), strings(&v["order"]))).collect(),
        _ => BTreeMap::new(),
    };
    OrganizeState { collapsed, expanded, order }
}
