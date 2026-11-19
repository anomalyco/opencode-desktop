//! Desktop host adapter for `opencode-feature-egg` (`features/egg`).

pub use opencode_feature_egg::*;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, Phase, SurfaceDescriptor, SurfaceKind};

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

    fn interface_sounds(&self) -> bool {
        self.settings.sounds.interface
    }

    fn focus_composer(&mut self) {
        self.composer.focused = true;
    }
}

/// Kernel feature registration for the new-session Easter egg.
pub struct EggFeature;

impl Feature<App> for EggFeature {
    fn id(&self) -> &'static str {
        "feature.egg"
    }

    fn register(&self, builder: &mut KernelBuilder<App>) {
        builder.add_command(
            CommandEntry::from_key(
                "egg.play",
                opencode_i18n::key!("rune.egg.play"),
                crate::commands::order::EGG_PLAY,
                |s, tx| run_play_command(s, tx),
            )
            .when(|s| can_play_command(s)),
        );
        builder.add_system(Phase::React, "egg.poll", |s, now, tx| {
            let changed = poll(s, now, tx);
            s.note_tick_change(changed)
        });
        builder.add_visibility("egg.visibility", |s, shown, tx| visibility(s, shown, tx));
        builder.add_surface(SurfaceDescriptor::new("egg", SurfaceKind::Custom("egg"), "Wordmark Egg"));
    }
}
