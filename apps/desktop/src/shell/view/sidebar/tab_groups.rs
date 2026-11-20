//! Grouped side tabs (Settings › Experimental › Tab groups; `tab-groups-ui.tsx`,
//! `tab-groups.css`): the filter and grouping menu above the tabs (`VerticalTabGroupsControls`),
//! then each group's header (`TabGroupHeader`) over its tabs, indented along a rail in the
//! group's colour, with ungrouped runs between. A header's label toggles the group, dragging it
//! moves the whole group, and its menu renames, recolours, ungroups or closes it.

use crate::popup::{self, Item, Menu, MenuButton as _};
use crate::tabs::groups::{self as tg, Color as Hue, Group, Kind, Mode, RenameEvent, drag};
use rune_kit::shell::TabOrientation;

use super::*;
use opencode_i18n::t;

/// The group chip's label (`[data-tab-group-chip]`: 12/500).
const CHIP: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).tracking(-0.04);
/// A collapsed group's count (`text-[11px]`).
const COUNT: TextStyle = TextStyle::new(11.0, 400.0).lh(16.0);
const MENU_TRIGGER: &str = "tab-groups-menu";

/// A group colour's chip fill, ink and border, and its rail (`tab-groups.css`'s Fluent palette:
/// a soft mineral wash and deep ink in light mode, a deep jewel tint and luminous ink in dark).
struct Tint {
    bg: Color,
    fg: Color,
    border: Color,
    rail: Color,
}

fn tint(c: Hue, dark: bool) -> Tint {
    use rune::{oklch, oklcha};
    // (hue, chroma of the dark fill, dark ink, light fill, light ink)
    let (h, bd, fd, bl, fl) = match c {
        Hue::Blue => (254.0, 0.056, 0.068, 0.032, 0.105),
        Hue::Cyan => (206.0, 0.046, 0.06, 0.028, 0.092),
        Hue::Green => (158.0, 0.044, 0.058, 0.028, 0.095),
        Hue::Orange => (56.0, 0.045, 0.062, 0.03, 0.1),
        Hue::Purple => (292.0, 0.056, 0.068, 0.032, 0.105),
        Hue::Pink => (350.0, 0.05, 0.064, 0.03, 0.102),
        Hue::Gray if dark => return Tint { bg: oklch(0.248, 0.006, 260.0), fg: oklch(0.855, 0.008, 260.0), border: oklcha(0.35, 0.008, 260.0, 0.5), rail: oklcha(0.64, 0.01, 260.0, 0.78) },
        Hue::Gray => return Tint { bg: oklch(0.942, 0.004, 260.0), fg: oklch(0.39, 0.008, 260.0), border: oklcha(0.865, 0.006, 260.0, 0.72), rail: oklcha(0.54, 0.01, 260.0, 0.78) },
    };
    if dark {
        Tint { bg: oklch(0.252, bd, h), fg: oklch(0.875, fd, h), border: oklcha(0.36, bd * 1.3, h, 0.5), rail: oklcha(0.65, fd * 1.45, h, 0.78) }
    } else {
        Tint { bg: oklch(0.942, bl, h), fg: oklch(0.39, fl, h), border: oklcha(0.865, bl * 1.55, h, 0.72), rail: oklcha(0.55, fl, h, 0.78) }
    }
}

fn color_label(c: Hue) -> &'static str {
    match c {
        Hue::Blue => t!("tabs.groups.color.blue"),
        Hue::Green => t!("tabs.groups.color.green"),
        Hue::Orange => t!("tabs.groups.color.orange"),
        Hue::Purple => t!("tabs.groups.color.purple"),
        Hue::Pink => t!("tabs.groups.color.pink"),
        Hue::Cyan => t!("tabs.groups.color.cyan"),
        Hue::Gray => t!("tabs.groups.color.gray"),
    }
}

