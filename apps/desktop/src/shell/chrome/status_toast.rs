//! Desktop host adapter for `opencode_ui::toast` (`crates/opencode-ui`).
//!
//! Defines desktop-only toast actions (rewind undo, deferred undo, onboarding tips, crash recovery
//! buttons, and Luau plugin buttons) and implements [`ToastHost`] for [`App`].

pub use opencode_ui::toast::{
    ActionMeta, ComposerClearance, DURATION, FailureNotice, ToastButton, ToastHost, dismiss,
    dismiss_id, fail, fail_details, from_server, hotkey, key, plain, poll, say, show, succeed,
    uploads_shown, view, visibility, waiting, write_title,
};

use std::path::Path;

use rune::{El, LiveRegion, Tx};

use crate::App;
use crate::palette::Pal;
use crate::rewind::UndoRewind;

/// Built-in recovery toast buttons (`crate::crash_notice`): first is primary neutral, rest ghost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeButton {
    OpenSkippedSession,
    ExportLogs,
}

/// A plugin toast's button (`ToastAction` with `run`): it closes the toast, then runs `run` in its
/// plugin or opens `url`. `secondary` is Desktop's ghost button, the rest its neutral one.
#[derive(Clone, Debug)]
pub struct PluginButton {
    pub label: String,
    pub secondary: bool,
    pub plugin: std::rc::Rc<str>,
    pub run: Option<mlua::Function>,
    pub url: Option<String>,
}

/// Desktop-specific toast action payloads carried by `opencode_ui::toast`.
#[derive(Clone, Debug)]
pub enum DesktopToastAction {
    UndoRewind(UndoRewind),
    /// Puts back what the toast says was done (`crate::undo`): its offer goes with the toast.
    Undo,
    /// A tip's Install (`crate::tips`): the tip is done and its page opens.
    Tip(crate::tips::queue::Tip),
    /// Built-in recovery button (`crate::crash_notice`).
    Native(NativeButton),
    /// Luau plugin toast button.
    Plugin(PluginButton),
}

pub type StatusToast = opencode_ui::toast::StatusToast<DesktopToastAction>;
pub type Entry = opencode_ui::toast::Entry<DesktopToastAction>;
#[allow(dead_code)]
pub type Shown = opencode_ui::toast::Shown<DesktopToastAction>;
pub type ToastAction = opencode_ui::toast::ToastAction<DesktopToastAction>;

impl ToastHost for App {
    type CustomAction = DesktopToastAction;

    fn toast(&self) -> &StatusToast {
        &self.dialogs.toast
    }

    fn toast_mut(&mut self) -> &mut StatusToast {
        &mut self.dialogs.toast
    }

    fn clock(&self) -> f64 {
        *self.clock
    }

    fn reduced_motion(&self) -> bool {
        *self.appearance_state.reduced_motion
    }

    fn viewport_width(&self) -> f32 {
        self.layout.narrow.width()
    }

    fn composer_clearance(&self) -> ComposerClearance {
        if self.settings.open {
            return ComposerClearance::None;
        }
        match &*self.route {
            crate::Route::Session { .. }
                if !self.narrow() || self.narrow_view() == crate::narrow::machine::CONVERSATION =>
            {
                ComposerClearance::Session
            }
            crate::Route::Draft(_) => ComposerClearance::Draft,
            _ => ComposerClearance::None,
        }
    }

    fn set_status(&mut self, status: String) {
        *self.diag.status = status;
    }

    fn announce(&mut self, words: String, region: LiveRegion) {
        self.kernel.slots.slot::<crate::speech::Speech>().say(words, region);
    }

    fn announce_urgent(&mut self) {
        self.kernel.slots.slot::<crate::speech::Speech>().urgent();
    }

    fn on_focus_taken(&mut self) {
        self.composer.focused = false;
    }

    fn has_uploads(&self) -> bool {
        !self.composer_ext.attach.uploads.is_empty()
    }

    fn uploads_card(&self, p: &Pal, content_shows: bool) -> Option<El> {
        Some(crate::attachments::toast::card(self, p, content_shows))
    }

    fn custom_action_meta(&self, action: &DesktopToastAction) -> ActionMeta {
        match action {
            DesktopToastAction::UndoRewind(_) | DesktopToastAction::Undo => ActionMeta {
                key: "toast-undo",
                label: opencode_i18n::t!("session.rewind.toast.undo").to_owned(),
                dedup: "undo".to_owned(),
            },
            DesktopToastAction::Tip(tip) => ActionMeta {
                key: "toast-tip-install",
                label: tip.install().to_owned(),
                dedup: tip.install().to_owned(),
            },
            DesktopToastAction::Native(_) | DesktopToastAction::Plugin(_) => {
                ActionMeta { key: "toast-action", label: String::new(), dedup: String::new() }
            }
        }
    }

    fn custom_action_blocks_escape(&self, action: &DesktopToastAction) -> bool {
        // A tip isn't a failure: it stays until its own × (Sonner's toasts take no Escape).
        matches!(action, DesktopToastAction::Tip(_))
    }

