use rune::{Color, Runtime};
use serde_json::Value;

use super::*;
use crate::fixture_runtime as runtime;
use crate::palette::{Pal, pal};
use crate::themes;

fn frames(rt: &mut Runtime<App>, n: usize) {
    for _ in 0..n {
        rt.frame(1.0 / 60.0);
    }
}

fn tap(rt: &mut Runtime<App>, target: &str) {
    let (x, y) = rt.find(target).unwrap_or_else(|| panic!("{target} is on screen"));
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    frames(rt, 1);
}

/// The fixture on Settings, in a window tall enough to show every row.
fn settings() -> Runtime<App> {
    let mut app = App::fixture_session();
    app.open_settings();
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 2000.0), rune::Env::new(true, 1.0));
    rt.set_srgb_blend(true);
    rt.poll = Some(crate::poll);
    frames(&mut rt, 2);
    rt
}

/// Theme `id`'s colour `token` in `mode`, as Desktop resolved it into themes.json.
fn desktop(id: &str, mode: &str, token: &str) -> Color {
    let list: Vec<Value> = serde_json::from_str(include_str!("../assets/themes.json")).unwrap();
    let t = list.iter().find(|t| t["id"] == id).unwrap();
    rune::color::hex(t[mode][token].as_str().unwrap()).unwrap()
}

fn srgb_pal(dark: bool) -> Pal {
    rune::theme::with_blend(true, || *pal(dark))
}

#[test]
fn the_default_theme_paints_as_before() {
    themes::set_active(themes::DEFAULT);
    for dark in [false, true] {
        let p = srgb_pal(dark);
        assert_eq!(p.theme, 0);
        assert!(p.code.is_none(), "code keeps rune-kit's colours");
        assert_eq!((p.term_fg, p.diff_add, p.diff_del), (p.text, p.success, p.danger));
    }
    let t = rune::Theme::from_json(include_str!("../assets/theme.json"), true).unwrap();
    assert_eq!(srgb_pal(true).bg_base, rune::theme::with_blend(true, || t.color("background-bg-base")));
}

#[test]
fn a_theme_paints_desktops_colours() {
    themes::set_active("tokyonight");
    for (dark, mode) in [(false, "light"), (true, "dark")] {
        let p = srgb_pal(dark);
        assert_eq!(p.bg_base, desktop("tokyonight", mode, "background-bg-base"));
        assert_eq!(p.text, desktop("tokyonight", mode, "text-text-base"));
        assert_eq!(p.term_fg, desktop("tokyonight", mode, "text-stronger"));
        assert_eq!(p.diff_add, desktop("tokyonight", mode, "syntax-diff-add"));
        let code = p.code.expect("a theme brings its code colours");
        assert_eq!(code.keyword, desktop("tokyonight", mode, "syntax-keyword"));
        assert_eq!(code.comment, desktop("tokyonight", mode, "syntax-comment"));
    }
    themes::set_active(themes::DEFAULT);
}

#[test]
fn the_theme_menu_lists_every_theme_and_swaps_at_once() {
    let mut rt = settings();
    tap(&mut rt, "theme-select");
    let menu = rt.popup_menu().expect("the Theme button opens a menu");
    assert!(menu.find(themes::DEFAULT).is_some_and(|i| i.is_checked()));
    assert_eq!(menu.find("dracula").map(|i| i.title()), Some("Dracula"));
    rt.resolve_popup(Some("dracula")).unwrap();
    frames(&mut rt, 1);
    assert_eq!(rt.state.settings.theme, "dracula");
    assert_eq!(themes::all()[themes::active()].id, "dracula");
    assert!(rt.elements().iter().any(|e| e.text.as_deref() == Some("Dracula")), "the button names the new theme");
}

#[test]
fn reduce_motion_follows_the_setting_the_flag_and_the_system() {
    let mut rt = settings();
    tap(&mut rt, "reduce-motion-on");
    assert_eq!(rt.state.settings.reduce_motion, ReduceMotion::On);
    assert!(rt.state.reduced_motion);
    tap(&mut rt, "reduce-motion-system");
    assert!(!rt.state.reduced_motion, "the headless system doesn't reduce motion");
    rt.state.motion.note_system(true);
    assert!(poll(&mut rt.state));
    assert!(rt.state.reduced_motion, "System follows macOS");
    rt.state.set_reduce_motion(ReduceMotion::Off);
    assert!(!rt.state.reduced_motion);
    rt.state.motion = Motion::new(true);
    rt.state.apply_reduce_motion();
    assert!(rt.state.reduced_motion, "--reduced-motion wins over Off");
}

