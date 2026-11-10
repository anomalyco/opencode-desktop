//! The narrow titlebar's tabs (`titlebar.tsx:581-706`, `titlebar.css:32-107`): one trigger in
//! place of Home, Search and the tab strip, showing the open tab's avatar, title and the tab
//! count, which opens a drawer of every tab, New session, Home, Settings and Help.

use rune::{El, TextStyle, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::machine::TabsEvent;
use crate::palette::Pal;
use crate::view::INSTANT;
use crate::{App, Route};
use opencode_i18n::t;

/// The drawer's rows' 13 / 16 at 440.
const ROW: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);
/// A tab's title in the drawer (`tab-title`: 530, `text-base`).
const TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(16.0);

pub const SHEET: &str = "narrow-tabs-sheet";

/// `mobile-tabs-trigger`: 28 tall, `px-2 gap-2`, 6 pt corners; the layer-02 wash under the
/// hover overlay while its drawer is open.
pub fn trigger(s: &App, p: &Pal) -> El {
    let current = s.current_tab();
    let tab = current.as_deref().and_then(|uid| s.tab(uid));
    let lead = match tab {
        None => icon(&ic::GRID_PLUS, 16.0).color(p.icon),
        Some(t) if t.is_draft() => icon(&ic::EDIT, 16.0).color(p.icon),
        Some(t) => avatar(s, p, t),
    };
    let title = match tab {
        None => t!("home.title").to_owned(),
        Some(t) => crate::view::tab_strip::face(s, t).title,
    };
    let busy = tab.is_some_and(|t| !t.is_draft() && s.tab_status_of(t) == Some(crate::TabStatus::Busy));
    let open = s.layout.narrow.tabs.is_open();
    let mut b = row()
        .key("narrow-tabs-trigger")
        .role("button")
        .label(t!("titlebar.tabs"))
        .h(28.0)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .no_window_drag()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let open = !s.layout.narrow.tabs.is_open();
            s.tabs_drawer(TabsEvent::Changed { open }, tx);
        })
        .children(open.then(|| wash(p.hover, 6.0)))
        .child(row().key("avatar").size(16.0, 16.0).center().child(lead))
        .child(text(title).key("title").font(ROW).color(p.text).truncate().grow(1.0).shrink().min_w(0.0))
        .children(busy.then(|| crate::view::activity_ring(p)))
        .child(text(s.tabs.len().to_string()).key("count").font(ROW).color(p.muted));
    if open {
        b = b.fill(p.layer2);
    }
    b
}

/// A tab's project avatar, on its own server, as the strip draws it.
fn avatar(s: &App, p: &Pal, t: &crate::tabs::Tab) -> El {
    let face = crate::view::tab_strip::face(s, t);
    crate::icons::badge(s, p, face.project, &crate::view::project_or_dir(face.project, face.dir))
}

/// The drawer's contents (`mobile-tabs-drawer`): 8 below the handle, its sections 8 apart.
pub fn drawer(s: &App, p: &Pal) -> El {
    let home = s.route == Route::Home && !s.settings.open;
    let tabs = s.tabs.iter().fold(col().key("narrow-tabs-list").w_full().gap(2.0), |list, t| list.child(tab_row(s, p, t)));
    let new = button("narrow-tabs-new", None, &ic::PLUS, 16.0, t!("command.session.new"), p.text, p.icon)
        .hover(p.layer2)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.new_session(tx);
            s.tabs_drawer(TabsEvent::Close, tx);
        });
    let mut home_row = button("narrow-tabs-home", home.then_some(p.pressed), &ic::GRID_PLUS, 16.0, t!("home.title"), if home { p.text } else { p.faint }, if home { p.icon } else { p.icon_faint })
        .w_full()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            if !(s.route == Route::Home && !s.settings.open) {
                s.toggle_home(tx);
            }
            s.tabs_drawer(TabsEvent::Close, tx);
        });
    if home {
        home_row = home_row.fill(p.layer2);
    }
    let settings = button("narrow-tabs-settings", None, &ic::SETTINGS_GEAR, 14.0, t!("sidebar.settings"), p.faint, p.icon_faint)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .hover(p.layer2)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            let now = s.clock;
            s.tabs_drawer(TabsEvent::SettingsRequested { now }, tx);
        });
    let help = button("narrow-tabs-help", None, &ic::HELP, 14.0, t!("sidebar.help"), p.faint, p.icon_faint)
        .hover(p.layer2)
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            s.tabs_drawer(TabsEvent::Close, tx);
            let _ = tx.open_url("https://opencode.ai/desktop-feedback");
        });
    let hairline = crate::view::chrome().hairline;
    col()
        .key("narrow-tabs-drawer")
        .w_full()
        .mt(8.0)
        .gap(8.0)
        .min_h(0.0)
        .child(col().key("narrow-tabs-scroll").w_full().shrink().min_h(0.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(tabs))
        .child(new.w_full())
        .child(
            col()
                .key("narrow-tabs-foot")
                .w_full()
                .gap(4.0)
                .pt(8.0)
                .child(el().key("rule").pin(Some(0.0), Some(0.0), Some(0.0), None).h(hairline).fill(p.border_muted))
                .child(home_row)
                .child(row().key("narrow-tabs-utility").w_full().gap(4.0).center_y().child(settings).child(help)),
        )
}

