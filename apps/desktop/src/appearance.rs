//! Settings › Appearance, as Desktop has it (`settings/appearance/appearance.tsx`): colour
//! scheme, theme (`themes.rs`), interface zoom, Reduce motion (System / On / Off) and the
//! terminal font. Desktop's UI and code font rows wait on Rune: its sans and mono families are
//! fixed when the window opens.

use std::cell::Cell;

use rune::{Color, El, PopupAnchor, PopupMenu, Tx, icon, row, text};
use rune_theme_opencode::icons as ic;

use crate::App;
use crate::palette::Pal;
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
pub const TERMINAL_FONTS: [&str; 3] = ["Commit Mono", "IBM Plex Mono", "SF Mono"];

/// `--color-theme <id>` paints a run in one of Desktop's themes (`themes::all`).
pub fn apply_args(app: &mut App) {
    let args = rune::extra_args();
    if let Some(id) = args.iter().position(|a| a == "--color-theme").and_then(|i| args.get(i + 1)) {
        app.settings.theme = id.clone();
    }
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

    /// Every theme in a native menu over the Theme row's button, the current one checked.
    pub fn open_theme_menu(&mut self, tx: &mut Tx) {
        let themes = crate::themes::all();
        let menu = PopupMenu::picker(themes.iter().map(|t| (t.id.clone(), t.name.clone())), &themes[crate::themes::index(&self.settings.theme)].id);
        tx.popup_menu(menu, PopupAnchor::Key("theme-select".into()), |s: &mut App, picked, tx| {
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

/// The rows, in Desktop's order (its font rows come later).
pub fn rows(s: &App, p: &Pal) -> Vec<(&'static str, &'static str, &'static str, El)> {
    vec![
        ("appearance", "Color scheme", "Choose whether OpenCode follows the system, light, or dark theme", crate::settings::scheme_control(s, p)),
        ("theme", "Theme", "Customise how OpenCode is themed.", theme_select(s, p)),
        ("zoom", "Zoom", "Make text and controls larger across the app. ⌘+ ⌘− ⌘0 do the same.", crate::settings::zoom_control(s, p)),
        ("reduce-motion", "Reduce motion", "Reduce animations or match your system", reduce_motion_control(s, p)),
        ("terminal-font", "Terminal Font", "Customise the font used in the terminal", terminal_font_select(s, p)),
    ]
}

fn theme_select(s: &App, p: &Pal) -> El {
    let current = &crate::themes::all()[crate::themes::index(&s.settings.theme)];
    select(p, "theme-select", "Choose theme", &current.name).on_press_tx(|s: &mut App, tx: &mut Tx| s.open_theme_menu(tx))
}

fn terminal_font_select(s: &App, p: &Pal) -> El {
    let current = TERMINAL_FONTS[s.settings.terminal_font.min(TERMINAL_FONTS.len() - 1)];
    select(p, "terminal-font-select", "Choose terminal font", current).on_press_tx(|s: &mut App, tx: &mut Tx| {
        let menu = PopupMenu::picker(TERMINAL_FONTS.iter().enumerate().map(|(i, f)| (i.to_string(), *f)), &s.settings.terminal_font.to_string());
        tx.popup_menu(menu, PopupAnchor::Key("terminal-font-select".into()), |s: &mut App, picked, tx| {
            if let Some(i) = picked.and_then(|i| i.parse().ok()) {
                s.set_terminal_font(i, tx);
            }
        });
    })
}

/// Desktop's Select as a native pop-up button: the current choice and a chevron, opening the
/// choices with the current one checked over the button.
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
