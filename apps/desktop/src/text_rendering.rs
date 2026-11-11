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

/// A Text rendering choice, as saved (`textRenderer`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextRenderer {
    /// Rune's default here: Windfoil with Chromium's weight, or Retina-like on macOS.
    #[default]
    Default,
    WindfoilMac,
    WindfoilChromium,
    Slug,
    WindfoilRgb,
    /// DirectWrite grayscale with Skia's gamma on Windows, hinted grayscale elsewhere.
    ChromiumLike,
    ClearType,
    RetinaLike,
    HintedGpu,
}

impl TextRenderer {
    pub const ALL: [TextRenderer; 9] = [
        TextRenderer::Default,
        TextRenderer::WindfoilMac,
        TextRenderer::WindfoilChromium,
        TextRenderer::Slug,
        TextRenderer::WindfoilRgb,
        TextRenderer::ChromiumLike,
        TextRenderer::ClearType,
        TextRenderer::RetinaLike,
        TextRenderer::HintedGpu,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::WindfoilMac => "windfoil",
            Self::WindfoilChromium => "windfoil-chromium",
            Self::Slug => "slug",
            Self::WindfoilRgb => "windfoil-rgb",
            Self::ChromiumLike => "chromium",
            Self::ClearType => "cleartype",
            Self::RetinaLike => "retina",
            Self::HintedGpu => "hinted-gpu",
        }
    }

    /// A saved name; `classic`, the first build's Off position, is the system's own text.
    pub fn parse(s: Option<&str>) -> TextRenderer {
        match s {
            Some("classic") if cfg!(target_os = "macos") => Self::RetinaLike,
            Some("classic") => Self::ChromiumLike,
            _ => Self::ALL.into_iter().find(|r| Some(r.name()) == s).unwrap_or_default(),
        }
    }

    pub fn label(self) -> &'static str {
        use opencode_i18n::t;
        match self {
            Self::Default if cfg!(target_os = "macos") => t!("rune.textRendering.default.mac"),
            Self::Default => t!("rune.textRendering.default"),
            Self::WindfoilMac => t!("rune.textRendering.windfoil"),
            Self::WindfoilChromium => t!("rune.textRendering.windfoilChromium"),
            Self::Slug => t!("rune.textRendering.slug"),
            Self::WindfoilRgb => t!("rune.textRendering.windfoilRgb"),
            Self::ChromiumLike if cfg!(target_os = "windows") => t!("rune.textRendering.chromium.windows"),
            Self::ChromiumLike => t!("rune.textRendering.chromium"),
            Self::ClearType => t!("rune.textRendering.cleartype"),
            Self::RetinaLike => t!("rune.textRendering.retina"),
            Self::HintedGpu => t!("rune.textRendering.hintedGpu"),
        }
    }

    /// The mode it draws with, when Rune has one for it.
    fn mode(self) -> Option<FontSmoothing> {
        Some(match self {
            Self::Default => FontSmoothing::default(),
            Self::WindfoilMac => FontSmoothing::Windfoil,
            Self::WindfoilChromium => FontSmoothing::WindfoilChromium,
            Self::Slug => FontSmoothing::Slug,
            Self::ChromiumLike if cfg!(target_os = "windows") => FontSmoothing::DirectWrite,
            Self::ChromiumLike => FontSmoothing::Hinted,
            Self::RetinaLike => FontSmoothing::On,
            Self::HintedGpu => FontSmoothing::HintedGpu,
            Self::WindfoilRgb => FontSmoothing::WindfoilRgb,
            Self::ClearType => FontSmoothing::ClearType,
        })
    }

    /// Whether this machine draws it as named, and why not in a line.
    pub fn support(self) -> SmoothingSupport {
        match self.mode() {
            Some(FontSmoothing::ClearType) if !cfg!(target_os = "windows") => SmoothingSupport::No(opencode_i18n::t!("rune.textRendering.reason.windowsOnly")),
            Some(mode) => mode.support(),
            None => SmoothingSupport::No(""),
        }
    }

    /// The next choice after this one that draws here, for the palette.
    pub fn cycle(self) -> TextRenderer {
        let at = Self::ALL.iter().position(|r| *r == self).unwrap_or(0);
        (1..Self::ALL.len()).map(|i| Self::ALL[(at + i) % Self::ALL.len()])
            .find(|r| !matches!(r.support(), SmoothingSupport::No(_))).unwrap_or(self)
    }
}

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
            Rendering::Unapplied => FontSmoothing::default(),
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
        (_, SmoothingSupport::No(reason) | SmoothingSupport::Fallback(reason)) => Rendering::FellBack { choice, mode: FontSmoothing::default(), reason },
        (_, _) => Rendering::FellBack { choice, mode: FontSmoothing::default(), reason: "" },
    }
}

// Flips Text rendering from the palette, to compare renderers on the same screen.
rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("text-rendering.cycle", opencode_i18n::key!("rune.textRendering.cycle"), |s, _| s.set_text_renderer(s.settings.text_renderer.cycle())),
]);

/// Draws this window's text as Appearance › Text rendering says; Rune redraws only when the
/// mode changes.
pub fn poll(s: &mut App, tx: &mut Tx) {
    let effects = s.text_rendering.step(RenderingEvent::Poll(s.settings.text_renderer));
    s.apply_rendering(effects);
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