#[test]
fn the_terminal_font_row_sets_the_terminal_font() {
    let mut rt = runtime();
    rt.state.set_terminal_font(2, &mut Tx::default());
    assert_eq!(rt.state.knobs.get(&crate::terminal::TERM_FONT), 2.0);
    assert_eq!(rt.state.settings.terminal_font, 2);
    assert_eq!(ReduceMotion::parse(Some("on")), ReduceMotion::On);
    assert_eq!(ReduceMotion::parse(None), ReduceMotion::System);
}

#[test]
fn markdown_code_diffs_and_the_odometer_share_one_kit_that_follows_the_theme() {
    let looks = |id: &str| {
        themes::set_active(id);
        let p = srgb_pal(true);
        let k = rune::theme::with_blend(true, || crate::palette::kit(&p).clone());
        let md = rune_kit::MarkdownStyle::from_kit(&k);
        let code = crate::code_view::CodeLook::new(&p);
        (k.c.text_base, k.c.border_muted, format!("{md:?}"), k.c.success.icon, code.syntax.color(rune_kit::syntax::Syntax::Keyword), code.add.bar)
    };
    let (default, dracula) = (looks(themes::DEFAULT), looks("dracula"));
    themes::set_active(themes::DEFAULT);
    assert_ne!(default.0, dracula.0, "the kit's text (the odometer's) follows the theme");
    assert_ne!(default.2, dracula.2, "markdown's look");
    assert_ne!(default.3, dracula.3, "markdown's diff colours");
    assert_ne!(default.4, dracula.4, "code");
    assert_ne!(default.5, dracula.5, "diffs");
    assert_eq!(dracula.4, desktop("dracula", "dark", "syntax-keyword"));
    let p = srgb_pal(true);
    let before = rune::theme::with_blend(true, || rune_kit::syntax::SyntaxTheme::from_kit(rune_theme_opencode::kit(true)));
    assert_eq!(format!("{:?}", crate::code_view::CodeLook::new(&p).syntax), format!("{before:?}"), "the default's code keeps its colours");
}

/// `app` in a window with the app's own fonts and families, as it launches.
fn launched(app: App, scale: f32) -> Runtime<App> {
    let spec = crate::spec(App::empty);
    let text = rune::text::TextSystem::new(spec.fonts, spec.families);
    let mut rt = Runtime::with_text(app, crate::view::app, (1280.0, 820.0), rune::Env::new(true, scale), text);
    rt.poll = Some(crate::poll);
    frames(&mut rt, 2);
    rt
}

/// Runs `set` as a handler and draws the frame after it; the rows the timeline built.
fn switch(rt: &mut Runtime<App>, set: impl FnOnce(&mut App, &mut Tx)) -> usize {
    crate::view::timeline_reset_row_builds();
    rt.update(set);
    frames(rt, 1);
    crate::view::timeline_row_builds()
}

/// Scopes `rune::text::measure` on this thread to the launch families, so a switch here moves
/// only this test's measuring and the font tests run in parallel with every other test.
fn own_families() -> rune::text::FamiliesGuard {
    rune::text::with_families(UI_FONTS[0].family, CODE_FONTS[0].family)
}

