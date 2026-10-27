//! OpenCode's v2 tokens, resolved once per appearance from `assets/theme.json`
//! (generated from `@opencode/ui` by `tools/theme.py`).

use std::sync::OnceLock;

use rune::{Color, Theme};

#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub dark: bool,
    pub bg_base: Color,
    pub bg_deep: Color,
    pub layer1: Color,
    pub layer2: Color,
    pub layer3: Color,
    pub contrast: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub icon: Color,
    pub icon_muted: Color,
    pub icon_faint: Color,
    pub icon_contrast: Color,
    pub border: Color,
    pub border_muted: Color,
    pub hover: Color,
    pub pressed: Color,
    pub ring: Color,
    pub shadow: Color,
    /// --v2-elevation-floating's far drop.
    pub shadow_far: Color,
    pub success: Color,
    pub danger: Color,
    pub warning: Color,
    pub bg_accent: Color,
    pub search_bg: Color,
    pub inline_code_bg: Color,
    pub selection: Color,
    pub avatar_fg: Color,
    pub avatars: [(Color, Color); 9],
    /// The question dock's tokens (`question-prompt.css`, `button.css`).
    pub q: Question,
}

#[derive(Clone, Copy, Debug)]
pub struct Question {
    pub layer4: Color,
    pub border_strong: Color,
    /// `--v2-question-answer-bg-selected`: the highlighted answer.
    pub selected: Color,
    pub neutral: Color,
    pub neutral_ring: Color,
    pub neutral_shadow: Color,
    pub submit: Color,
    pub submit_ring: Color,
    pub submit_text: Color,
    pub contrast_text: Color,
}

const THEME: &str = include_str!("../assets/theme.json");

fn build(dark: bool) -> Pal {
    let t = Theme::from_json(THEME, dark).unwrap_or_else(|e| panic!("{e}"));
    let c = |n: &str| t.color(n);
    let avatar = |n: &str| (c(&format!("avatar-bg-{n}")), c(&format!("avatar-border-{n}")));
    let text = c("text-text-base");
    let layer2 = c("background-bg-layer-02");
    Pal {
        dark,
        bg_base: c("background-bg-base"),
        bg_deep: c("background-bg-deep"),
        layer1: c("background-bg-layer-01"),
        layer2,
        layer3: c("background-bg-layer-03"),
        contrast: c("background-bg-contrast"),
        text,
        muted: c("text-text-muted"),
        faint: c("text-text-faint"),
        accent: c("text-text-accent"),
        icon: c("icon-icon-base"),
        icon_muted: c("icon-icon-muted"),
        icon_faint: c("icon-icon-faint"),
        icon_contrast: c("icon-icon-contrast"),
        border: c("border-border-base"),
        border_muted: c("border-border-muted"),
        hover: c("overlay-simple-overlay-hover"),
        pressed: c("overlay-simple-overlay-pressed"),
        // --v2-elevation-raised: a 0.5px ring over two drop shadows.
        ring: if dark { c("alpha-light-16") } else { c("alpha-dark-8") },
        shadow: if dark { c("alpha-dark-30") } else { c("alpha-dark-8") },
        shadow_far: if dark { c("alpha-dark-30") } else { c("alpha-dark-4") },
        success: c("state-fg-success"),
        danger: c("state-fg-danger"),
        warning: c("state-fg-warning"),
        bg_accent: c("background-bg-accent"),
        search_bg: t.alpha(layer2, 0.6),
        // color-mix(in oklch, text-base 8%, transparent)
        inline_code_bg: t.alpha(text, 0.08),
        selection: t.alpha(c("text-text-accent"), 0.3),
        avatar_fg: c("avatar-fg"),
        avatars: [
            avatar("orange"),
            avatar("yellow"),
            avatar("cyan"),
            avatar("green"),
            avatar("red"),
            avatar("pink"),
            avatar("blue"),
            avatar("purple"),
            avatar("gray"),
        ],
        q: Question {
            layer4: c("background-bg-layer-04"),
            border_strong: c("border-border-strong"),
            selected: if dark { t.alpha(c("blue-500"), 0x26 as f32 / 255.0) } else { rune::color::hex("#f5f8ff").unwrap_or(Color::WHITE) },
            neutral: c("background-bg-button-neutral"),
            neutral_ring: if dark { c("alpha-light-20") } else { c("alpha-dark-14") },
            neutral_shadow: if dark { c("alpha-dark-40") } else { c("alpha-dark-10") },
            submit: c("background-bg-icon-button-contrast"),
            submit_ring: if dark { c("alpha-light-40") } else { c("grey-800") },
            submit_text: if dark { c("text-text-inverse") } else { c("text-text-contrast") },
            contrast_text: c("text-text-contrast"),
        },
    }
}

pub fn pal(dark: bool) -> &'static Pal {
    static DARK: OnceLock<Pal> = OnceLock::new();
    static LIGHT: OnceLock<Pal> = OnceLock::new();
    if dark { DARK.get_or_init(|| build(true)) } else { LIGHT.get_or_init(|| build(false)) }
}
