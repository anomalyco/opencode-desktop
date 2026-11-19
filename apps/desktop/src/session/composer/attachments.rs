//! Desktop host adapter for `opencode-feature-composer-input::attachments`
//! (`features/composer-input`) and kernel registration for `ComposerInputFeature`.

pub use opencode_feature_composer_input::attachments::*;

use std::path::Path;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, OverlayRegistration, Phase};
use opencode_kit::client::{PathAttachment, Server};
use rune::{El, Tx};
use session_view::palette::Pal;

use crate::keymap::Bind;
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

/// Kernel feature registration for composer attachments, dropzone, slash commands, and suggestions.
pub struct ComposerInputFeature;

impl Feature<App> for ComposerInputFeature {
    fn id(&self) -> &'static str {
        "feature.composer_input"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        // The composer's modes (`composer/model.ts:579-599`).
        builder.add_command(
            CommandEntry::<App>::from_key("prompt.mode.toggle", opencode_i18n::key!("command.prompt.mode.toggle"), |s, tx| {
                tx.animated();
                if s.composer_ext.add.shell() { s.exit_shell() } else { s.enter_shell(tx) }
            })
            .bind_id(Bind::PromptModeToggle.id())
            .order(470)
            .when(App::has_composer),
        );
        builder.add_command(
            CommandEntry::<App>::from_key("prompt.mode.shell", opencode_i18n::key!("command.prompt.mode.shell"), |s, tx| {
                tx.instant();
                s.enter_shell(tx);
            })
            .order(480)
            .when(|s| s.has_composer() && !s.composer_ext.add.shell()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key("prompt.mode.normal", opencode_i18n::key!("command.prompt.mode.normal"), |s, tx| {
                tx.instant();
                s.exit_shell();
            })
            .order(490)
            .when(|s| s.has_composer() && s.composer_ext.add.shell()),
        );
        builder.add_command(
            CommandEntry::<App>::from_key("context.addSelection", opencode_i18n::key!("command.context.addSelection"), |s, tx| s.add_selection(tx))
                .describe(opencode_i18n::key!("command.context.addSelection.description"))
                .bind_id(Bind::AddSelection.id())
                .when(App::can_add_selection),
        );
        builder.register_overlay(
            OverlayRegistration::<App>::new("overlay.attachments.viewer")
                .with_z(160)
                .with_render(|s, cx| {
                    let p = crate::palette::pal(s.dark(cx));
                    viewer(s, p, cx)
                }),
        );
        builder.add_setup("composer_input.attach_waker", |s, waker| s.composer_ext.attach.set_waker(waker));
        builder.add_setup("composer_input.slash_waker", |s, waker| s.composer_ext.add.slash.set_waker(waker));
        builder.add_system(Phase::Derive, "composer_input.dropzone", |s, _, tx| crate::dropzone::poll(s, tx));
        builder.add_system(Phase::Derive, "composer_input.uploads", |s, _, tx| s.poll_uploads(tx));
        builder.add_system(Phase::Derive, "composer_input.commands", |s, _, tx| s.poll_commands(tx));
        builder.add_system(Phase::Derive, "composer_input.suggestions", |s, _, tx| s.poll_suggestions(tx));
    }
}
