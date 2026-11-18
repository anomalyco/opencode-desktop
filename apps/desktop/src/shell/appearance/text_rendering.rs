//! Settings › Appearance › Text rendering: every way Rune can rasterise text, picked live, so the
//! team can compare them on the same screen. Each choice names a [`FontSmoothing`]; one this
//! machine can't draw shows disabled with its reason. Which mode the windows draw is a state
//! machine ([`Rendering`]): the saved choice, applied as is or fallen back with a reason, and
//! moved again when the renderer reports what it can do (a GPU that can't build the outline
//! shader, say). Each pick says which it is in a toast, so a screenshot names its renderer, and
//! the palette's Cycle text rendering steps through every choice that draws.

use rune::machine::Effects;
use rune::{FontSmoothing, SmoothingSupport, Tx};

use crate::App;

pub use opencode_feature_settings::TextRenderer;

/// Which mode the windows draw for the saved choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rendering {
    /// Nothing applied yet: the first poll applies the saved choice.
    #[default]
    Unapplied,
    /// The choice draws as named.
    Applied { choice: TextRenderer, mode: FontSmoothing },
    /// It draws `mode` instead, for `reason`: what the choice falls back to here.
    FellBack { choice: TextRenderer, mode: FontSmoothing, reason: &'static str },
}

#[derive(Clone, Copy, Debug)]
pub enum RenderingEvent {
    /// The user picked a choice (Settings, the palette).
    Pick(TextRenderer),
    /// A frame, with the saved choice: what this machine can draw may have changed.
    Poll(TextRenderer),
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderingEffect {
    /// Toast "Text rendering: …".
    Announce(String),
    /// Log that the choice fell back, and why.
    Log(String),
}

impl Rendering {
    pub fn mode(self) -> FontSmoothing {
        match self {
            Rendering::Unapplied => TextRenderer::default_mode(),
            Rendering::Applied { mode, .. } | Rendering::FellBack { mode, .. } => mode,
        }
    }

    pub fn step(&mut self, event: RenderingEvent) -> Effects<RenderingEffect> {
        let (choice, picked) = match event {
            RenderingEvent::Pick(c) => (c, true),
            RenderingEvent::Poll(c) => (c, false),
        };
        let next = resolve(choice);
        let changed = next != *self;
        *self = next;
        let mut effects = Effects::none();
        if picked {
            let text = match next {
                Rendering::FellBack { reason, .. } => format!("{} ({reason})", choice.label()),
                _ => choice.label().to_owned(),
            };
            effects.push(RenderingEffect::Announce(text));
        }
        if changed && let Rendering::FellBack { mode, reason, .. } = next {
            effects.push(RenderingEffect::Log(format!("text rendering: {} draws as {mode:?}: {reason}", choice.name())));
        }
        effects
    }
}

fn resolve(choice: TextRenderer) -> Rendering {
    match (choice.mode(), choice.support()) {
        (Some(mode), SmoothingSupport::Yes) => Rendering::Applied { choice, mode },
        (Some(mode), SmoothingSupport::Fallback(reason)) => Rendering::FellBack { choice, mode, reason },
        (_, SmoothingSupport::No(reason) | SmoothingSupport::Fallback(reason)) => Rendering::FellBack { choice, mode: TextRenderer::default_mode(), reason },
        (_, _) => Rendering::FellBack { choice, mode: TextRenderer::default_mode(), reason: "" },
    }
}

// Flips Text rendering from the palette, to compare renderers on the same screen.
rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("text-rendering.cycle", opencode_i18n::key!("rune.textRendering.cycle"), |s, _| s.set_text_renderer(s.settings.text_renderer.cycle())),
]);

/// Draws this window's text as Appearance › Text rendering says; Rune redraws only when the
/// mode changes.
pub fn poll(s: &mut App, tx: &mut Tx) {
    // Every poll comes here: the state is written only when the choice resolves differently.
    if resolve(s.settings.text_renderer) != *s.text_rendering {
        let effects = s.text_rendering.step(RenderingEvent::Poll(s.settings.text_renderer));
        s.apply_rendering(effects);
    }
    let (mode, name) = match measured() {
        Some(forced) => forced,
        None => (s.text_rendering.mode(), s.settings.text_renderer.name()),
    };
    crate::launch::trace::renderer(name);
    tx.set_font_smoothing(mode);
}

/// `--font-smoothing <mode>`, for the launch bench only: the run draws with that mode whatever
/// Text rendering says, including `original`, the CPU outline rasteriser Windows drew with before
/// DirectWrite (`FontSmoothing::Off`), which the picker doesn't offer.
fn measured() -> Option<(FontSmoothing, &'static str)> {
    static FORCED: std::sync::OnceLock<Option<(FontSmoothing, &'static str)>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| {
        Some(match crate::arg("--font-smoothing")?.as_str() {
            "original" => (FontSmoothing::Off, "original"),
            "classic" => (FontSmoothing::DirectWrite, "classic"),
            "windfoil-chromium" => (FontSmoothing::WindfoilChromium, "windfoil-chromium"),
            "windfoil" => (FontSmoothing::Windfoil, "windfoil"),
            _ => return None,
        })
    })
}

impl App {
    /// Saves Text rendering and says which it is in a toast; [`poll`] redraws with it.
    pub fn set_text_renderer(&mut self, r: TextRenderer) {
        self.change_settings(|s| s.text_renderer = r);
        let effects = self.text_rendering.step(RenderingEvent::Pick(r));
        self.apply_rendering(effects);
    }

    fn apply_rendering(&mut self, effects: Effects<RenderingEffect>) {
        for e in effects {
            match e {
                RenderingEffect::Announce(name) => crate::status_toast::say(self, opencode_i18n::t!("rune.textRendering.toast", name = name), ""),
                RenderingEffect::Log(line) => eprintln!("{line}"),
            }
        }
    }
}
