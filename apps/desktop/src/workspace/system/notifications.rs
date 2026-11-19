//! Desktop host adapter for `opencode-feature-notifications` (`features/notifications`).

#[allow(unused_imports)]
pub use opencode_feature_notifications::{
    Ask, CAP, Choice, Coordinator, Kind, Kinds, Note, NotificationReply, NotificationsHost,
    Notifier, Outcome, Sink, actions, answered, host, notify_event, notify_plugin, on_action,
    poll, quitting, seen,
};

use opencode_kernel::{Feature, KernelBuilder};
use opencode_kit::client::Server;
use opencode_kit::servers::ServerKey;
use rune::Tx;

use crate::App;

impl NotificationsHost for App {
    fn notifier(&self) -> &Notifier {
        &self.services.notify
    }

    fn notifier_mut(&mut self) -> &mut Notifier {
        &mut self.services.notify
    }

    fn notification_prefs(&self) -> Kinds {
        self.settings.notify
    }

    fn live_server(&self) -> Option<&Server> {
        match &*self.source {
            crate::Source::Live { server, .. } => Some(server),
            crate::Source::Fixture => None,
        }
    }

    fn window_id(&self) -> &str {
        &self.window.id
    }

    fn window_exists(&self, window_id: &str) -> bool {
        crate::windows::handle(window_id).is_some()
    }

    fn active_server_key(&self) -> ServerKey {
        crate::servers::runtime::active(self).clone()
    }

    fn own_server_base(&self) -> Option<String> {
        crate::servers::runtime::own_server().map(|s| s.base)
    }

    fn session_title(&self, session_id: &str) -> Option<String> {
        self.session(session_id).map(|s| s.title.clone())
    }

    fn root_session_id<'a>(&'a self, session_id: &'a str) -> &'a str {
        self.root_of(session_id)
    }

    fn is_root_session(&self, session_id: &str) -> bool {
        self.session(session_id).is_none_or(|s| s.parent.is_none())
    }

    fn has_running_child(&self, session_id: &str) -> bool {
        self.running
            .iter()
            .any(|c| c != session_id && self.session(c).and_then(|s| s.parent.as_deref()) == Some(session_id))
    }

    fn is_session_silenced(&self, session_id: &str) -> bool {
        crate::session_menu::preferences::silenced(self, crate::servers::runtime::active(self), session_id)
    }

    fn auto_accepts_permission(&self, session_id: &str) -> bool {
        self.auto_accepts(session_id)
    }

    fn is_tab_open(&self, session_id: &str) -> bool {
        let server = crate::servers::runtime::active(self);
        self.tabs.iter().any(|t| t.id == session_id && t.server == *server)
    }

    fn tab_title(&self, session_id: &str) -> Option<String> {
        let server = crate::servers::runtime::active(self);
        let tab = self.tabs.iter().find(|t| t.id == session_id && t.server == *server)?;
        let title = self.session(session_id).map_or(tab.title.as_str(), |s| s.title.as_str());
        Some(title.to_owned())
    }

    fn has_tab_for_root(&self, root_session_id: &str) -> bool {
        self.tabs.iter().any(|t| t.id == root_session_id)
    }

    fn has_pending_request(&self, request_id: &str) -> bool {
        self.pending.iter().any(|p| p.id() == request_id)
    }

    fn read_requests(&mut self, session_id: &str) {
        App::read_requests(self, session_id);
    }

    fn open_session_from_notification(&mut self, server: &ServerKey, session_id: &str, tx: &mut Tx) {
        self.close_settings();
        crate::servers::runtime::switch(self, server);
        self.open(session_id, tx);
    }

    fn reply_permission_or_question(&mut self, request_id: &str, reply: NotificationReply<'_>) {
        match reply {
            NotificationReply::Permission(decision) => self.reply_permission(request_id, decision),
            NotificationReply::Question(answers) => {
                self.send_form(request_id, crate::question::reply::Action::Reply(answers));
            }
        }
    }

    fn show_plugin_notification(&mut self, title: String, body: String, _session_id: Option<&str>) {
        crate::status_toast::say(self, title, body);
    }
}

/// Posts the notification event `e` calls for, if any (from `App::react`).
pub fn event(app: &mut App, e: &crate::events::StreamEvent, sid: &str) {
    notify_event(app, &e.ty, &e.data, sid, &mut |want| crate::events::StreamEvent::take(&e.notice, want));
}

/// A notification's link, opened with the app (`deep_links`).
pub fn clicked(_: &mut App, click: &rune_notify::Click) {
    opencode_feature_notifications::clicked(click);
}

/// Gives a windowed run the app's identity before its window opens.
#[cfg(not(target_arch = "wasm32"))]
pub fn identify() {
    opencode_feature_notifications::identify(&crate::launch::product(), crate::headless());
}

/// Settles Windows toast registration and starts serving toast activations off the UI thread.
pub fn settle_identity() {
    opencode_feature_notifications::settle_identity(&crate::launch::product(), crate::headless());
}

/// Captures a COM toast-button activation when launched after quit.
#[cfg(not(target_arch = "wasm32"))]
pub fn take_activation(args: &[String]) {
    opencode_feature_notifications::take_activation(args, &crate::launch::product());
}

impl App {
    pub(crate) fn server_base(&self) -> &str {
        match &*self.source {
            crate::Source::Live { server, .. } => &server.base,
            crate::Source::Fixture => "",
        }
    }

    pub(crate) fn live_status(&self) -> String {
        format!("Live · {}", self.server_base().trim_start_matches("http://"))
    }
}

/// Kernel feature registration for system notifications and actionable toasts.
pub struct NotificationsFeature;

impl Feature<App> for NotificationsFeature {
    fn id(&self) -> &'static str {
        "feature.notifications"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.schedule(
            crate::tick::stage::INGEST_DISPATCH,
            "notifications.poll",
            |s, _, tx| poll(s, tx),
        );
    }
}
