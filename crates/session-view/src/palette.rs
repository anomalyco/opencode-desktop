//! OpenCode's v2 tokens, resolved once per appearance from `assets/theme.json`
//! (generated from `@opencode/ui` by `tools/theme.py`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use rune::{Color, El, Shadow, Theme};
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
    /// `--v2-elevation-raised`: two drops, a 0.5px ring outside the box and, in dark, the 0.5px
    /// lit top edge (`0 -0.5px alpha-light-6`). Put them on a surface with [`Elevate::elevate`].
    pub raised: [Shadow; 4],
    /// `--v2-elevation-floating`: menus, popovers, toasts and the composer lane's cards.
    pub floating: [Shadow; 4],
    /// `--v2-elevation-overlay`: dialogs.
    pub overlay: [Shadow; 4],
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
    /// Desktop's v1 tokens the permission dock still paints with (`theme.css`): `--text-base`
    /// for its patterns, `--text-weak` for a tool's arguments, `--icon-warning-base` for its
    /// warning. A chosen theme, which carries only v2 tokens here, gets its nearest v2 ones.
    pub v1_text_base: Color,
    pub v1_text_weak: Color,
    pub v1_icon_warning: Color,
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
    /// Every theme's code colours, the default's included: replies' code blocks, as
    /// Desktop's `markdown.worker.ts` colours them.
    pub syntax: Code,
    /// `text-text-accent-hover`: a link under the pointer.
    pub accent_hover: Color,
    /// Desktop's code block fill: the v1 `background-stronger` (`.highlights`).
    pub code_bg: Color,
    /// Desktop's v1 raised surface, as `menu-v2`'s standard look draws it (the Windows app menu).
    pub standard: Standard,
    /// `state-bg-danger` and `state-border-danger`: a file's diagnostics.
    pub danger_bg: Color,
    pub danger_border: Color,
    /// A diff's colours, as Desktop's Pierre theme and file rows draw them.
    pub diff: DiffInk,
    /// The rest of Desktop's v2 semantic tokens, each theme's own.
    pub v2: V2,
}

impl Pal {
    /// `box-shadow: 0 0 0 0.5px c`, the hairline of Desktop's elevations: just outside the box,
    /// at its fractional width, as Chromium draws a box-shadow. A `border` snaps to whole device
    /// pixels instead, as a CSS border does.
    pub fn ring_out(c: Color) -> Shadow {
        Shadow { spread: 0.5, color: c, ..Shadow::default() }
    }

    /// `box-shadow: inset 0 0 0 0.5px c`: just inside the box, at its fractional width.
    pub fn ring_in(c: Color) -> Shadow {
        Shadow { spread: 0.5, color: c, inset: true, ..Shadow::default() }
    }
}

/// Desktop's v2 semantic tokens (`theme.css`, `mapping.ts`) that `Pal` doesn't carry above:
/// what rune-kit's controls paint with, so in a chosen theme they follow it.
#[derive(Clone, Copy, Debug)]
pub struct V2 {
    pub bg_inverse: Color,
    pub bg_code_path: Color,
    pub text_inverse: Color,
    pub text_code_accent: Color,
    pub text_code_path: Color,
    pub icon_inverse: Color,
    pub icon_accent: Color,
    pub icon_accent_hover: Color,
    pub border_inverse: Color,
    /// `border-border-focus`: focus rings.
    pub border_focus: Color,
    pub contrast_hover: Color,
    pub contrast_pressed: Color,
    /// `state-{bg,fg,border}-*`, the fg the icon too.
    pub success: rune_kit::StateColors,
    pub warning: rune_kit::StateColors,
    pub danger: rune_kit::StateColors,
    pub info: rune_kit::StateColors,
}

/// A diff's colours (`session-ui/src/pierre/index.ts`, `diff-changes.css`).
#[derive(Clone, Copy, Debug)]
pub struct DiffInk {
    /// `--text-diff-add-base` and `-delete-base`: a file row's `+N -M`, Created and Deleted.
    pub add_text: Color,
    pub del_text: Color,
    /// A changed line's fill, its number cell's too.
    pub add_line: Color,
    pub del_line: Color,
    /// A changed word's fill, over its line's.
    pub add_word: Color,
    pub del_word: Color,
    /// A changed word's text.
    pub add_word_text: Color,
    pub del_word_text: Color,
    /// A changed line's 2 pt bar and number.
    pub add_bar: Color,
    pub del_bar: Color,
    /// The quiet row between hunks.
    pub separator: Color,
}

