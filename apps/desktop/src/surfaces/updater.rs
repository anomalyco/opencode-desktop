//! Desktop host adapter for `opencode-feature-updater` (`features/updater`).
//!
//! Keeps `build_channel`, `build_version`, and `build_commit` (including the `.rdata`
//! `OPENCODE_RUNE_BUILD_STAMP` provenance record) inside `apps/desktop` so release
//! verification reads the desktop crate's own build environment.

pub use opencode_feature_updater::{
    BuildInfo, StateView, Updater, UpdaterConfig, UpdaterHost, poll, poll_owed, start_if_packaged,
};
#[cfg(not(target_arch = "wasm32"))]
pub use opencode_feature_updater::{finish_quit, installs_on_quit};

use opencode_kernel::{Feature, KernelBuilder, Phase};
use rune_luau::{Completer, Data};

use crate::App;

fn state_to_data(view: StateView) -> Data {
    let fields = [
        ("phase", Data::String(view.phase.into())),
        ("version", view.version.map_or(Data::Nil, Data::String)),
        ("progress", view.progress.map_or(Data::Nil, Data::Number)),
        ("error", view.error.map_or(Data::Nil, Data::String)),
    ];
    Data::Map(fields.into_iter().filter(|(_, v)| *v != Data::Nil).map(|(k, v)| (k.to_owned(), v)).collect())
}

/// Where the updater stands, as the `UpdaterState` Luau record (`Data::Nil` when unavailable).
pub fn state_data(s: &dyn UpdaterHost) -> Data {
    opencode_feature_updater::state_view(s).map_or(Data::Nil, state_to_data)
}

/// `Updater:CheckAsync()`: checks now and completes `done` with the `UpdaterState` record.
pub fn check_async(s: &dyn UpdaterHost, done: Completer) {
    opencode_feature_updater::check_with(s, move |view| {
        done.complete(view.map_or(Data::Nil, state_to_data));
    });
}

/// `Updater:InstallAsync()`: restarts into the ready version or opens a required download;
/// completes `done` with `Data::Nil` on hand-off or an error string.
pub fn install_async(s: &dyn UpdaterHost, done: Completer) {
    opencode_feature_updater::install_with(s, move |err| {
        done.complete(err.map_or(Data::Nil, Data::String));
    });
}

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
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::UPDATER,
            "updater.poll",
            |s, _, tx| poll(s, tx),
        );
        builder.add_system_ordered(
            Phase::Persist,
            crate::tick::order::persist::UPDATER_OWED,
            "updater.poll_owed",
            |s, _, _| {
                poll_owed(s);
                false
            },
        );
        builder.add_close("updater.closing", |s| s.services.updater.closing());
    }
}
