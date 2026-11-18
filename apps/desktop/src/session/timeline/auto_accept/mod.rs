//! Auto-accept permissions, as in Desktop (`session/requests/auto-approve.ts`, the `permissions.autoaccept`
//! command on ⇧⌘A, Settings › General). One client-local switch covers every session and project: while
//! it's on, each permission request is answered "once" the moment it's known, pending ones included,
//! and the permission card stays hidden. Forms and questions still wait for a person.

use opencode_kit::model::Pending;
use rune::Tx;

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

    /// The open session's project, which ⇧⌘A and the command act on (`sessionProject`).
    fn auto_accept_project(&self) -> Option<String> {
        let crate::Route::Session { id, .. } = &*self.route else { return None };
        self.session(id).and_then(|i| self.project_for_session(i)).map(|p| p.id.clone())
    }

    /// Whether auto-accept is on where ⇧⌘A acts (`autoAccepting`).
    pub(crate) fn auto_accepting(&self) -> bool {
        match self.auto_accept_project() {
            Some(p) => self.settings.project_auto_accept.get(&p).copied().unwrap_or(self.settings.auto_accept),
            None => self.settings.auto_accept,
        }
    }

    /// ⇧⌘A (`toggleAutoAccept`, `use-session-commands.tsx:439-467`): from a session it sets the
    /// project's own choice, back to the default when they match; elsewhere the default. A toast
    /// says what it did.
    pub fn toggle_auto_accept(&mut self, tx: &mut Tx) {
        use opencode_i18n::t;
        tx.instant();
        let project = self.auto_accept_project();
        let active = !self.auto_accepting();
        match &project {
            None => self.set_auto_accept(active),
            Some(p) => self.set_project_auto_accept(p, (active != self.settings.auto_accept).then_some(active)),
        }
        let (title, description) = match (active, project.is_some()) {
            (false, _) => (t!("toast.permissions.autoaccept.off.title"), t!("toast.permissions.autoaccept.off.description")),
            (true, true) => (t!("toast.permissions.autoaccept.project.title"), t!("toast.permissions.autoaccept.project.description")),
            (true, false) => (t!("toast.permissions.autoaccept.on.title"), t!("toast.permissions.autoaccept.on.description")),
        };
        crate::status_toast::say(self, title, description);
    }

    /// A request that just arrived: answered at once when auto-accept is on.
    pub(crate) fn auto_answer(&mut self, id: &str) {
        if self.pending.iter().any(|p| matches!(p, Pending::Permission { id: pid, session, .. } if pid == id && self.auto_accepts(session))) {
            self.reply_permission(id, "once");
        }
    }
}
