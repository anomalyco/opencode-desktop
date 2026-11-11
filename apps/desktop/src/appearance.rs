//! Settings › Appearance, as Desktop has it (`settings/appearance/appearance.tsx`): colour
//! scheme, theme (`themes.rs`), the UI, code and terminal fonts, interface zoom and Reduce
//! motion (System / On / Off). The font rows take any family name, as Desktop's do: one of the
//! faces the app bundles picks it, and any other installed family stands before the default,
//! which Rune loads from the system by name.

use std::cell::Cell;

use rune::Tx;

use crate::App;
use crate::popup::{self, Menu};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReduceMotion {
    #[default]
    System,
    On,
    Off,
}

impl ReduceMotion {
    const ALL: [ReduceMotion; 3] = [ReduceMotion::System, ReduceMotion::On, ReduceMotion::Off];

    pub fn name(self) -> &'static str {
        match self { Self::System => "system", Self::On => "on", Self::Off => "off" }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => opencode_i18n::t!("settings.appearance.row.reduceMotion.system"),
            Self::On => opencode_i18n::t!("settings.appearance.row.reduceMotion.on"),
            Self::Off => opencode_i18n::t!("settings.appearance.row.reduceMotion.off"),
        }
    }

    pub fn parse(s: Option<&str>) -> ReduceMotion {
        Self::ALL.into_iter().find(|m| Some(m.name()) == s).unwrap_or_default()
    }
}

/// Where `App::reduced_motion` comes from: `--reduced-motion`, the setting and macOS's
/// Reduce motion, which only the view can read (`note_system`).
#[derive(Debug, Default)]
pub struct Motion {
    flag: bool,
    system: Cell<bool>,
    applied: bool,
}

impl Motion {
    pub fn new(flag: bool) -> Motion {
        Motion { flag, ..Motion::default() }
    }

    /// The view's reading of the system preference, for the next poll.
    pub fn note_system(&self, on: bool) {
        self.system.set(on);
    }
}

/// The Terminal font knob's choices (`terminal::TERM_FONT`), which the setting picks.
pub const TERMINAL_FONTS: [&str; 4] = ["Commit Mono", "IBM Plex Mono", "SF Mono", "JetBrains Mono"];

/// A font a row offers: its name, and the family list behind Rune's sans or mono slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Font {
    pub name: &'static str,
    pub family: &'static str,
}

const fn font(name: &'static str, family: &'static str) -> Font {
    Font { name, family }
}

/// The UI font row's choices, Desktop's default (Inter) first.
pub const UI_FONTS: [Font; 4] = [
    font("Inter", "Inter"),
    font("SF Pro", "System Font, Inter"),
    font("IBM Plex Mono", "IBM Plex Mono, Inter"),
    font("Commit Mono", "CommitMono, Inter"),
];

/// The code font row's choices, Desktop's default (IBM Plex Mono) first.
pub const CODE_FONTS: [Font; 3] = [
    font("IBM Plex Mono", "IBM Plex Mono"),
    font("Commit Mono", "CommitMono, IBM Plex Mono"),
    font("SF Mono", "ui-monospace, IBM Plex Mono"),
];

/// The index of the font named `name` in `fonts`, the default when there's none.
pub fn font_index(fonts: &[Font], name: Option<&str>) -> usize {
    fonts.iter().position(|f| Some(f.name) == name).unwrap_or(0)
}

/// Experimental › New icons: the new artwork in place of each icon that has it (`setNewIcons`).
pub fn apply_icons(settings: &crate::settings::Settings) {
    let swap: fn(&rune::Icon) -> Option<&'static rune::Icon> = rune_theme_opencode::new_icons::new_icon;
    rune::text::set_icon_swap(settings.new_icons.then_some(swap));
    // The new artwork is drawn on a 16 px grid, so a small icon it replaces stays 16 rather than
    // shrinking to 14 (`icon.tsx`'s `pixelSize`).
    let size: fn(f32) -> f32 = |size| if size == 14.0 { 16.0 } else { size };
    rune::text::set_icon_swap_size(settings.new_icons.then_some(size));
}

/// The name typed for a font row when it isn't one of `fonts` (an installed family); empty
/// for one of them or none.
pub fn custom_name(fonts: &[Font], name: Option<&str>) -> String {
    let name = name.map(str::trim).unwrap_or("");
    if fonts.iter().any(|f| f.name == name) { String::new() } else { name.to_owned() }
}

/// `name` in a CSS family list (Desktop's settings `family`): bare when it is one word,
/// quoted otherwise.
pub fn css_family(name: &str) -> String {
    if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return name.to_owned();
    }
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The sans and mono families `settings` picks, as `AppSpec::families` and
/// `Tx::set_font_families` take them: a typed family before the default (`sansFontFamily`).
pub fn families(settings: &crate::settings::Settings) -> (String, String) {
    let pick = |custom: &str, chosen: &Font, base: &Font| if custom.is_empty() { chosen.family.to_owned() } else { format!("{}, {}", css_family(custom), base.family) };
    (
        pick(&settings.ui_font_name, &UI_FONTS[settings.ui_font.min(UI_FONTS.len() - 1)], &UI_FONTS[0]),
        pick(&settings.code_font_name, &CODE_FONTS[settings.code_font.min(CODE_FONTS.len() - 1)], &CODE_FONTS[0]),
    )
}

/// The families a window opens with: the saved settings (a fixture run's are the defaults),
/// with `--ui-font` and `--code-font` applied as `apply_args` applies them.
pub fn launch_families() -> (&'static str, &'static str) {
    let mut settings = if crate::arg("--fixture").is_some() { Default::default() } else { crate::settings::load() };
    apply_font_args(&mut settings);
    let (sans, mono) = families(&settings);
    // Once per launch: the window's spec keeps them for the app's life.
    (Box::leak(sans.into_boxed_str()), Box::leak(mono.into_boxed_str()))
}

