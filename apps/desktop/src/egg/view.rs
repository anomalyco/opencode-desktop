//! The egg's elements on the new-session page (`view/draft.rs`): the wordmark with the lodger's
//! grip, the slots the egg draws in, the panel's pointer surface, the game's surface and HUD, and
//! what the egg does to the composer. At rest it's the plain wordmark and two invisible hit areas
//! in the "e"; the rest exists only while the egg is out.

use rune::{Anim, Curve, Cursor, El, Easing, PointerEvent, Transition, Tx, el, row, text};
use session_view::palette::Pal;

use super::lodger::HOLE;
use super::wordmark::{self, Grip, State};
use super::{Geo, gpu};
use crate::App;

/// The drop hint: the composer gives a little under the carried lodger (`wordmark.css:31-37`).
pub const SINK: Easing = Easing::cubic(0.34, 1.56, 0.64, 1.0);

/// The new-session wordmark (`view.tsx:105-113`): its box spans 80% of up to 720 pt at the logo's
/// 234:42; the logo inside is 78x14 whole-point pitches, `max(3, round(box / 78))`, its origin
/// rounded in the panel (`fitStage`). `width` and `height` are the panel's, from the last layout;
/// `top` is the box's top in the panel.
pub fn wordmark(s: &App, p: &Pal, width: f32, top: Option<f32>, dark: bool) -> El {
    let geo = rest_geo(width, top, dark);
    s.egg.rest.set(geo);
    let stage = geo.stage();
    let live = s.egg.live.as_deref();
    if let Some(live) = live {
        live.geo.set(read_geo(geo));
    }
    let [bx, by, bw, bh] = geo.logo;
    let k = stage.k as f32;
    let at = [(stage.x - bx) as f32, (stage.y - by) as f32];
    // The egg's parts come and go at once: their motion is the egg's own.
    let mut boxed = el().key("wordmark-box").w(bw as f32).h(bh as f32).instant_changes();
    let (stem, weak) = letter_colors(p);
    let (stem, weak) = (stem.alpha(stem.a * 0.16), weak.alpha(weak.a * 0.16));
    let still = crate::view::logo_at(k, stem, weak).key("wordmark").absolute(at[0], at[1]);
    // While the egg draws the letters (or the board stands in for them), the page's own hide at
    // once, in the frame the egg's show: never both, never a fade between.
    boxed = boxed.child(match live {
        None => still,
        Some(l) => still.opacity(if l.page.flat == super::Flat::Page { 1.0 } else { 0.0 }).opacity_motion(Anim::INSTANT),
    });
    if live.is_some() {
        boxed = boxed.child(el().key("egg-mid").transition(Transition::none()).absolute(at[0], at[1]).size(234.0 * k, 42.0 * k).effect(gpu::MID).a11y_hidden());
    }
    let state = live.map_or(State::Hidden, |l| l.state);
    // The eyes peek when the pointer comes within 24 units of the hole: a box around that reach,
    // until the egg is out and the panel takes the pointer.
    if live.is_none_or(|l| l.state == State::Hidden && l.board.is_none()) {
        let zone = [HOLE[0] - 24.0, HOLE[1] - 24.0];
        boxed = boxed.child(
            el().key("egg-near")
                .absolute(at[0] + zone[0] as f32 * k, at[1] + zone[1] as f32 * k)
                .size(48.0 * k, 48.0 * k)
                .a11y_hidden()
                .on_pointer_move_local(move |s: &mut App, e: PointerEvent, tx: &mut Tx| {
                    let (x, y) = (stage.x + zone[0] * stage.k + f64::from(e.x), stage.y + zone[1] * stage.k + f64::from(e.y));
                    super::pointer_move_at(s, geo, x, y, tx);
                })
                .on_hover_change(|s: &mut App, inside, tx| if !inside { super::pointer_leave(s, tx) }),
        );
    }
    let grip = wordmark::grip(state);
    if grip != Grip::None {
        let (gx, gy, gw, gh) = if grip == Grip::Hidden { (213.0, 8.0, 18.0, 14.0) } else { (234.0, 18.0, 24.0, 24.0) };
        let held = matches!(state, State::Drag | State::Carry { .. });
        boxed = boxed.child(
            el().key("egg-grip")
                .absolute(at[0] + gx as f32 * k, at[1] + gy as f32 * k)
                .size(gw as f32 * k, gh as f32 * k)
                // The keyboard's way in, where the pointer's is: in the tab order (Desktop's egg is
                // the pointer's alone), Enter or Space plays it as a hand would.
                .role("button")
                .label(opencode_i18n::t!("rune.egg.play"))
                .focusable(true)
                // A pull with the pointer leaves the composer focused, as Desktop's grip (its
                // `pointerdown` prevents the default) doesn't take focus.
                .preserve_focus_on_press(true)
                .on_key(|s: &mut App, ev, tx| super::grip_key(s, ev, tx))
                .cursor(if held { Cursor::Grabbing } else { Cursor::Grab })
                .on_drag(move |s: &mut App, d, tx| {
                    let (x, y) = (stage.x + gx * stage.k + f64::from(d.x), stage.y + gy * stage.k + f64::from(d.y));
                    match d.phase {
                        rune::Phase::Down if tx.button() == rune::PointerButton::Primary => super::grab_at(s, geo, x, y, tx),
                        rune::Phase::Down => {}
                        rune::Phase::Move => super::pointer_move_at(s, geo, x, y, tx),
                        rune::Phase::Up => super::release(s, tx),
                    }
                }),
        );
    }
    let lifted = live.is_some_and(|l| wordmark::lifted(l.state));
    row().key("wordmark-row").w_full().justify_center().paint_order(if lifted { 1 } else { 0 }).child(boxed)
}