/// `VerticalTabGroupsControls`: the filter field (`TabFilterInput`), its grouping menu at its
/// end, `my-2.5` between the actions and the tabs.
pub(super) fn controls(s: &App, p: &Pal) -> El {
    let st = tg::state(s);
    let placeholder = t!("tabs.groups.filter.placeholder");
    let input = session_view::style::field(&st.filter, UI)
        .key("tab-groups-filter-input")
        .label(placeholder)
        .spellcheck(false)
        .single_line()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .color(p.text)
        .placeholder(placeholder, p.faint)
        .caret(p.text, p.selection)
        .selection_style(p.field_selection)
        .on_edit(|s: &mut App| &mut s.kernel.slots.get_mut(&tg::TAB_GROUPS).filter)
        .on_input_key(|s: &mut App, ev, tx| {
            if ev.key != Key::Escape {
                return false;
            }
            // Escape clears a query, then leaves the field.
            tx.instant();
            let filter = &mut s.kernel.slots.get_mut(&tg::TAB_GROUPS).filter;
            if filter.text.is_empty() {
                tx.blur();
            } else {
                *filter = rune::TextEdit::new("");
                filter.focused = true;
            }
            true
        });
    let mut field = row()
        .key("tab-groups-filter")
        .w_full()
        .h(28.0)
        .pl(8.0)
        .pr(4.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(if st.filter.focused { p.layer2 } else { p.layer1 })
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(input);
    if !st.filter.text.is_empty() {
        field = field.child(
            super::super::home::projects::icon_button(p, "tab-groups-filter-clear", t!("common.clear"), &ic::CLOSE, false).on_press_tx(|s: &mut App, tx: &mut Tx| {
                tx.instant();
                let filter = &mut s.kernel.slots.get_mut(&tg::TAB_GROUPS).filter;
                *filter = rune::TextEdit::new("");
                filter.focused = true;
                tx.focus("tab-groups-filter-input", false);
            }),
        );
    }
    let open = s.popup.open_at(MENU_TRIGGER);
    let menu = super::super::home::projects::icon_button(p, MENU_TRIGGER, t!("tabs.groups.menu"), &ic::OUTLINE_SLIDERS, open).help(t!("tabs.groups.menu")).menu_button(MENU_TRIGGER, open_grouping_menu);
    row().key("tab-groups-controls").w_full().my(10.0).child(field.child(menu))
}

/// `TabGroupingMenu`: Automatic grouping's None, Server and Project, and New group while
/// grouping is none.
fn open_grouping_menu(s: &mut App, tx: &mut Tx) {
    let mode = tg::state(s).doc.mode;
    let label = |m: Mode| match m {
        Mode::None => t!("tabs.groups.mode.none"),
        Mode::Server => t!("tabs.groups.mode.server"),
        Mode::Project => t!("tabs.groups.mode.project"),
    };
    let mut menu = Mode::ALL.into_iter().fold(Menu::new(t!("tabs.groups.menu")).min_width(168.0).gap(4.0).group(t!("tabs.groups.groupBy")), |m, x| m.item(Item::new(format!("mode:{}", x.name()), label(x)).checked(x == mode)));
    if tg::effective_mode(s) == Mode::None {
        menu = menu.separator().item(Item::new("new", t!("tabs.groups.new")).icon(&ic::PLUS));
    }
    tx.instant();
    popup::open_drawn(s, menu, MENU_TRIGGER.to_owned(), tx, |s: &mut App, picked, tx| {
        tx.instant();
        match picked {
            Some("new") => tg::create(s, None),
            Some(id) => {
                if let Some(m) = id.strip_prefix("mode:").map(|m| Mode::parse(Some(m))) {
                    tg::set_mode(s, m);
                }
            }
            None => {}
        }
    });
}

/// The side tabs, grouped; ungrouped (one strip) while nothing groups them.
pub(super) fn list(s: &App, p: &Pal) -> El {
    let items = tg::metas(s);
    let Some(groups) = tg::groups(s, &items) else {
        return crate::view::tab_strip::tab_strip(s, p, "tabs", TabOrientation::Vertical, None, None);
    };
    let st = tg::state(s);
    let mut sections = col().key("tab-groups").w_full();
    for (n, g) in groups.iter().enumerate() {
        sections = sections.child(section(s, p, g, n == 0));
    }
    if let Some(y) = st.drag.line() {
        // Where the dragged group would land.
        sections = sections.child(el().key("tab-groups-drop").absolute(0.0, y - 1.0).w_full().h(2.0).radius(1.0).fill(p.interactive).raise());
    }
    let viewport = col().grow(1.0).shrink().min_h(0.0);
    session_view::scroller::scroller("tab-groups-scroll").build::<App>(p, viewport, sections)
}

/// One section (`[data-tab-group]`): a group's header and its tabs along the rail, or an
/// ungrouped run's tabs.
fn section(s: &App, p: &Pal, g: &Group, first: bool) -> El {
    let key = g.key.clone();
    let layout_key = key.clone();
    let grouped = g.kind != Kind::Ungrouped;
    let mut c = col()
        .key(format!("tg-{key}"))
        .w_full()
        .gap(1.0)
        .on_layout_tx(move |s: &mut App, r, tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&tg::RECTS).borrow_mut().insert(layout_key.clone(), r);
        });
    if grouped && !first {
        c = c.mt(2.0);
    }
    if grouped {
        c = c.child(header(s, p, g));
    }
    if !g.visible.is_empty() {
        let strip_key = format!("tg-tabs-{key}");
        let h = rune::memo::layout_with(&format!("{strip_key}-list"), |r| r[3]).unwrap_or(0.0);
        let strip = col().key(format!("{strip_key}-box")).w_full().h(h).child(crate::view::tab_strip::section_strip(s, p, &strip_key, &g.visible, false));
        c = if grouped {
            let rail = el().key("rail").absolute(8.0, 3.0).w(1.5).h((h - 8.0).max(0.0)).radius(1.0).fill(tint(g.color, p.dark).rail);
            c.child(col().key("items").w_full().pl(16.0).child(rail).child(strip))
        } else {
            c.child(strip)
        };
    }
    match tg::state(s).drag.dragging(&key) {
        // It follows the pointer over the strip.
        Some(dy) => c.raise().opacity(0.9).offset(0.0, dy).offset_motion(rune::Anim::INSTANT),
        None => c,
    }
}

