//! OpenCode's v2 tokens, resolved once per appearance from `assets/theme.json`
//! (generated from `@opencode/ui` by `tools/theme.py`).

use std::sync::{Mutex, OnceLock};

use rune::{Color, Shadow, Theme};
use rune_kit::{Kit, ScrollBarStyle};
use rune_kit::syntax::SyntaxTheme;

#[derive(Clone, Copy, Debug)]
pub struct Pal {
    pub dark: bool,
    /// The theme painted, as an index into `themes::all` (0 is OpenCode's default).
    pub theme: usize,
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
    /// `overlay-simple-overlay-scrim`: behind dialogs and the image viewer.
    pub scrim: Color,
    /// A highlighted list row: the hover overlay, lifted to alpha-light-10 in dark.
    pub row_active: Color,
    /// `state-fg-info`.
    pub info: Color,
    pub ring: Color,
    pub shadow: Color,
    /// --v2-elevation-floating's far drop.
    pub shadow_far: Color,
    /// --v2-elevation-raised as Desktop draws it: two drops and a 0.5px ring outside the box.
    /// Dark mode's 0.5px top highlight is left out: no lit rims.
    pub raised: [Shadow; 3],
    /// `color-mix(in srgb, bg-base 92%, transparent)`: the tint over a frosted surface.
    pub frost: Color,
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
    /// The terminal's text: Desktop's `text-stronger`; the default theme keeps `text`.
    pub term_fg: Color,
    /// A diff's added and removed colour: `syntax-diff-add`/`-delete`; the default keeps
    /// `success` and `danger`.
    pub diff_add: Color,
    pub diff_del: Color,
    /// Text and icons on a filled accent or danger button, and a lit menu item.
    pub on_fill: Color,
    /// The scrim behind the delete confirmation; a chosen theme's `overlay-simple-overlay-scrim`.
    pub dim: Color,
    /// The settings switch's thumb (Desktop's `grey-200`, a little darker in dark).
    pub switch_thumb: Color,
    /// The surface lift under a just-arrived prompt.
    pub lift: Color,
    /// A chosen theme's code colours (Desktop's `syntax-*`); None for the default, whose code
    /// keeps rune-kit's colours.
    pub code: Option<Code>,
}

/// Desktop's `syntax-*` tokens, as `highlight-theme.tsx` assigns them to token kinds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Code {
    pub comment: Color,
    pub string: Color,
    pub keyword: Color,
    pub primitive: Color,
    pub property: Color,
    pub ty: Color,
    pub constant: Color,
    pub punctuation: Color,
    pub variable: Color,
}

