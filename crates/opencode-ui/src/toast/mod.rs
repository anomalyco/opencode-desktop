//! Toast notifications at the window's bottom-right, matching OpenCode Desktop's `Toast.Region`
//! (`packages/ui/src/feedback/toast/toast.tsx`, `toast.css`), with instant entrance and exit
//! (`docs/kit-design.md`).
//!
//! Toasts stack as Sonner stacks them (`rune_kit::ToastStack`): the newest in front, up to two
//! more peeking behind it; hovering the stack spreads them out, and while hovered or while the
//! window is hidden no toast's timer advances. A toast shown again in front pulses.
//!
//! Pure toast state ([`StatusToast<C>`], [`Entry<C>`], [`Shown<C>`], [`ToastAction<C>`],
//! [`ToastButton<C>`]) is parameterized on a host-defined custom action payload `C` (defaulting to
//! `()`). Runtime transitions ([`show`], [`say`], [`succeed`], [`fail`], [`fail_details`],
//! [`waiting`], [`dismiss`], [`dismiss_id`], [`poll`], [`visibility`], [`hotkey`], [`key`]) and
//! rendering ([`view`]) operate over any host implementing [`ToastHost`].

use std::path::{Path, PathBuf};

use rune::{Anim, Curve, Easing, El, Icon, LiveRegion, Tx};
use rune_kit::ToastStack;
use rune_kit::toast_stack::{CONTENT_FADE_SECS, FADE_SECS, MOVE_SECS};
use crate::palette::Pal;

mod state;
mod view;

pub use state::{
    dismiss, dismiss_id, fail, fail_details, from_server, hotkey, key, noted, plain, poll, say,
    show, succeed, uploads_shown, visibility, waiting, write_title,
};
pub use view::{neutral_lift, view};

/// Desktop's `duration={5000}` on `Toast.Region`.
pub const DURATION: f64 = 5.0;

/// `toast.css`: `transform 280ms cubic-bezier(0.2, 0, 0, 1)`, `height` the same.
pub(super) const MOVE: Anim = Anim::new(Curve::tween(MOVE_SECS, Easing::cubic(0.2, 0.0, 0.0, 1.0)));
/// `opacity 160ms ease-out`.
pub(super) const FADE: Anim = Anim::new(Curve::tween(FADE_SECS, Easing::EASE_OUT));
/// Sonner's `[data-sonner-toast] > * { transition: opacity 400ms }`.
pub(super) const CONTENT_FADE: Anim = Anim::new(Curve::tween(CONTENT_FADE_SECS, Easing::EASE));

/// The stack's key: Sonner's list (`ol`, `tabIndex={-1}`), which the hotkey focuses.
pub(super) const STACK: &str = "toast-stack";

/// Metadata describing a host-defined [`ToastAction::Custom`] button.
#[derive(Clone, Debug)]
pub struct ActionMeta {
    /// Element key (prefix with `"toast-"` so keyboard focus expands the stack).
    pub key: &'static str,
    /// Visible button label.
    pub label: String,
    /// Deduplication tag folded into [`show`]'s stack key.
    pub dedup: String,
}

/// How the toast stack clears a bottom composer or dock surface when placed in [`view`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ComposerClearance {
    /// No composer on screen: stay at the standard bottom offset (48 pt desktop, 16 pt narrow).
    #[default]
    None,
    /// Active session view: lift 8 pt above `"dock"` / `"composer"` when horizontally overlapping.
    Session,
    /// Draft view: lift 8 pt above `"composer"` only when both horizontally and vertically
    /// overlapping.
    Draft,
}

/// Structured failure text for [`fail_details`]: what happened, why, an optional server quote and
/// next-step hint, clipboard details, and a single-line status summary.
#[derive(Clone, Copy, Debug)]
pub struct FailureNotice<'a> {
    pub what: &'a str,
    pub why: &'a str,
    pub quote: Option<&'a str>,
    pub next: Option<&'a str>,
    pub details: &'a str,
    pub status_summary: &'a str,
}

/// Primary single-button action attached to a status toast [`Entry`].
#[derive(Clone, Debug)]
pub enum ToastAction<C = ()> {
    /// Copies a failure's diagnostic details to the clipboard.
    CopyDetails(String),
    /// A button (its label) that reveals a path in the platform file manager.
    Reveal(String, PathBuf),
    /// A host-defined action (such as undo, rewind undo, or onboarding tip install).
    Custom(C),
}

