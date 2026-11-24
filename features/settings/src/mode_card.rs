//! The radio cards of Settings › General's layout and navigation rows (`interface-mode.tsx`'s
//! `ModeSketch` and `ModeCheck`, `interface-mode.css`; one style for both since opencode#65800):
//! a 120 pt card whose window shows only its navigation and fades into the card's name.

use opencode_ui::palette::{Pal, kit};
use rune::{Color, Direction, El, col, el, row, text};

/// The card's name (`interface-mode-name`, `navigation-mode-name`): 12 pt, medium (530), the
/// compact line height.
const NAME: rune::TextStyle = rune::TextStyle::new(12.0, 530.0).lh(16.0);

/// The cards' distance in their row (`gap: 10px`).
pub(crate) const GAP: f32 = 10.0;

/// The window's height (`height: 56px`); the name starts 14 pt above its foot.
const WINDOW_H: f32 = 56.0;

/// What a card's window shows (`ModeSketch`'s `mode`): tabs across the top or down the side, or
/// sessions grouped under headings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sketch {
    Top,
    Side,
    Inbox,
    List,
}

#[derive(Clone, Copy)]
enum Line {
    Heading,
    Session,
    Approval,
    Done,
    Working,
}

/// Mirrors the inbox: a session that needs you, then finished work, then sessions still running.
const INBOX_ROWS: [Line; 6] = [Line::Heading, Line::Approval, Line::Done, Line::Heading, Line::Working, Line::Working];

const LIST_ROWS: [Line; 6] = [Line::Heading, Line::Session, Line::Session, Line::Heading, Line::Session, Line::Session];

/// `interface-mode-option`, `navigation-mode-option`: a 120 pt `bg-base` card with 8 pt corners
/// and a 1 pt border that takes its room, 78 pt tall. The chosen card has a 2 pt neutral border
/// and a check outside its corner, so it never reads as the blue keyboard ring.
pub(crate) fn card(p: &Pal, key: &'static str, label: &'static str, sketch: Sketch, checked: bool) -> El {
    let c = &kit(p).c;
    let ease = rune::Curve::tween(0.12, rune::Easing::cubic(0.25, 0.1, 0.25, 1.0));
    let name = text(label).font(NAME).color(if checked { c.text_base } else { c.text_muted }).group_color(c.text_base).truncate();
    let place = |e: El| e.pin(Some(10.0), Some(10.0), Some(10.0), None).h(WINDOW_H);
    // `mask-image: linear-gradient(to bottom, #000 45%, transparent calc(100% - 12px))`, as the
    // card's ground laid over the window: fully faded before the name's text starts. Outside the
    // window, whose opacity it must not share.
    let clear = c.bg_base.alpha(0.0);
    let fade = place(el().key("fade")).linear_gradient(
        std::f32::consts::PI,
        [(0.0, clear), (0.45, clear), ((WINDOW_H - 12.0) / WINDOW_H, c.bg_base), (1.0, c.bg_base)],
    );
    let body = col()
        .key("body")
        .w_full()
        .border(1.0, Color::TRANSPARENT)
        .border_layout()
        // overflow: hidden, here rather than on the card, which would clip its own focus ring.
        .clip_rounded(8.0)
        // `margin: 10px 10px 0` round the window, less the name's `margin-top: -14px`.
        .child(
            el().key("sketch-box")
                .w_full()
                .h(10.0 + WINDOW_H - 14.0)
                .a11y_hidden()
                .child(place(window(p, sketch, checked)).opacity(if checked { 1.0 } else { 0.45 }))
                .child(fade),
        )
        .child(row().key("name").w_full().px(10.0).pb(8.0).center().child(name));
    let ring = |key: &'static str, color: Color, w: f32| el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(8.0).border(w, color);
    el().key(key)
        .role("radio")
        .label_static(label)
        .focusable(true)
        .focus_selected(checked)
        // `:has(input:focus-visible)`: a 2pt `border-focus` outline 2pt out.
        .focus_ring(2.0, 2.0, c.border_focus)
        .w(120.0)
        .shrink()
        .min_w(0.0)
        .radius(8.0)
        .fill(c.bg_base)
        .hover_group()
        .hover_motion(ease, ease)
        .child(body)
        .child(ring("border", c.border_muted, 1.0).group_opacity(1.0, 0.0))
        .child(ring("hover-border", c.border_strong, 1.0).group_opacity(0.0, 1.0))
        .child(ring("checked-border", c.border_strong, 2.0).opacity(if checked { 1.0 } else { 0.0 }).opacity_motion(ease))
        .children(checked.then(|| check_badge(p)))
}

