//! Desktop host adapter for `opencode-feature-history::message_nav` (`features/history`) and
//! kernel feature registration for `HistoryFeature`.

pub use opencode_feature_history::message_nav::*;

use std::rc::Rc;

use opencode_kernel::{Feature, KernelBuilder, OverlayEntry, OverlayEvent, OverlayKind, Phase};
use opencode_kit::model::Transcript;
use rune::{KeyEvent, LiveRegion, Tx};
use session_view::timeline::Projection;

use crate::pickers::PickerHost as _;
use crate::{App, Route, Source};

impl MessageNavHost for App {
    fn message_nav(&self) -> &MessageNav {
        &self.message_nav
    }

    fn message_nav_mut(&mut self) -> &mut MessageNav {
        &mut self.message_nav
    }

    fn jump(&self) -> &JumpState {
        &self.jump
    }

    fn jump_mut(&mut self) -> &mut JumpState {
        &mut self.jump
    }

    fn active_session_id(&self) -> Option<&str> {
        App::active_session_id(self)
    }

    fn displayed_session_id(&self) -> Option<&str> {
        let id = App::active_session_id(self)?;
        self.displayed_session(id)
    }

    fn is_session_open(&self, session: &str) -> bool {
        *self.route == Route::here(session.to_owned())
    }

    fn transcript(&self, session: &str) -> Option<&Transcript> {
        self.transcripts.get(session)
    }

    fn timeline_rows(&self, session: &str) -> Option<Rc<Projection>> {
        self.transcripts.get(session).map(|t| session_view::timeline::rows(self, t))
    }

    fn session_revert(&self, session: &str) -> Option<&str> {
        self.session(session).and_then(|i| i.revert.as_deref())
    }

    fn timeline_at_end(&self, session: &str) -> bool {
        self.ends.at_end(session)
    }

    fn more_above(&self, session: &str) -> bool {
        session_view::TimelineHost::more_above(self, session)
    }

    fn all_pages_loaded(&self, session: &str) -> bool {
        self.pages.first_read_done(session) && !self.pages.more_above(session)
    }

    fn read_toward_place(&mut self, session: &str, tx: &mut Tx) {
        App::read_toward_place(self, session, tx);
    }

    fn is_live(&self) -> bool {
        self.live()
    }

    fn spawn_history_load(&mut self, request: jump::HistoryRequest) -> bool {
        let Source::Live { server, .. } = &*self.source else { return false };
        let server = server.clone();
        let deliver = self.deliverer();
        rune::task::spawn("jump-history", move || {
            let reply = jump::fetch_history(&server, &request);
            deliver(Box::new(move |s: &mut App, tx: &mut Tx| {
                jump::apply_history_reply(s, reply, tx);
            }));
        })
        .is_ok()
    }

    fn announce(&mut self, words: String, region: LiveRegion) {
        self.kernel.slots.slot::<crate::speech::Speech>().say(words, region);
    }

    fn is_narrow(&self) -> bool {
        self.narrow()
    }

    fn zoom(&self) -> f32 {
        *self.zoom
    }

    fn spellcheck(&self) -> bool {
        self.settings.spellcheck
    }

    fn clock(&self) -> f64 {
        *self.clock
    }

    fn now_ms(&self) -> i64 {
        *self.now_ms
    }

    fn before_jump_open(&mut self) -> bool {
        let restore = self.composer.focused || (!self.switcher.open && self.switcher.restore_composer());
        self.close_fork_dialog(false);
        self.close_picker(false);
        self.close_switcher(false);
        self.close_project_switcher(false);
        self.close_message_menu();
        self.composer.focused = false;
        restore
    }

    fn restore_composer_focus(&mut self, focused: bool) {
        self.composer.focused = focused;
    }

    fn handle_shortcut(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        self.shortcut(ev, tx)
    }

    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>) {
        let mut entry = OverlayEntry::new(id, OverlayKind::Dialog);
        if let Some(focus) = return_focus {
            entry = entry.with_return_focus(focus);
        }
        self.kernel.step_overlay(OverlayEvent::Push(entry));
    }

    fn on_overlay_closed(&mut self, id: &'static str) {
        self.kernel.step_overlay(OverlayEvent::Close(id));
    }
}

impl App {
    pub fn navigate_message(&mut self, offset: i32, tx: &mut Tx) {
        navigate(self, offset, tx);
    }

    pub fn open_jump_dialog(&mut self, tx: &mut Tx) -> bool {
        jump::open(self, tx)
    }

    pub fn close_jump_dialog(&mut self, restore_focus: bool) {
        jump::close(self, restore_focus);
    }

    pub fn jump_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        jump::key(self, ev, tx)
    }
}

/// Kernel feature registration for session timeline message navigation, Jump to message, Rewind, and Fork.
pub struct HistoryFeature;

impl Feature<App> for HistoryFeature {
    fn id(&self) -> &'static str {
        "feature.history"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_overlay_dismiss(JUMP_OVERLAY_ID, |s, _| jump::close(s, true));
        builder.register_overlay_dismiss(crate::fork::FORK_OVERLAY_ID, |s, _| crate::fork::close(s, true));
        builder.add_system(Phase::Derive, "history.message_nav", |s, _, tx| poll(s, tx));
        builder.add_system(Phase::Derive, "history.rewind", |s, _, tx| crate::rewind::revert_timeouts(s, tx));
        builder.add_system(Phase::Derive, "history.fork", |s, _, tx| crate::fork::poll(s, tx));
        builder.add_system(Phase::Derive, "history.jump", |s, _, tx| jump::poll(s, tx));
    }
}
