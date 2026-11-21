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
    /// The system's contrast theme paints the window (`themes::contrast`): where official's look
    /// shows a highlight only as a faint wash, the app adds an edge in the text colour so it
    /// stands out at least 3:1.
    pub forced: bool,
    /// `themes::generation` when it was built: a plugin theme reloaded under the same index
    /// builds a new palette, and what memoizes on the theme keys on both.
    pub generation: u64,
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
    /// `--icon-interactive-base`: the code views' gutter `+`.
    pub interactive: Color,
    pub search_bg: Color,
    pub inline_code_bg: Color,
    pub selection: Color,
    /// A text field's selection: Chromium's default (`::selection` unstyled), white text on its
    /// blue while the window is active and #323232 on grey while it isn't.
    pub field_selection: rune::SelectionStyle,
    /// The timeline's selection (`quote-highlight.css`): `blue-1100` at 10% in light, `blue-400`
    /// at 25% in dark, the text keeping its colour.
    pub timeline_selection: Color,
    /// A cancelled background shell or subagent's notice: `yellow-900`, `state-fg-warning` in dark.
    pub cancelled: Color,
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
    /// `--icon-strong-base` and `--icon-weak-base`: OpenCode's mark (`logo.tsx` `Mark`), as an
    /// empty side pane draws it.
    pub v1_icon_strong: Color,
    pub v1_icon_weak: Color,
    /// The terminal's text: Desktop's `text-stronger` (`terminal.tsx:232`).
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
    /// `--text-diff-add-base` and `-delete-base`: a count's `+N -M` and a file's status letter.
    /// At run time they resolve to `state-fg-success` and `state-fg-danger`.
    pub add_text: Color,
    pub del_text: Color,
    /// `--icon-diff-add-base` and `-delete-base`: a file row's Created and Deleted
    /// (`message-part.css`, `[data-slot="apply-patch-change"]`), as they resolve at run time.
    pub add_icon: Color,
    pub del_icon: Color,
    /// A changed line's fill, its number cell's too.
    pub add_line: Color,
    pub del_line: Color,
    /// [`Self::add_line`] and [`Self::del_line`] laid over the diff's background, opaque, for a
    /// row drawn on a surface of another colour.
    pub add_row: Color,
    pub del_row: Color,
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
    /// An unchanged line under the pointer (Pierre's `lineHoverHighlight`: the diff's
    /// background mixed 3% toward black, 9% toward white in dark, in Lab). A changed line keeps
    /// its fill, which Pierre mixes toward itself.
    pub hover: Color,
    /// A selected line and its number (`--diffs-bg-selection`: `bg-accent` at 20%, over the
    /// diff's background), its number in `text-accent` (`--diffs-selection-number-fg`).
    pub selected: Color,
    /// The empty side of a split row (`--diffs-bg-context-gutter`, which session-ui puts on
    /// `[data-content-buffer]` in place of Pierre's hatching).
    pub empty: Color,
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
    /// `link_text` in Markdown.
    pub info: Color,
    pub operator: Color,
    pub regexp: Color,
}

impl Code {
    /// Desktop's highlight theme (`highlight-theme.tsx`) on rune-kit's token kinds: functions
    /// `syntax-primitive`, numbers and constants `syntax-constant`, properties and variables
    /// `syntax-variable`, attributes `syntax-property`, headings bold `syntax-keyword`.
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
            // `title` and `link_text`.
            (K::Heading, self.keyword),
            (K::Link, self.info),
            (K::Operator, self.operator),
            (K::Regex, self.regexp),
            (K::Attribute, self.property),
            (K::Variable, self.variable),
        ]
        .into_iter()
        .fold(base, |t, (k, c)| t.with(k, c))
        .with_weight(K::Heading, 700.0)
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

/// OpenCode's default theme, which every other lays its tokens over.
pub(crate) const THEME: &str = include_str!("../../../apps/desktop/assets/theme.json");

