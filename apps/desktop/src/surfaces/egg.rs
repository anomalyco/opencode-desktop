//! Desktop host adapter for `opencode-feature-egg` (`features/egg`).

pub use opencode_feature_egg::*;

use opencode_kernel::{CommandEntry, Feature, KernelBuilder, Phase, Slot, SurfaceDescriptor, SurfaceKind};

use crate::{App, Route};

pub(crate) static EGG: Slot<Egg> = Slot::new();

impl opencode_feature_egg::EggHost for App {
    fn egg(&self) -> &Egg {
        // Untracked: egg frames step and draw without the view running; `egg::poll` says when
        // the view must rebuild.
        self.kernel.slots.untracked(&EGG)
    }

    fn egg_mut(&mut self) -> &mut Egg {
        self.kernel.slots.get_mut(&EGG)
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
        builder.register_slot(&EGG);
        builder.add_command(
            CommandEntry::from_key(
                "egg.play",
                opencode_i18n::key!("rune.egg.play"),
                crate::commands::order::EGG_PLAY,
                |s, tx| run_play_command(s, tx),
            )
            .when(|s| can_play_command(s)),
        );
        builder.add_system_ordered(
            Phase::Ingest,
            crate::tick::order::ingest::EGG,
            "egg.poll",
            |s, now, tx| poll(s, now, tx),
        );
        builder.add_visibility("egg.visibility", |s, shown, tx| visibility(s, shown, tx));
        builder.add_surface(SurfaceDescriptor::new("egg", SurfaceKind::Custom("egg"), "Wordmark Egg"));
    }
}
