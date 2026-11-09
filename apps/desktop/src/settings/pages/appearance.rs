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
use opencode_i18n::t;

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
        let named = |custom: &str, name: &str, default: bool| TextEdit::new(if !custom.is_empty() { custom } else if default { "" } else { name });
        Fields {
            ui: named(&s.ui_font_name, UI_FONTS[s.ui_font.min(UI_FONTS.len() - 1)].name, s.ui_font == 0),
            code: named(&s.code_font_name, CODE_FONTS[s.code_font.min(CODE_FONTS.len() - 1)].name, s.code_font == 0),
            terminal: named(&s.terminal_font_name, TERMINAL_FONTS[s.terminal_font.min(TERMINAL_FONTS.len() - 1)], s.terminal_font == 3),
        }
    }

    pub fn blur(&mut self) {
        self.ui.focused = false;
        self.code.focused = false;
        self.terminal.focused = false;
    }
}

/// The bundled face a typed name is: matched without case or spaces. Any other name is an
/// installed family's, which stands before the default.
fn find(names: &[&str], typed: &str) -> Option<usize> {
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    let typed = squash(typed);
    names.iter().position(|n| squash(n) == typed)
}

fn ui_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.ui.text.trim().to_owned();
    let names: Vec<&str> = UI_FONTS.iter().map(|f| f.name).collect();
    match if typed.is_empty() { Some(0) } else { find(&names, &typed) } {
        Some(i) => s.set_ui_font(i, tx),
        None => s.set_ui_font_named(0, &typed, tx),
    }
}

fn code_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.code.text.trim().to_owned();
    let names: Vec<&str> = CODE_FONTS.iter().map(|f| f.name).collect();
    match if typed.is_empty() { Some(0) } else { find(&names, &typed) } {
        Some(i) => s.set_code_font(i, tx),
        None => s.set_code_font_named(0, &typed, tx),
    }
}

fn terminal_changed(s: &mut App, tx: &mut Tx) {
    let typed = s.settings_pages.fonts.terminal.text.trim().to_owned();
    let found = if typed.is_empty() { Some(3) } else { find(&TERMINAL_FONTS, &typed).or_else(|| find(&[TERMINAL_DEFAULT, "CommitMono"], &typed).map(|i| [3, 0][i])) };
    match found {
        Some(i) => s.set_terminal_font(i, "", tx),
        None => s.set_terminal_font(3, &typed, tx),
    }
}

pub fn view(s: &App, p: &Pal) -> El {
    let st = &s.settings;
    let f = &s.settings_pages.fonts;
    let themes = crate::themes::all();
    let theme_about = row().child(about(p, &format!("{} ", t!("settings.general.row.theme.description")))).child(link(p, "settings-theme-docs", t!("common.learnMore"), "https://opencode.ai/docs/themes"));
    let zoom = (st.zoom_percent(s.zoom), ZOOM_LEVELS);
    let mut levels: Vec<(String, String)> = zoom.1.iter().map(|l| (format!("{}", (l * 100.0).round() as i32), opencode_i18n::percent(f64::from(*l)))).collect();
    if !levels.iter().any(|(id, _)| *id == zoom.0) {
        levels.push((zoom.0.clone(), opencode_i18n::percent(f64::from(s.zoom))));
        levels.sort_by_key(|(id, _)| id.parse::<i32>().unwrap_or(0));
    }
    let face = |family: Family| {
        let mut t = TextStyle::new(13.0, 440.0).lh(16.0).tracking(-0.04);
        t.family = family;
        t
    };
    let rows = vec![
        ("settings-color-scheme", setting(p, "settings-color-scheme", t!("settings.general.row.colorScheme.title"), t!("settings.general.row.colorScheme.description"),
            select(s, p, "settings-color-scheme-select", t!("settings.general.row.colorScheme.title"), Scheme::ALL.iter().map(|c| (c.name().to_owned(), c.label().to_owned())).collect(), st.scheme.name(), |s, id, _| s.set_scheme(Scheme::parse(id))))),
        ("settings-theme", setting_with(p, "settings-theme", t!("settings.general.row.theme.title"), theme_about,
            select(s, p, "settings-theme-select", t!("settings.general.row.theme.title"), themes.iter().map(|t| (t.id.clone(), t.name.clone())).collect(), &themes[crate::themes::index(&st.theme)].id, |s, id, tx| s.set_theme(id, tx)), true)),
        ("settings-ui-font", setting(p, "settings-ui-font", t!("settings.general.row.uiFont.title"), t!("settings.general.row.uiFont.description"),
            text_field(p, "settings-ui-font-input", t!("settings.general.row.uiFont.title"), UI_DEFAULT, &f.ui, face(Family::Sans), |s| &mut s.settings_pages.fonts.ui, ui_changed))),
        ("settings-code-font", setting(p, "settings-code-font", t!("settings.general.row.font.title"), t!("settings.general.row.font.description"),
            text_field(p, "settings-code-font-input", t!("settings.general.row.font.title"), CODE_DEFAULT, &f.code, face(Family::Mono), |s| &mut s.settings_pages.fonts.code, code_changed))),
        ("settings-terminal-font", setting(p, "settings-terminal-font", t!("settings.general.row.terminalFont.title"), t!("settings.general.row.terminalFont.description"),
            text_field(p, "settings-terminal-font-input", t!("settings.general.row.terminalFont.title"), TERMINAL_DEFAULT, &f.terminal, face(Family::Named(TERMINAL_DEFAULT)), |s| &mut s.settings_pages.fonts.terminal, terminal_changed))),
        ("settings-zoom", setting(p, "settings-zoom", t!("settings.appearance.row.zoom.title"), t!("settings.appearance.row.zoom.description"),
            select(s, p, "settings-zoom-select", t!("settings.appearance.row.zoom.title"), levels, &zoom.0, |s, id, tx| {
                tx.instant();
                s.set_zoom_level(id.parse::<f32>().unwrap_or(100.0) / 100.0);
            }))),
        ("settings-reduce-motion", setting(p, "settings-reduce-motion", t!("settings.appearance.row.reduceMotion.title"), t!("settings.appearance.row.reduceMotion.description"),
            select(s, p, "settings-reduce-motion-select", t!("settings.appearance.row.reduceMotion.title"), [ReduceMotion::System, ReduceMotion::On, ReduceMotion::Off].iter().map(|m| (m.name().to_owned(), m.label().to_owned())).collect(), st.reduce_motion.name(), |s, id, _| s.set_reduce_motion(ReduceMotion::parse(Some(id)))))),
    ];
    let list = settings::list(p, "appearance-list", super::rows(s, p, 20.0, rows));
    page(p, "settings-appearance-page", header(p, t!("settings.general.section.appearance"), t!("settings.appearance.description")), list)
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
