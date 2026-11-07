//! Auto-accept permissions, as in Desktop (`session/requests/auto-approve.ts`, the `permissions.autoaccept`
//! command on ⇧⌘A, Settings › General). One client-local switch covers every session and project: while
//! it's on, each permission request is answered "once" the moment it's known, pending ones included,
//! and the permission card stays hidden. Forms and questions still wait for a person.

use opencode_kit::model::Pending;
use rune::{El, Tx, row};

use crate::palette::Pal;
use rune::icon;
use rune_theme_opencode::icons as ic;

use crate::App;

impl App {
    pub fn set_auto_accept(&mut self, on: bool) {
        self.change_settings(|s| s.auto_accept = on);
        self.answer_auto_accepted();
    }

    /// A project's own Auto-accept permissions: on, off, or `None` for the default.
    pub fn set_project_auto_accept(&mut self, project: &str, on: Option<bool>) {
        let project = project.to_owned();
        self.change_settings(|s| match on {
            Some(on) => {
                s.project_auto_accept.insert(project, on);
            }
            None => {
                s.project_auto_accept.remove(&project);
            }
        });
        self.answer_auto_accepted();
    }

    /// Whether session `session`'s permission requests are approved without asking: its
    /// project's own setting, else the default (`permissions.autoApproves`).
    pub fn auto_accepts(&self, session: &str) -> bool {
        let project = self.session(session).and_then(|i| self.project_for_session(i)).map(|p| p.id.as_str());
        project.and_then(|id| self.settings.project_auto_accept.get(id).copied()).unwrap_or(self.settings.auto_accept)
    }

    /// Answers the waiting permission requests auto-accept now covers.
    fn answer_auto_accepted(&mut self) {
        let open: Vec<String> = self.pending.iter()
            .filter_map(|p| match p { Pending::Permission { id, session, .. } if self.auto_accepts(session) => Some(id.clone()), _ => None })
            .collect();
        for id in open {
            self.reply_permission(&id, "once");
        }
    }

    pub fn toggle_auto_accept(&mut self, tx: &mut Tx) {
        tx.instant();
        self.set_auto_accept(!self.settings.auto_accept);
    }

    /// A request that just arrived: answered at once when auto-accept is on.
    pub(crate) fn auto_answer(&mut self, id: &str) {
        if self.pending.iter().any(|p| matches!(p, Pending::Permission { id: pid, session, .. } if pid == id && self.auto_accepts(session))) {
            self.reply_permission(id, "once");
        }
    }
}

/// The composer toolbar's mark that auto-accept is on; pressing it turns it off.
pub fn indicator(s: &App, p: &Pal) -> Option<El> {
    s.settings.auto_accept.then(|| {
        row()
            .key("auto-accept")
            .role("button")
            .label("Stop auto-accepting permissions")
            .size(28.0, 28.0)
            .center()
            .radius(6.0)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(|s: &mut App, tx: &mut Tx| s.toggle_auto_accept(tx))
            .child(icon(&ic::CHECK_SMALL, 16.0).color(p.accent))
    })
}

#[cfg(test)]
mod tests;
