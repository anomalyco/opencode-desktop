//! Desktop host adapter for `opencode-feature-widgets` (`features/widgets`): agent widgets in
//! the timeline, their states and template programs read through the server connection's job
//! thread from the widgets plugin (RPC `opencode.widget` in the session's location), and their
//! answers posted there.

pub use opencode_feature_widgets::*;

use std::sync::Arc;

use opencode_kernel::{Feature, KernelBuilder, Slot};
use opencode_kit::client::WidgetError;
use opencode_kit::model::{WidgetCall, widget_calls, widget_key};
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

    fn widget_calls(&self, session: &str) -> Vec<WidgetCall> {
        self.transcripts.get(session).map(|t| widget_calls(&t.messages)).unwrap_or_default()
    }

    fn fetch_states(&mut self, session: &str) {
        let session = session.to_owned();
        // A fixture run or story has no server: its states are seeded.
        if !self.live() {
            return;
        }
        let directory = self.widget_directory(&session);
        self.job(move |srv| {
            // Not loaded yet in the location (`rpc.unavailable`): read again on `plugin.updated`.
            let result = srv.widget_states(&directory, &session).map_err(|e| e.to_string());
            Box::new(move |s: &mut App, _tx: &mut Tx| states(s, &session, result)) as Apply
        });
    }

    fn fetch_program(&mut self, session: &str, name: &str, version: &str) {
        let (name, version) = (name.to_owned(), version.to_owned());
        let directory = self.widget_directory(session);
        self.job(move |srv| {
            // Compiled on the job thread too: the window only takes the finished program.
            let result = srv.widget_program(&directory, &name, &version).map_err(|e| e.to_string()).and_then(|p| compile_as(&p.program, &p.mode, grant(&p.capabilities), offset(&p.program, &p.source))).map(Arc::new);
            Box::new(move |s: &mut App, _tx: &mut Tx| loaded(s, &name, &version, result)) as Apply
        });
    }

    fn post_report(&mut self, session: &str, message: &str, call: &str, error: Value) {
        if !self.live() {
            return;
        }
        let (directory, session, message, call) = (self.widget_directory(session), session.to_owned(), message.to_owned(), call.to_owned());
        self.job(move |srv| {
            // Best effort: the error already shows here; the agent hears of it when this goes.
            if let Err(e) = srv.widget_report(&directory, &session, &message, &call, error) {
                eprintln!("widget report failed: {e}");
            }
            Box::new(|_: &mut App, _: &mut Tx| {}) as Apply
        });
    }

    fn widget_notify(&self) -> Arc<dyn Fn() + Send + Sync> {
        // A run without a window (headless) has nothing to wake: its clock runs on its own.
        (*self.waker).clone().unwrap_or_else(|| Arc::new(|| {}))
    }

    fn post_message(&mut self, session: &str, message: &str, call: &str, value: Value, wake: bool) {
        let id = widget_key(message, call);
        // A fixture run or story has no server to take it: it is taken, as a server would.
        if !self.live() {
            self.later(Box::new(move |s: &mut App, _tx: &mut Tx| sent(s, &id, Ok(()))));
            return;
        }
        let (directory, session, message, call) = (self.widget_directory(session), session.to_owned(), message.to_owned(), call.to_owned());
        self.job(move |srv| {
            let result = srv.widget_send(&directory, &session, &message, &call, value, wake).map_err(respond_error);
            Box::new(move |s: &mut App, _tx: &mut Tx| sent(s, &id, result)) as Apply
        });
    }

    fn post_response(&mut self, session: &str, message: &str, call: &str, response: Value, attempt: u32) {
        let id = widget_key(message, call);
        // A fixture run or story has no server to take it: it is taken, as a server would.
        if !self.live() {
            self.later(Box::new(move |s: &mut App, _tx: &mut Tx| responded(s, &id, attempt, Ok(()))));
            return;
        }
        let (directory, session, message, call) = (self.widget_directory(session), session.to_owned(), message.to_owned(), call.to_owned());
        self.job(move |srv| {
            let result = srv.widget_respond(&directory, &session, &message, &call, response).map_err(respond_error);
            Box::new(move |s: &mut App, _tx: &mut Tx| responded(s, &id, attempt, result)) as Apply
        });
    }
}

/// `closed`: answered already, here or on another client.
fn respond_error(e: WidgetError) -> RespondError {
    match e {
        WidgetError::Closed => RespondError::Conflict,
        e => RespondError::Failed(e.to_string()),
    }
}

impl App {
    /// The location the widgets plugin answers `session`'s calls in: its directory.
    fn widget_directory(&self, session: &str) -> String {
        self.session(session).map(|i| i.directory.clone()).unwrap_or_default()
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
