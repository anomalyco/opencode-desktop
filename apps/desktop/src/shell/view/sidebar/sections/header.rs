//! A Whisper header: no box, no border, no colour; a quiet label on the tabs' own spine, its
//! disclosure triangle where their avatars sit (shown on hover, or while folded). Folded, it
//! counts its tabs and shows whether one works or waits. A press folds it, a double press or
//! Rename section names it in place, and a right-click opens its menu.

use super::*;
use crate::tabs::sections::{self as sec, Drop, FIELD, LIVE, SECTIONS, Section, naming};
use opencode_i18n::t;
use rune::{Drag, Key, Phase};
use rune_kit::shell::TabItemStatus;

/// `text-[12px] font-medium`.
pub(super) const LABEL: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0);
/// A folded section's count, `text-[11px]` in even figures.
const COUNT: TextStyle = TextStyle::new(11.0, 500.0).lh(16.0).features("\"tnum\"");
/// The other names under a new section's field.
const CHIP: TextStyle = TextStyle::new(11.5, 460.0).lh(16.0);

/// What a folded section's tabs are doing, most pressing first.
enum Mark {
    Busy,
    Waiting,
    Unread,
}

fn mark(s: &App, tabs: &[usize]) -> Option<Mark> {
    let statuses: Vec<TabItemStatus> = tabs.iter().filter_map(|&i| s.tabs.get(i)).filter_map(|t| crate::view::tab_strip::status(s, t)).collect();
    if statuses.iter().any(|m| matches!(m, TabItemStatus::Busy)) {
        Some(Mark::Busy)
    } else if statuses.iter().any(|m| matches!(m, TabItemStatus::Attention)) {
        Some(Mark::Waiting)
    } else if statuses.iter().any(|m| matches!(m, TabItemStatus::Unread | TabItemStatus::Failed)) {
        Some(Mark::Unread)
    } else {
        None
    }
}

pub(super) fn header(s: &App, p: &Pal, section: &Section, tabs: &[usize], first: bool) -> El {
    let st = sec::state(s);
    let collapsed = section.collapsed;
    let editing = st.naming.editing().filter(|(id, _, _)| *id == section.id).map(|(_, field, _)| field);
    let target = matches!(st.drag.drop(), Some(Drop::Into(id)) if *id == section.id);
    let triangle = icon(&ic::FILL_TRIANGLE_DOWN, 12.0)
        .key("triangle")
        .color(p.icon_faint)
        .group_color(p.icon_muted)
        .rotate(if collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 })
        .mirror_in_rtl();
    let slot = el().key("disclosure").size(16.0, 16.0).center().a11y_hidden().child(triangle);
    let slot = if collapsed || editing.is_some() { slot } else { session_view::style::hover_reveal(slot) };
    let label: El = match editing {
        Some(field) => name_field(p, field),
        None => text(section.name.clone())
            .key("name")
            .font(LABEL)
            .color(if target { p.text } else { p.muted })
            .group_color(p.text)
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .truncate()
            .dir(rune::Direction::Auto),
    };
    let mut end = row().key("end").h(20.0).min_w(20.0).gap(6.0).center_y();
    if collapsed && editing.is_none() {
        let meta = row()
            .key("meta")
            .gap(6.0)
            .center_y()
            .child(text(tabs.len().to_string()).key("count").font(COUNT).color(p.faint))
            .children(mark(s, tabs).map(|m| match m {
                Mark::Busy => activity_ring(p).key("ring"),
                Mark::Waiting => el().key("waiting").size(6.0, 6.0).radius(3.0).fill(p.warning).a11y_hidden(),
                Mark::Unread => el().key("unread").size(6.0, 6.0).radius(3.0).fill(p.accent).a11y_hidden(),
            }));
        end = end.child(meta.group_opacity(1.0, 0.0));
    }
    if editing.is_none() {
        let id = section.id.clone();
        let remove = row()
            .key("remove")
            .role("button")
            .label_static(t!("rune.sections.remove"))
            .help(t!("rune.sections.remove"))
            .pin(None, Some(0.0), Some(0.0), None)
            .size(20.0, 20.0)
            .center()
            .radius(5.0)
            .focus_inset(p)
            .hover(p.hover)
            .press(p.pressed)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| sec::remove_section(s, &id, tx))
            .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));
        end = end.child(session_view::style::hover_reveal(remove));
    }
    let (toggle_id, rename_id, menu_id, layout_key) = (section.id.clone(), section.id.clone(), section.id.clone(), format!("head:{}", section.id));
    let a11y = if collapsed { format!("{} ({})", section.name, tabs.len()) } else { section.name.clone() };
    let mut head = row()
        .key(format!("sec-head-{}", section.id))
        .role("button")
        .label(a11y)
        .expanded(!collapsed)
        .w_full()
        .h(26.0)
        .mt(if first { 0.0 } else { 12.0 })
        .pl(8.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .focus_inset(p)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&LIVE).rects.borrow_mut().insert(layout_key.clone(), r);
        })
        .child(slot)
        .child(label)
        .child(end);
    if editing.is_none() {
        head = head
            .on_press_tx(move |s: &mut App, tx: &mut Tx| sec::toggle(s, &toggle_id, tx))
            .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
                // The double press folded it twice: it reads as it was, and names it.
                sec::start_rename(s, &rename_id, tx);
            })
            .on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| super::menu::header(s, &menu_id, press.into(), tx));
    }
    if target {
        head = head.fill(p.accent.alpha(0.10));
    }
    head
}