/// `TabGroupHeader`: the chip (or its name field while renaming), a collapsed group's count,
/// the chevron, and More options.
fn header(s: &App, p: &Pal, g: &Group) -> El {
    let ink = tint(g.color, p.dark);
    let st = tg::state(s);
    let label: El = match st.rename.editing(&g.key) {
        Some(edit) => session_view::style::field(edit, CHIP)
            .key("tab-group-name")
            .label(t!("tabs.groups.name.placeholder"))
            .single_line()
            .grow(1.0)
            .shrink()
            .min_w(0.0)
            .color(ink.fg)
            .placeholder(if g.default_name.is_empty() { t!("tabs.groups.name.placeholder").to_owned() } else { g.default_name.clone() }, p.faint)
            .caret(ink.fg, p.selection)
            .selection_style(p.field_selection)
            .on_edit(|s: &mut App| {
                let st = s.kernel.slots.get_mut(&tg::TAB_GROUPS);
                match &mut st.rename {
                    tg::Rename::Arming { field, .. } | tg::Rename::Editing { field, .. } => field,
                    // The field shows only while editing.
                    tg::Rename::Idle => &mut st.filter,
                }
            })
            .on_input_change(|s: &mut App, tx: &mut Tx| tg::rename_step(s, RenameEvent::Typed, tx))
            .on_input_key(|s: &mut App, ev, tx| {
                if !matches!(ev.key, Key::Escape | Key::Enter) {
                    return false;
                }
                tg::rename_step(s, RenameEvent::Done, tx);
                tx.blur();
                true
            }),
        None => text(g.name.clone()).font(CHIP).color(ink.fg).shrink().truncate().dir(rune::Direction::Auto),
    };
    let chip = row().key("chip").h(20.0).px(7.5).radius(5.0).fill(ink.bg).border(1.0, ink.border).center_y().shrink().min_w(0.0).child(label);
    let count = g.all.len();
    let collapsed = g.collapsed;
    let chevron = icon(&ic::CHEVRON_DOWN, 16.0).key("chevron").color(p.icon_muted).rotate(if collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 }).mirror_in_rtl();
    let (press_key, drag_key) = (g.key.clone(), g.key.clone());
    let mut toggle = row()
        .key("toggle")
        .role("button")
        .label(g.name.clone())
        .expanded(!collapsed)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h_full()
        .gap(6.0)
        .center_y()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| tg::header_press(s, &press_key, tx))
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| header_drag(s, &drag_key, d, tx))
        .child(chip);
    if collapsed && count > 0 {
        toggle = toggle.child(text(count.to_string()).key("count").font(COUNT).color(p.faint));
    }
    toggle = toggle.child(spacer()).child(if collapsed { chevron } else { chevron.group_opacity(0.0, 1.0) });
    let menu_key = format!("tg-menu-{}", g.key);
    let open = s.popup.open_at(&menu_key);
    let (trigger_key, group_key, ctx_key) = (menu_key.clone(), g.key.clone(), g.key.clone());
    let dots = super::super::home::projects::icon_button(p, &menu_key, t!("common.moreOptions"), &ic::OUTLINE_DOTS, open).menu_button(menu_key.clone(), move |s: &mut App, tx: &mut Tx| open_group_menu(s, &group_key, trigger_key.clone().into(), tx));
    row()
        .key(format!("tg-head-{}", g.key))
        .w_full()
        .h(26.0)
        .pr(2.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open_group_menu(s, &ctx_key, press.into(), tx))
        .child(toggle)
        .child(if open { dots } else { session_view::style::hover_reveal(dots) })
}