/// A button shown in a toast's multi-button action row (`button-v2` small, neutral or ghost).
///
/// Covers both built-in recovery notices and plugin-supplied toast buttons without coupling
/// `opencode-ui` to host app types or scripting runtimes.
#[derive(Clone, Debug)]
pub struct ToastButton<C = ()> {
    /// Explicit element key (prefix with `"toast-"` so focus expands the stack). Defaults to
    /// `"toast-action-{index}"` when `None`.
    pub key: Option<&'static str>,
    pub label: String,
    /// `true` for Desktop's ghost button, `false` for its primary neutral button.
    pub secondary: bool,
    /// Host-defined payload dispatched through [`ToastHost::on_custom_action`].
    pub action: C,
}

impl<C> ToastButton<C> {
    /// A primary neutral toast button.
    pub fn primary(label: impl Into<String>, action: C) -> Self {
        Self { key: None, label: label.into(), secondary: false, action }
    }

    /// A secondary ghost toast button.
    pub fn secondary(label: impl Into<String>, action: C) -> Self {
        Self { key: None, label: label.into(), secondary: true, action }
    }

    /// Assigns an explicit element key to this button.
    pub fn with_key(mut self, key: &'static str) -> Self {
        self.key = Some(key);
        self
    }
}

/// Data for a single status toast card in [`StatusToast`].
#[derive(Clone, Debug)]
pub struct Entry<C = ()> {
    pub id: u64,
    pub title: String,
    pub description: Option<String>,
    /// Someone else's words, shown as theirs under the description (a server's explanation).
    pub quote: Option<String>,
    /// What to do now, a line of its own under the rest.
    pub next: Option<String>,
    pub action: Option<ToastAction<C>>,
    /// The glyph before the words: a success toast's tick.
    pub icon: Option<&'static Icon>,
    pub persistent: bool,
    /// Multi-button actions (recovery buttons, plugin buttons), rendered in place of `action`.
    pub buttons: Vec<ToastButton<C>>,
    /// Optional caller-assigned handle (e.g. a plugin's `ToastHandle`) for update/dismiss lookup.
    pub handle: Option<u64>,
}

impl<C> Entry<C> {
    /// Creates a minimal toast entry with `title`, optional `description`, and `persistent` flag.
    pub fn new(title: impl Into<String>, description: Option<String>, persistent: bool) -> Self {
        Self {
            id: 0,
            title: title.into(),
            description,
            quote: None,
            next: None,
            action: None,
            icon: None,
            persistent,
            buttons: Vec::new(),
            handle: None,
        }
    }
}

/// What a toast in the stack shows.
#[derive(Clone, Debug)]
pub enum Shown<C = ()> {
    Status(Box<Entry<C>>),
    /// The uploads in flight (`toaster.show`'s custom upload toast).
    Uploads,
}

/// Per-window toast stack and front-toast re-show pulse state.
pub struct StatusToast<C = ()> {
    pub stack: ToastStack<Shown<C>>,
    pulse: state::Pulse,
}

impl<C> Default for StatusToast<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> StatusToast<C> {
    pub fn new() -> Self {
        Self { stack: ToastStack::default(), pulse: state::Pulse::default() }
    }

    /// The newest status toast.
    pub fn current(&self) -> Option<&Entry<C>> {
        self.stack.entries().iter().rev().find_map(|e| match &e.options {
            Shown::Status(entry) => Some(entry.as_ref()),
            Shown::Uploads => None,
        })
    }

    /// [`Self::current`], to change.
    pub fn current_mut(&mut self) -> Option<&mut Entry<C>> {
        self.stack.entries_mut().iter_mut().rev().find_map(|e| match &mut e.options {
            Shown::Status(entry) => Some(entry.as_mut()),
            Shown::Uploads => None,
        })
    }

    /// Stack id of the status toast registered with `handle`, if still showing.
    pub fn id_for_handle(&self, handle: u64) -> Option<u64> {
        self.stack.entries().iter().find_map(|e| match &e.options {
            Shown::Status(entry) if entry.handle == Some(handle) => Some(e.id),
            _ => None,
        })
    }

