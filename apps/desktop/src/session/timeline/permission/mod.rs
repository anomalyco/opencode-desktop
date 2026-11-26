//! Desktop host adapter for `opencode-feature-timeline-extras::permission`
//! (`features/timeline-extras`).

pub use opencode_feature_timeline_extras::permission::*;

use std::time::Duration;

use opencode_kit::client::Write;
use opencode_kit::model::Pending;
use opencode_kit::wire_error::WireError;
use rune::machine::Attempt;

use crate::question::reply::Action;
use crate::{App, Route, Source};

pub(crate) static PERMISSION_SENT: opencode_kernel::Slot<Sent> = opencode_kernel::Slot::new();

impl PermissionHost for App {
    fn permission_sent(&self) -> &Sent {
        self.kernel.slots.get(&PERMISSION_SENT)
    }

    fn permission_sent_mut(&mut self) -> &mut Sent {
        self.kernel.slots.get_mut(&PERMISSION_SENT)
    }

    fn pending_session(&self, id: &str) -> Option<String> {
        self.pending.iter().find(|p| p.id() == id).map(|p| p.session().to_owned())
    }

    fn permission_asked(&self) -> Option<Pending> {
        session_view::dock::permission::asked(self)
    }

    fn dock_face(&self) -> Option<String> {
        session_view::dock::shown(self)
    }

    fn writes_open(&self) -> bool {
        matches!(&*self.source, Source::Live { allow_send: true, writes: Some(_), .. })
    }

    fn answers_permission_at_once(&self) -> bool {
        self.fixture_run.stage.is_some() || matches!(*self.source, Source::Fixture)
    }

    fn settles_form_at_once(&self) -> bool {
        self.fixture_run.stage.is_some()
    }

    fn write(&mut self, write: Write) -> bool {
        App::write(self, write)
    }

    fn schedule_reply_timeout(&mut self, kind: ReplyTimeoutKind, id: String, attempt: Attempt, delay: Duration) {
        self.job_after(delay, move |_| {
            Box::new(move |s: &mut App, _: &mut rune::Tx| {
                opencode_feature_timeline_extras::permission::reply_timed_out(s, kind, &id, attempt);
            }) as crate::jobs::Apply
        });
    }

    fn on_permission_replied(&mut self, id: &str) {
        self.pending.retain(|p| p.id() != id);
        self.requests.retain(|(_, r)| r != id);
        crate::notifications::answered(self, id);
    }

    fn forget_request(&mut self, id: &str) {
        self.forget(id);
    }

    fn format_wire_error(&self, err: &WireError) -> String {
        crate::errors::said(self, "", err)
    }
}

impl App {
    pub fn permission_asked(&self) -> Option<Pending> {
        opencode_feature_timeline_extras::permission::permission_asked(self)
    }

    pub fn dock_face(&self) -> Option<String> {
        opencode_feature_timeline_extras::permission::dock_face(self)
    }

    pub fn dock_shown(&self) -> bool {
        opencode_feature_timeline_extras::permission::dock_shown(self)
    }

    pub fn reply_permission(&mut self, id: &str, decision: &str) {
        opencode_feature_timeline_extras::permission::reply_permission(self, id, decision);
    }

    pub(crate) fn send_form(&mut self, id: &str, action: Action) {
        opencode_feature_timeline_extras::permission::send_form(self, id, action);
    }

    pub(crate) fn reply_answered(&mut self, id: &str, result: Result<(), &str>) {
        opencode_feature_timeline_extras::permission::reply_answered(self, id, result);
    }
}

pub fn failed(s: &mut App, e: &WireError) -> bool {
    opencode_feature_timeline_extras::permission::failed(s, e)
}

// The fixture session at rest; a bash permission arrives 0.6 s in (`--at 1.5` for a still).
rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("permission", |s| {
        crate::open_scene(s);
        s.finish_stream();
        let Route::Session { id, .. } = (*s.route).clone() else { return };
        let ask = scene_permission_ask(&id);
        s.fixture_run.playback.push_back((*s.clock + 0.6, "permission.asked".into(), ask));
        s.wake_at_playback();
    }),
]);

mod stories;