/// A drawer button: 28 tall, `px-2 gap-2`, 6 pt corners, `glyph` at `size` and `label`.
fn button(key: &str, washed: Option<rune::Color>, glyph: &'static rune::Icon, size: f32, label: &str, ink: rune::Color, glyph_ink: rune::Color) -> El {
    row()
        .key(key.to_owned())
        .role("button")
        .label(label.to_owned())
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .hover_motion(INSTANT, INSTANT)
        .children(washed.map(|c| wash(c, 6.0)))
        .child(row().size(16.0, 16.0).center().child(icon(glyph, size).color(glyph_ink)))
        .child(text(label.to_owned()).font(ROW).color(ink).truncate().shrink().min_w(0.0))
}

/// One tab in the drawer (`TitlebarTabStrip` vertical in `mobile-tabs-drawer`): 44 tall, `px-1.5
/// gap-1.5`, its avatar and title at 530, the 44 pt close button always showing at its end, the
/// open tab on the pressed wash.
fn tab_row(s: &App, p: &Pal, t: &crate::tabs::Tab) -> El {
    let uid = t.uid();
    let active = s.current_tab().as_deref() == Some(uid.as_str());
    let lead = if t.is_draft() { icon(&ic::EDIT, 16.0).color(p.icon) } else { avatar(s, p, t) };
    // Mixed servers' tabs, in strip order, with no server badge (`TitlebarTabStrip`).
    let title = crate::view::tab_strip::face(s, t).title;
    let mark = if t.is_draft() { s.tab_status(&t.id) } else { s.tab_status_of(t) };
    let status = mark.filter(|_| !active);
    let (select, close) = (uid.clone(), uid.clone());
    let mut r = row()
        .key(format!("narrow-tab-{uid}"))
        .role("button")
        .label(title.clone())
        .w_full()
        .min_h(44.0)
        .h(44.0)
        .px(6.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            s.select_tab(&select, tx);
            s.tabs_drawer(TabsEvent::Close, tx);
        })
        .children(active.then(|| wash(p.pressed, 6.0)))
        .child(row().size(16.0, 16.0).center().child(lead))
        .child(text(title).key("title").font(TITLE).color(p.text).truncate().grow(1.0).shrink().min_w(0.0).mr(if status.is_some() { 58.0 - 6.0 } else { 38.0 - 6.0 }))
        .children(status.map(|st| status_mark(p, st).pin(None, Some(12.0), Some(44.0), None)))
        .child(
            row()
                .key("close")
                .role("button")
                .label(t!("common.close"))
                .size(44.0, 44.0)
                .radius(4.0)
                .center()
                .pin(None, Some(0.0), Some(0.0), None)
                .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&close, tx))
                .child(icon(&ic::XMARK_SMALL, 16.0).color(p.icon_faint)),
        );
    if active {
        r = r.fill(p.bg_deep);
    }
    crate::session_menu::on(r, crate::view::tab_strip::target(s, t))
}

/// `background: linear-gradient(over, over), base`'s overlay: the first child of an element
/// filled with `base`, under its content.
pub(super) fn wash(over: rune::Color, radius: f32) -> El {
    el().key("wash").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(radius).fill(over)
}

/// The tab's mark in its 20 pt slot, 12 down, before the close button.
fn status_mark(p: &Pal, st: crate::TabStatus) -> El {
    let dot = |c| el().size(7.0, 7.0).radius(3.5).fill(c);
    let mark = match st {
        crate::TabStatus::Busy => session_view::style::tab_busy_glyph(crate::palette::kit(p), 14.0),
        crate::TabStatus::Attention => dot(p.warning),
        crate::TabStatus::Unread => dot(p.bg_accent),
        crate::TabStatus::Failed => dot(p.danger),
    };
    row().key("status").size(20.0, 20.0).center().child(mark)
}