impl Code {
    /// Desktop's highlight theme on rune-kit's token kinds: functions `syntax-primitive`,
    /// numbers `syntax-constant`, properties `syntax-variable`.
    pub fn syntax(&self, base: SyntaxTheme, add: Color, del: Color) -> SyntaxTheme {
        use rune_kit::syntax::Syntax as K;
        [
            (K::Keyword, self.keyword),
            (K::String, self.string),
            (K::Number, self.constant),
            (K::Comment, self.comment),
            (K::Type, self.ty),
            (K::Function, self.primitive),
            (K::Property, self.variable),
            (K::Punctuation, self.punctuation),
            (K::Inserted, add),
            (K::Deleted, del),
        ]
        .into_iter()
        .fold(base, |t, (k, c)| t.with(k, c))
    }
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

const THEME: &str = include_str!("../../../apps/desktop/assets/theme.json");

fn build(theme: usize, dark: bool) -> Pal {
    let src = crate::themes::json(theme, THEME);
    let t = Theme::from_json(src.as_deref().unwrap_or(THEME), dark).unwrap_or_else(|e| panic!("{e}"));
    let c = |n: &str| t.color(n);
    let avatar = |n: &str| (c(&format!("avatar-bg-{n}")), c(&format!("avatar-border-{n}")));
    let text = c("text-text-base");
    let layer2 = c("background-bg-layer-02");
    let themed = theme > 0;
    let success = c("state-fg-success");
    let danger = c("state-fg-danger");
    Pal {
        dark,
        theme,
        on_fill: Color::WHITE,
        dim: if themed { c("overlay-simple-overlay-scrim") } else { Color::BLACK.alpha(if dark { 0.5 } else { 0.25 }) },
        switch_thumb: if dark { rune::color::hex("#d9d9d9").unwrap_or(Color::WHITE) } else { Color::WHITE },
        lift: if dark { text } else { Color::WHITE },
        term_fg: if themed { c("text-stronger") } else { text },
        diff_add: if themed { c("syntax-diff-add") } else { success },
        diff_del: if themed { c("syntax-diff-delete") } else { danger },
        code: themed.then(|| Code {
            comment: c("syntax-comment"),
            string: c("syntax-string"),
            keyword: c("syntax-keyword"),
            primitive: c("syntax-primitive"),
            property: c("syntax-property"),
            ty: c("syntax-type"),
            constant: c("syntax-constant"),
            punctuation: c("syntax-punctuation"),
            variable: c("syntax-variable"),
        }),
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
        scrim: c("overlay-simple-overlay-scrim"),
        row_active: if dark { c("alpha-light-10") } else { c("overlay-simple-overlay-hover") },
        info: c("state-fg-info"),
        // --v2-elevation-raised: a 0.5px ring over two drop shadows.
        ring: if dark { c("alpha-light-16") } else { c("alpha-dark-8") },
        shadow: if dark { c("alpha-dark-30") } else { c("alpha-dark-8") },
        shadow_far: if dark { c("alpha-dark-30") } else { c("alpha-dark-4") },
        raised: {
            let drop = |y, blur, spread, color| Shadow { y, blur, spread, color, ..Shadow::default() };
            if dark {
                [drop(2.0, 4.0, 0.0, c("alpha-dark-30")), drop(1.0, 2.0, 0.0, c("alpha-dark-30")), drop(0.0, 0.0, 0.5, c("alpha-light-16"))]
            } else {
                [drop(2.0, 4.0, 0.0, c("alpha-dark-4")), drop(1.0, 2.0, -1.0, c("alpha-dark-8")), drop(0.0, 0.0, 0.5, c("alpha-dark-12"))]
            }
        },
        frost: t.alpha(c("background-bg-base"), 0.92),
        success,
        danger,
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
            // Light: `blue-100`, which Desktop's themes don't carry; they get a wash of their blue.
            selected: if dark {
                t.alpha(c("blue-500"), 0x26 as f32 / 255.0)
            } else if themed {
                t.alpha(c("blue-500"), 0x14 as f32 / 255.0)
            } else {
                rune::color::hex("#f5f8ff").unwrap_or(Color::WHITE)
            },
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

/// The tokens for an appearance in this thread's theme (`themes::active`), built once each.
pub fn pal(dark: bool) -> &'static Pal {
    static PALS: [[OnceLock<Pal>; 2]; 2] = [const { [const { OnceLock::new() }; 2] }; 2];
    static THEMED: Mutex<Vec<((usize, bool, bool), &'static Pal)>> = Mutex::new(Vec::new());
    let srgb = rune::theme::blends_srgb();
    let theme = crate::themes::active();
    if theme == 0 {
        return PALS[dark as usize][srgb as usize].get_or_init(|| rune::theme::with_blend(srgb, || build(0, dark)));
    }
    let key = (theme, dark, srgb);
    let mut themed = THEMED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = themed.iter().find(|(k, _)| *k == key) {
        return p;
    }
    let p: &'static Pal = Box::leak(Box::new(rune::theme::with_blend(srgb, || build(theme, dark))));
    themed.push((key, p));
    p
}

/// The one rune-kit Kit, for markdown, code, diffs and the odometer: OpenCode's kit, its
/// colours taken from `p` in a chosen theme so it follows the theme (the default keeps it as is). Built once per theme, appearance and blend.
pub fn kit(p: &Pal) -> &'static Kit {
    static KITS: Mutex<Vec<((usize, bool, bool), &'static Kit)>> = Mutex::new(Vec::new());
    let key = (p.theme, p.dark, rune::theme::blends_srgb());
    let mut kits = KITS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, k)) = kits.iter().find(|(k, _)| *k == key) {
        return k;
    }
    let k: &'static Kit = Box::leak(Box::new(kit_from(p)));
    kits.push((key, k));
    k
}

/// The timeline's and Settings' scroll bar thumb, as OpenCode Desktop draws it: `border_muted`
/// and 4 points wide at rest, `border_strong` and 8 points under the pointer.
pub fn scroll_bar(p: &Pal) -> ScrollBarStyle {
    ScrollBarStyle::from_kit(kit(p)).thumb(p.border_muted).thumb_hover(p.q.border_strong).width(4.0).hover_width(8.0)
}

fn kit_from(p: &Pal) -> Kit {
    let mut k = rune_theme_opencode::kit(p.dark).clone();
    if p.theme == 0 {
        return k;
    }
    let c = &mut k.c;
    (c.page, c.bg_base, c.bg_deep, c.layer_01, c.layer_02, c.layer_03, c.layer_04) = (p.bg_deep, p.bg_base, p.bg_deep, p.layer1, p.layer2, p.layer3, p.q.layer4);
    (c.bg_contrast, c.bg_accent, c.bg_button_neutral, c.bg_icon_button_contrast) = (p.contrast, p.bg_accent, p.q.neutral, p.q.submit);
    (c.text_base, c.text_muted, c.text_faint, c.text_accent, c.text_contrast) = (p.text, p.muted, p.faint, p.accent, p.q.contrast_text);
    (c.icon_base, c.icon_muted, c.icon_faint, c.icon_contrast) = (p.icon, p.icon_muted, p.icon_faint, p.icon_contrast);
    (c.border_base, c.border_muted, c.border_strong) = (p.border, p.border_muted, p.q.border_strong);
    (c.hover, c.pressed, c.scrim) = (p.hover, p.pressed, p.scrim);
    for (state, fg) in [(&mut c.success, p.diff_add), (&mut c.danger, p.diff_del), (&mut c.warning, p.warning), (&mut c.info, p.info)] {
        (state.fg, state.icon) = (fg, fg);
    }
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokens the switcher, the image viewer and the mention badge once parsed for themselves.
    /// Whatever blend mode earlier tests left on this thread, `pal` must match the theme read in it.
    #[test]
    fn pal_carries_what_the_switcher_viewer_and_badge_read_from_the_theme() {
        for srgb in [true, false] {
            rune::theme::with_blend(srgb, || {
                for dark in [false, true] {
                    let t = Theme::from_json(THEME, dark).unwrap();
                    let c = |n: &str| t.color(n);
                    let p = pal(dark);
                    assert_eq!(p.scrim, c("overlay-simple-overlay-scrim"));
                    assert_eq!(p.row_active, if dark { c("alpha-light-10") } else { c("overlay-simple-overlay-hover") });
                    assert_eq!(p.q.layer4, c("background-bg-layer-04"));
                    assert_eq!(p.shadow_far, if dark { c("alpha-dark-30") } else { c("alpha-dark-4") });
                    assert_eq!(p.shadow, if dark { c("alpha-dark-30") } else { c("alpha-dark-8") });
                    assert_eq!(p.info, c("state-fg-info"));
                    assert_eq!(p.search_bg, t.alpha(c("background-bg-layer-02"), 0.6));
                }
            });
        }
    }
}
