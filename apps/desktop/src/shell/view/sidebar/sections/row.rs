//! A side tab in the sections list: the strip's own row (28 tall, `px-2 gap-2`, a 16 pt avatar,
//! the title, its mark or a close button on hover), on the same spine in a section or out of one.
//! A selected tab wears a soft accent wash. Each row stands on the sidebar's own ground, so two
//! that cross as they glide never show through one another.

use super::*;
use crate::tabs::sections::{self as sec, LIVE};
use opencode_i18n::t;
use rune::{Drag, PointerPress};
use rune_kit::shell::TabItemStatus;

/// The slot at a row's end, 8 pt in from it: a tab's mark, a folded section's, and the button
/// that takes the mark's place under the pointer, all on one column down the list.
pub(super) const SLOT: f32 = 20.0;

/// `what` centred in the slot, out of flow: the button that shares the slot can't push it aside.
pub(super) fn in_slot(what: El) -> El {
    row().key("mark").absolute(0.0, 0.0).size(SLOT, SLOT).center().a11y_hidden().child(what)
}

/// What a status draws in the slot: the busy glyph, or a 7 pt dot.
pub(super) fn status_mark(p: &Pal, status: TabItemStatus) -> El {
    let dot = |key: &'static str, c: Color| el().key(key).size(7.0, 7.0).radius(3.5).fill(c).a11y_hidden();
    match status {
        TabItemStatus::Busy => session_view::style::tab_busy_glyph(crate::palette::kit(p), 14.0),
        TabItemStatus::Attention => dot("attention", p.warning),
        TabItemStatus::Unread => dot("unread", p.accent),
        TabItemStatus::Failed => dot("failed", p.danger),
    }
}

/// `over`: this tab flies above the others (its section was just made).
pub(super) fn tab_row(s: &App, p: &Pal, t: &crate::tabs::Tab, index: usize, nav_order: &[String], show_hints: bool, over: bool) -> El {
    let st = sec::state(s);
    let uid = t.uid();
    let draft = t.is_draft();
    let face = crate::view::tab_strip::face(s, t);
    let target = crate::view::tab_strip::target(s, t);
    let marked = s.kernel.slots.get(&crate::session_menu::MENUS).on.as_ref() == Some(&target);
    let active = !s.settings_open() && s.current_tab().as_deref() == Some(uid.as_str());
    let picked = st.selection.contains(&uid);
    let carried = st.drag.carried(&uid);
    let mark = crate::view::tab_strip::status(s, t);
    let shortcut = nav_order.iter().position(|u| u == &uid).filter(|&i| i < 9).map(|i| i + 1);
    let avatar = match (draft, shortcut) {
        (true, Some(n)) => crate::view::tab_strip::draft_avatar(p, active, n, show_hints),
        (true, None) => icon(&crate::view::tab_strip::DRAFT_PENCIL, 16.0).color(if active { p.icon } else { p.icon_muted }),
        (false, _) => crate::icons::tab_badge(s, p, face.project, &project_or_dir(face.project, face.dir), shortcut, show_hints),
    };
    let title = match crate::session_menu_view::field(s, p, &target, UI_MED) {
        Some(field) => field,
        None => text(face.title.clone())
            .font(UI)
            .color(if active || picked || mark.is_some() { p.text } else { p.muted })
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate_fade()
            .dir(rune::Direction::Auto),
    };
    // The open tab has been read: only what it is doing or asking shows.
    let status = mark.filter(|m| !active || matches!(m, TabItemStatus::Busy | TabItemStatus::Attention)).map(|m| status_mark(p, m));
    let close_uid = uid.clone();
    let close = row()
        .key(format!("sec-close-{uid}"))
        .role("button")
        .label_static(t!("common.closeTab"))
        .focusable(true)
        .absolute(0.0, 0.0)
        .size(SLOT, SLOT)
        .center()
        .radius(4.0)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&close_uid, tx))
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));
    let tail = el().key("tail").size(SLOT, SLOT).children(status.map(|m| in_slot(m).group_opacity(1.0, 0.0))).child(session_view::style::hover_reveal(close));

    let (press_uid, middle_uid, hover_uid, drag_uid, menu_uid) = (uid.clone(), uid.clone(), uid.clone(), uid.clone(), uid.clone());
    let anchor = format!("sec-tab-{uid}");
    let hover_anchor = anchor.clone();
    let layout_key = format!("row:{uid}");
    let rename = target.clone();
    let lifted = carried.is_some_and(|dy| dy != 0.0);
    let fill = if picked {
        p.accent.alpha(if p.dark { 0.15 } else { 0.11 })
    } else if active || lifted {
        p.pressed
    } else if marked {
        p.hover
    } else {
        Color::TRANSPARENT
    };
    let r = row()
        .key(anchor)
        .role("tab")
        .label(face.title.clone())
        .focus_selected(active)
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .focus_inset(p)
        .fill(fill)
        .hover(if picked || active || lifted || marked { Color::TRANSPARENT } else { p.hover })
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        // The wash comes and goes on the list's spring.
        .motion(super::GLIDE)
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&LIVE).rects.borrow_mut().insert(layout_key.clone(), r);
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| sec::press(s, &press_uid, tx))
        .closable_tab(move |s: &mut App, tx: &mut Tx| s.close_tab(&middle_uid, tx))
        .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.begin_rename(&rename);
        })
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                s.prefetch_session(&hover_uid);
            }
            s.preview_hover_at(&hover_uid, &hover_anchor, inside, tx);
        })
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| sec::tab_drag(s, &drag_uid, index, d, tx))
        .on_secondary_press(move |s: &mut App, press: PointerPress, tx: &mut Tx| super::menu::tab(s, &menu_uid, press, tx))
        .child(el().size(16.0, 16.0).center().child(avatar))
        .child(title)
        .child(tail);
    // Always drawn, clear when not picked, so the edge fades with the wash rather than popping: a
    // hairline that only firms the wash's rim, never an outline.
    let r = r.box_shadow(Pal::ring_in(p.accent.alpha(if picked { 0.14 } else { 0.0 })));
    // Into a section, back out, or past a fold: the tab glides to its slot on the sidebar's own
    // ground, unseen at rest, so a tab it crosses on the way never shows through it. The tabs of
    // a section just made fly up over the ones they pass.
    let slot = el().key(format!("sec-slot-{uid}")).w_full().radius(6.0).fill(p.bg_deep).motion(super::GLIDE).matched(format!("{}-{uid}", crate::view::tab_strip::MATCHED)).child(r);
    let slot = if over { slot.raise() } else { slot };
    match carried {
        Some(dy) if dy != 0.0 => slot.raise().opacity(0.96).offset(0.0, dy).offset_motion(rune::Anim::INSTANT),
        Some(_) => slot.opacity(0.4),
        None => slot,
    }
}
