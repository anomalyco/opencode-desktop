//! Settings › General › Interface layout (`interface-mode.tsx`, `interface-mode.css`): three
//! 120pt cards, each a sketch of its layout over its name. Picking one switches the shell live,
//! around the Settings page, while you watch.

use opencode_i18n::t;
use rune::{Color, Direction, El, Tx, col, el, row, text};
use opencode_ui::palette::{Pal, kit};

use crate::host::SettingsHost;
use crate::types::TabLayout;

/// The card's name (`interface-mode-name`): 12pt, medium (530), the compact line height.
const NAME: rune::TextStyle = rune::TextStyle::new(12.0, 530.0).lh(16.0);

/// The inbox sketch's rows: a session that needs you, finished work, then two still running.
#[derive(Clone, Copy)]
enum Dot {
    Approval,
    Done,
    Working,
}

const INBOX_ROWS: [Dot; 4] = [Dot::Approval, Dot::Done, Dot::Working, Dot::Working];

/// The row's control: the three cards, 12pt apart.
pub(crate) fn control<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let current = s.tab_layout();
    let group = row()
        .key("interface-mode")
        .role("radiogroup")
        .label_static(t!("settings.general.row.interfaceMode.title"))
        .focus_group()
        .gap(12.0)
        .shrink()
        .min_w(0.0);
    let group = if crate::rows_wrap() { group.w_full().max_w(384.0) } else { group };
    TabLayout::ALL.into_iter().fold(group, |r, layout| {
        r.child(card_shell(p, layout, layout == current).on_press_tx(move |s: &mut H, tx: &mut Tx| set_layout(s, layout, tx)))
    })
}

fn set_layout(host: &mut dyn SettingsHost, layout: TabLayout, tx: &mut Tx) {
    host.set_tab_layout(layout, tx);
}

fn key(layout: TabLayout) -> &'static str {
    match layout {
        TabLayout::Horizontal => "interface-mode-top",
        TabLayout::Vertical => "interface-mode-side",
        TabLayout::Inbox => "interface-mode-inbox",
    }
}

/// `interface-mode-option`: a 120pt `layer-02` card with 8pt corners and a 1pt border that
/// takes its room, as CSS's does.
fn card_shell(p: &Pal, layout: TabLayout, checked: bool) -> El {
    let c = &kit(p).c;
    let ease = rune::Curve::tween(0.12, rune::Easing::cubic(0.25, 0.1, 0.25, 1.0));
    let after = checked.then(|| {
        el().key("ring")
            .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .radius(7.0)
            .box_shadow(rune::Shadow { spread: 1.0, color: c.border_strong, inset: true, ..rune::Shadow::default() })
    });
    let body = col()
        .key("body")
        .w_full()
        .border(1.0, Color::TRANSPARENT)
        .border_layout()
        // overflow: hidden, here rather than on the card, which would clip its own focus ring.
        .clip_rounded(8.0)
        .child(sketch(p, layout, checked).opacity(if checked { 1.0 } else { 0.45 }))
        .child(
            row()
                .key("name")
                .w_full()
                .px(10.0)
                .py(6.0)
                .border_sides([0.5, 0.0, 0.0, 0.0], c.border_base)
                .border_layout()
                .fill(c.bg_base)
                .child(text(layout.label()).font(NAME).color(if checked { c.text_base } else { c.text_muted }).group_color(c.text_base).truncate()),
        )
        .children(after);
    let ring = |key: &'static str, color: Color| el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(8.0).border(1.0, color);
    el().key(key(layout))
        .role("radio")
        .label(layout.label())
        .focusable(true)
        .focus_selected(checked)
        // `:has(input:focus-visible)`: a 2pt `border-focus` outline 2pt out.
        .focus_ring(2.0, 2.0, c.border_focus)
        .w(120.0)
        .shrink()
        .min_w(0.0)
        .radius(8.0)
        .fill(c.layer_02)
        .hover_group()
        .hover_motion(ease, ease)
        .child(body)
        .child(ring("border", c.border_muted).group_opacity(1.0, 0.0))
        .child(ring("hover-border", c.border_strong).group_opacity(0.0, 1.0))
        .child(ring("checked-border", c.border_strong).opacity(if checked { 1.0 } else { 0.0 }).opacity_motion(ease))
        .children(checked.then(|| check_badge(p)))
}

