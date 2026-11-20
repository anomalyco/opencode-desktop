//! Desktop host adapter for `opencode-feature-widgets` (`features/widgets`): agent widgets in
//! the timeline, their template programs fetched through the server connection's job thread,
//! and their responses posted there.

pub use opencode_feature_widgets::*;

use std::sync::Arc;

use opencode_kernel::{Feature, KernelBuilder, Slot};
use opencode_kit::model::{Message, Widget as WidgetMessage};
use rune::Tx;
use serde_json::Value;

use crate::App;
use crate::jobs::Apply;

pub(crate) static WIDGETS: Slot<Widgets> = Slot::new();

impl WidgetsHost for App {
    fn widgets(&self) -> &Widgets {
        self.kernel.slots.get(&WIDGETS)
    }

    fn widgets_mut(&mut self) -> &mut Widgets {
        self.kernel.slots.get_mut(&WIDGETS)
    }

    fn widget_session(&self) -> Option<(String, u64)> {
        let id = session_view::TimelineHost::active_session_id(self)?.to_owned();
        let version = session_view::TimelineHost::transcript_version(self, &id)?;
        Some((id, version))
    }

    fn widget_messages(&self, session: &str) -> Vec<WidgetMessage> {
        let Some(t) = self.transcripts.get(session) else { return Vec::new() };
        t.messages.iter().filter_map(|m| if let Message::Widget(w) = m { Some(w.clone()) } else { None }).collect()
    }

    fn fetch_program(&mut self, name: &str, version: &str) {
        let (name, version) = (name.to_owned(), version.to_owned());
        self.job(move |srv| {
            // Compiled on the job thread too: the window only takes the finished program.
            let result = srv.widget_program(&name, &version).map_err(|e| e.to_string()).and_then(|p| compile(&p)).map(Arc::new);
            Box::new(move |s: &mut App, _tx: &mut Tx| loaded(s, &name, &version, result)) as Apply
        });
    }

    fn post_message(&mut self, session: &str, message: &str, body: Value) {
        let (session, message) = (session.to_owned(), message.to_owned());
        // A fixture run or story has no server to take it: it is taken, as a server would.
        if !self.live() {
            self.later(Box::new(move |s: &mut App, _tx: &mut Tx| sent(s, &message, Ok(()))));
            return;
        }
        let (s2, m2) = (session.clone(), message.clone());
        self.widget_write(message, move |srv| srv.send_widget_message(&s2, &m2, body), |s, id, r| sent(s, id, r));
    }

    fn post_response(&mut self, session: &str, message: &str, body: Value, attempt: u32) {
        let (session, message) = (session.to_owned(), message.to_owned());
        // A fixture run or story has no server to take it: it is taken, as a server would.
        if !self.live() {
            self.later(Box::new(move |s: &mut App, _tx: &mut Tx| responded(s, &message, attempt, Ok(()))));
            return;
        }
        self.job(move |srv| {
            let result = srv.respond_widget(&session, &message, body).map_err(|e| match e.status() {
                Some(409) => RespondError::Conflict,
                _ => RespondError::Failed(e.to_string()),
            });
            Box::new(move |s: &mut App, _tx: &mut Tx| responded(s, &message, attempt, result)) as Apply
        });
    }
}

impl App {
    fn widget_write(&self, message: String, work: impl FnOnce(&opencode_kit::client::Server) -> Result<(), opencode_kit::wire_error::WireError> + Send + 'static, done: fn(&mut App, &str, Result<(), RespondError>)) {
        self.job(move |srv| {
            let result = work(srv).map_err(|e| match e.status() {
                Some(409) => RespondError::Conflict,
                _ => RespondError::Failed(e.to_string()),
            });
            Box::new(move |s: &mut App, _tx: &mut Tx| done(s, &message, result)) as Apply
        });
    }
}

/// Kernel feature registration for agent widgets.
pub struct WidgetsFeature;

impl Feature<App> for WidgetsFeature {
    fn id(&self) -> &'static str {
        "feature.widgets"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&WIDGETS);
        // After the stream applied this frame's events, so a posted or answered widget opens or
        // closes the frame it arrives, before the timeline derives its rows.
        builder.schedule(crate::tick::stage::INGEST_STREAM, "widgets.sync", |s, _, _| sync(s));
        builder.schedule(crate::tick::stage::INGEST_STREAM, "widgets.poll", |s, now, tx| poll(s, now, tx)).after("widgets.sync");
        builder.add_rest("widgets.rest", |s, keep| rest(s, keep));
    }
}
