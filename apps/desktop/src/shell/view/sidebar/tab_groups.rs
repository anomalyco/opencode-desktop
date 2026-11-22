//! Grouped side tabs view (`packages/app/src/shell/titlebar/tab-groups-ui.tsx`, `tab-groups.css`,
//! #64407): inline filter + grouping menu (`controls`), collapsible colored group headers, RTL-safe
//! colored rails, direct tab rows (no shared-`TabDragState` cross-group bleed, no 0-height first-frame
//! collapse), full 4-intent drag indicators/shifts, and horizontal tear-out support.

use rune_kit::shell::{TabItemStatus, TabOrientation};

use super::*;
use crate::popup::{self, Item, Menu, MenuButton as _};
use crate::tabs::groups::{self as tg, Color as Hue, Group, Kind, Mode, RenameEvent, drag};
use opencode_i18n::t;

const CHIP: TextStyle = TextStyle::new(12.0, 500.0).lh(16.0).tracking(-0.04);
const COUNT: TextStyle = TextStyle::new(11.0, 400.0).lh(16.0);
const MENU_TRIGGER: &str = "tab-groups-menu";

struct Tint {
    bg: Color,
    fg: Color,
    border: Color,
    rail: Color,
}

fn tint(c: Hue, dark: bool) -> Tint {
    use rune::{oklch, oklcha};
    let (h, bd, fd, bl, fl) = match c {
        Hue::Blue => (254.0, 0.056, 0.068, 0.032, 0.105),
        Hue::Cyan => (206.0, 0.046, 0.06, 0.028, 0.092),
        Hue::Green => (158.0, 0.044, 0.058, 0.028, 0.095),
        Hue::Orange => (56.0, 0.045, 0.062, 0.03, 0.1),
        Hue::Purple => (292.0, 0.056, 0.068, 0.032, 0.105),
        Hue::Pink => (350.0, 0.05, 0.064, 0.03, 0.102),
        Hue::Gray if dark => {
            return Tint {
                bg: oklch(0.248, 0.006, 260.0),
                fg: oklch(0.855, 0.008, 260.0),
                border: oklcha(0.35, 0.008, 260.0, 0.5),
                rail: oklcha(0.64, 0.01, 260.0, 0.78),
            };
        }
        Hue::Gray => {
            return Tint {
                bg: oklch(0.942, 0.004, 260.0),
                fg: oklch(0.39, 0.008, 260.0),
                border: oklcha(0.865, 0.006, 260.0, 0.72),
                rail: oklcha(0.54, 0.01, 260.0, 0.78),
            };
        }
    };
    if dark {
        Tint {
            bg: oklch(0.252, bd, h),
            fg: oklch(0.875, fd, h),
            border: oklcha(0.36, bd * 1.3, h, 0.5),
            rail: oklcha(0.65, fd * 1.45, h, 0.78),
        }
    } else {
        Tint {
            bg: oklch(0.942, bl, h),
            fg: oklch(0.39, fl, h),
            border: oklcha(0.865, bl * 1.55, h, 0.72),
            rail: oklcha(0.55, fl, h, 0.78),
        }
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

/// Filter field and automatic grouping menu above the side tabs (`VerticalTabGroupsControls`).
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
    let menu = super::super::home::projects::icon_button(p, MENU_TRIGGER, t!("tabs.groups.menu"), &ic::OUTLINE_SLIDERS, open)
        .menu_button(MENU_TRIGGER, open_grouping_menu);
    let menu = if open { menu } else { menu.help(t!("tabs.groups.menu")) };
    row().key("tab-groups-controls").w_full().my(10.0).child(field.child(menu))
}

fn open_grouping_menu(s: &mut App, tx: &mut Tx) {
    let mode = tg::state(s).doc.mode;
    let label = |m: Mode| match m {
        Mode::None => t!("tabs.groups.mode.none"),
        Mode::Server => t!("tabs.groups.mode.server"),
        Mode::Project => t!("tabs.groups.mode.project"),
    };
    let mut menu = Mode::ALL.into_iter().fold(
        Menu::new(t!("tabs.groups.menu")).min_width(168.0).gap(4.0).group(t!("tabs.groups.groupBy")),
        |m, x| m.item(Item::new(format!("mode:{}", x.name()), label(x)).checked(x == mode)),
    );
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

/// Grouped side tabs list; falls back to the plain `tab_strip` when ungrouped and unfiltered.
pub(super) fn list(s: &App, p: &Pal) -> El {
    let items = tg::metas(s);
    let Some(groups) = tg::groups(s, &items) else {
        return crate::view::tab_strip::tab_strip(s, p, "tabs", TabOrientation::Vertical, None, None);
    };
    let nav_order = s.nav_order();
    let show_hints = crate::tab_hints::shown(s);
    let st = tg::state(s);
    let mut sections = col()
        .key("tab-groups")
        .w_full()
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&tg::LAYOUT).rects.borrow_mut().insert("list".into(), r);
            s.strip_laid_out(r);
        });
    for (n, g) in groups.iter().enumerate() {
        sections = sections.child(section(s, p, g, n == 0, &nav_order, show_hints));
    }
    if let Some(drag::Indicator::Line { visual_y, indented, color, .. }) = st.drag.indicator() {
        let line_color = color.map_or(p.interactive, |c| tint(c, p.dark).rail);
        let inset = if *indented { 16.0 } else { 0.0 };
        sections = sections.child(
            el().key("tab-groups-drop")
                .pin(Some(inset), Some(*visual_y - 1.0), Some(0.0), None)
                .h(2.0)
                .radius(1.0)
                .fill(line_color)
                .raise()
                .a11y_hidden(),
        );
    }
    let viewport = col().grow(1.0).shrink().min_h(0.0).focus_inset(p);
    session_view::scroller::scroller("tab-groups-scroll").build::<App>(p, viewport, sections)
}