/// `menu-v2`'s standard look: Desktop's v1 tokens, as each theme resolves them.
#[derive(Clone, Copy, Debug)]
pub struct Standard {
    /// `surface-raised-stronger-non-alpha`: the surface.
    pub surface: Color,
    /// `text-strong`: an item's label.
    pub text: Color,
    /// `text-weak`: a group's label and a disabled item's.
    pub weak: Color,
    /// `surface-raised-base-hover`: the lit item.
    pub hover: Color,
    /// `icon-base`: a submenu's chevron.
    pub icon: Color,
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
    /// `variable.parameter`: the v1 `text-base`.
    pub parameter: Color,
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
            (K::Parameter, self.parameter),
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
    /// `border-border-focus` (`blue-500`): Classic's picked wash and keyboard ring.
    pub focus: Color,
    /// `grey-300`: the dot in a picked Classic radio.
    pub dot: Color,
}

const THEME: &str = include_str!("../../../apps/desktop/assets/theme.json");

/// Token `name` of the active theme in this appearance (a Desktop CSS variable without its
/// `--`: `text-strong`, `background-bg-layer-04`), for what draws by name rather than through
/// [`Pal`] (native plugins). Each theme is read once.
pub fn token(dark: bool, name: &str) -> Option<Color> {
    thread_local! {
        static THEMES: std::cell::RefCell<HashMap<(usize, bool, bool), Theme>> = std::cell::RefCell::new(HashMap::new());
    }
    let theme = crate::themes::active();
    let key = (theme, dark, rune::theme::blends_srgb());
    THEMES.with(|t| {
        let mut t = t.borrow_mut();
        let read = match t.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let src = crate::themes::json(theme, THEME);
                v.insert(Theme::from_json(src.as_deref().unwrap_or(THEME), dark).ok()?)
            }
        };
        read.try_color(name)
    })
}

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
    let diff_add = if themed { c("syntax-diff-add") } else { success };
    let diff_del = if themed { c("syntax-diff-delete") } else { danger };
    let code_bg = c("background-stronger");
    let syntax = Code {
        comment: c("syntax-comment"),
        string: c("syntax-string"),
        keyword: c("syntax-keyword"),
        primitive: c("syntax-primitive"),
        property: c("syntax-property"),
        ty: c("syntax-type"),
        constant: c("syntax-constant"),
        punctuation: c("syntax-punctuation"),
        variable: c("syntax-variable"),
        parameter: c("text-base"),
    };
    let hex = |h: &str| rune::color::hex(h).unwrap_or(text);
    Pal {
        dark,
        theme,
        on_fill: Color::WHITE,
        dim: if themed { c("overlay-simple-overlay-scrim") } else { Color::BLACK.alpha(if dark { 0.5 } else { 0.25 }) },
        switch_thumb: if dark { rune::color::hex("#d9d9d9").unwrap_or(Color::WHITE) } else { Color::WHITE },
        lift: if dark { text } else { Color::WHITE },
        term_fg: if themed { c("text-stronger") } else { text },
        diff_add,
        diff_del,
        accent_hover: c("text-text-accent-hover"),
        code_bg,
        danger_bg: c("state-bg-danger"),
        danger_border: c("state-border-danger"),
        diff: diff_ink(&t, themed, dark, diff_add, diff_del, text, code_bg),
        v2: {
            let state = |n: &str| {
                let fg = c(&format!("state-fg-{n}"));
                rune_kit::StateColors { bg: c(&format!("state-bg-{n}")), fg, border: c(&format!("state-border-{n}")), icon: fg }
            };
            V2 {
                bg_inverse: c("background-bg-inverse"),
                bg_code_path: c("background-bg-code-path"),
                text_inverse: c("text-text-inverse"),
                text_code_accent: c("text-text-code-accent"),
                text_code_path: c("text-text-code-path"),
                icon_inverse: c("icon-icon-inverse"),
                icon_accent: c("icon-icon-accent"),
                icon_accent_hover: c("icon-icon-accent-hover"),
                border_inverse: c("border-border-inverse"),
                border_focus: c("border-border-focus"),
                contrast_hover: c("overlay-simple-overlay-contrast-hover"),
                contrast_pressed: c("overlay-simple-overlay-contrast-pressed"),
                success: state("success"),
                warning: state("warning"),
                danger: state("danger"),
                info: state("info"),
            }
        },
        standard: Standard {
            surface: c("surface-raised-stronger-non-alpha"),
            text: c("text-strong"),
            weak: c("text-weak"),
            hover: c("surface-raised-base-hover"),
            icon: c("icon-base"),
        },
        code: themed.then_some(syntax),
        syntax,
        v1_text_base: match (themed, dark) {
            (true, _) => c("text-text-muted"),
            (false, true) => Color::WHITE.alpha(0.618),
            (false, false) => hex("#6f6f6f"),
        },
        v1_text_weak: match (themed, dark) {
            (true, _) => c("text-text-faint"),
            (false, true) => Color::WHITE.alpha(0.422),
            (false, false) => hex("#8f8f8f"),
        },
        v1_icon_warning: match (themed, dark) {
            (true, _) => c("state-fg-warning"),
            (false, true) => hex("#fbb73c"),
            (false, false) => hex("#ebb76e"),
        },
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
        raised: elevation(&t, dark, 2.0),
        floating: elevation(&t, dark, 8.0),
        overlay: elevation(&t, dark, 16.0),
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
            focus: c("border-border-focus"),
            dot: c("grey-300"),
        },
    }
}

