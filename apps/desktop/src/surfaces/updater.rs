//! Desktop host adapter for `opencode-feature-updater` (`features/updater`).
//!
//! Keeps `build_channel`, `build_version`, and `build_commit` (including the `.rdata`
//! `OPENCODE_RUNE_BUILD_STAMP` provenance record) inside `apps/desktop` so release
//! verification reads the desktop crate's own build environment. On `wasm32`, the native
//! self-updater crate is omitted and the Luau `Updater` service reports no updater.

#[cfg(not(target_arch = "wasm32"))]
pub use opencode_feature_updater::{
    BuildInfo, StateView, Updater, UpdaterConfig, UpdaterHost, finish_quit, installs_on_quit, poll,
    poll_owed,
};

#[cfg(not(target_arch = "wasm32"))]
use opencode_kernel::{Feature, KernelBuilder};
use rune_luau::{Completer, Data};

use crate::App;

#[cfg(target_arch = "wasm32")]
const NO_UPDATER: &str = "No updater runs in this build";

#[cfg(not(target_arch = "wasm32"))]
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
pub fn state_data(s: &App) -> Data {
    #[cfg(not(target_arch = "wasm32"))]
    {
        opencode_feature_updater::state_view(s).map_or(Data::Nil, state_to_data)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        Data::Nil
    }
}

/// `Updater:CheckAsync()`: checks now and completes `done` with the `UpdaterState` record.
pub fn check_async(s: &App, done: Completer) {
    #[cfg(not(target_arch = "wasm32"))]
    opencode_feature_updater::check_with(s, move |view| {
        done.complete(view.map_or(Data::Nil, state_to_data));
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        done.complete(Data::Map(
            [
                ("phase".to_owned(), Data::String("error".into())),
                ("error".to_owned(), Data::String(NO_UPDATER.into())),
            ]
            .into_iter()
            .collect(),
        ));
    }
}

/// `Updater:InstallAsync()`: restarts into the ready version or opens a required download;
/// completes `done` with `Data::Nil` on hand-off or an error string.
pub fn install_async(s: &App, done: Completer) {
    #[cfg(not(target_arch = "wasm32"))]
    opencode_feature_updater::install_with(s, move |err| {
        done.complete(err.map_or(Data::Nil, Data::String));
    });
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        done.complete(Data::String(NO_UPDATER.into()));
    }
}

/// Starts the update service after the first frame when this build updates itself.
pub fn start_if_packaged(s: &mut App) {
    #[cfg(not(target_arch = "wasm32"))]
    opencode_feature_updater::start_if_packaged(s);
    #[cfg(target_arch = "wasm32")]
    let _ = s;
}

/// Reactive revision of the updater service (`0` on `wasm32`, where no updater runs).
pub(crate) fn revision(s: &App) -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.kernel.slots.get(&UPDATER).revision()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        0
    }
}

/// Untracked revision of the updater service for stamp hashing (`0` on `wasm32`).
pub(crate) fn revision_untracked(s: &App) -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.kernel.slots.untracked(&UPDATER).revision()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        0
    }
}

/// Running version shown in Settings › About (bundle version once reported, else build version).
pub(crate) fn display_version(s: &App) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let updater = s.kernel.slots.get(&UPDATER);
        if updater.version.is_empty() {
            build_version().to_owned()
        } else {
            updater.version.clone()
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = s;
        build_version().to_owned()
    }
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
    #[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) static UPDATER: opencode_kernel::Slot<Updater> = opencode_kernel::Slot::new();

#[cfg(not(target_arch = "wasm32"))]
impl UpdaterHost for App {
    fn updater(&self) -> &Updater {
        self.kernel.slots.get(&UPDATER)
    }

    fn updater_mut(&mut self) -> &mut Updater {
        self.kernel.slots.get_mut(&UPDATER)
    }
}

/// Kernel feature registration for the self-updater.
#[cfg(not(target_arch = "wasm32"))]
pub struct UpdaterFeature;

#[cfg(not(target_arch = "wasm32"))]
impl Feature<App> for UpdaterFeature {
    fn id(&self) -> &'static str {
        "feature.updater"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.register_slot(&UPDATER);
        builder.schedule(crate::tick::stage::INGEST_LOCAL, "updater.poll", |s, _, tx| poll(s, tx));
        builder.schedule(crate::tick::stage::PERSIST_FLUSH, "updater.poll_owed", |s, _, _| {
            poll_owed(s);
            false
        });
        builder.add_close("updater.closing", |s| s.kernel.slots.get_mut(&UPDATER).closing());
    }
}
