//! Desktop host adapter for `opencode-feature-history::message_nav` (`features/history`) and
//! kernel feature registration for `HistoryFeature`.

pub use opencode_feature_history::message_nav::*;

use std::rc::Rc;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration, Slot};
use opencode_kit::model::Transcript;
use rune::{KeyEvent, LiveRegion, Tx};
use session_view::timeline::Projection;

use crate::keymap::Bind;
use crate::pickers::PickerHost as _;
use crate::{App, Route, Source};

pub(crate) static MESSAGE_NAV: Slot<MessageNav> = Slot::new();
pub(crate) static JUMP: Slot<JumpState> = Slot::new();

impl MessageNavHost for App {
    fn message_nav(&self) -> &MessageNav {
        self.kernel.slots.get(&MESSAGE_NAV)
    }

    fn message_nav_mut(&mut self) -> &mut MessageNav {
        self.kernel.slots.get_mut(&MESSAGE_NAV)
    }

    fn jump(&self) -> &JumpState {
        self.kernel.slots.get(&JUMP)
    }

    fn jump_mut(&mut self) -> &mut JumpState {
        self.kernel.slots.get_mut(&JUMP)
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
        self.kernel.slots.get_mut(&crate::speech::SPEECH).say(words, region);
    }

    fn is_narrow(&self) -> bool {
        self.narrow()
    }

    fn before_jump_open(&mut self) -> bool {
        // Replacing an open or dispatching palette inherits its focus-return origin
        // (`packages/ui/src/context/dialog-machine.ts:5-8,166-181`,
        // `packages/app/src/session/commands/use-session-commands.tsx:286-292`).
        let restore = self.composer.focused || self.kernel.slots.get(&crate::switcher::SWITCHER).restore_composer();
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
                "session.undo",
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
                "session.redo",
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
                "session.fork",
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
        builder.register_slot(&MESSAGE_NAV);
        builder.register_slot(&JUMP);
        builder.register_slot(&crate::fork::FORK);
        builder.register_slot(&crate::rewind::REWIND);
        builder.register_overlay(
            OverlayRegistration::<App>::new(crate::fork::FORK_OVERLAY_ID)
                .with_render(crate::overlays::layer::DIALOG, |s, cx| {
                    if s.kernel.slots.get(&crate::fork::FORK).is_open() {
                        let p = crate::palette::pal(s.dark(cx));
                        crate::fork::view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                // Visually in DIALOG (above `overlay.diagram`), kept in `key_band::MID` after
                // `glitch`/`demo`/`overlay.diagram` to preserve existing key precedence.
                .with_key_in(crate::overlays::key_band::MID, |s, ev, tx| s.fork_key(ev, tx))
                .after_key("overlay.diagram")
                .with_dismiss(|s, _| crate::fork::close(s, true)),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new(JUMP_OVERLAY_ID)
                .with_render(crate::overlays::layer::DIALOG, |s, cx| {
                    if s.kernel.slots.get(&JUMP).open() {
                        let p = crate::palette::pal(s.dark(cx));
                        jump::view(s, p, cx).into()
                    } else {
                        Vec::new()
                    }
                })
                .above(crate::fork::FORK_OVERLAY_ID)
                // Visually above `history.fork`, kept in `key_band::MID` after `history.fork` to
                // preserve existing key order (mutually exclusive in practice).
                .with_key_in(crate::overlays::key_band::MID, |s, ev, tx| s.jump_key(ev, tx))
                .after_key(crate::fork::FORK_OVERLAY_ID)
                .with_dismiss(|s, _| jump::close(s, true)),
        );
        builder.schedule(
            crate::tick::stage::INGEST_SESSION_PRE,
            "history.message_nav",
            |s, _, tx| poll(s, tx),
        );
        // Known: `tick.stream` delivers revert settlements/events before timeout checks.
        builder
            .schedule(crate::tick::stage::INGEST_STREAM, "history.rewind", |s, _, tx| {
                crate::rewind::revert_timeouts(s, tx)
            })
            .after("tick.stream");
        // Known: runs after `tick.stream` for fork settlements; precaution: shares `sessions` and
        // `App::toast` with `history.rewind`.
        builder
            .schedule(crate::tick::stage::INGEST_STREAM, "history.fork", |s, _, tx| {
                crate::fork::poll(s, tx)
            })
            .after("history.rewind");
        // Known: reads `prompt_ids` (depends on `session.revert` from `history.rewind`) and
        // `displayed_session_id` (which `history.fork` can update).
        builder
            .schedule(crate::tick::stage::INGEST_STREAM, "history.jump", |s, _, tx| {
                jump::poll(s, tx)
            })
            .after("history.fork");
    }
}