    fn on_custom_action(&mut self, id: u64, action: DesktopToastAction, tx: &mut Tx) {
        match action {
            DesktopToastAction::Undo => {
                // The offer is taken before its toast goes, which would withdraw it.
                crate::undo::pressed(self, id, tx);
                dismiss_id(self, id);
            }
            DesktopToastAction::UndoRewind(undo) => {
                dismiss_id(self, id);
                self.undo_rewind(undo, tx);
            }
            DesktopToastAction::Tip(tip) => {
                dismiss_id(self, id);
                crate::tips::installed(self, id, tip, tx);
            }
            DesktopToastAction::Native(button) => match button {
                NativeButton::OpenSkippedSession => crate::crash_notice::open_anyway(self, id, tx),
                NativeButton::ExportLogs => self.export_logs(),
            },
            DesktopToastAction::Plugin(b) => {
                dismiss_id(self, id);
                if let Some(url) = &b.url {
                    let _ = tx.open_url(url);
                }
                let Some(run) = &b.run else { return };
                let Some(index) = self.plugins.running().find(|(_, pl)| pl.half.id == b.plugin).map(|(i, _)| i) else {
                    return;
                };
                crate::plugin_host::commands::spawn(self, index, run, (), tx);
            }
        }
    }

    fn on_dismissed(&mut self, id: u64, tx: &mut Tx) {
        crate::tips::dismissed(self, id, tx);
    }

    fn on_closed(&mut self, id: u64) {
        crate::undo::toast_closed(self, id);
        crate::crash_notice::toast_closed(self, id);
    }

    fn reveal_path(&mut self, path: &Path) {
        crate::plugin_host::reveal(&path.to_string_lossy());
    }
}

/// Converts Luau plugin toast buttons into shared `opencode_ui::toast::ToastButton` items.
pub fn plugin_buttons(buttons: Vec<PluginButton>) -> Vec<ToastButton<DesktopToastAction>> {
    buttons
        .into_iter()
        .map(|b| {
            let label = b.label.clone();
            if b.secondary {
                ToastButton::secondary(label, DesktopToastAction::Plugin(b))
            } else {
                ToastButton::primary(label, DesktopToastAction::Plugin(b))
            }
        })
        .collect()
}

/// Non-modal next-launch notice after an unexpected close (`crate::crash_notice`).
pub fn crash_notice(
    s: &mut App,
    title: String,
    description: String,
    restored_line: Option<String>,
    native_buttons: Vec<NativeButton>,
) -> u64 {
    let summary = match &restored_line {
        Some(r) => format!("{title}: {description} {r}"),
        None => format!("{title}: {description}"),
    };
    *s.diag.status = summary;
    let buttons = native_buttons
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let (key, label) = match b {
                NativeButton::OpenSkippedSession => ("toast-open-anyway", opencode_i18n::t!("rune.crash.openAnyway")),
                NativeButton::ExportLogs => ("toast-export-logs", opencode_i18n::t!("command.logs.export")),
            };
            let btn = if i > 0 {
                ToastButton::secondary(label, DesktopToastAction::Native(b))
            } else {
                ToastButton::primary(label, DesktopToastAction::Native(b))
            };
            btn.with_key(key)
        })
        .collect();
    let id = show(
        s,
        Entry {
            next: restored_line,
            buttons,
            ..Entry::new(title, Some(description), true)
        },
        None,
    );
    s.kernel.slots.slot::<crate::speech::Speech>().urgent();
    id
}

/// Shows `"Reverted"` with an `"Undo"` button bound to `undo`.
pub fn rewound(s: &mut App, undo: UndoRewind) -> u64 {
    let e = Entry {
        action: Some(ToastAction::Custom(DesktopToastAction::UndoRewind(undo))),
        ..Entry::new(opencode_i18n::t!("session.rewind.toast.title"), None, false)
    };
    show(s, e, Some(DURATION))
}

/// Shows `title` with an `Undo` button for [`crate::undo::SECS`] s; its id, which the offer goes by.
pub fn undoable(s: &mut App, title: &str) -> u64 {
    let e = Entry {
        action: Some(ToastAction::Custom(DesktopToastAction::Undo)),
        ..Entry::new(title, None, false)
    };
    show(s, e, Some(crate::undo::SECS))
}

/// Shows `tip` until the person closes it or presses Install (`tips/model.ts`: `persistent`,
/// one action); its id.
pub fn tip(s: &mut App, tip: crate::tips::queue::Tip) -> u64 {
    let e = Entry {
        action: Some(ToastAction::Custom(DesktopToastAction::Tip(tip))),
        ..Entry::new(tip.title(), Some(tip.description().to_owned()), true)
    };
    show(s, e, None)
}

/// A call titled `what` failed with `wire`: why and what next, the server's words quoted where
/// it gave them, built from the typed error (`crate::errors`).
pub fn failed_wire(s: &mut App, what: &str, wire: &opencode_kit::wire_error::WireError) {
    let server = crate::servers::runtime::label(crate::servers::runtime::active(s));
    error(s, &crate::errors::UserError::from_wire(what, &server, wire));
}

/// A read or write that failed (`Incoming::Failed`): a write shows Desktop's toast for it, said
/// from the typed error; a read stays in the debug bar, since the app retries those itself.
pub fn from_wire(s: &mut App, e: opencode_kit::wire_error::WireError) {
    if e.method == "GET" {
        *s.diag.status = e.to_string();
        return;
    }
    let server = crate::servers::runtime::label(crate::servers::runtime::active(s));
    let title = if e.method.is_empty() { opencode_i18n::t!("common.requestFailed") } else { write_title(e.method, &e.path) };
    error(s, &crate::errors::UserError::from_wire(title, &server, &e));
}

/// Shows `e`: what happened, why and what to do, with Copy details, until dismissed.
pub fn error(s: &mut App, e: &crate::errors::UserError) {
    let status_summary = e.description().replace('\n', " ");
    fail_details(
        s,
        FailureNotice {
            what: &e.what,
            why: &e.why,
            quote: e.quote.as_deref(),
            next: e.next.as_deref(),
            details: &e.details,
            status_summary: &status_summary,
        },
    );
}
