//! Desktop host adapter for `opencode-feature-updater` (`features/updater`).
//!
//! Keeps `build_channel`, `build_version`, and `build_commit` (including the `.rdata`
//! `OPENCODE_RUNE_BUILD_STAMP` provenance record) inside `apps/desktop` so release
//! verification reads the desktop crate's own build environment.

pub use opencode_feature_updater::{
    BuildInfo, Updater, UpdaterConfig, UpdaterHost, check_async, install_async, poll, poll_owed,
    start_if_packaged, state_data,
};
#[cfg(not(target_arch = "wasm32"))]
pub use opencode_feature_updater::{finish_quit, installs_on_quit};

use opencode_kernel::{Feature, KernelBuilder, Phase};

use crate::App;

#[cfg(windows)]
pub mod apply {
    pub fn run_if_asked() {
        opencode_feature_updater::apply::run_if_asked(crate::launch::product);
    }
}

pub fn build_channel() -> &'static str {
    match option_env!("OPENCODE_RUNE_BUILD_CHANNEL") {
        Some(ch) if !ch.is_empty() => ch,
        _ => "local",
    }
}

/// This build's version: `OPENCODE_RUNE_VERSION` when the build set it, else the crate's.
pub fn build_version() -> &'static str {
    match option_env!("OPENCODE_RUNE_VERSION") {
        Some(v) if !v.is_empty() => v,
        _ => env!("CARGO_PKG_VERSION"),
    }
}

/// The commit this build was made from (`build.rs`), when known. Kept in `.rdata` as
/// `{"sha":"<commit>"}` for binary provenance checks.
pub fn build_commit() -> Option<&'static str> {
    std::hint::black_box(option_env!("OPENCODE_RUNE_BUILD_STAMP"));
    option_env!("OPENCODE_RUNE_COMMIT").filter(|c| !c.is_empty())
}

/// Configures the extracted updater crate with desktop's provenance and updates path.
pub fn init() {
    opencode_feature_updater::configure(UpdaterConfig {
        build: BuildInfo {
            channel: build_channel(),
            version: build_version(),
            commit: build_commit(),
            product_name: crate::launch::window_title(),
        },
        updates_dir: crate::dirs::updates,
        plain_error: crate::status_toast::plain,
    });
}

impl UpdaterHost for App {
    fn updater(&self) -> &Updater {
        &self.services.updater
    }

    fn updater_mut(&mut self) -> &mut Updater {
        &mut self.services.updater
    }
}

/// Kernel feature registration for the self-updater.
pub struct UpdaterFeature;

impl Feature<App> for UpdaterFeature {
    fn id(&self) -> &'static str {
        "feature.updater"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_system(Phase::Ingest, "updater.poll", |s, _, tx| poll(s, tx));
        builder.add_system(Phase::Persist, "updater.poll_owed", |s, _, _| {
            poll_owed(s);
            false
        });
        builder.add_close("updater.closing", |s| s.services.updater.closing());
    }
}
