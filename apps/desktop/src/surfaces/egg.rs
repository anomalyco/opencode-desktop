//! Desktop host adapter for `opencode-feature-egg` (`features/egg`).

pub use opencode_feature_egg::*;

use opencode_kernel::{Feature, KernelBuilder, Phase, SurfaceDescriptor, SurfaceKind};

use crate::{App, Route};

impl opencode_feature_egg::EggHost for App {
    fn egg(&self) -> &Egg {
        &self.egg
    }

    fn egg_mut(&mut self) -> &mut Egg {
        &mut self.egg
    }

    fn is_draft_route(&self) -> bool {
        matches!(&*self.route, Route::Draft(_))
    }

    fn reduced_motion(&self) -> bool {
        *self.reduced_motion
    }

    fn interface_sounds(&self) -> bool {
        self.settings.sounds.interface
    }

    fn persists(&self) -> bool {
        self.persists()
    }

    fn focus_composer(&mut self) {
        self.composer.focused = true;
    }
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("egg.play", opencode_i18n::key!("rune.egg.play"), run_play_command)
        .when(can_play_command),
]);

/// Kernel feature registration for the new-session Easter egg.
pub struct EggFeature;

impl Feature<App> for EggFeature {
    fn id(&self) -> &'static str {
        "feature.egg"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_system(Phase::React, "egg.poll", |s, now, tx| poll(s, now, tx));
        builder.add_visibility("egg.visibility", |s, shown, tx| visibility(s, shown, tx));
        builder.add_surface(SurfaceDescriptor::new("egg", SurfaceKind::Custom("egg"), "Wordmark Egg"));
    }
}