#[test]
fn picking_a_font_again_rebuilds_nothing_and_a_new_one_lays_out_again() {
    let _g = own_families();
    let mut rt = launched(App::fixture_session(), 1.0);
    assert_eq!(rt.font_families(), (UI_FONTS[0].family, CODE_FONTS[0].family));
    // Any update builds the few rows that aren't memoized (the open session's last turn).
    let live = switch(&mut rt, |_, _| {});
    let epoch = rune::text::font_epoch();
    assert_eq!(switch(&mut rt, |s, tx| s.set_code_font(0, tx)), live, "the same code font rebuilds no more than any update");
    assert_eq!(switch(&mut rt, |s, tx| s.set_ui_font(0, tx)), live, "nor the same UI font");
    assert_eq!(rune::text::font_epoch(), epoch, "and changes no measured size");
    let built = switch(&mut rt, |s, tx| s.set_code_font(1, tx));
    assert_eq!(rt.font_families(), ("Inter", "CommitMono, IBM Plex Mono"));
    assert!(built > live, "a new code font builds the memoized rows again: {built} vs {live}");
    let epoch = rune::text::font_epoch();
    let built = switch(&mut rt, |s, tx| s.set_ui_font(1, tx));
    assert_eq!(rt.font_families(), ("System Font, Inter", "CommitMono, IBM Plex Mono"));
    assert!(built > live, "a new UI font builds the memoized rows again: {built} vs {live}");
    assert_ne!(rune::text::font_epoch(), epoch, "and measures in it");
    assert_eq!((rt.state.settings.ui_font, rt.state.settings.code_font), (1, 1));
    let saved = crate::settings::Settings::from_json(&rt.state.settings.to_json());
    assert_eq!((saved.ui_font, saved.code_font), (1, 1), "saved by name");
    switch(&mut rt, |s, tx| s.set_code_font(0, tx));
    switch(&mut rt, |s, tx| s.set_ui_font(0, tx));
    assert_eq!(rt.font_families(), (UI_FONTS[0].family, CODE_FONTS[0].family));
}

#[test]
fn a_font_switch_drops_the_terminals_cached_cell_sizes() {
    let _g = own_families();
    let mut rt = launched(App::fixture_session(), 1.0);
    let style = rune::TextStyle::new(13.0, 400.0).mono();
    let first = crate::terminal::grid::cell_metrics(99, style);
    let cached = crate::terminal::grid::cached_metrics();
    assert_eq!(crate::terminal::grid::cell_metrics(99, style), first);
    assert_eq!(crate::terminal::grid::cached_metrics(), cached, "a repeat is a cache hit");
    switch(&mut rt, |s, tx| s.set_code_font(2, tx));
    crate::terminal::grid::cell_metrics(99, style);
    assert_eq!(crate::terminal::grid::cached_metrics(), cached + 1, "the switch measures again");
    switch(&mut rt, |s, tx| s.set_ui_font(1, tx));
    crate::terminal::grid::cell_metrics(99, style);
    assert_eq!(crate::terminal::grid::cached_metrics(), cached + 2, "so does a UI font switch");
}

#[test]
fn the_font_rows_sit_after_theme_and_pick_by_name() {
    let mut rt = settings();
    let y = |rt: &Runtime<App>, k: &str| rt.find(k).unwrap_or_else(|| panic!("{k}")).1;
    assert!(y(&rt, "theme-select") < y(&rt, "ui-font-select"));
    assert!(y(&rt, "ui-font-select") < y(&rt, "code-font-select"));
    assert!(y(&rt, "code-font-select") < y(&rt, "terminal-font-select"));
    assert!(y(&rt, "terminal-font-select") < y(&rt, "reduce-motion-control"));
    tap(&mut rt, "ui-font-select");
    let menu = rt.popup_menu().expect("the UI font button opens a menu");
    assert!(menu.find("0").is_some_and(|i| i.is_checked() && i.title() == "Inter"));
    assert_eq!(menu.find("1").map(|i| i.title()), Some("SF Pro"));
    rt.resolve_popup(None).unwrap();
    assert_eq!(font_index(&CODE_FONTS, Some("SF Mono")), 2);
    assert_eq!(font_index(&CODE_FONTS, Some("Comic Sans")), 0, "an unknown name is the default");
    let s = crate::settings::Settings { ui_font: 1, code_font: 2, ..Default::default() };
    assert_eq!(families(&s), ("System Font, Inter", "ui-monospace, IBM Plex Mono"));
}

#[test]
fn a_long_description_wraps_before_its_control_in_a_narrow_window() {
    let mut app = App::fixture_session();
    app.open_settings();
    let mut rt = Runtime::new(app, crate::view::app, (900.0, 2000.0), rune::Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    frames(&mut rt, 2);
    let rect = |key: &str| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map(|e| e.rect).unwrap_or_else(|| panic!("{key}"));
    let (about, control) = (rect("setting-appearance-about"), rect("scheme"));
    assert!(about[0] + about[2] <= control[0], "the description's box ends at {} but the control starts at {}", about[0] + about[2], control[0]);
    // Unwrapped, it is one 20 pt line painting past its box; wrapped, two lines inside it.
    assert!(about[3] > 21.0, "the description wraps inside its box: {about:?}");
}
