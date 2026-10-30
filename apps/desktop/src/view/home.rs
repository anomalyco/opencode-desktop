use super::*;
use crate::session_menu::{Spot, Target};

// ---------------------------------------------------------------------------------------------
// Home: search and recent sessions by day (Desktop's home sessions column).

pub(super) fn home(s: &App, p: &Pal) -> El {
    let q = s.search.text.to_lowercase();
    let mut groups: Vec<(&str, Vec<&SessionInfo>)> = vec![("Today", vec![]), ("Yesterday", vec![]), ("Older", vec![])];
    let day = |ms: i64| ms.div_euclid(86_400_000);
    for ses in roots(s).filter(|x| q.is_empty() || x.title.to_lowercase().contains(&q)).take(60) {
        let d = day(s.now_ms) - day(ses.updated);
        groups[if d <= 0 { 0 } else if d == 1 { 1 } else { 2 }].1.push(ses);
    }
    let nav_ids = std::rc::Rc::new(groups.iter().flat_map(|(_, items)| items.iter().map(|s| s.id.clone())).collect::<Vec<_>>());
    let mut list = col().key("groups").role("list").focus_group().w_full().pr(12.0).pb(64.0);
    let mut first = true;
    for (title, items) in groups.into_iter().filter(|g| !g.1.is_empty()) {
        let mut header = row().key(format!("h-{title}")).h(28.0).pl(12.0).center_y().child(text(title).font(UI).color(p.muted));
        if first {
            header = header.child(spacer()).child(
                row()
                    .key("new-session")
                    .role("button")
                    .h(28.0)
                    .px(8.0)
                    .gap(6.0)
                    .center_y()
                    .radius(6.0)
                    .hover(p.hover)
                    .on_press_tx(|s: &mut App, tx: &mut Tx| s.new_session(tx))
                    .child(icon(&ic::EDIT, 16.0).color(p.icon_muted))
                    .child(text("New session").font(UI_MED).color(p.muted)),
            );
        }
        let mut rows = col().key(format!("g-{title}")).gap(1.0).pt(16.0).mb(24.0);
        for ses in items {
            let index = nav_ids.iter().position(|x| x == &ses.id).unwrap_or(0);
            rows = rows.child(session_row(s, p, ses, &nav_ids, index));
        }
        list = list.child(header).child(rows);
        first = false;
    }
    if !q.is_empty() && first {
        list = list.child(
            row().key("no-results").h(40.0).pl(12.0).center_y().child(text(format!("No sessions found for {}", s.search.text)).font(UI).color(p.muted)),
        );
    }

    let search = row()
        .key("search")
        .h(36.0)
        .pl(12.0)
        .pr(8.0)
        .gap(8.0)
        .center_y()
        .radius(6.0)
        .fill(p.search_bg)
        .hover(p.layer2)
        .child(icon(&ic::MAGNIFYING_GLASS, 16.0).color(p.icon_muted))
        .child(
            text_input(&s.search, UI.lh(20.0))
                .key("search-input")
                .grow(1.0)
                .shrink()
                .h(20.0)
                .placeholder("Search sessions", p.faint)
                .caret(p.text, p.selection)
                .on_edit(|s: &mut App| &mut s.search)
                .on_input_key(|s: &mut App, ev, tx| s.shortcut(ev, tx)),
        );

    col()
        .key("home")
        .grow(1.0)
        .w_full()
        .scroll(ScrollSpec { stick_bottom: false })
        .clip_soft(0.0)
        .child(
            col()
                .key("home-content")
                .w_full()
                .center()
                .px(24.0)
                .pt(48.0)
                .child(col().key("home-column").w_full().max_w(720.0).child(search).child(list.mt(12.0))),
        )
}

/// One session in Home's list: opens on a click, and has Desktop's right-click menu. While it's
/// being renamed the row is the field, and doesn't open.
pub(crate) fn session_row(s: &App, p: &Pal, ses: &SessionInfo, nav_ids: &std::rc::Rc<Vec<String>>, index: usize) -> El {
    let project = s.project_for_session(ses);
    let proj_name = project_or_dir(project, &ses.directory);
    let proj = proj_name.as_str();
    let id = ses.id.clone();
    let hover_id = id.clone();
    let nav = nav_ids.clone();
    let target = Target::new(&ses.id, Spot::Row);
    let editing = crate::session_menu_view::field(s, p, &target, UI_MED);
    let marked = s.menus.on.as_ref() == Some(&target);
    let mut r = row()
        .key(format!("r-{}", ses.id))
        .role("button")
        .label(ses.title.clone())
        .focusable(true)
        .focus_selected(index == 0)
        .h(40.0)
        .pl(12.0)
        .pr(16.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .on_hover_change(move |s: &mut App, inside, _: &mut Tx| if inside { s.prefetch_session(&hover_id); })
        .on_key(move |s: &mut App, ev, _: &mut Tx| s.prefetch_navigation(&nav, index, ev))
        .child(crate::icons::badge(s, p, project, proj));
    if marked {
        r = r.fill(p.hover);
    }
    r = match editing {
        Some(field) => r.child(field.grow(1.0)),
        None => r.on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&id, tx)).child(text(title_of(ses)).font(UI_MED).color(p.text).truncate_fade()),
    };
    crate::session_menu::on(
        r.child(text(proj.to_owned()).font(UI).color(p.faint).truncate_fade())
            .child(spacer())
            .child(text(model::ago(s.now_ms, ses.updated)).font(UI).color(p.faint)),
        target,
    )
}