/// `GroupEditorMenuItems`: Rename (the header's name field), Group color, then Ungroup for a
/// manual group and Close group.
fn open_group_menu(s: &mut App, key: &str, at: rune::PopupAnchor, tx: &mut Tx) {
    let items = tg::metas(s);
    let Some(g) = tg::groups(s, &items).into_iter().flatten().find(|g| g.key == key) else { return };
    let colors = tg::model::AUTO_COLORS.into_iter().fold(Menu::new(t!("tabs.groups.color")), |m, c| m.item(Item::new(format!("color:{}", c.name()), color_label(c)).checked(c == g.color)));
    let mut menu = Menu::new(g.name.clone()).min_width(192.0).item(Item::new("rename", t!("common.rename"))).submenu("color", t!("tabs.groups.color"), colors);
    if g.manual_id().is_some() || !g.all.is_empty() {
        menu = menu.separator();
    }
    if g.manual_id().is_some() {
        menu = menu.item(Item::new("ungroup", t!("tabs.groups.ungroup")));
    }
    if !g.all.is_empty() {
        menu = menu.item(Item::new("close", t!("tabs.groups.closeGroup")));
    }
    tx.instant();
    let (key, name, manual) = (g.key.clone(), g.name.clone(), g.manual_id().map(str::to_owned));
    popup::open(s, menu, at, tx, move |s: &mut App, picked, tx| {
        tx.instant();
        match picked {
            Some("rename") => tg::rename_step(s, RenameEvent::Start { group: &key, name: &name }, tx),
            Some("ungroup") => {
                if let Some(id) = &manual {
                    tg::ungroup(s, id);
                }
            }
            Some("close") => tg::close(s, &key, tx),
            Some(id) => {
                if let Some(c) = id.strip_prefix("color:").and_then(|c| tg::model::Color::parse(Some(c))) {
                    tg::recolor(s, &key, c);
                }
            }
            None => {}
        }
    });
}

/// A header drag (`onPointerDown` and its window listeners): the strip's blocks are read once,
/// as the press becomes a drag. No drag while a filter hides tabs.
fn header_drag(s: &mut App, key: &str, d: &mut Drag, tx: &mut Tx) {
    if !tg::state(s).filter.text.trim().is_empty() {
        return;
    }
    let event = match d.phase {
        Phase::Down => drag::Event::Down { group: key.to_owned(), touch: session_view::style::touch_controls() },
        Phase::Move => {
            // Measured as the press turns into a drag (`selectNeedsDragMeasurement`).
            let pending = matches!(tg::state(s).drag, drag::State::Pending { .. });
            let (blocks, top) = if pending { blocks(s) } else { (vec![], 0.0) };
            drag::Event::Move { dx: d.dx, dy: d.dy, grab: d.start_y - top, blocks }
        }
        Phase::Up if d.cancelled() => drag::Event::Cancel,
        Phase::Up => drag::Event::Up,
    };
    tg::drag_step(s, event, tx);
}

/// `measureGeometry`'s blocks, in points from the first section's top: each group whole, each
/// ungrouped tab on its own (an ungrouped run's height shared among its tabs).
fn blocks(s: &App) -> (Vec<drag::Block>, f32) {
    let items = tg::metas(s);
    let groups = tg::groups(s, &items).unwrap_or_default();
    let rects = s.kernel.slots.untracked(&tg::RECTS).borrow();
    let top = groups.first().and_then(|g| rects.get(&g.key)).map_or(0.0, |r| r[1]);
    let mut out = vec![];
    for g in &groups {
        let Some(r) = rects.get(&g.key) else { continue };
        let (y, h) = (r[1] - top, r[3]);
        if g.kind != Kind::Ungrouped {
            out.push(drag::Block::Group { key: g.key.clone(), tabs: g.all.iter().map(|&i| items[i].key.clone()).collect(), top: y, bottom: y + h });
            continue;
        }
        let step = h / g.visible.len().max(1) as f32;
        for (n, &i) in g.visible.iter().enumerate() {
            let at = y + step * n as f32;
            out.push(drag::Block::Tab { key: items[i].key.clone(), top: at, bottom: at + step });
        }
    }
    (out, top)
}
