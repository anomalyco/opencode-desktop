//! Shell kernel assembly: wires extracted feature crates and native surface descriptors into
//! `opencode_kernel::Kernel<App>`.
//!
//! New Rust features register here via `Feature::register` instead of adding fields to `App` or
//! editing central dispatch chains in `main.rs`, `tick.rs`, `keys.rs`, or `view.rs`.

use opencode_kernel::{Kernel, KernelBuilder, SurfaceDescriptor, SurfaceKind};

use crate::App;

/// Builds a window's kernel with all decoupled features and core surfaces registered.
pub fn build() -> Kernel<App> {
    let mut b = KernelBuilder::new();
    b.with_feature(&crate::surfaces::egg::EggFeature);
    #[cfg(feature = "office")]
    b.with_feature(&crate::surfaces::office::OfficeFeature);
    b.with_feature(&crate::surfaces::updater::UpdaterFeature);
    b.with_feature(&crate::surfaces::code_view::CodeViewFeature);
    b.with_feature(&crate::line_comments::AnnotationsFeature);
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
    b.with_feature(&crate::review::ReviewFeature);
    b.with_feature(&crate::session_menu::SessionMenuFeature);
    b.with_feature(&crate::settings::SettingsFeature);
    b.with_feature(&crate::subagents::SubagentsFeature);
    b.with_feature(&crate::switcher::SwitcherFeature);
    b.with_feature(&crate::surfaces::terminal::TerminalSurfaceFeature);
    b.with_feature(&crate::tips::TipsFeature);
    b.with_feature(&crate::usage::UsageFeature);
    b.add_surface(
        SurfaceDescriptor::new("browser", SurfaceKind::Browser, "Browser")
            .with_multi_instance(true),
    );
    b.build()
}