/// `mode-option-check`: the chosen card's 16 pt check outside its top end corner, the accent blue
/// with `text-contrast` inked, in a 2 pt ring of `layer-01` (opencode#65757; `icon-muted` on the
/// ground's colour before). The card's border stays neutral, so it never reads as the blue
/// keyboard ring (opencode#64909).
pub(crate) fn check_badge(p: &Pal) -> El {
    let c = &kit(p).c;
    el().key("check")
        .pin(None, Some(-7.0), Some(-7.0), None)
        .size(16.0, 16.0)
        .radius(8.0)
        .center()
        .fill(c.bg_accent)
        .box_shadow(rune::Shadow { spread: 2.0, color: c.layer_01, ..rune::Shadow::default() })
        .a11y_hidden()
        .child(rune::icon(&rune_theme_opencode::icons::CHECK_SMALL, 16.0).color(c.text_contrast))
}

/// `interface-mode-sketch`: a 52pt window in miniature, 10pt in from the card's top and sides,
/// with a 0.5pt border on its top and sides and its tabs inked.
fn sketch(p: &Pal, layout: TabLayout, checked: bool) -> El {
    let c = &kit(p).c;
    let strong = if checked { c.bg_accent.alpha(0.7 * c.bg_accent.a) } else { c.layer_04 };
    let dim = strong.alpha(0.45 * strong.a);
    let tab = |ink: Color| el().h(5.0).radius(2.0).fill(ink).motion(rune::Spring::NONE);
    let frame = |inner: El| {
        inner
            .key("sketch")
            .h(52.0)
            .mx(10.0)
            .mt(10.0)
            .border_sides([0.5, 0.5, 0.0, 0.5], c.border_base)
            .border_layout()
            .corners([4.0, 4.0, 0.0, 0.0])
            .clip_corners([4.0, 4.0, 0.0, 0.0])
            .fill(c.bg_base)
    };
    match layout {
        TabLayout::Horizontal => {
            let strip = (0..3).fold(
                row().key("strip").gap(3.0).px(5.0).py(4.0).border_sides([0.0, 0.0, 0.5, 0.0], c.border_base).border_layout().fill(c.pressed),
                |r, i| r.child(tab(if i == 0 { strong } else { dim }).w(18.0)),
            );
            frame(col()).child(strip).child(page(p))
        }
        TabLayout::Vertical => {
            let rail = (0..4).fold(rail(c.pressed, c.border_base), |r, i| r.child(tab(if i == 0 { strong } else { dim }).w_full()));
            frame(row()).child(rail).child(page(p))
        }
        TabLayout::Inbox => {
            let rail = INBOX_ROWS.into_iter().enumerate().fold(rail(c.pressed, c.border_base), |r, (i, dot)| {
                let ink = match dot {
                    Dot::Approval => c.warning.fg,
                    Dot::Done => c.success.fg,
                    Dot::Working => c.info.fg,
                };
                let bar = el().h(3.0).grow(1.0).radius(1.5).fill(if i == 0 { strong } else { dim }).motion(rune::Spring::NONE);
                r.child(row().h(5.0).w_full().gap(2.0).center_y().child(el().size(4.0, 4.0).radius(2.0).fill(ink)).child(bar))
            });
            frame(row()).child(rail).child(page(p))
        }
    }
}

/// The sketches' side rail (`preview-rail`): 28pt with its 0.5pt inline-end border, its rows 4pt
/// apart.
fn rail(fill: Color, border: Color) -> El {
    let end = match crate::ui_direction() {
        Direction::Rtl => [0.0, 0.0, 0.0, 0.5],
        _ => [0.0, 0.5, 0.0, 0.0],
    };
    col().key("rail").w(28.0).h_full().px(5.0).py(6.0).gap(4.0).border_sides(end, border).border_layout().fill(fill)
}

/// The miniature page beside the tabs (`preview-page`): two lines and a composer.
fn page(p: &Pal) -> El {
    let c = &kit(p).c;
    let line = |f: f32| el().w_frac(f).h(3.0).radius(1.5).fill(c.layer_03);
    // Keep `gap(4.0)` on the two lines only so `spacer()` acts like CSS `margin-top: auto` on the
    // composer (`interface-mode.css:163-167`) without adding an extra 4 pt gap that pushes it 1 px low.
    col()
        .key("page")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .min_h(0.0)
        .padding(6.0)
        .child(col().w_full().gap(4.0).child(line(0.8)).child(line(0.56)))
        .child(rune::spacer())
        .child(el().w_full().h(8.0).radius(3.0).fill(c.layer_02).box_shadow(Pal::ring_in(c.border_base)))
}
