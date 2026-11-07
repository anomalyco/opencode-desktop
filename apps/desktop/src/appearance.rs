//! Settings › Appearance, as Desktop has it (`settings/appearance/appearance.tsx`): colour
//! scheme, theme (`themes.rs`), the UI, code and terminal fonts, interface zoom and Reduce
//! motion (System / On / Off). Desktop's font rows take any family name; here they pick from
//! the faces the app bundles or macOS always has, since Rune loads only those.

use std::borrow::Cow;
use std::cell::Cell;

use rune::{Color, El, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
use crate::popup::{self, Menu};
use crate::view::UI;

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

    fn label(self) -> &'static str {
        match self { Self::System => "System", Self::On => "On", Self::Off => "Off" }
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

/// The sans and mono families `settings` picks, as `AppSpec::families` and
/// `Tx::set_font_families` take them.
pub fn families(settings: &crate::settings::Settings) -> (&'static str, &'static str) {
    (UI_FONTS[settings.ui_font.min(UI_FONTS.len() - 1)].family, CODE_FONTS[settings.code_font.min(CODE_FONTS.len() - 1)].family)
}

/// The families a window opens with: the saved settings (a fixture run's are the defaults),
/// with `--ui-font` and `--code-font` applied as `apply_args` applies them.
pub fn launch_families() -> (&'static str, &'static str) {
    let mut settings = if crate::arg("--fixture").is_some() { Default::default() } else { crate::settings::load() };
    apply_font_args(&mut settings);
    families(&settings)
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

/// `--color-theme <id>` paints a run in one of Desktop's themes (`themes::all`); `--ui-font`
/// and `--code-font` pick its fonts.
pub fn apply_args(app: &mut App) {
    let args = rune::extra_args();
    if let Some(id) = args.iter().position(|a| a == "--color-theme").and_then(|i| args.get(i + 1)) {
        app.settings.theme = id.clone();
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
    /// Applies saved appearance settings that live outside `Settings`: the theme, Reduce
    /// motion and the terminal font knob.
    pub fn apply_appearance(&mut self) {
        crate::themes::set_active(&self.settings.theme);
        self.apply_reduce_motion();
        self.knobs.set(&crate::terminal::TERM_FONT, self.settings.terminal_font as f32);
    }

    pub fn set_terminal_font(&mut self, i: usize, tx: &mut Tx) {
        tx.instant();
        self.change_settings(|s| s.terminal_font = i);
        self.knobs.set(&crate::terminal::TERM_FONT, i as f32);
    }

    /// Switches the UI font in this frame: every window lays its text out again in it.
    pub fn set_ui_font(&mut self, i: usize, tx: &mut Tx) {
        tx.instant();
        self.change_settings(|s| s.ui_font = i.min(UI_FONTS.len() - 1));
        let (sans, mono) = families(&self.settings);
        tx.set_font_families(sans, mono);
    }

    /// Switches the code font in this frame.
    pub fn set_code_font(&mut self, i: usize, tx: &mut Tx) {
        tx.instant();
        self.change_settings(|s| s.code_font = i.min(CODE_FONTS.len() - 1));
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
        let menu = Menu::picker("Theme", themes.iter().map(|t| (t.id.clone(), t.name.clone())), &themes[crate::themes::index(&self.settings.theme)].id);
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

/// The rows, in Desktop's order.
pub fn rows(s: &App, p: &Pal) -> Vec<(&'static str, &'static str, Cow<'static, str>, El)> {
    vec![
        ("appearance", "Color scheme", "Choose whether OpenCode follows the system, light, or dark theme".into(), crate::settings::scheme_control(s, p)),
        ("theme", "Theme", "Customise how OpenCode is themed.".into(), theme_select(s, p)),
        ("ui-font", "UI Font", "Customise the font used throughout the interface".into(), ui_font_select(s, p)),
        ("code-font", "Code Font", "Customise the font used in code blocks".into(), code_font_select(s, p)),
        ("terminal-font", "Terminal Font", "Customise the font used in the terminal".into(), terminal_font_select(s, p)),
        ("zoom", "Zoom", zoom_about().into(), crate::settings::zoom_control(s, p)),
        ("reduce-motion", "Reduce motion", "Reduce animations or match your system".into(), reduce_motion_control(s, p)),
    ]
}

/// "⌘+ ⌘− ⌘0 do the same", in this platform's keys.
fn zoom_about() -> String {
    use crate::keymap::{Bind, label};
    let chord = |b: Bind, i: usize| b.keybind().chords().get(i).map_or_else(String::new, |c| label(*c));
    format!("Make text and controls larger across the app. {} {} {} do the same.", chord(Bind::ZoomIn, 1), chord(Bind::ZoomOut, 0).replace('-', "−"), chord(Bind::ZoomReset, 0))
}

fn ui_font_select(s: &App, p: &Pal) -> El {
    font_select(p, "ui-font-select", "Choose UI font", &UI_FONTS, s.settings.ui_font, App::set_ui_font)
}

fn code_font_select(s: &App, p: &Pal) -> El {
    font_select(p, "code-font-select", "Choose code font", &CODE_FONTS, s.settings.code_font, App::set_code_font)
}

/// A font row's select: the current font's name, opening the choices over the button.
fn font_select(p: &Pal, key: &'static str, label: &'static str, fonts: &'static [Font], current: usize, set: fn(&mut App, usize, &mut Tx)) -> El {
    let current = current.min(fonts.len() - 1);
    select(p, key, label, fonts[current].name).on_press_tx(move |s: &mut App, tx: &mut Tx| {
        let menu = Menu::picker(label, fonts.iter().enumerate().map(|(i, f)| (i.to_string(), f.name)), &current.to_string());
        popup::open(s, menu, key, tx, move |s: &mut App, picked, tx| {
            if let Some(i) = picked.and_then(|i| i.parse().ok()) {
                set(s, i, tx);
            }
        });
    })
}

fn theme_select(s: &App, p: &Pal) -> El {
    let current = &crate::themes::all()[crate::themes::index(&s.settings.theme)];
    select(p, "theme-select", "Choose theme", &current.name).on_press_tx(|s: &mut App, tx: &mut Tx| s.open_theme_menu(tx))
}

fn terminal_font_select(s: &App, p: &Pal) -> El {
    let current = TERMINAL_FONTS[s.settings.terminal_font.min(TERMINAL_FONTS.len() - 1)];
    select(p, "terminal-font-select", "Choose terminal font", current).on_press_tx(|s: &mut App, tx: &mut Tx| {
        let menu = Menu::picker("Terminal font", TERMINAL_FONTS.iter().enumerate().map(|(i, f)| (i.to_string(), *f)), &s.settings.terminal_font.to_string());
        popup::open(s, menu, "terminal-font-select", tx, |s: &mut App, picked, tx| {
            if let Some(i) = picked.and_then(|i| i.parse().ok()) {
                s.set_terminal_font(i, tx);
            }
        });
    })
}

/// Desktop's Select: the current choice and a chevron, opening the choices with the current
/// one checked (a native pop-up button on macOS, a drawn list below it elsewhere).
fn select(p: &Pal, key: &'static str, label: &'static str, current: &str) -> El {
    row()
        .key(key)
        .role("button")
        .label(label)
        .h(28.0)
        .pl(10.0)
        .pr(6.0)
        .gap(4.0)
        .center_y()
        .radius(6.0)
        .fill(p.layer2)
        .hover(p.hover)
        .child(text(current.to_owned()).font(UI).color(p.text))
        .child(icon(&ic::CHEVRON_DOWN, 16.0).color(p.icon_muted))
}

fn reduce_motion_control(s: &App, p: &Pal) -> El {
    let mut r = row().key("reduce-motion-control").gap(2.0).padding(2.0).radius(6.0).fill(p.layer2);
    for m in ReduceMotion::ALL {
        let on = s.settings.reduce_motion == m;
        r = r.child(
            row()
                .key(format!("reduce-motion-{}", m.name()))
                .role("button")
                .h(24.0)
                .px(10.0)
                .center()
                .radius(4.0)
                .fill(if on { p.bg_base } else { Color::TRANSPARENT })
                .hover(if on { p.bg_base } else { p.hover })
                .on_press(move |s: &mut App| s.set_reduce_motion(m))
                .child(text(m.label()).font(UI).color(if on { p.text } else { p.muted })),
        );
    }
    r
}

#[cfg(test)]
#[path = "appearance_tests.rs"]
mod tests;
