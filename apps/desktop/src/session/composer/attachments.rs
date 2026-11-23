//! Desktop host adapter for `opencode-feature-composer-input::attachments`
//! (`features/composer-input`) and kernel registration for `ComposerInputFeature`.

pub use opencode_feature_composer_input::attachments::*;

use std::path::Path;

use opencode_kernel::{CommandEntry, EscapeLayer, Feature, KernelBuilder, OverlayRegistration};
use opencode_kit::client::{PathAttachment, Server};
use rune::{El, Tx};
use session_view::palette::Pal;

use crate::keymap::Bind;
use crate::pickers::{Kind, PickerHost as _};
use crate::{App, Source};

/// `App`-specialized upload toast wrapper.
pub mod toast {
    use rune::El;
    use session_view::palette::Pal;

    use crate::App;

    pub fn card(s: &App, p: &Pal, content_shows: bool) -> El {
        opencode_feature_composer_input::attachments::toast::card::<App>(s, p, content_shows)
    }
}

impl AttachmentsHost for App {
    fn attach_ref(&self) -> AttachRef<'_> {
        self.composer_ext.attach.as_ref_parts()
    }

    fn attach_mut(&mut self) -> AttachMut<'_> {
        self.composer_ext.attach.as_mut_parts()
    }

    fn composer_directory(&self) -> String {
        App::composer_directory(self)
    }

    fn draft_key(&self) -> Option<String> {
        crate::restore::draft_key(&self.route)
    }

    fn live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn attachment_input(&self) -> session_view::pickers::AttachmentInput {
        App::attachment_input(self)
    }

    fn on_draft_edited(&mut self) {
        self.suggest_edited();
    }

    fn on_uploads_shown(&mut self) {
        crate::status_toast::uploads_shown(self);
    }

    fn land_upload(&mut self, draft: Option<&str>, attachment: PathAttachment) {
        App::land_upload(self, draft, attachment);
    }

    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        crate::auto_focus_composer(tx)
    }

    fn focus_composer(&mut self) {
        self.composer.focused = true;
    }
}

impl App {
    pub fn attach_path(&mut self, path: &Path) {
        opencode_feature_composer_input::attachments::attach_path(self, path);
    }

    pub fn poll_uploads(&mut self, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::attachments::poll_uploads(self, tx)
    }

    pub fn cancel_upload(&mut self, id: &str) {
        opencode_feature_composer_input::attachments::cancel_upload(self, id);
    }

    pub fn pick_attachment(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::attachments::pick_attachment(self, tx);
    }
}

pub(crate) fn remove_button(p: &Pal, name: &str, w: f32, on: impl Fn(&mut App) + 'static) -> El {
    session_view::composer::attachments::remove_button(p, name, w, on)
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    ("reply-image", |s| {
        crate::open_scene(s);
        s.finish_stream();
        if let crate::Route::Session { id, .. } = (*s.route).clone()
            && let Some(t) = s.transcripts.get_mut(&id)
        {
            opencode_feature_composer_input::attachments::add_reply_image(t, &s.composer_ext.attach.store);
        }
    }),
]);

fn agent_shown(s: &App) -> bool {
    s.has_composer() && s.picker_show_agent()
}

/// Kernel feature registration for composer attachments, dropzone, slash commands, and suggestions.
pub struct ComposerInputFeature;

