//! Desktop host adapter for `opencode-feature-timeline-extras::auto_accept`
//! (`features/timeline-extras`).

pub use opencode_feature_timeline_extras::auto_accept::*;

use opencode_kit::model::Pending;
use rune::Tx;

use crate::{App, Route};

impl AutoAcceptHost for App {
    fn auto_accept_default(&self) -> bool {
        self.settings.auto_accept
    }

    fn project_auto_accept(&self, project_id: &str) -> Option<bool> {
        self.settings.project_auto_accept.get(project_id).copied()
    }

    fn set_auto_accept_default(&mut self, on: bool) {
        self.change_settings(|s| s.auto_accept = on);
    }

    fn set_project_auto_accept_override(&mut self, project_id: &str, on: Option<bool>) {
        let project = project_id.to_owned();
        self.change_settings(|s| match on {
            Some(on) => {
                s.project_auto_accept.insert(project, on);
            }
            None => {
                s.project_auto_accept.remove(&project);
            }
        });
    }

    fn active_session_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    fn project_id_for_session(&self, session_id: &str) -> Option<&str> {
        self.session(session_id)
            .and_then(|i| self.project_for_session(i))
            .map(|p| p.id.as_str())
    }

    fn pending(&self) -> &[Pending] {
        &self.pending
    }

    fn reply_permission(&mut self, id: &str, decision: &str) {
        App::reply_permission(self, id, decision);
    }

    fn toast_say(&mut self, title: String, description: String) {
        crate::status_toast::say(self, title, description);
    }
}

impl App {
    pub fn set_auto_accept(&mut self, on: bool) {
        opencode_feature_timeline_extras::auto_accept::set_auto_accept(self, on);
    }

    pub fn set_project_auto_accept(&mut self, project: &str, on: Option<bool>) {
        opencode_feature_timeline_extras::auto_accept::set_project_auto_accept(self, project, on);
    }

    pub fn auto_accepts(&self, session: &str) -> bool {
        opencode_feature_timeline_extras::auto_accept::auto_accepts(self, session)
    }

    pub(crate) fn auto_accepting(&self) -> bool {
        opencode_feature_timeline_extras::auto_accept::auto_accepting(self)
    }

    pub fn toggle_auto_accept(&mut self, tx: &mut Tx) {
        opencode_feature_timeline_extras::auto_accept::toggle_auto_accept(self, tx);
    }

    pub(crate) fn auto_answer(&mut self, id: &str) {
        opencode_feature_timeline_extras::auto_accept::auto_answer(self, id);
    }
}
