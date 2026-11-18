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
    b.with_feature(&crate::dictation::DictationFeature);
    b.with_feature(&crate::tips::TipsFeature);
    b.with_feature(&crate::usage::UsageFeature);
    b.add_surface(
        SurfaceDescriptor::new("terminal", SurfaceKind::Terminal, "Terminal")
            .with_multi_instance(true),
    );
    b.add_surface(
        SurfaceDescriptor::new("browser", SurfaceKind::Browser, "Browser")
            .with_multi_instance(true),
    );
    b.build()
}