    /// Mutable status toast registered with `handle`, if still showing.
    pub fn by_handle_mut(&mut self, handle: u64) -> Option<&mut Entry<C>> {
        self.stack.entries_mut().iter_mut().find_map(|e| match &mut e.options {
            Shown::Status(entry) if entry.handle == Some(handle) => Some(entry.as_mut()),
            _ => None,
        })
    }
}

/// Host capabilities a Rune application supplies to the shared toast stack.
///
/// Only [`toast`](Self::toast), [`toast_mut`](Self::toast_mut), and [`clock`](Self::clock) are
/// required. Every other hook carries a sensible default so minimal embedders need only wire the
/// capabilities they use.
pub trait ToastHost: Sized + 'static {
    /// Host-defined payload carried by [`ToastAction::Custom`] and [`ToastButton::action`].
    type CustomAction: Clone + 'static;

    /// Read-only access to the host's toast stack (must not dirty `rune::Tracked` cells).
    fn toast(&self) -> &StatusToast<Self::CustomAction>;

    /// Mutable access to the host's toast stack (called only when toast state actually changes).
    fn toast_mut(&mut self) -> &mut StatusToast<Self::CustomAction>;

    /// Current frame clock in seconds.
    fn clock(&self) -> f64;

    /// Whether reduced motion is active (disables front-toast re-show pulse).
    fn reduced_motion(&self) -> bool {
        false
    }

    /// Current viewport width in window points, used for the <= 600 pt mobile layout. Returning
    /// `0.0` uses the standard 320 pt desktop width.
    fn viewport_width(&self) -> f32 {
        0.0
    }

    /// Whether a composer or dock surface is visible that the toast stack should clear.
    fn composer_clearance(&self) -> ComposerClearance {
        ComposerClearance::None
    }

    /// Records the latest toast summary line (e.g. for `--debug`'s status bar).
    fn set_status(&mut self, _status: String) {}

    /// Announces a newly shown toast to assistive technology.
    fn announce(&mut self, _words: String, _region: LiveRegion) {}

    /// Escalates the most recent announcement to an urgent alert (`role="alert"`).
    fn announce_urgent(&mut self) {}

    /// Called when Alt+T expands and focuses the toast stack, letting the host blur active text
    /// fields such as the session composer.
    fn on_focus_taken(&mut self) {}

    /// Whether any uploads are currently in flight; when `false`, an open [`Shown::Uploads`]
    /// toast closes on the next [`poll`].
    fn has_uploads(&self) -> bool {
        false
    }

    /// Renders the custom card for [`Shown::Uploads`]. Returning `None` skips rendering it.
    fn uploads_card(&self, _p: &Pal, _content_shows: bool) -> Option<El> {
        None
    }

    /// Element key, visible button label, and deduplication tag for a [`ToastAction::Custom`].
    fn custom_action_meta(&self, _action: &Self::CustomAction) -> ActionMeta {
        ActionMeta { key: "toast-custom", label: String::new(), dedup: String::new() }
    }

    /// Whether a persistent toast with this custom action ignores bare `Escape` (e.g. onboarding
    /// tips that stay until closed via their close button).
    fn custom_action_blocks_escape(&self, _action: &Self::CustomAction) -> bool {
        false
    }

    /// Invoked when a [`ToastAction::Custom`] button or [`ToastButton`] is pressed.
    ///
    /// The host decides whether and when to call [`dismiss_id`] (for example, claiming an undo
    /// offer before closing, or keeping a recovery toast open after exporting logs).
    fn on_custom_action(&mut self, _id: u64, _action: Self::CustomAction, _tx: &mut Tx) {}

    /// Invoked after the user explicitly clicks a toast's close (`×`) button.
    fn on_dismissed(&mut self, _id: u64, _tx: &mut Tx) {}

    /// Invoked whenever any toast leaves the stack (`ToastEffect::Closed`), whether by timeout,
    /// dismissal, or replacement.
    fn on_closed(&mut self, _id: u64) {}

    /// Reveals `path` in the platform file manager when a [`ToastAction::Reveal`] button is
    /// pressed.
    fn reveal_path(&mut self, _path: &Path) {}
}