fn section(s: &App, p: &Pal, g: &Group, first: bool, nav_order: &[String], show_hints: bool) -> El {
    let key = g.key.clone();
    let layout_key = format!("section:{key}");
    let grouped = g.kind != Kind::Ungrouped;
    let mut c = col()
        .key(format!("tg-{key}"))
        .w_full()
        .gap(2.0)
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&tg::LAYOUT).rects.borrow_mut().insert(layout_key.clone(), r);
        });
    if grouped {
        c = c.role("group").label(g.name.clone());
    }
    if grouped && !first {
        c = c.mt(4.0);
    }
    if grouped {
        c = c.child(header(s, p, g));
    }
    if !g.visible.is_empty() {
        let mut rows = col()
            .key(format!("tg-rows-{key}"))
            .role("tablist")
            .label(if grouped { g.name.clone() } else { t!("settings.general.row.interfaceMode.side").to_owned() })
            .w_full()
            .gap(2.0);
        for &idx in &g.visible {
            if let Some(tab) = s.tabs.get(idx) {
                rows = rows.child(tab_row(s, p, g, tab, idx, nav_order, show_hints));
            }
        }
        c = if grouped {
            // Logical start rail (`pin(Some(8.0), ...)`) so RTL mirrors automatically.
            let rail = el()
                .key("rail")
                .pin(Some(8.0), Some(3.0), None, Some(5.0))
                .w(1.5)
                .radius(1.0)
                .fill(tint(g.color, p.dark).rail)
                .a11y_hidden();
            c.child(col().key("items").w_full().pl(16.0).child(rail).child(rows))
        } else {
            c.child(rows)
        };
    }
    let drag_st = &tg::state(s).drag;
    if let Some(dy) = drag_st.dragging_group(&key) {
        return c.raise().opacity(0.9).offset(0.0, dy).offset_motion(rune::Anim::INSTANT);
    }
    let shift = drag_st.group_shift(&key);
    if shift != 0.0 {
        c = c.offset(0.0, shift);
    }
    c
}