/// The name field, the name selected so typing replaces it, an accent hairline under it.
fn name_field(p: &Pal, field: &rune::TextEdit) -> El {
    let input = session_view::style::field(field, LABEL)
        .key(FIELD)
        .label(t!("rune.sections.name"))
        .spellcheck(false)
        .single_line()
        .w_full()
        .color(p.text)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .on_edit(|s: &mut App| {
            let st = s.kernel.slots.get_mut(&SECTIONS);
            match st.naming.field_mut() {
                Some(f) => f,
                None => &mut st.scratch,
            }
        })
        .on_input_key(|s: &mut App, ev, tx| {
            let event = match ev.key {
                Key::Enter => naming::Event::Commit,
                Key::Escape => naming::Event::Cancel,
                _ => return false,
            };
            sec::naming_step(s, event, tx);
            tx.blur();
            true
        });
    el().key("name-field")
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h(20.0)
        .child(row().pin(Some(0.0), Some(0.0), Some(0.0), Some(1.0)).center_y().child(input))
        .child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(p.accent.alpha(0.55)).a11y_hidden())
}

/// The other names offered while a new section is named, under its field: a press takes one.
pub(super) fn alternates(s: &App, p: &Pal, section: &Section) -> Option<El> {
    let (_, field, names) = sec::state(s).naming.editing().filter(|(id, _, names)| *id == section.id && !names.is_empty())?;
    let chips = names.iter().map(|name| {
        let on = field.text.trim() == name.as_str();
        let pick = name.clone();
        row()
            .key(format!("alt-{name}"))
            .role("button")
            .label(name.clone())
            .h(20.0)
            .px(6.0)
            .center_y()
            .radius(5.0)
            .fill(if on { p.accent.alpha(0.12) } else { Color::TRANSPARENT })
            .hover(if on { p.accent.alpha(0.12) } else { p.hover })
            .hover_group()
            .hover_motion(INSTANT, INSTANT)
            // On the press itself: the field's blur on the way up would end naming first.
            .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
                if d.phase == Phase::Down {
                    sec::naming_step(s, naming::Event::Pick(&pick), tx);
                    tx.blur();
                }
            })
            .child(text(name.clone()).font(CHIP).color(if on { p.text } else { p.faint }).group_color(p.text).truncate())
    });
    Some(row().key(format!("sec-alts-{}", section.id)).w_full().pl(26.0).pr(4.0).pt(2.0).pb(4.0).gap(2.0).flex_wrap().children(chips))
}
