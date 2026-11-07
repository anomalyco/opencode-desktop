//! Settings › General › Interface layout (`interface-mode.tsx`, `interface-mode.css`): three
//! 120pt cards, each a sketch of its layout over its name. Picking one switches the shell live,
//! around the Settings page, while you watch.

use rune::{Color, El, Tx, col, el, row, text};

use crate::App;
use crate::layout_state::TabLayout;
use crate::palette::Pal;

/// The card's name (`interface-mode-name`): 12pt, medium, the compact line height.
const NAME: rune::TextStyle = rune::TextStyle::new(12.0, 500.0).lh(16.0);

/// The inbox sketch's rows: a session that needs you, finished work, then two still running.
#[derive(Clone, Copy)]
enum Dot {
    Approval,
    Done,
    Working,
}

const INBOX_ROWS: [Dot; 4] = [Dot::Approval, Dot::Done, Dot::Working, Dot::Working];

/// The row's control: the three cards, 12pt apart.
pub(super) fn control(s: &App, p: &Pal) -> El {
    let current = s.tab_layout();
    TabLayout::ALL.into_iter().fold(row().key("interface-mode").role("radiogroup").label("Interface layout").focus_group().gap(12.0), |r, layout| {
        r.child(card(p, layout, layout == current))
    })
}

fn key(layout: TabLayout) -> &'static str {
    match layout {
        TabLayout::Horizontal => "interface-mode-top",
        TabLayout::Vertical => "interface-mode-side",
        TabLayout::Inbox => "interface-mode-inbox",
    }
}

fn card(p: &Pal, layout: TabLayout, checked: bool) -> El {
    let k = crate::palette::kit(p);
    let c = &k.c;
    let hairline = crate::view::chrome().hairline.max(0.5);
    let card = col()
        .key(key(layout))
        .role("radio")
        .label(layout.label())
        .focusable(true)
        .focus_selected(checked)
        .w(120.0)
        .radius(8.0)
        .fill(c.layer_02)
        .border(1.0, if checked { c.border_focus } else { c.border_base })
        .clip_rounded(8.0)
        .hover_group()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.set_tab_layout(layout, tx))
        .child(sketch(p, layout, checked, hairline))
        .child(el().key("rule").w_full().h(hairline.min(0.5)).fill(c.border_base))
        .child(
            row()
                .key("name")
                .w_full()
                .px(10.0)
                .py(6.0)
                .fill(c.bg_base)
                .child(text(layout.label()).font(NAME).color(if checked { c.text_base } else { c.text_muted }).group_color(c.text_base).truncate_fade()),
        );
    // The border darkens under the pointer (`border-strong`) and the picked card's inner ring
    // shows over the opaque name row (`::after`), both on `border-color`'s 120ms ease.
    let ease = rune::Curve::tween(0.12, rune::Easing::cubic(0.25, 0.1, 0.25, 1.0));
    card.child(el().key("hover-ring").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(8.0).border(1.0, c.border_strong).group_opacity(0.0, if checked { 0.0 } else { 1.0 }))
        .child(el().key("ring").pin(Some(1.0), Some(1.0), Some(1.0), Some(1.0)).radius(7.0).border(1.0, c.border_focus).opacity(if checked { 1.0 } else { 0.0 }).opacity_motion(ease))
}

/// `interface-mode-sketch`: a 52pt window in miniature with its tabs inked.
fn sketch(p: &Pal, layout: TabLayout, checked: bool, hairline: f32) -> El {
    let c = &crate::palette::kit(p).c;
    let strong = if checked { c.bg_accent.alpha(0.7 * c.bg_accent.a) } else { c.layer_04 };
    let dim = strong.alpha(0.45 * strong.a);
    let rule = hairline.min(0.5);
    let frame = el()
        .key("sketch")
        .h(52.0)
        .mx(10.0)
        .mt(10.0)
        .corners([4.0, 4.0, 0.0, 0.0])
        .clip_corners([4.0, 4.0, 0.0, 0.0])
        .fill(c.bg_base)
        .child(el().key("edge").pin(Some(0.0), Some(0.0), Some(0.0), Some(-4.0)).corners([4.0, 4.0, 0.0, 0.0]).border(rule, c.border_base));
    let page = page(p);
    match layout {
        TabLayout::Horizontal => {
            let strip = (0..3).fold(row().key("strip").gap(3.0).px(5.0).py(4.0).fill(c.pressed), |r, i| {
                r.child(el().size(18.0, 5.0).radius(2.0).fill(if i == 0 { strong } else { dim }))
            });
            frame.child(col().w_full().h_full().child(strip).child(el().w_full().h(rule).fill(c.border_base)).child(page))
        }
        TabLayout::Vertical => {
            let rail = (0..4).fold(rail(c.pressed), |r, i| r.child(el().h(5.0).w_full().radius(2.0).fill(if i == 0 { strong } else { dim })));
            frame.child(row().w_full().h_full().child(rail).child(el().w(rule).h_full().fill(c.border_base)).child(page))
        }
        TabLayout::Inbox => {
            let rail = INBOX_ROWS.into_iter().enumerate().fold(rail(c.pressed), |r, (i, dot)| {
                let ink = match dot {
                    Dot::Approval => c.warning.fg,
                    Dot::Done => c.success.fg,
                    Dot::Working => c.info.fg,
                };
                let bar = el().h(3.0).grow(1.0).radius(1.5).fill(if i == 0 { strong } else { dim });
                r.child(row().h(5.0).w_full().gap(2.0).center_y().child(el().size(4.0, 4.0).radius(2.0).fill(ink)).child(bar))
            });
            frame.child(row().w_full().h_full().child(rail).child(el().w(rule).h_full().fill(c.border_base)).child(page))
        }
    }
}

/// The sketches' side rail (`preview-rail`): 28pt, its rows 4pt apart.
fn rail(fill: Color) -> El {
    col().key("rail").w(28.0).h_full().px(5.0).py(6.0).gap(4.0).fill(fill)
}

/// The miniature page beside the tabs (`preview-page`): two lines and a composer.
fn page(p: &Pal) -> El {
    let c = &crate::palette::kit(p).c;
    let line = |f: f32| el().w_frac(f).h(3.0).radius(1.5).fill(c.layer_03);
    col()
        .key("page")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .padding(6.0)
        .gap(4.0)
        .child(line(0.8))
        .child(line(0.56))
        .child(rune::spacer())
        .child(el().w_full().h(8.0).radius(3.0).fill(c.layer_02).border(0.5, c.border_base))
}
