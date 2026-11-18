//! Shell kernel assembly: wires extracted feature crates and native surface descriptors into
//! `opencode_kernel::Kernel<App>`.
//!
//! New Rust features register here via `Feature::register` instead of adding fields to `App` or
//! editing central dispatch chains in `main.rs`, `tick.rs`, `keys.rs`, or `view.rs`.

use opencode_kernel::{
    AppEnv, AppFeedback, AppOverlays, Kernel, KernelBuilder, OverlayEntry, OverlayEvent,
    OverlayKind, OverlayRegistration, SurfaceDescriptor, SurfaceKind,
};

use crate::{App, Route};

impl AppEnv for App {
    fn clock(&self) -> f64 {
        *self.clock
    }

    fn now_ms(&self) -> i64 {
        *self.now_ms
    }

    fn zoom(&self) -> f32 {
        *self.zoom
    }

    fn spellcheck(&self) -> bool {
        self.settings.spellcheck
    }

    fn reduced_motion(&self) -> bool {
        *self.reduced_motion
    }

    fn is_live(&self) -> bool {
        self.live()
    }

    fn persists(&self) -> bool {
        App::persists(self)
    }

    fn is_headless(&self) -> bool {
        crate::headless()
    }

    fn waker(&self) -> Option<rune::Waker> {
        (*self.waker).clone()
    }

    fn active_session_id(&self) -> Option<&str> {
        match &*self.route {
            Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        }
    }
}

impl AppFeedback for App {
    fn toast_say(&mut self, title: String, description: String) {
        crate::status_toast::say(self, title, description);
    }

    fn toast_succeed(&mut self, title: String, description: String) {
        crate::status_toast::succeed(self, title, description);
    }

    fn toast_fail(&mut self, title: String, description: String) {
        crate::status_toast::fail(self, title, description);
    }
}

impl AppOverlays for App {
    fn on_overlay_opened(&mut self, id: &'static str, return_focus: Option<String>) {
        let mut entry = OverlayEntry::new(id, OverlayKind::Dialog);
        if let Some(focus) = return_focus {
            entry = entry.with_return_focus(focus);
        }
        let _ = self.kernel.step_overlay(OverlayEvent::Push(entry));
    }

    fn on_overlay_closed(&mut self, id: &'static str) {
        let _ = self.kernel.step_overlay(OverlayEvent::Close(id));
    }
}

/// Builds a window's kernel with all decoupled features and core surfaces registered.
pub fn build() -> Kernel<App> {
    let mut b = KernelBuilder::new();
    b.with_feature(&crate::surfaces::egg::EggFeature);
    #[cfg(feature = "office")]
    b.with_feature(&crate::surfaces::office::OfficeFeature);
    b.with_feature(&crate::surfaces::updater::UpdaterFeature);
    b.with_feature(&crate::surfaces::code_view::CodeViewFeature);
    b.with_feature(&crate::line_comments::AnnotationsFeature);
    b.with_feature(&crate::attachments::ComposerInputFeature);
    b.with_feature(&crate::connect::ConnectFeature);
    b.with_feature(&crate::dictation::DictationFeature);
    b.with_feature(&crate::files::FilesFeature);
    b.with_feature(&crate::find::FindFeature);
    b.with_feature(&crate::message_nav::HistoryFeature);
    b.with_feature(&crate::queue::InboxFeature);
    b.with_feature(&crate::mcp::McpFeature);
    b.with_feature(&crate::mentions::MentionsFeature);
    b.with_feature(&crate::narrow::NarrowFeature);
    b.with_feature(&crate::notifications::NotificationsFeature);
    b.with_feature(&crate::open_in::OpenInFeature);
    b.with_feature(&crate::panels::PanelsFeature);
    b.with_feature(&crate::projects::ProjectsFeature);
    b.with_feature(&crate::restore::RestoreFeature);
    b.with_feature(&crate::review::ReviewFeature);
    b.with_feature(&crate::session_menu::SessionMenuFeature);
    b.with_feature(&crate::settings::SettingsFeature);
    b.with_feature(&crate::subagents::SubagentsFeature);
    b.with_feature(&crate::switcher::SwitcherFeature);
    b.with_feature(&crate::surfaces::terminal::TerminalSurfaceFeature);
    b.with_feature(&crate::todos::TimelineExtrasFeature);
    b.with_feature(&crate::tips::TipsFeature);
    b.with_feature(&crate::usage::UsageFeature);
    b.with_feature(&crate::new_worktree::WorkspaceToolsFeature);
    b.with_feature(&crate::plugin_host::PluginHostFeature);
    b.register_overlay(
        OverlayRegistration::<App>::new("overlay.diagram")
            .with_z(170)
            .with_key_priority(195)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::diagram::view(s, p, cx)
            })
            .with_key(crate::diagram::key),
    );
    b.register_overlay(
        OverlayRegistration::<App>::new("dialog.move_recovery")
            .with_z(470)
            .with_key_priority(150)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::view::move_recovery::view(s, p, cx)
            })
            .with_key(crate::view::move_recovery::key),
    );
    b.register_overlay(
        OverlayRegistration::<App>::new("dialog.servers")
            .with_z(490)
            .with_render(|s, cx| {
                let p = crate::palette::pal(s.dark(cx));
                crate::servers::dialog::view(s, p, cx)
            }),
    );
    b.add_surface(
        SurfaceDescriptor::new("browser", SurfaceKind::Browser, "Browser")
            .with_multi_instance(true),
    );
    b.build()
}
