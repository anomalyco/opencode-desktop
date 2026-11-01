//! The inbox above the sidebar's tabs: Needs you, Done, then Projects, as one list the arrows
//! rove; ⏎ opens the row, E archives a task.

use rune::{Anim, El, Key, Tx, col, el, row, spacer, text};

use super::Kind;
use crate::palette::Pal;
use crate::view::{UI, UI_MED, activity_ring, title_of};
use crate::{App, Route};

const INSTANT: Anim = Anim::INSTANT;

/// Nothing while no session is a project.
pub fn section(s: &App, p: &Pal) -> Option<El> {
    let marks = s.project_marks();
    if marks.is_empty() {
        return None;
    }
    let open = match &s.route {
        Route::Session(id) => Some(id.as_str()),
        _ => None,
    };
    let rows = s.inbox_rows();
    let mut list = col().key("inbox").role("list").focus_group().w_full().gap(4.0).pb(16.0);
    for (kind, label) in [(Kind::Needs, "Needs you"), (Kind::Done, "Done")] {
        let of: Vec<_> = rows.iter().filter(|r| r.kind == kind).collect();
        if of.is_empty() {
            continue;
        }
        list = list.child(head(p, &format!("{label} · {}", of.len())));
        for r in of {
            list = list.child(task_row(s, p, r.info, kind, r.project, open == Some(r.info.id.as_str())));
        }
    }
    list = list.child(head(p, "Projects"));
    let current = open.map(|id| s.root_of(id));
    for (id, _) in marks {
        list = list.child(project_row(s, p, id, open == Some(id) || (current == Some(id) && s.project_of_task(open.unwrap_or("")).is_none())));
    }
    Some(list)
}

fn head(p: &Pal, label: &str) -> El {
    row().key(format!("h-{label}")).w_full().h(24.0).pl(6.0).center_y().child(text(label.to_owned()).font(UI).color(p.faint))
}

fn line(key: String, label: String, p: &Pal, active: bool) -> El {
    let r = row()
        .key(key)
        .role("button")
        .label(label)
        .focusable(true)
        .focus_selected(active)
        .w_full()
        .h(28.0)
        .pl(6.0)
        .pr(8.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT);
    if active { r.fill(p.pressed) } else { r }
}

fn glyph(p: &Pal, kind: Kind) -> El {
    let dot = el().size(7.0, 7.0).radius(3.5).fill(if kind == Kind::Needs { p.warning } else { p.bg_accent });
    el().key("glyph").size(16.0, 16.0).center().child(dot)
}

fn task_row(s: &App, p: &Pal, info: &opencode_kit::model::SessionInfo, kind: Kind, project: &str, active: bool) -> El {
    let title = title_of(info);
    let (open_id, key_id) = (info.id.clone(), info.id.clone());
    let project_name = s.session(project).map(title_of).unwrap_or_default();
    let age = opencode_kit::model::ago(s.now_ms, info.idle.unwrap_or(info.updated).max(info.updated)).trim_end_matches(" ago").to_owned();
    line(format!("ib-{}", info.id), title.clone(), p, active)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&open_id, tx))
        .on_key(move |s: &mut App, ev, tx: &mut Tx| match &ev.key {
            Key::Text(t) if t.eq_ignore_ascii_case("e") && !ev.mods.cmd && !ev.mods.ctrl => {
                s.mark_task_read(&key_id, tx);
                true
            }
            _ => false,
        })
        .child(glyph(p, kind))
        .child(text(title).font(UI_MED).color(if active { p.text } else { p.muted }).shrink().min_w(0.0).truncate_fade())
        .child(text(project_name).font(UI).color(p.faint).max_w(64.0).truncate_fade())
        .child(spacer())
        .child(text(age).font(UI).color(p.faint))
}

fn project_row(s: &App, p: &Pal, id: &str, active: bool) -> El {
    let info = s.session(id);
    let title = info.map(title_of).unwrap_or_else(|| "Project".into());
    let project = info.and_then(|i| s.project_for_session(i));
    let name = crate::view::project_or_dir(project, info.map_or("", |i| i.directory.as_str()));
    let working = s.project_working(id);
    let open_id = id.to_owned();
    let mut r = line(format!("ip-{id}"), title.clone(), p, active)
        .on_press_tx(move |s: &mut App, tx: &mut Tx| s.open(&open_id, tx))
        .child(crate::icons::badge(s, p, project, &name))
        .child(text(title).font(UI_MED).color(if active { p.text } else { p.muted }).shrink().min_w(0.0).truncate_fade())
        .child(spacer());
    if working > 0 {
        r = r.child(text(format!("{working} working")).font(UI).color(p.faint)).child(activity_ring(p));
    } else if s.busy(id) {
        r = r.child(activity_ring(p));
    }
    crate::session_menu::on(r, crate::session_menu::Target::new(id, crate::session_menu::Spot::Inbox))
}