/// `--ui-font <name>` and `--code-font <name>` pick a row's font by name ("SF Pro").
fn apply_font_args(settings: &mut crate::settings::Settings) {
    if let Some(name) = crate::arg("--ui-font") {
        settings.ui_font = font_index(&UI_FONTS, Some(&name));
    }
    if let Some(name) = crate::arg("--code-font") {
        settings.code_font = font_index(&CODE_FONTS, Some(&name));
    }
}

/// `--color-theme <id>` paints a run in one of Desktop's themes (`themes::all`), `--scheme
/// light|dark|system` in one of its modes; `--ui-font` and `--code-font` pick its fonts;
/// `--text-renderer <name>` sets Text rendering (`text_rendering::TextRenderer::name`).
pub fn apply_args(app: &mut App) {
    let args = rune::extra_args();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    if let Some(id) = value("--color-theme") {
        app.settings.theme = id.clone();
    }
    if let Some(name) = value("--scheme") {
        app.settings.scheme = crate::settings::Scheme::parse(name);
    }
    if let Some(name) = value("--text-renderer") {
        app.settings.text_renderer = crate::text_rendering::TextRenderer::parse(Some(name));
    }
    apply_font_args(&mut app.settings);
}

/// Picks up a change in macOS's Reduce motion.
pub fn poll(s: &mut App) -> bool {
    let system = s.motion.system.get();
    if system == s.motion.applied {
        return false;
    }
    s.motion.applied = system;
    s.apply_reduce_motion()
}

impl App {
    /// Applies saved appearance settings that live outside `Settings`: the theme, the language,
    /// Reduce motion and the terminal font knob.
    pub fn apply_appearance(&mut self) {
        crate::language::apply(self);
        crate::themes::set_active(&self.settings.theme);
        crate::keymap::overrides::apply(&self.settings.keybinds);
        self.apply_reduce_motion();
        self.knobs.set(&crate::terminal::TERM_FONT, self.settings.terminal_font as f32);
        crate::terminal::set_custom_font(&self.settings.terminal_font_name);
        apply_icons(&self.settings);
        rune::spell::set_languages(&self.settings.spellcheck_languages);
    }

    /// The terminal's font: choice `i`, or `name`, an installed family, before the default.
    pub fn set_terminal_font(&mut self, i: usize, name: &str, tx: &mut Tx) {
        tx.instant();
        let name = name.to_owned();
        crate::terminal::set_custom_font(&name);
        self.change_settings(|s| {
            s.terminal_font = i;
            s.terminal_font_name = name;
        });
        self.knobs.set(&crate::terminal::TERM_FONT, i as f32);
    }

    /// Switches the UI font in this frame: every window lays its text out again in it.
    pub fn set_ui_font(&mut self, i: usize, tx: &mut Tx) {
        self.set_ui_font_named(i, "", tx);
    }

    /// The UI font: choice `i`, or `name`, an installed family, before the default.
    pub fn set_ui_font_named(&mut self, i: usize, name: &str, tx: &mut Tx) {
        tx.instant();
        let name = name.to_owned();
        self.change_settings(|s| {
            s.ui_font = i.min(UI_FONTS.len() - 1);
            s.ui_font_name = name;
        });
        let (sans, mono) = families(&self.settings);
        tx.set_font_families(sans, mono);
    }

    /// Switches the code font in this frame.
    pub fn set_code_font(&mut self, i: usize, tx: &mut Tx) {
        self.set_code_font_named(i, "", tx);
    }

    /// The code font: choice `i`, or `name`, an installed family, before the default.
    pub fn set_code_font_named(&mut self, i: usize, name: &str, tx: &mut Tx) {
        tx.instant();
        let name = name.to_owned();
        self.change_settings(|s| {
            s.code_font = i.min(CODE_FONTS.len() - 1);
            s.code_font_name = name;
        });
        let (sans, mono) = families(&self.settings);
        tx.set_font_families(sans, mono);
    }

    /// Sets `reduced_motion` from the flag, the setting and the system; true when it changed.
    pub fn apply_reduce_motion(&mut self) -> bool {
        let on = self.motion.flag
            || match self.settings.reduce_motion {
                ReduceMotion::System => self.motion.applied,
                ReduceMotion::On => true,
                ReduceMotion::Off => false,
            };
        let changed = on != self.reduced_motion;
        self.reduced_motion = on;
        changed
    }

    pub fn set_reduce_motion(&mut self, m: ReduceMotion) {
        self.change_settings(|s| s.reduce_motion = m);
        self.apply_reduce_motion();
    }

    /// Every theme in a menu from the Theme row's button, the current one checked.
    pub fn open_theme_menu(&mut self, tx: &mut Tx) {
        let themes = crate::themes::all();
        let menu = Menu::picker(opencode_i18n::t!("settings.general.row.theme.title"), themes.iter().map(|t| (t.id.clone(), t.name.clone())), &themes[crate::themes::index(&self.settings.theme)].id);
        popup::open(self, menu, "theme-select", tx, |s: &mut App, picked, tx| {
            if let Some(id) = picked {
                s.set_theme(id, tx);
            }
        });
    }

    /// Swaps the theme in this frame, as Desktop does: no crossfade.
    pub fn set_theme(&mut self, id: &str, tx: &mut Tx) {
        tx.instant();
        let id = crate::themes::all()[crate::themes::index(id)].id.clone();
        crate::themes::set_active(&id);
        self.change_settings(|s| s.theme = id);
    }
}