fn header(s: &App, p: &Pal, g: &Group) -> El {
    let ink = tint(g.color, p.dark);
    let st = tg::state(s);
    let label_el: El = match st.rename.editing(&g.key) {
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
                    tg::Rename::Idle => &mut st.scratch,
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
    let chip = row().key("chip").h(20.0).px(7.5).radius(5.0).fill(ink.bg).border(1.0, ink.border).center_y().shrink().min_w(0.0).child(label_el);
    let count = g.all.len();
    let collapsed = g.collapsed;
    let chevron = icon(&ic::CHEVRON_DOWN, 16.0).key("chevron").color(p.icon_muted).rotate(if collapsed { -std::f32::consts::FRAC_PI_2 } else { 0.0 }).mirror_in_rtl();
    let (press_key, drag_key, layout_key) = (g.key.clone(), g.key.clone(), format!("header:{}", g.key));
    let a11y_label = if collapsed && count > 0 { format!("{} ({count})", g.name) } else { g.name.clone() };
    let mut toggle = row()
        .key("toggle")
        .role("button")
        .label(a11y_label)
        .expanded(!collapsed)
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .h_full()
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
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
    let dots = super::super::home::projects::icon_button(p, &menu_key, t!("common.moreOptions"), &ic::OUTLINE_DOTS, open)
        .menu_button(menu_key.clone(), move |s: &mut App, tx: &mut Tx| open_group_menu(s, &group_key, trigger_key.clone().into(), tx));
    let joining = st.drag.join_group_key() == Some(g.key.as_str());
    let mut head = row()
        .key(format!("tg-head-{}", g.key))
        .w_full()
        .h(26.0)
        .pr(2.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover_group()
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&tg::LAYOUT).rects.borrow_mut().insert(layout_key.clone(), r);
        })
        .on_secondary_press(move |s: &mut App, press: rune::PointerPress, tx: &mut Tx| open_group_menu(s, &ctx_key, press.into(), tx))
        .child(toggle)
        .child(if open { dots } else { session_view::style::hover_reveal(dots) });
    if joining {
        head = head.fill(ink.bg).box_shadow(Pal::ring_in(ink.rail));
    }
    let shift = st.drag.header_shift(&g.key);
    if shift != 0.0 {
        head = head.offset(0.0, shift);
    }
    head
}

