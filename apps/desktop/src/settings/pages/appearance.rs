//! Settings › Appearance (`settings/appearance/appearance.tsx`): Color scheme, Theme (with Learn
//! more), the UI, Code and Terminal fonts as 220 pt fields in their own faces, Zoom and Reduce
//! motion. A font field takes a family name; the names Rune can load (the faces the app bundles,
//! and SF Mono) apply as typed, an empty field is the default.

use rune::{Family, TextEdit, TextStyle, Tx, El, row};

use super::{header, page, select, text_field};
use crate::App;
use crate::appearance::{CODE_FONTS, ReduceMotion, TERMINAL_FONTS, UI_FONTS};
use crate::palette::Pal;
use crate::settings::{self, Scheme, about, link, setting, setting_with};

/// Desktop's defaults, shown as each field's placeholder (`sansDefault`, `monoDefault`, `terminalDefault`).
const UI_DEFAULT: &str = "Inter";
const CODE_DEFAULT: &str = "IBM Plex Mono";
const TERMINAL_DEFAULT: &str = "JetBrainsMono Nerd Font Mono";

use crate::shortcuts::ZOOM_LEVELS;

/// The three font fields as typed.
#[derive(Default)]
pub struct Fields {
    pub ui: TextEdit,
    pub code: TextEdit,
    pub terminal: TextEdit,
}

impl Fields {
    /// The fields for the saved fonts: empty for a default, its name otherwise.
    pub fn from(s: &settings::Settings) -> Fields {
        let named = |name: &str, default: bool| TextEdit::new(if default { "" } else { name });
        Fields {
            ui: named(UI_FONTS[s.ui_font.min(UI_FONTS.len() - 1)].name, s.ui_font == 0),
            code: named(CODE_FONTS[s.code_font.min(CODE_FONTS.len() - 1)].name, s.code_font == 0),
            terminal: named(TERMINAL_FONTS[s.terminal_font.min(TERMINAL_FONTS.len() - 1)], s.terminal_font == 3),
        }
    }

    pub fn blur(&mut self) {
        self.ui.focused = false;
        self.code.focused = false;
        self.terminal.focused = false;
    }
}

/// The face a typed name is, if Rune has it: matched without case or spaces.
fn find(names: &[&str], typed: &str) -> Option<usize> {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    let typed = squash(typed);
    names.iter().position(|n| squash(n) == typed)
}

fn ui_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.ui.text.trim().to_owned();
    let names: Vec<&str> = UI_FONTS.iter().map(|f| f.name).collect();
    if let Some(i) = if typed.is_empty() { Some(0) } else { find(&names, &typed) } {
        s.set_ui_font(i, tx);
    }
}

fn code_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.code.text.trim().to_owned();
    let names: Vec<&str> = CODE_FONTS.iter().map(|f| f.name).collect();
    if let Some(i) = if typed.is_empty() { Some(0) } else { find(&names, &typed) } {
        s.set_code_font(i, tx);
    }
}

fn terminal_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.terminal.text.trim().to_owned();
    let found = if typed.is_empty() { Some(3) } else { find(&TERMINAL_FONTS, &typed).or_else(|| find(&[TERMINAL_DEFAULT, "CommitMono"], &typed).map(|i| [3, 0][i])) };
    if let Some(i) = found {
        s.set_terminal_font(i, tx);
    }
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let f = &s.settings_pages.fonts;
    let themes = crate::themes::all();
    let theme_about = row().child(about(p, "Customise how OpenCode is themed. ")).child(link(p, "settings-theme-docs", "Learn more", "https://opencode.ai/v2/docs/themes"));
    let zoom = (st.zoom_percent(s.zoom), ZOOM_LEVELS);
    let mut levels: Vec<(String, String)> = zoom.1.iter().map(|l| (format!("{}", (l * 100.0).round() as i32), format!("{}%", (l * 100.0).round() as i32))).collect();
    if !levels.iter().any(|(id, _)| *id == zoom.0) {
        levels.push((zoom.0.clone(), format!("{}%", zoom.0)));
        levels.sort_by_key(|(id, _)| id.parse::<i32>().unwrap_or(0));
    }
    let face = |family: Family| {
        let mut t = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
        t.family = family;
        t
    };
    let rows = vec![
        ("settings-color-scheme", setting(p, "settings-color-scheme", "Color scheme", "Choose whether OpenCode follows the system, light, or dark theme",
            select(s, p, "settings-color-scheme-select", "Color scheme", Scheme::ALL.iter().map(|c| (c.name().to_owned(), c.label().to_owned())).collect(), st.scheme.name(), |s, id, _| s.set_scheme(Scheme::parse(id))))),
        ("settings-theme", setting_with(p, "settings-theme", "Theme", theme_about,
            select(s, p, "settings-theme-select", "Theme", themes.iter().map(|t| (t.id.clone(), t.name.clone())).collect(), &themes[crate::themes::index(&st.theme)].id, |s, id, tx| s.set_theme(id, tx)), true)),
        ("settings-ui-font", setting(p, "settings-ui-font", "UI Font", "Customise the font used throughout the interface",
            text_field(p, "settings-ui-font-input", "UI Font", UI_DEFAULT, &f.ui, face(Family::Sans), |s| &mut s.settings_pages.fonts.ui, ui_changed))),
        ("settings-code-font", setting(p, "settings-code-font", "Code Font", "Customise the font used in code blocks",
            text_field(p, "settings-code-font-input", "Code Font", CODE_DEFAULT, &f.code, face(Family::Mono), |s| &mut s.settings_pages.fonts.code, code_changed))),
        ("settings-terminal-font", setting(p, "settings-terminal-font", "Terminal Font", "Customise the font used in the terminal",
            text_field(p, "settings-terminal-font-input", "Terminal Font", TERMINAL_DEFAULT, &f.terminal, face(Family::Named(TERMINAL_DEFAULT)), |s| &mut s.settings_pages.fonts.terminal, terminal_changed))),
        ("settings-zoom", setting(p, "settings-zoom", "Zoom", "Make text and controls larger across the app",
            select(s, p, "settings-zoom-select", "Zoom", levels, &zoom.0, |s, id, tx| {
                tx.instant();
                s.set_zoom_level(id.parse::<f32>().unwrap_or(100.0) / 100.0);
            }))),
        ("settings-reduce-motion", setting(p, "settings-reduce-motion", "Reduce motion", "Reduce animations or match your system",
            select(s, p, "settings-reduce-motion-select", "Reduce motion", [ReduceMotion::System, ReduceMotion::On, ReduceMotion::Off].iter().map(|m| (m.name().to_owned(), m.label().to_owned())).collect(), st.reduce_motion.name(), |s, id, _| s.set_reduce_motion(ReduceMotion::parse(Some(id)))))),
    ];
    let list = settings::list(p, "appearance-list", super::rows(s, p, 20.0, rows));
    page(p, "settings-appearance-page", header(p, "Appearance", "Customize theme and fonts"), list)
}

impl settings::Settings {
    /// The zoom as a whole percentage, as the select names it.
    fn zoom_percent(&self, zoom: f32) -> String {
        format!("{}", (zoom * 100.0).round() as i32)
    }
}

impl App {
    /// Sets the window's zoom to `level` (Appearance › Zoom), clamped as the zoom keys clamp it.
    pub fn set_zoom_level(&mut self, level: f32) {
        self.zoom = level.clamp(crate::shortcuts::ZOOM_MIN, crate::shortcuts::ZOOM_MAX);
        self.zoom_pinch.leveled();
        if self.persists() {
            crate::tabs::save_zoom(self.zoom);
        }
    }
}