/// `mode-option-check`: the chosen card's 16 pt check outside its top end corner, the accent blue
/// with `text-contrast` inked, in a 2 pt ring of `layer-01` (opencode#65757). Official's `-7px` counts from
/// inside the card's 1 px border, so the badge sits 6 pt outside the card's edge (measured in both apps).
fn check_badge(p: &Pal) -> El {
    let c = &kit(p).c;
    el().key("check")
        .pin(None, Some(-6.0), Some(-6.0), None)
        .size(16.0, 16.0)
        .radius(8.0)
        .center()
        .fill(c.bg_accent)
        .box_shadow(rune::Shadow { spread: 2.0, color: c.layer_01, ..rune::Shadow::default() })
        .a11y_hidden()
        .child(rune::icon(&rune_theme_opencode::icons::CHECK_SMALL, 16.0).color(c.text_contrast))
}

/// `interface-mode-sketch`, `navigation-mode-sketch`: colour gives the structure, the window on
/// `layer-01`, its navigation on `layer-02`, the chosen tab or row brightest; hairlines only
/// finish the edges. Only the chosen card shows accent.
fn window(p: &Pal, sketch: Sketch, checked: bool) -> El {
    let c = &kit(p).c;
    let accent = |f: f32| c.bg_accent.alpha(f * c.bg_accent.a);
    let frame = |inner: El| inner.key("sketch").corners([5.0, 5.0, 0.0, 0.0]).clip_corners([5.0, 5.0, 0.0, 0.0]).fill(c.layer_01).box_shadow(Pal::ring_in(c.border_muted));
    // `[data-active]`: its own fill in a hairline, or 40% accent and no hairline on the chosen card.
    let active = |e: El, fill: Color| if checked { e.fill(accent(0.4)) } else { e.fill(fill).box_shadow(Pal::ring_in(c.border_base)) };
    if sketch == Sketch::Top {
        let tab = |on: bool| {
            let t = el().w(24.0).h(8.0).radius(2.5);
            if on { active(t, c.text_faint) } else { t.fill(c.layer_03) }
        };
        let tabs = row().key("tabs").w_full().gap(3.0).padding(5.0).fill(c.layer_02).border_sides([0.0, 0.0, 0.5, 0.0], c.border_muted);
        return frame(col()).child((0..3).fold(tabs, |r, i| r.child(tab(i == 0))));
    }
    let end = match crate::ui_direction() {
        Direction::Rtl => [0.0, 0.0, 0.0, 0.5],
        _ => [0.0, 0.5, 0.0, 0.0],
    };
    let list = col().key("list").w(44.0).h_full().px(4.0).py(5.0).gap(3.0).fill(c.layer_02).border_sides(end, c.border_muted);
    // `sketch-row`: a 4 pt square and a line on one centre line; the chosen one a 6 pt bar.
    let tab_row = |on: bool| {
        let r = row().h(8.0).w_full().center_y();
        if on {
            return r.child(active(el().w_full().h(6.0).radius(2.0), c.layer_04));
        }
        r.px(2.0).gap(3.0).child(el().size(4.0, 4.0).radius(1.0).fill(c.layer_04)).child(el().h(2.0).grow(1.0).radius(1.0).fill(c.layer_04))
    };
    // `sketch-row[data-detail]`: a title and a shorter detail line beside the session's mark,
    // round where it is a status.
    let detail = |line: Line, after_heading: bool| {
        let (ink, radius) = match line {
            Line::Approval => (accent(1.0), 2.0),
            Line::Done => (accent(0.6), 2.0),
            Line::Working => (accent(0.3), 2.0),
            Line::Heading | Line::Session => (accent(0.6), 1.0),
        };
        let bar = || el().h(2.0).fill(c.layer_04);
        row()
            .h(10.0)
            .w_full()
            .mt(if after_heading { 1.0 } else { 0.0 })
            .px(2.0)
            .gap(3.0)
            .items_start()
            .child(el().size(4.0, 4.0).mt(1.0).radius(radius).fill(if checked { ink } else { c.layer_04 }))
            .child(col().grow(1.0).min_w(0.0).gap(2.0).child(bar().w_full().corners([1.0, 1.0, 0.0, 0.0])).child(bar().w_frac(0.6).corners([0.0, 0.0, 0.0, 1.0])))
    };
    let heading = |first: bool| el().w(14.0).h(2.0).mx(2.0).mt(if first { 2.0 } else { 3.0 }).radius(1.0).fill(c.text_faint).opacity(0.5);
    let list = match sketch {
        Sketch::Side => (0..5).fold(list, |l, i| l.child(tab_row(i == 1))),
        _ => {
            let rows = if sketch == Sketch::Inbox { INBOX_ROWS } else { LIST_ROWS };
            (0..rows.len()).fold(list, |l, i| {
                l.child(match rows[i] {
                    Line::Heading => heading(i == 0),
                    line => detail(line, i > 0 && matches!(rows[i - 1], Line::Heading)),
                })
            })
        }
    };
    frame(row()).child(list)
}
