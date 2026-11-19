//! Desktop host adapter for `opencode-feature-history::message_nav` (`features/history`) and
//! kernel feature registration for `HistoryFeature`.

pub use opencode_feature_history::message_nav::*;

use std::rc::Rc;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration, Phase};
use opencode_kit::model::Transcript;
use rune::{KeyEvent, LiveRegion, Tx};
use session_view::timeline::Projection;

use crate::keymap::Bind;
use crate::pickers::PickerHost as _;
use crate::{App, Route, Source};

impl MessageNavHost for App {
    fn message_nav(&self) -> &MessageNav {
        &self.timeline_fx.message_nav
    }

    fn message_nav_mut(&mut self) -> &mut MessageNav {
        &mut self.timeline_fx.message_nav
    }

    fn jump(&self) -> &JumpState {
        &self.dialogs.jump
    }

    fn jump_mut(&mut self) -> &mut JumpState {
        &mut self.dialogs.jump
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
        self.timeline_fx.ends.at_end(session)
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

    fn before_jump_open(&mut self) -> bool {
        let restore = self.composer.focused || self.dialogs.switcher.restore_composer();
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
        builder.add_command(
            CommandEntry::<App>::from_key(
                "undo",
                opencode_i18n::key!("command.session.undo"),
                crate::commands::order::UNDO,
                |s, tx| {
                    s.undo_last(tx);
                },
            )
            .describe(opencode_i18n::key!("command.session.undo.description"))
            .slash("undo")
            .section("session")
            .when(App::can_undo_session),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "redo",
                opencode_i18n::key!("command.session.redo"),
                crate::commands::order::REDO,
                |s, tx| {
                    s.redo_revert(tx);
                },
            )
            .describe(opencode_i18n::key!("command.session.redo.description"))
            .slash("redo")
            .section("session")
            .when(App::can_redo_session),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "fork",
                opencode_i18n::key!("command.session.fork"),
                crate::commands::order::FORK,
                |s, tx| {
                    s.open_fork_dialog(tx);
                },
            )
            .describe(opencode_i18n::key!("command.session.fork.description"))
            .slash("fork")
            .section("session")
            .when(App::can_fork_session),
        );
        // `use-session-commands.tsx:674-681`, before the message keys as Desktop registers it.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "session.jumpToMessage",
                opencode_i18n::key!("dialog.jump.title"),
                crate::commands::order::SESSION_JUMP_TO_MESSAGE,
                |s, tx| {
                    s.open_jump_dialog(tx);
                },
            )
            .describe(opencode_i18n::key!("command.session.jumpToMessage.description"))
            .bind_id(Bind::JumpToMessage.id())
            .when(|s| s.active_session_id().is_some()),
        );
        // `use-session-commands.tsx:636-653`: offered over any open session.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "message.previous",
                opencode_i18n::key!("command.message.previous"),
                crate::commands::order::MESSAGE_PREVIOUS,
                |s, tx| s.navigate_message(-1, tx),
            )
            .describe(opencode_i18n::key!("command.message.previous.description"))
            .bind_id(Bind::MessagePrev.id())
            .when(|s| s.active_session_id().is_some()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "message.next",
                opencode_i18n::key!("command.message.next"),
                crate::commands::order::MESSAGE_NEXT,
                |s, tx| s.navigate_message(1, tx),
            )
            .describe(opencode_i18n::key!("command.message.next.description"))
            .bind_id(Bind::MessageNext.id())
            .when(|s| s.active_session_id().is_some()),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(crate::fork::FORK_OVERLAY_ID)
                .with_z(300)
                .with_key_priority(200)
                .with_render(|s, cx| {
                    if s.dialogs.fork.is_open() {
                        let p = crate::palette::pal(s.dark(cx));
                        crate::fork::view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                .with_key(|s, ev, tx| s.fork_key(ev, tx))
                .with_dismiss(|s, _| crate::fork::close(s, true)),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(JUMP_OVERLAY_ID)
                .with_z(310)
                .with_key_priority(210)
                .with_render(|s, cx| {
                    if s.dialogs.jump.open() {
                        let p = crate::palette::pal(s.dark(cx));
                        jump::view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                .with_key(|s, ev, tx| s.jump_key(ev, tx))
                .with_dismiss(|s, _| jump::close(s, true)),
        );
        builder.add_system(Phase::Derive, "history.message_nav", |s, _, tx| poll(s, tx));
        builder.add_system(Phase::Derive, "history.rewind", |s, _, tx| crate::rewind::revert_timeouts(s, tx));
        builder.add_system(Phase::Derive, "history.fork", |s, _, tx| crate::fork::poll(s, tx));
        builder.add_system(Phase::Derive, "history.jump", |s, _, tx| jump::poll(s, tx));
    }
}