pub trait Elevate {
    /// Lifts a surface by one of Desktop's elevations (`p.raised`, `p.floating`, `p.overlay`),
    /// its first CSS shadow on top as CSS paints it.
    fn elevate(self, elevation: [Shadow; 4]) -> Self;
}

impl Elevate for El {
    fn elevate(self, elevation: [Shadow; 4]) -> El {
        elevation.into_iter().rev().fold(self, El::box_shadow)
    }
}

/// Desktop's `--v2-elevation-raised` (`far` 2), `-floating` (8) and `-overlay` (16), in each theme's
/// alphas (`theme.css`, `mapping.ts`): a far and a near drop, the 0.5px ring outside the box,
/// and dark's lit top edge, a copy of the box 0.5px up that shows above it.
fn elevation(t: &Theme, dark: bool, far: f32) -> [Shadow; 4] {
    let c = |n: &str| t.color(n);
    let drop = |y: f32, blur: f32, spread: f32, color| Shadow { y, blur, spread, color, ..Shadow::default() };
    let near = far / 2.0;
    if dark {
        return [
            drop(far, far * 2.0, 0.0, c("alpha-dark-30")),
            drop(near, near * 2.0, 0.0, c("alpha-dark-30")),
            drop(0.0, 0.0, 0.5, c("alpha-light-16")),
            drop(-0.5, 0.0, 0.0, c("alpha-light-6")),
        ];
    }
    // Only raised pulls its near drop in by 1px.
    let spread = if far == 2.0 { -1.0 } else { 0.0 };
    [
        drop(far, far * 2.0, 0.0, c("alpha-dark-4")),
        drop(near, near * 2.0, spread, c("alpha-dark-8")),
        drop(0.0, 0.0, 0.5, c("alpha-dark-12")),
        Shadow::default(),
    ]
}

/// Pierre's diff colours over Desktop's v2 green and red ramps; a theme without those ramps
/// tints with its own diff colours instead.
#[allow(clippy::too_many_arguments)]
fn diff_ink(t: &Theme, themed: bool, dark: bool, add: Color, del: Color, text: Color, bg: Color) -> DiffInk {
    let ramp = |name: &str, or: Color| if themed { or } else { t.try_color(name).unwrap_or(or) };
    let hex = |s: &str, or: Color| if themed { or } else { rune::color::hex(s).unwrap_or(or) };
    let (green, red) = (ramp("green-600", add), ramp("red-600", del));
    let fill = if dark { 0.1 } else { 0.05 };
    let word = if dark { 0.4 } else { 0.2 };
    let mixer = if dark { Color::WHITE } else { Color::BLACK };
    DiffInk {
        add_text: if dark { hex("#9bcd97", add) } else { hex("#3a8437", add) },
        del_text: if dark { hex("#fc533a", del) } else { hex("#ed4831", del) },
        add_line: t.alpha(if dark { ramp("green-800", add) } else { green }, fill),
        del_line: t.alpha(red, fill),
        add_word: t.alpha(green, word),
        del_word: t.alpha(red, word),
        add_word_text: if dark { text } else { ramp("green-1200", text) },
        del_word_text: if dark { text } else { ramp("red-1200", text) },
        add_bar: if dark { ramp("green-400", add) } else { ramp("green-900", add) },
        del_bar: if dark { ramp("red-500", del) } else { ramp("red-800", del) },
        separator: bg.mix(mixer, if dark { 0.05 } else { 0.02 }),
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

/// The timeline's and Settings' scroll bar thumb, as OpenCode Desktop's ScrollView draws it
/// (scroll-view.css): an overlay hidden at rest and shown while the pointer is over the area or it
/// scrolls, a 4 point `border_muted` pill that turns `border_strong` under the pointer.
pub fn scroll_bar(p: &Pal) -> ScrollBarStyle {
    ScrollBarStyle::scroll_view(kit(p)).thumb(p.border_muted).thumb_hover(p.q.border_strong)
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
    let v = &p.v2;
    (c.bg_inverse, c.text_inverse, c.icon_inverse, c.icon_accent, c.border_focus) = (v.bg_inverse, v.text_inverse, v.icon_inverse, v.icon_accent, v.border_focus);
    for (state, theirs, fg) in [(&mut c.success, v.success, p.diff_add), (&mut c.danger, v.danger, p.diff_del), (&mut c.warning, v.warning, p.warning), (&mut c.info, v.info, p.info)] {
        *state = rune_kit::StateColors { fg, icon: fg, ..theirs };
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