/// Token `name` of the active theme in this appearance (a Desktop CSS variable without its
/// `--`: `text-strong`, `background-bg-layer-04`), for what draws by name rather than through
/// [`Pal`]. Each theme is read once.
pub fn token(dark: bool, name: &str) -> Option<Color> {
    thread_local! {
        static THEMES: std::cell::RefCell<HashMap<(usize, u64, bool, bool), Theme>> = std::cell::RefCell::new(HashMap::new());
    }
    let theme = crate::themes::active();
    let key = (theme, crate::themes::generation(), dark, rune::theme::blends_srgb());
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

fn build(theme: usize, dark: bool, raised: bool) -> Pal {
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
    let hex = |h: &str| rune::color::hex(h).unwrap_or(text);
    // Resolved `oc-2` `--text-base` override (`oc-2.json:20,273`): `#6f6f6f` light / `#a0a0a0` dark.
    let v1_text_base = match (themed, dark) {
        (true, _) => c("text-text-muted"),
        (false, true) => hex("#a0a0a0"),
        (false, false) => hex("#6f6f6f"),
    };
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
        parameter: v1_text_base,
        // The default theme names no `syntax-info`; Desktop's resolves it to these.
        info: t.try_color("syntax-info").unwrap_or_else(|| rune::color::hex(if dark { "#facffe" } else { "#8c1896" }).unwrap_or(text)),
        operator: c("syntax-operator"),
        regexp: t.try_color("syntax-regexp").unwrap_or(v1_text_base),
    };
    Pal {
        dark,
        theme,
        // The contrast theme's highlights (edges where official has only a wash), also over any
        // theme when the system asks for more contrast.
        forced: raised || crate::themes::all().get(theme).is_some_and(|i| i.id == crate::themes::CONTRAST),
        generation: crate::themes::generation(),
        on_fill: Color::WHITE,
        dim: if themed { c("overlay-simple-overlay-scrim") } else { Color::BLACK.alpha(if dark { 0.5 } else { 0.25 }) },
        switch_thumb: if dark { rune::color::hex("#d9d9d9").unwrap_or(Color::WHITE) } else { Color::WHITE },
        lift: if dark { text } else { Color::WHITE },
        // Resolved `oc-2` `--text-stronger` (`terminal.tsx:232`, `themes.json:6,31`): `#171717` light / `#ededed` dark.
        term_fg: match (themed, dark) {
            (true, _) => c("text-stronger"),
            (false, true) => hex("#ededed"),
            (false, false) => hex("#171717"),
        },
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
        // Resolved `oc-2` `--text-base` and `--text-weak` overrides (`oc-2.json:20-21,273-274`).
        v1_text_base,
        v1_text_weak: match (themed, dark) {
            (true, _) => c("text-text-faint"),
            (false, true) => hex("#707070"),
            (false, false) => hex("#8f8f8f"),
        },
        // Resolved `oc-2` `--icon-warning-base` (`amber[isDark ? 8 : 6]`, `theme/resolve.ts:17,327`).
        v1_icon_warning: match (themed, dark) {
            (true, _) => c("state-fg-warning"),
            (false, true) => hex("#c78b05"),
            (false, false) => hex("#f9b13f"),
        },
        v1_icon_strong: match (themed, dark) {
            (true, _) => text,
            (false, true) => hex("#ededed"),
            (false, false) => hex("#171717"),
        },
        v1_icon_weak: match (themed, dark) {
            (true, _) => c("icon-icon-faint"),
            (false, true) => hex("#343434"),
            (false, false) => hex("#dbdbdb"),
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
        interactive: c("icon-interactive-base"),
        search_bg: t.alpha(layer2, 0.6),
        // color-mix(in oklch, text-base 8%, transparent)
        inline_code_bg: t.alpha(text, 0.08),
        selection: t.alpha(c("text-text-accent"), 0.3),
        field_selection: rune::SelectionStyle::chromium(),
        timeline_selection: if dark { t.alpha(c("blue-400"), 0.25) } else { t.alpha(c("blue-1100"), 0.1) },
        cancelled: if dark { c("state-fg-warning") } else { c("yellow-900") },
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
    let add_fill = if dark { ramp("green-800", add) } else { green };
    DiffInk {
        add_text: add,
        del_text: del,
        add_icon: if dark { hex("#c4ffc0", add) } else { hex("#167517", add) },
        del_icon: if dark { hex("#ec2f14", del) } else { hex("#fa3012", del) },
        add_line: t.alpha(add_fill, fill),
        del_line: t.alpha(red, fill),
        add_row: over(bg, add_fill, fill),
        del_row: over(bg, red, fill),
        add_word: t.alpha(green, word),
        del_word: t.alpha(red, word),
        add_word_text: if dark { text } else { ramp("green-1200", text) },
        del_word_text: if dark { text } else { ramp("red-1200", text) },
        add_bar: if dark { ramp("green-400", add) } else { ramp("green-900", add) },
        del_bar: if dark { ramp("red-500", del) } else { ramp("red-800", del) },
        separator: mix_lab(bg, mixer, if dark { 0.05 } else { 0.02 }),
        hover: mix_lab(bg, mixer, if dark { 0.09 } else { 0.03 }),
        selected: over(bg, t.color("background-bg-accent"), 0.2),
        // `--diffs-bg-context` (the background 1.5% toward the mixer, 7.5% in dark) mixed 90%
        // (45% in dark) into the background, in Lab.
        empty: mix_lab(bg, mixer, if dark { 0.075 * 0.45 } else { 0.015 * 0.9 }),
    }
}

/// `c` at opacity `a` laid over the opaque `bg` as Chromium paints it, in sRGB: Skia
/// premultiplies the colour into 8 bits, adds the backdrop's share and rounds, so 6% accent
/// over `#151515` comes out `#181a23` where exact arithmetic gives `#171922`. Opaque, so it
/// paints alike in either blend mode.
pub fn over(bg: Color, c: Color, a: f32) -> Color {
    use rune::color::{srgb_decode, srgb_encode};
    let m = |b: f32, f: f32| srgb_decode(((a * srgb_encode(f) * 255.0).round() + (1.0 - a) * srgb_encode(b) * 255.0).round() / 255.0);
    Color { r: m(bg.r, c.r), g: m(bg.g, c.g), b: m(bg.b, c.b), a: 1.0 }
}

/// `top` over the opaque `bottom`, blended in sRGB as a browser does, as one opaque colour.
pub fn flat(top: Color, bottom: Color) -> Color {
    let srgb = |c: Color| {
        let h = c.hex();
        [1, 3, 5].map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32)
    };
    let (t, b) = (srgb(top), srgb(bottom));
    let m = |i: usize| (t[i] * top.a + b[i] * (1.0 - top.a)).round() as u8;
    rune::color::hex(&format!("#{:02x}{:02x}{:02x}", m(0), m(1), m(2))).unwrap_or(bottom)
}

/// `t` of the way from `a` to `b` in CIE Lab, as CSS `color-mix(in lab, a, b t)` mixes:
/// session-ui's `--diffs-bg-separator-override`.
pub(crate) fn mix_lab(a: Color, b: Color, t: f32) -> Color {
    const M: [[f32; 3]; 3] = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
    const INV: [[f32; 3]; 3] = [[3.2406, -1.5372, -0.4986], [-0.9689, 1.8758, 0.0415], [0.0557, -0.2040, 1.0570]];
    const WHITE: [f32; 3] = [0.9505, 1.0, 1.089];
    let f = |v: f32| if v > 0.008856 { v.cbrt() } else { 7.787 * v + 16.0 / 116.0 };
    let finv = |v: f32| if v.powi(3) > 0.008856 { v.powi(3) } else { (v - 16.0 / 116.0) / 7.787 };
    let lab = |c: Color| {
        let xyz = M.map(|r| r[0] * c.r + r[1] * c.g + r[2] * c.b);
        let [x, y, z] = [0, 1, 2].map(|i| f(xyz[i] / WHITE[i]));
        [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
    };
    let (la, lb) = (lab(a), lab(b));
    let [l, aa, bb] = [0, 1, 2].map(|i| la[i] + (lb[i] - la[i]) * t);
    let y = (l + 16.0) / 116.0;
    let xyz = [finv(y + aa / 500.0) * WHITE[0], finv(y) * WHITE[1], finv(y - bb / 200.0) * WHITE[2]];
    let [r, g, b2] = INV.map(|r| (r[0] * xyz[0] + r[1] * xyz[1] + r[2] * xyz[2]).clamp(0.0, 1.0));
    Color { r, g, b: b2, a: a.a + (b.a - a.a) * t }
}
/// The tokens for an appearance in this thread's theme (`themes::active`), built once each.
pub fn pal(dark: bool) -> &'static Pal {
    type PalKey = (usize, u64, bool, bool, bool);
    static PALS: [[[OnceLock<Pal>; 2]; 2]; 2] = [const { [const { [const { OnceLock::new() }; 2] }; 2] }; 2];
    static THEMED: Mutex<Vec<(PalKey, &'static Pal)>> = Mutex::new(Vec::new());
    let srgb = rune::theme::blends_srgb();
    let theme = crate::themes::active();
    let raised = crate::themes::raised_contrast();
    if theme == 0 {
        return PALS[dark as usize][srgb as usize][raised as usize].get_or_init(|| rune::theme::with_blend(srgb, || build(0, dark, raised)));
    }
    let key = (theme, crate::themes::generation(), dark, srgb, raised);
    let mut themed = THEMED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = themed.iter().find(|(k, _)| *k == key) {
        return p;
    }
    let p: &'static Pal = Box::leak(Box::new(rune::theme::with_blend(srgb, || build(theme, dark, raised))));
    themed.push((key, p));
    p
}

/// The one rune-kit Kit, for markdown, code, diffs and the odometer: OpenCode's kit, its
/// colours taken from `p` in a chosen theme so it follows the theme (the default keeps it as is). Built once per theme, appearance and blend.
pub fn kit(p: &Pal) -> &'static Kit {
    type KitKey = (usize, u64, bool, bool);
    static KITS: Mutex<Vec<(KitKey, &'static Kit)>> = Mutex::new(Vec::new());
    let key = (p.theme, p.generation, p.dark, rune::theme::blends_srgb());
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
    // The kit's controls ring in the app's `border-focus` (`assets/theme.json`), the same as
    // `crate::focus`, in the default theme too.
    k.c.border_focus = p.v2.border_focus;
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
    (c.bg_inverse, c.text_inverse, c.icon_inverse, c.icon_accent) = (v.bg_inverse, v.text_inverse, v.icon_inverse, v.icon_accent);
    for (state, theirs, fg) in [(&mut c.success, v.success, p.diff_add), (&mut c.danger, v.danger, p.diff_del), (&mut c.warning, v.warning, p.warning), (&mut c.info, v.info, p.info)] {
        *state = rune_kit::StateColors { fg, icon: fg, ..theirs };
    }
    // The system contrast theme (`forced-colors: active`): fields draw a text-colour edge and
    // focus as a ring in its highlight, the accent there (opencode#65100).
    k.forced = crate::themes::all().get(p.theme).is_some_and(|i| i.id == crate::themes::CONTRAST).then_some(p.bg_accent);
    k
}