/// The panel's geometry from arithmetic on its size, as the draft page lays it out: what the egg
/// starts from before a layout of its own has been read.
fn rest_geo(width: f32, top: Option<f32>, dark: bool) -> Geo {
    let width = f64::from(if width > 0.0 { width } else { 1280.0 });
    let span = 0.8 * (width - 48.0).min(720.0);
    let content = (width - 48.0).min(880.0);
    let x = 24.0 + (width - 48.0 - content) / 2.0 + (content - span) / 2.0;
    let y = f64::from(top.unwrap_or(0.0));
    Geo { panel: [0.0, 0.0, width, 0.0], logo: [x, y, span, span * 42.0 / 234.0], composer: None, wrap: None, radius: f64::from(session_view::composer::RADIUS), dark }
}

/// The geometry the last layout put on screen, read while the egg is out (each read re-runs
/// the view when it moves).
fn read_geo(rest: Geo) -> Geo {
    let Some(panel) = rune::memo::layout("draft") else { return rest };
    let rel = |r: [f32; 4]| [f64::from(r[0] - panel[0]), f64::from(r[1] - panel[1]), f64::from(r[2]), f64::from(r[3])];
    let logo = rune::memo::layout("wordmark-box").map_or(rest.logo, rel);
    let composer = rune::memo::layout("composer").map(rel);
    let wrap = rune::memo::layout("draft-composer").map(rel);
    Geo { panel: [f64::from(panel[0]), f64::from(panel[1]), f64::from(panel[2]), f64::from(panel[3])], logo, composer, wrap, ..rest }
}

/// The slot under everything on the panel (the pins behind the composer, the paddle), and the
/// panel's pointer surface: the board leans toward the pointer everywhere on the panel, and a
/// press anywhere off its controls knocks pins out.
pub fn under(s: &App) -> [Option<El>; 2] {
    let Some(live) = s.egg.live.as_deref() else { return [None, None] };
    let slot = (live.board.is_some() || live.game.is_some()).then(|| el().key("egg-back").transition(Transition::none()).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).effect(gpu::BACK).a11y_hidden());
    let surface = el()
        .key("egg-panel")
        .transition(Transition::none())
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .a11y_hidden()
        .on_pointer_move_local(|s: &mut App, e: PointerEvent, tx: &mut Tx| super::pointer_move(s, f64::from(e.x), f64::from(e.y), tx))
        .on_hover_change(|s: &mut App, inside, tx| if !inside { super::pointer_leave(s, tx) } else { super::pointer_entered(s, tx) })
        .on_drag(|s: &mut App, d, tx| match d.phase {
            rune::Phase::Down if tx.button() == rune::PointerButton::Primary => super::panel_press(s, f64::from(d.x), f64::from(d.y), tx),
            rune::Phase::Up => super::panel_release(s),
            _ => {}
        });
    [slot, Some(surface)]
}

/// The pins popped toward the viewer, between the composer's tray and the composer (`FRONT`).
pub fn front(s: &App) -> Option<El> {
    let live = s.egg.live.as_deref()?;
    live.board.as_ref()?;
    Some(el().key("egg-front").transition(Transition::none()).absolute(0.0, 0.0).size(1.0, 1.0).effect(gpu::FRONT).a11y_hidden())
}

/// The composer while the egg is out: it sinks a little under the carried lodger, and fades out
/// for the game (160 ms ease-out), when it takes no input.
pub fn composer_wrap(s: &App, wrap: El) -> El {
    let Some(live) = s.egg.live.as_deref() else { return wrap };
    let over = wordmark::drop_over(live.state);
    let sink = Anim::new(Curve::tween(0.26, SINK));
    let wrap = wrap.offset(0.0, if over { 3.0 } else { 0.0 }).scale(if over { 0.985 } else { 1.0 }).offset_motion(sink).scale_motion(sink);
    if live.page.composer_hidden { wrap.inert() } else { wrap }
}

