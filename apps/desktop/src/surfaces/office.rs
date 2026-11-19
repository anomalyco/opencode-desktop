//! Desktop host adapter for `opencode-feature-office` (`features/office`).

pub use opencode_feature_office::*;

use opencode_kernel::{Feature, KernelBuilder, Phase};

use crate::App;

impl opencode_feature_office::OfficeHost for App {}

/// Wires desktop's cache directory and wall clock into the Office feature crate once at startup.
pub fn init() {
    configure(OfficeEnv {
        cache_dir: crate::dirs::cache,
        wall_ms: Some(crate::wall_ms),
    });
}

/// Kernel feature registration for Office document previews.
pub struct OfficeFeature;

impl Feature<App> for OfficeFeature {
    fn id(&self) -> &'static str {
        "feature.office"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::OFFICE,
            "office.poll",
            |s, _, tx| poll(s, tx),
        );
    }
}