impl Feature<App> for ComposerInputFeature {
    fn id(&self) -> &'static str {
        "feature.composer_input"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // The composer's commands, after the session's, as Desktop registers them (`composer/commands.tsx:111-161`,
        // mounted after `use-session-commands.tsx`): a palette query lists them in that order.
        builder.add_command(
            CommandEntry::<App>::from_key(
                "model",
                opencode_i18n::key!("command.model.choose"),
                crate::commands::order::MODEL,
                |s, _| s.open_picker(Kind::Model),
            )
            .describe(opencode_i18n::key!("command.model.choose.description"))
            .slash("model")
            .bind_id(Bind::Model.id())
            .when(App::has_composer),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "variant",
                opencode_i18n::key!("command.model.variant.cycle"),
                crate::commands::order::VARIANT,
                |s, tx| s.cycle_picker(Kind::Effort, false, tx),
            )
            .describe(opencode_i18n::key!("command.model.variant.cycle.description"))
            .bind_id(Bind::Variant.id())
            .when(|s| s.has_composer() && s.has_effort()),
        );
        // Only for a model that has other speeds, so nothing about speeds shows for the rest
        // (`composer/commands.tsx`, opencode#65749). It has no key until Settings gives it one.
        builder.add_command(
            CommandEntry::<App>::from_key(
                session_view::pickers::speed::CYCLE_COMMAND,
                opencode_i18n::key!("command.model.speed.cycle"),
                crate::commands::order::SPEED,
                |s, tx| s.cycle_picker(Kind::Speed, false, tx),
            )
            .describe(opencode_i18n::key!("command.model.speed.cycle.description"))
            .slash("speed")
            .section("models")
            .when(|s| s.has_composer() && s.has_speeds()),
        );
        // Offered while the agent shows, as Desktop's `disabled: !local.agent.visible()` (`composer/commands.tsx:145-161`).
        builder.add_command(
            CommandEntry::<App>::from_key(
                "agent",
                opencode_i18n::key!("command.agent.cycle"),
                crate::commands::order::AGENT,
                |s, tx| s.cycle_picker(Kind::Agent, false, tx),
            )
            .describe(opencode_i18n::key!("command.agent.cycle.description"))
            .slash("agent")
            .bind_id(Bind::Agent.id())
            .when(agent_shown),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "agent-back",
                opencode_i18n::key!("command.agent.cycle.reverse"),
                crate::commands::order::AGENT_BACK,
                |s, tx| s.cycle_picker(Kind::Agent, true, tx),
            )
            .describe(opencode_i18n::key!("command.agent.cycle.reverse.description"))
            .bind_id(Bind::AgentBack.id())
            .when(agent_shown),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "focus",
                opencode_i18n::key!("command.input.focus"),
                crate::commands::order::FOCUS,
                |s, tx| {
                    s.focus_composer(tx);
                },
            )
            .bind_id(Bind::FocusInput.id())
            .when(App::has_composer),
        );
        // The composer's `file.attach` and `prompt.stop` (`composer/model.ts`, opencode#64394); their
        // keys stay with the composer's own handlers (`add_menu`, `stop::key`).
        builder.add_command(
            CommandEntry::<App>::from_key("file.attach", opencode_i18n::key!("prompt.action.attachFile"), crate::commands::order::FILE_ATTACH, |s, tx| s.pick_attachment(tx))
                .bind_id(Bind::Attach.id())
                .when(|s| s.has_composer() && !s.kernel.slots.get(&crate::add_menu::ADD_MENU).shell()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key("prompt.stop", opencode_i18n::key!("command.prompt.stop"), crate::commands::order::PROMPT_STOP, |s, tx| crate::stop::press(s, tx))
                .bind_id(Bind::Stop.id())
                .when(|s| matches!(&*s.route, crate::Route::Session { id, .. } if s.busy(id))),
        );
        // The composer's modes (`composer/model.ts:579-599`).
        builder.add_command(
            CommandEntry::<App>::from_key(
                "prompt.mode.toggle",
                opencode_i18n::key!("command.prompt.mode.toggle"),
                crate::commands::order::PROMPT_MODE_TOGGLE,
                |s, tx| {
                    tx.animated();
                    if s.kernel.slots.get(&crate::add_menu::ADD_MENU).shell() { s.exit_shell() } else { s.enter_shell(tx) }
                },
            )
            .bind_id(Bind::PromptModeToggle.id())
            .when(App::has_composer),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "prompt.mode.shell",
                opencode_i18n::key!("command.prompt.mode.shell"),
                crate::commands::order::PROMPT_MODE_SHELL,
                |s, tx| {
                    tx.instant();
                    s.enter_shell(tx);
                },
            )
            .when(|s| s.has_composer() && !s.kernel.slots.get(&crate::add_menu::ADD_MENU).shell()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "prompt.mode.normal",
                opencode_i18n::key!("command.prompt.mode.normal"),
                crate::commands::order::PROMPT_MODE_NORMAL,
                |s, tx| {
                    tx.instant();
                    s.exit_shell();
                },
            )
            .when(|s| s.has_composer() && s.kernel.slots.get(&crate::add_menu::ADD_MENU).shell()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key(
                "context.addSelection",
                opencode_i18n::key!("command.context.addSelection"),
                crate::commands::order::CONTEXT_ADD_SELECTION,
                |s, tx| s.add_selection(tx),
            )
            .describe(opencode_i18n::key!("command.context.addSelection.description"))
            .bind_id(Bind::AddSelection.id())
            .when(App::can_add_selection),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new("overlay.slash")
                .with_render(crate::overlays::layer::SURFACE_POPOVER, |s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    crate::slash::commands_view(s, p, cx).into_iter().collect()
                })
                .above("overlay.mentions"),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new("overlay.history_suggestions")
                .with_render(crate::overlays::layer::SURFACE_POPOVER, |s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    crate::suggestions::history::view(s, p, cx).into_iter().collect()
                })
                .above("overlay.slash"),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new("overlay.add_menu")
                .with_render(crate::overlays::layer::SURFACE_POPOVER, |s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    crate::add_menu::menu(s, p).into_iter().flatten().collect()
                })
                .above("overlay.history_suggestions"),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new("overlay.attachments.viewer")
                .with_render(crate::overlays::layer::MEDIA_LIGHTBOX, |s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    viewer(s, p, cx)
                }),
        );
        // Pressing Escape during a file drag clears the dropzone (`attachments.ts:212`).
        builder.register_escape_layer(
            EscapeLayer::<App>::new(
                "escape.dropzone",
                crate::overlays::escape_band::CHROME,
                |s| s.composer_ext.attach.drop.shown(),
                |s, _, tx| crate::dropzone::hover(s, false, tx),
            )
            .after("escape.quote"),
        );
        builder
            .register_slot(&crate::prompt_history::PROMPTS)
            .register_slot(&crate::suggestions::SUGGEST)
            .register_slot(&crate::add_menu::ADD_MENU);
        builder.add_setup("composer_input.attach_waker", |s, waker| s.composer_ext.attach.set_waker(waker));
        builder.add_setup("composer_input.slash_waker", |s, waker| s.kernel.slots.get_mut(&crate::add_menu::ADD_MENU).slash.set_waker(waker));
        builder.schedule(
            crate::tick::stage::INGEST_SESSION_PRE,
            "composer_input.dropzone",
            |s, _, tx| crate::dropzone::poll(s, tx),
        );
        // Known: shares `suggestions::SUGGEST` with `mentions.poll`.
        builder
            .schedule(
                crate::tick::stage::DERIVE_COMPOSER,
                "composer_input.commands",
                |s, _, tx| s.poll_commands(tx),
            )
            .after("mentions.poll");
        // Known: shares `suggestions::SUGGEST` with `composer_input.commands` (and reads
        // `search.focused` finalized in `DERIVE_SEARCH`).
        builder
            .schedule(
                crate::tick::stage::DERIVE_COMPOSER,
                "composer_input.suggestions",
                |s, _, tx| s.poll_suggestions(tx),
            )
            .after("composer_input.commands");
        // Precaution: shares `App::toast` with `session_menu.poll_delete`.
        builder
            .schedule(
                crate::tick::stage::DERIVE_COMPOSER,
                "composer_input.uploads",
                |s, _, tx| s.poll_uploads(tx),
            )
            .after("session_menu.poll_delete");
        builder.schedule(crate::tick::stage::REACT_PANELS, "composer_input.attach", |s, _, _| {
            if s.composer_ext.attach.poll() {
                rune::Tracked::touch(&s.composer_ext.attach);
                true
            } else {
                false
            }
        });
    }
}
