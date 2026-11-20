//! Settings › Preferences › Navigation (`interface-mode.tsx`'s `NavigationSetting`,
//! `interface-mode.css`'s `navigation-mode`, #64668): under Experimental › Consolidated, two
//! 120 pt radio cards (Tabs and List), plus Tab position (`Top tabs` / `Side tabs`) when Tabs is chosen.

use opencode_i18n::t;
use opencode_ui::palette::{Pal, kit};
use rune::{Color, Direction, El, Tx, col, el, row, text};

use crate::host::SettingsHost;
use crate::prefs::{Choice as _, Navigation};
use crate::types::TabLayout;

const NAME: rune::TextStyle = rune::TextStyle::new(12.0, 530.0).lh(16.0);

/// Two 120 pt radio cards, 12 pt apart (`navigation-mode`).
pub(crate) fn control<H: SettingsHost + 'static>(s: &H, p: &Pal) -> El {
    let current = s.settings().navigation();
    let group = row()
        .key("navigation-mode")
        .role("radiogroup")
        .label_static(t!("settings.general.row.navigation.title"))
        .focus_group()
        .gap(12.0)
        .shrink()
        .min_w(0.0);
    Navigation::ALL.iter().copied().fold(group, |r, mode| {
        r.child(card(p, mode, mode == current).on_press_tx(move |s: &mut H, tx: &mut Tx| pick(s, mode, tx)))
    })
}

fn pick(host: &mut dyn SettingsHost, mode: Navigation, tx: &mut Tx) {
    crate::change_settings(host, |x| x.navigation = mode);
    host.sync_tab_layout(tx);
}

/// Tab position choices (`settings-tab-position`): Top tabs or Side tabs.
pub(crate) fn positions() -> Vec<(String, String)> {
    [TabLayout::Horizontal, TabLayout::Vertical].into_iter().map(|l| (l.name().to_owned(), l.label().to_owned())).collect()
}

pub(crate) fn pick_position(host: &mut dyn SettingsHost, id: &str, tx: &mut Tx) {
    host.set_tab_layout(TabLayout::parse(Some(id)), tx);
}

fn card(p: &Pal, mode: Navigation, checked: bool) -> El {
    let c = &kit(p).c;
    let ease = rune::Curve::tween(0.12, rune::Easing::cubic(0.25, 0.1, 0.25, 1.0));
    let name = text(mode.label())
        .font(NAME)
        .color(if checked { c.text_base } else { c.text_muted })
        .group_color(c.text_base)
        .truncate();
    let body = col()
        .key("body")
        .w_full()
        .items_center()
        .clip_rounded(8.0)
        .child(sketch(p, mode))
        .child(row().key("name").w_full().px(10.0).pt(6.0).pb(8.0).center().child(name));
    let ring = |key: &'static str, color: Color, w: f32| el().key(key).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(8.0).border(w, color);
    el().key(match mode {
        Navigation::Tabs => "navigation-mode-tabs",
        Navigation::List => "navigation-mode-list",
    })
    .role("radio")
    .label(mode.label())
    .focusable(true)
    .focus_selected(checked)
    .focus_ring(2.0, 2.0, c.border_focus)
    .w(120.0)
    .shrink()
    .min_w(0.0)
    .radius(8.0)
    .fill(c.bg_base)
    .hover_group()
    .hover_motion(ease, ease)
    .child(body)
    .child(ring("border", c.border_base, 1.0).group_opacity(1.0, 0.0))
    .child(ring("hover-border", c.border_strong, 1.0).group_opacity(0.0, 1.0))
    .child(ring("checked-border", c.border_focus, 2.0).opacity(if checked { 1.0 } else { 0.0 }).opacity_motion(ease))
}

fn sketch(p: &Pal, mode: Navigation) -> El {
    let c = &kit(p).c;
    let frame = |inner: El| {
        inner
            .key("sketch")
            .w_full()
            .h(56.0)
            .mt(10.0)
            .corners([5.0, 5.0, 0.0, 0.0])
            .clip_corners([5.0, 5.0, 0.0, 0.0])
            .fill(c.layer_01)
            .box_shadow(Pal::ring_in(c.border_muted))
    };
    let window = match mode {
        Navigation::Tabs => {
            let tab = |active: bool| {
                let t = el().w(24.0).h(8.0).radius(2.5).fill(if active { c.text_faint } else { c.layer_03 });
                if active { t.box_shadow(Pal::ring_in(c.border_base)) } else { t }
            };
            let strip = (0..3).fold(row().key("tabs").w_full().gap(3.0).padding(5.0).fill(c.layer_02), |r, i| r.child(tab(i == 0)));
            frame(col()).child(strip)
        }
        Navigation::List => {
            let line = |active: bool| {
                let r = row().h(8.0).w_full().px(2.0).gap(3.0).center_y().radius(2.0);
                if active {
                    return r.fill(c.layer_04).box_shadow(Pal::ring_in(c.border_base));
                }
                r.child(el().size(4.0, 4.0).radius(1.0).fill(c.layer_04)).child(el().h(2.0).grow(1.0).radius(1.0).fill(c.layer_04))
            };
            let end = match crate::ui_direction() {
                Direction::Rtl => [0.0, 0.0, 0.0, 0.5],
                _ => [0.0, 0.5, 0.0, 0.0],
            };
            let list = (0..5).fold(
                col().key("list").w(44.0).h_full().px(4.0).py(5.0).gap(3.0).border_sides(end, c.border_muted).border_layout().fill(c.layer_02),
                |r, i| r.child(line(i == 1)),
            );
            frame(row()).child(list)
        }
    };
    let fade = el()
        .key("fade")
        .pin(Some(0.0), Some(22.0), Some(0.0), Some(0.0))
        .a11y_hidden()
        .linear_gradient(std::f32::consts::PI, vec![(0.0, c.bg_base.alpha(0.0)), (1.0, c.bg_base)]);
    el().key("sketch-box").w_full().px(10.0).child(window).child(fade)
}