fn tab_row(s: &App, p: &Pal, g: &Group, t: &crate::tabs::Tab, tab_index: usize, nav_order: &[String], show_hints: bool) -> El {
    let uid = t.uid();
    let tkey = tg::tab_key(t);
    let draft = t.is_draft();
    let face = crate::view::tab_strip::face(s, t);
    let target = crate::view::tab_strip::target(s, t);
    let marked = s.kernel.slots.get(&crate::session_menu::MENUS).on.as_ref() == Some(&target);
    let active = !s.settings_open() && s.current_tab().as_deref() == Some(uid.as_str());
    let mark = crate::view::tab_strip::status(s, t);
    let shortcut = nav_order.iter().position(|u| u == &uid).filter(|&i| i < 9).map(|i| i + 1);
    let avatar = match (draft, shortcut) {
        (true, Some(n)) => crate::view::tab_strip::draft_avatar(p, active, n, show_hints),
        (true, None) => icon(&crate::view::tab_strip::DRAFT_PENCIL, 16.0).color(if active { p.icon } else { p.icon_muted }),
        (false, _) => crate::icons::tab_badge(s, p, face.project, &project_or_dir(face.project, face.dir), shortcut, show_hints),
    };
    let title_el = match crate::session_menu_view::field(s, p, &target, UI_MED) {
        Some(field) => field,
        None => text(face.title.clone()).font(UI).color(if active || mark.is_some() { p.text } else { p.muted }).grow(1.0).shrink().min_w(0.0).truncate_fade().dir(rune::Direction::Auto),
    };
    let status_el = match mark {
        Some(TabItemStatus::Busy) => Some(session_view::style::tab_busy_glyph(crate::palette::kit(p), 14.0)),
        Some(TabItemStatus::Attention) => Some(el().key("attention").size(7.0, 7.0).radius(3.5).fill(p.warning).a11y_hidden()),
        Some(TabItemStatus::Unread) if !active => Some(el().key("unread").size(7.0, 7.0).radius(3.5).fill(p.accent).a11y_hidden()),
        Some(TabItemStatus::Failed) if !active => Some(el().key("failed").size(7.0, 7.0).radius(3.5).fill(p.danger).a11y_hidden()),
        _ => None,
    };
    let close_uid = uid.clone();
    let close_btn = row()
        .key(format!("tg-close-{uid}"))
        .role("button")
        .label_static(t!("common.closeTab"))
        .focusable(true)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .hover(p.hover)
        .press(p.pressed)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.close_tab(&close_uid, tx))
        .child(icon(&ic::XMARK_SMALL, 14.0).color(p.icon_muted));
    let tail = el()
        .key("tail")
        .size(20.0, 20.0)
        .center()
        .children(status_el.map(|m| m.group_opacity(1.0, 0.0)))
        .child(session_view::style::hover_reveal(close_btn));

    let press_uid = uid.clone();
    let middle_uid = uid.clone();
    let dbl_target = target.clone();
    let hover_uid = uid.clone();
    let move_uid = uid.clone();
    let layout_key = format!("tab:{tkey}");
    let drag_tkey = tkey.clone();
    let drag_uid = uid.clone();
    let drag_gk = (g.kind != Kind::Ungrouped).then(|| g.key.clone());
    let drag_mid = g.manual_id().map(str::to_owned);

    let drag_st = &tg::state(s).drag;
    let combine = drag_st.combine_tab(&tkey);
    let dragging = drag_st.dragging_tab(&tkey);

    let mut slot = row()
        .key(format!("tg-tab-{uid}"))
        .role("tab")
        .label(face.title.clone())
        .focus_selected(active)
        .w_full()
        .h(28.0)
        .px(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_inset(p)
        .fill(if active || dragging.is_some() { p.pressed } else if marked { p.hover } else { Color::TRANSPARENT })
        .hover(if active || dragging.is_some() || marked { Color::TRANSPARENT } else { p.hover })
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .matched(format!("{}-{uid}", crate::view::tab_strip::MATCHED))
        .on_layout_tx(move |s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&tg::LAYOUT).rects.borrow_mut().insert(layout_key.clone(), r);
        })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| tg::tab_press(s, &press_uid, tx))
        .closable_tab(move |s: &mut App, tx: &mut Tx| s.close_tab(&middle_uid, tx))
        .on_double_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.begin_rename(&dbl_target);
        })
        .on_hover_change(move |s: &mut App, inside, tx: &mut Tx| {
            if inside {
                s.prefetch_session(&hover_uid);
            }
            s.preview_hover(&hover_uid, inside, tx);
        })
        .on_pointer_move_local(move |s: &mut App, pt: rune::PointerEvent, tx: &mut Tx| s.preview_pointer(&move_uid, pt.y, pt.window_y, tx))
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tab_drag(s, &drag_tkey, &drag_uid, tab_index, drag_gk.clone(), drag_mid.clone(), d, tx);
        })
        .child(el().size(16.0, 16.0).center().child(avatar))
        .child(title_el)
        .child(tail);

    if let Some((color, _)) = combine {
        let c = tint(color, p.dark);
        slot = slot.fill(c.bg).box_shadow(Pal::ring_in(c.rail));
    }
    if let Some((dy, lane)) = dragging {
        let dx = match lane {
            Some(drag::LaneShift::In) => 16.0,
            Some(drag::LaneShift::Out) => -16.0,
            None => 0.0,
        };
        slot = slot.raise().opacity(0.92).offset(dx, dy).offset_motion(rune::Anim::INSTANT);
    } else {
        let shift = drag_st.tab_shift(&tkey);
        if shift != 0.0 {
            slot = slot.offset(0.0, shift);
        }
    }
    crate::session_menu::on(slot, target)
}

