//! Canonical shared domain value types used across `apps/*` and `features/*`.
//!
//! Centralizes small headless data types that previously lived as identical copies across feature
//! crates (`panels`, `settings`, `notifications`, `restore`, `composer-input`, `inbox`,
//! `annotations`, `narrow`, `projects`, `session-menu`) and `apps/desktop`.

use opencode_kit::servers::{ServerKey, TabKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The pane a new terminal opens in beside no other (`terminalPlacement` in Desktop).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockPlacement {
    #[default]
    Side,
    Bottom,
}

impl DockPlacement {
    pub const ALL: [DockPlacement; 2] = [DockPlacement::Side, DockPlacement::Bottom];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Side => "side",
            Self::Bottom => "bottom",
        }
    }

    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("bottom") => Self::Bottom,
            _ => Self::Side,
        }
    }
}

/// What a notification is about, as Settings › Notifications groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    /// The agent finished (`session.execution.succeeded`).
    Agent,
    /// A permission is asked (`permission.asked`, or a web search provider's form).
    Permission,
    /// The agent asks a question, or an MCP server asks for input (`form.created`).
    Question,
    /// A run failed (`session.execution.failed`).
    Error,
}

impl NotifyKind {
    pub const ALL: [NotifyKind; 4] = [
        NotifyKind::Agent,
        NotifyKind::Permission,
        NotifyKind::Question,
        NotifyKind::Error,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Permission => "permissions",
            Self::Question => "questions",
            Self::Error => "errors",
        }
    }

    /// i18n message catalog key for [`Self::title`].
    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Agent => "notification.session.responseReady.title",
            Self::Permission => "notification.permission.title",
            Self::Question => "notification.question.title",
            Self::Error => "notification.session.error.title",
        }
    }

    /// Desktop's localized titles (`notification.session.responseReady.title` and friends).
    pub fn title(self) -> &'static str {
        match self {
            Self::Agent => opencode_i18n::t!("notification.session.responseReady.title"),
            Self::Permission => opencode_i18n::t!("notification.permission.title"),
            Self::Question => opencode_i18n::t!("notification.question.title"),
            Self::Error => opencode_i18n::t!("notification.session.error.title"),
        }
    }
}

/// Which kinds notify: Settings › Notifications, saved as Desktop's `notifications` object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NotifyKinds {
    pub agent: bool,
    pub permissions: bool,
    pub questions: bool,
    pub errors: bool,
}

impl Default for NotifyKinds {
    fn default() -> Self {
        Self { agent: false, permissions: true, questions: true, errors: false }
    }
}

impl NotifyKinds {
    pub fn get(&self, kind: NotifyKind) -> bool {
        match kind {
            NotifyKind::Agent => self.agent,
            NotifyKind::Permission => self.permissions,
            NotifyKind::Question => self.questions,
            NotifyKind::Error => self.errors,
        }
    }

    pub fn set(&mut self, kind: NotifyKind, on: bool) {
        match kind {
            NotifyKind::Agent => self.agent = on,
            NotifyKind::Permission => self.permissions = on,
            NotifyKind::Question => self.questions = on,
            NotifyKind::Error => self.errors = on,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "agent": self.agent,
            "permissions": self.permissions,
            "questions": self.questions,
            "errors": self.errors,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let d = Self::default();
        Self {
            agent: v["agent"].as_bool().unwrap_or(d.agent),
            permissions: v["permissions"].as_bool().unwrap_or(d.permissions),
            questions: v["questions"].as_bool().unwrap_or(d.questions),
            errors: v["errors"].as_bool().unwrap_or(d.errors),
        }
    }
}

/// A file's lines waiting in the composer, 1-based and in order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextFile {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

/// How a send ended, for whoever waits on it (such as the quote comment editor's send): the server
/// has it, it failed for good, or the outbox keeps it to send when it can (waiting for the link,
/// or for a receipt).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SendResult {
    Accepted,
    Failed,
    Queued,
}

/// Activity and notification badge status for an open tab or session row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TabStatus {
    Busy,
    Attention,
    Unread,
    /// Unread, and its run failed.
    Failed,
}

/// Pure-data descriptor of one open window tab shared by project tracking, Home landing, and
/// session context menus.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TabSummary {
    /// Unique tab identifier in the window (`Tab::uid`).
    pub uid: String,
    /// Session ID or draft ID.
    pub id: String,
    /// Working directory recorded on the tab (empty when unused).
    pub directory: String,
    /// Server the tab's session lives on, or where a draft's session will start.
    pub server: ServerKey,
    /// `true` for a "New session" draft tab.
    pub is_draft: bool,
}

impl TabSummary {
    /// Its identity in the window's stored rows ([`TabKey`]).
    pub fn key(&self) -> TabKey {
        if self.is_draft {
            TabKey::draft(self.id.clone())
        } else {
            TabKey::session(self.server.clone(), self.id.clone())
        }
    }
}

/// Desktop's zoom bounds (`desktopZoomLevels`, Chromium's from 50% to 200% preset steps).
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 3.0;

/// Desktop's zoom levels (`desktopZoomLevels`, Chromium's from 50% to 200%): the zoom keys and
/// Appearance › Zoom each pick one.
pub const ZOOM_LEVELS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// The level after `zoom` in `dir`'s direction (`nextDesktopZoomLevel`), from wherever a pinch
/// left it; `dir` 0 resets.
pub fn step_zoom(zoom: f32, dir: i32) -> f32 {
    if dir == 0 {
        return 1.0;
    }
    if dir > 0 {
        return ZOOM_LEVELS
            .into_iter()
            .find(|l| *l > zoom + 0.01)
            .unwrap_or(ZOOM_LEVELS[ZOOM_LEVELS.len() - 1]);
    }
    ZOOM_LEVELS
        .into_iter()
        .rev()
        .find(|l| *l < zoom - 0.01)
        .unwrap_or(ZOOM_LEVELS[0])
}