/// One of the composer wrap's parts (the composer, its tray) fading for the game.
pub fn composer_part(s: &App, part: El) -> El {
    let Some(live) = s.egg.live.as_deref() else { return part };
    part.opacity(if live.page.composer_hidden { 0.0 } else { 1.0 }).opacity_motion(Anim::new(Curve::tween(0.16, Easing::EASE_OUT)))
}

/// Brick Breaker's surface over the panel (it takes every press, as Desktop's capture-phase
/// listener does) and its HUD (`brick-breaker.tsx:56-86`).
pub fn over(s: &App, p: &Pal) -> Vec<El> {
    let Some(live) = s.egg.live.as_deref() else { return Vec::new() };
    let Some(g) = live.game.as_deref() else { return Vec::new() };
    // Tailwind's `transition-*` with `duration-300`: its default curve.
    let css = || Anim::new(Curve::tween(0.3, Easing::cubic(0.4, 0.0, 0.2, 1.0)));
    let pills = (0..super::brick::machine::LIVES - 1).map(|i| {
        let left = i < g.spares;
        el().key(format!("egg-spare-{i}")).size(20.0, 6.0).radius(3.0).fill(p.icon_faint)
            .opacity(if left { 1.0 } else { 0.0 }).offset(0.0, if left { 0.0 } else { 4.0 })
            .opacity_motion(css()).offset_motion(css())
    });
    let hud = el()
        .key("egg-hud")
        .transition(Transition::none())
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .a11y_hidden()
        .opacity(if g.hud { 1.0 } else { 0.0 })
        .opacity_motion(css())
        .child(el().key("egg-wall-l").pin(Some((g.l - 1.0) as f32), Some(0.0), None, Some(0.0)).w(1.0).fill(p.border))
        .child(el().key("egg-wall-r").pin(Some(g.r as f32), Some(0.0), None, Some(0.0)).w(1.0).fill(p.border))
        .child(text(g.score_text()).key("egg-score").font(SCORE).color(p.faint).pin(None, Some(12.0), Some(16.0), None))
        .child(row().key("egg-spares").gap(6.0).pin(Some(16.0), None, None, Some(16.0)).children(pills));
    let surface = el()
        .key("egg-game")
        .transition(Transition::none())
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        // What a reader says as the game takes focus: the keys it plays with.
        .role("group")
        .label(opencode_i18n::t!("rune.egg.help"))
        .cursor(if live.page.cursor_hidden { Cursor::Hidden } else { Cursor::Default })
        // It holds the keyboard while the game plays, as Desktop's window-wide key listener does.
        .focusable(true)
        .on_key(|s: &mut App, ev, tx| super::game_key(s, ev, true, tx))
        .on_key_up(|s: &mut App, ev, tx| super::game_key(s, ev, false, tx))
        .on_physical_input(|s: &mut App, ev, _| {
            if matches!(ev, rune::PhysicalInput::FocusLost) {
                super::blur(s);
            }
            false
        })
        .on_pointer_move_local(|s: &mut App, e: PointerEvent, tx: &mut Tx| super::pointer_move(s, f64::from(e.x), f64::from(e.y), tx))
        .on_drag(|s: &mut App, d, tx| match d.phase {
            rune::Phase::Down if tx.button() == rune::PointerButton::Primary => super::panel_press(s, f64::from(d.x), f64::from(d.y), tx),
            rune::Phase::Move => super::pointer_move(s, f64::from(d.x), f64::from(d.y), tx),
            _ => {}
        });
    vec![hud, surface]
}

/// The score (`font-mono text-[11px] leading-[16px] tabular-nums`).
const SCORE: rune::TextStyle = rune::TextStyle::new(11.0, 400.0).mono().lh(16.0).features("tnum");

/// The flat wordmark's ink (`wordmark.tsx:240-242`, `wordmark.css:44-46`): Desktop's v1
/// `--icon-base` for the letters and `--icon-weak-base` for the counters' lower halves (in
/// light, the letters' at 30%); a chosen theme's nearest tokens.
pub fn letter_colors(p: &Pal) -> (rune::Color, rune::Color) {
    let hex = |h: &str| rune::color::hex(h).unwrap_or(rune::Color::BLACK);
    match (p.theme, p.dark) {
        (0, true) => (hex("#7e7e7e"), hex("#343434")),
        (0, false) => (hex("#8f8f8f"), hex("#8f8f8f").alpha(0.3)),
        (_, _) => (p.standard.icon, p.standard.icon.alpha(0.3)),
    }
}