fn open_group_menu(s: &mut App, key: &str, at: rune::PopupAnchor, tx: &mut Tx) {
    let items = tg::metas(s);
    let Some(g) = tg::groups(s, &items).into_iter().flatten().find(|g| g.key == key) else { return };
    let colors = tg::model::AUTO_COLORS.into_iter().fold(Menu::new(t!("tabs.groups.color")), |m, c| {
        m.item(Item::new(format!("color:{}", c.name()), color_label(c)).checked(c == g.color))
    });
    let mut menu = Menu::new(g.name.clone())
        .min_width(192.0)
        .item(Item::new("rename", t!("common.rename")))
        .submenu("color", t!("tabs.groups.color"), colors);
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

fn header_drag(s: &mut App, key: &str, d: &mut Drag, tx: &mut Tx) {
    if !tg::state(s).filter.text.trim().is_empty() {
        return;
    }
    let now = *s.clock;
    let event = match d.phase {
        Phase::Down => drag::Event::Down {
            source: drag::Source::Group { group_key: key.to_owned() },
            touch: session_view::style::touch_controls(),
        },
        Phase::Move => {
            let pending = matches!(tg::state(s).drag, drag::State::Pending { .. });
            let (geom, top) = if pending { measure_geometry(s) } else { (None, 0.0) };
            let grab = if pending { d.start_y - top } else { 0.0 };
            drag::Event::Move { x: d.window_point()[0], dx: d.dx, dy: d.dy, grab, geometry: geom, now }
        }
        Phase::Up if d.cancelled() => drag::Event::Cancel,
        Phase::Up => drag::Event::Up,
    };
    tg::drag_step(s, event, tx);
}

#[allow(clippy::too_many_arguments)]
fn tab_drag(
    s: &mut App,
    tkey: &str,
    uid: &str,
    tab_index: usize,
    group_key: Option<String>,
    manual_id: Option<String>,
    d: &mut Drag,
    tx: &mut Tx,
) {
    // Mouse-down selects tabs immediately on desktop (`ours.md` §5.8), before any drag starts.
    if d.phase == Phase::Down && !session_view::style::touch_controls() && s.current_tab().as_deref() != Some(uid) {
        s.select_tab(uid, tx);
    }
    if tg::handle_tab_tear(s, uid, tab_index, d, tx) {
        return;
    }
    if !tg::state(s).filter.text.trim().is_empty() {
        return;
    }
    let now = *s.clock;
    let event = match d.phase {
        Phase::Down => drag::Event::Down {
            source: drag::Source::Tab { tab_key: tkey.to_owned(), group_key, manual_id },
            touch: session_view::style::touch_controls(),
        },
        Phase::Move => {
            let pending = matches!(tg::state(s).drag, drag::State::Pending { .. });
            let (geom, top) = if pending { measure_geometry(s) } else { (None, 0.0) };
            let grab = if pending { d.start_y - top } else { 0.0 };
            drag::Event::Move { x: d.window_point()[0], dx: d.dx, dy: d.dy, grab, geometry: geom, now }
        }
        Phase::Up if d.cancelled() => drag::Event::Cancel,
        Phase::Up => drag::Event::Up,
    };
    tg::drag_step(s, event, tx);
}

fn measure_geometry(s: &App) -> (Option<drag::Geometry>, f32) {
    let items = tg::metas(s);
    let Some(groups) = tg::groups(s, &items) else { return (None, 0.0) };
    let rects = s.kernel.slots.untracked(&tg::LAYOUT).rects.borrow();
    let list_rect = rects.get("list").copied().unwrap_or([0.0; 4]);
    let top = groups
        .first()
        .and_then(|g| rects.get(&format!("section:{}", g.key)))
        .map_or(list_rect[1], |r| r[1]);
    let mut blocks = Vec::new();
    let mut rows = Vec::new();
    for g in &groups {
        let grouped = g.kind != Kind::Ungrouped;
        let mid = g.manual_id().map(str::to_owned);
        if let Some(sr) = rects.get(&format!("section:{}", g.key)) {
            if grouped {
                blocks.push(drag::Block::Group {
                    key: g.key.clone(),
                    manual_id: mid.clone(),
                    tabs: g.all.iter().map(|&i| items[i].key.clone()).collect(),
                    top: sr[1] - top,
                    bottom: sr[1] - top + sr[3],
                });
            } else {
                for &i in &g.visible {
                    let tk = &items[i].key;
                    if let Some(tr) = rects.get(&format!("tab:{tk}")) {
                        blocks.push(drag::Block::Tab { key: tk.clone(), top: tr[1] - top, bottom: tr[1] - top + tr[3] });
                    }
                }
            }
        }
        if grouped
            && let Some(hr) = rects.get(&format!("header:{}", g.key))
        {
            rows.push(drag::StripRow::Header {
                group_key: g.key.clone(),
                manual_id: mid.clone(),
                color: g.color,
                collapsed: g.collapsed,
                first_tab: g.all.first().map(|&i| items[i].key.clone()),
                last_tab: g.all.last().map(|&i| items[i].key.clone()),
                top: hr[1] - top,
                bottom: hr[1] - top + hr[3],
            });
        }
        let vis_len = g.visible.len();
        for (pos, &i) in g.visible.iter().enumerate() {
            let tk = &items[i].key;
            if let Some(tr) = rects.get(&format!("tab:{tk}")) {
                rows.push(drag::StripRow::Tab {
                    tab_key: tk.clone(),
                    group_key: grouped.then(|| g.key.clone()),
                    manual_id: mid.clone(),
                    color: grouped.then_some(g.color),
                    is_last: pos + 1 == vis_len,
                    top: tr[1] - top,
                    bottom: tr[1] - top + tr[3],
                });
            }
        }
    }
    let next_color = tg::model::next_manual_color(&tg::state(s).doc.manual_groups);
    let geom = drag::Geometry {
        list_left: list_rect[0],
        freeform: tg::effective_mode(s) == Mode::None,
        next_color: Some(next_color),
        blocks,
        rows,
    };
    (Some(geom), top)
}

rune_register::register!(crate::scenes::SCENE_LIST: crate::scenes::Scene = [
    // Experimental Tab groups: side tabs grouped by project.
    ("sidebar-tab-groups-project", |s| {
        crate::scenes::open_scene(s);
        s.finish_stream();
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::List;
        s.settings.tab_groups = Some(true);
        tg::set_mode(s, Mode::Project);
    }),
    // Experimental Tab groups: side tabs with two colored manual groups + ungrouped tabs.
    ("sidebar-tab-groups-manual", |s| {
        crate::scenes::open_scene(s);
        s.finish_stream();
        crate::scenes::side_tabs(s);
        s.settings.sidebar_organize = crate::settings::Organize::List;
        s.settings.tab_groups = Some(true);
        let keys: Vec<String> = s.tabs.iter().map(tg::tab_key).collect();
        tg::update(s, |d| {
            d.mode = Mode::None;
            d.manual_groups = vec![
                tg::ManualGroup { id: "mg1".into(), name: "Frontend".into(), color: Hue::Blue },
                tg::ManualGroup { id: "mg2".into(), name: "Backend".into(), color: Hue::Green },
            ];
            if let Some(k) = keys.first() { d.manual_assignments.insert(k.clone(), "mg1".into()); }
            if let Some(k) = keys.get(1) { d.manual_assignments.insert(k.clone(), "mg1".into()); }
            if let Some(k) = keys.get(2) { d.manual_assignments.insert(k.clone(), "mg2".into()); }
            if let Some(k) = keys.get(3) { d.manual_assignments.insert(k.clone(), "mg2".into()); }
        });
    }),
]);
