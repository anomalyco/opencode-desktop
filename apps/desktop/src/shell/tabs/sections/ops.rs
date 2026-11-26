//! What a person does to sections and the selected tabs: gather them, move them, fold, rename,
//! remove a section (Undo puts it back), and close, archive or delete several tabs at once.

use std::collections::HashMap;

use opencode_i18n::t;
use opencode_kit::servers::ServerKey;
use rune::{Drag, Phase, Tx};

use super::model::{self, Doc, Drop, Section};
use super::{FIELD, LIVE, drag, listed_uids, naming, select, state, state_mut, update, visible_uids};
use crate::App;
use crate::tabs::Tab;

pub fn clear_selection(s: &mut App) {
    if !state(s).selection.is_empty() {
        state_mut(s).selection.step(select::Event::Clear);
    }
}

/// `uids` in the order the sidebar lists them.
fn in_list_order(s: &App, uids: &[String]) -> Vec<String> {
    listed_uids(s).into_iter().filter(|u| uids.contains(u)).collect()
}

pub fn naming_step(s: &mut App, event: naming::Event<'_>, tx: &mut Tx) {
    match state_mut(s).naming.step(event) {
        naming::Effect::None => {}
        naming::Effect::Focus => tx.focus(FIELD, false),
        naming::Effect::Save { section, name } => {
            // A commit from the field's key or its blur would land at once: the name settles
            // and the other names fold away on their springs.
            tx.animated();
            update(s, |d| {
                let name = d.unique_name(name.trim(), &section);
                if let Some(sec) = d.get_mut(&section) {
                    sec.name = name;
                }
            });
        }
    }
}

/// What a tab is called and its project's name, for guessing a section's name.
fn titled(s: &App, uid: &str) -> Option<(String, Option<String>)> {
    let t = s.tab(uid)?;
    let face = crate::view::tab_strip::face(s, t);
    Some((face.title.clone(), face.project.map(|p| p.name.clone())))
}

/// The names offered for a section of `uids`: the best first.
fn names_for(s: &App, uids: &[String]) -> Vec<String> {
    let known: Vec<(String, Option<String>)> = uids.iter().filter_map(|u| titled(s, u)).collect();
    let titles: Vec<&str> = known.iter().map(|(t, _)| t.as_str()).collect();
    let projects: Vec<Option<&str>> = known.iter().map(|(_, p)| p.as_deref()).collect();
    naming::suggest(&titles, &projects, t!("rune.sections.new"))
}

/// A new section of `uids` under the others, its name field open on the best guess.
pub fn create(s: &mut App, uids: &[String], tx: &mut Tx) {
    let uids = in_list_order(s, uids);
    if uids.is_empty() {
        return;
    }
    let names = names_for(s, &uids);
    let id = crate::windows::store::new_id();
    let mut name = String::new();
    update(s, |d| {
        d.pull(&uids);
        name = d.unique_name(&names[0], &id);
        d.sections.push(Section { id: id.clone(), name: name.clone(), collapsed: false, members: uids });
    });
    let alternates: Vec<String> = {
        let doc = &state(s).doc;
        names.iter().skip(1).map(|n| doc.unique_name(n, &id)).filter(|n| *n != name).take(3).collect()
    };
    clear_selection(s);
    naming_step(s, naming::Event::Start { section: &id, name: &name, alternates }, tx);
}

/// Rename section `id` in place.
pub fn start_rename(s: &mut App, id: &str, tx: &mut Tx) {
    let Some(sec) = state(s).doc.get(id).cloned() else { return };
    let mut alternates: Vec<String> = names_for(s, &sec.members).into_iter().filter(|n| !n.eq_ignore_ascii_case(&sec.name)).collect();
    alternates.truncate(3);
    naming_step(s, naming::Event::Start { section: id, name: &sec.name, alternates }, tx);
}

/// `uids` to the end of section `id`.
pub fn move_to(s: &mut App, uids: &[String], id: &str) {
    let uids = in_list_order(s, uids);
    let order: Vec<String> = s.tabs.iter().map(Tab::uid).collect();
    update(s, |d| _ = model::land(d, &uids, &Drop::Into(id.to_owned()), &order));
    clear_selection(s);
}

/// `uids` out of their sections, down among the ungrouped tabs.
pub fn ungroup(s: &mut App, uids: &[String]) {
    update(s, |d| d.pull(uids));
    clear_selection(s);
}

pub fn toggle(s: &mut App, id: &str, tx: &mut Tx) {
    tx.animated();
    update(s, |d| {
        if let Some(sec) = d.get_mut(id) {
            sec.collapsed = !sec.collapsed;
        }
    });
    // A folded section's tabs can't stay picked out of sight.
    if let Some(shown) = visible_uids(s) {
        state_mut(s).selection.step(select::Event::Retain(&shown));
    }
}

/// Section `id` goes; its tabs drop back below, none closed, and the toast's Undo puts it back.
pub fn remove_section(s: &mut App, id: &str, tx: &mut Tx) {
    let before = state(s).doc.clone();
    if before.get(id).is_none() {
        return;
    }
    tx.animated();
    naming_step(s, naming::Event::Gone(id), tx);
    update(s, |d| d.sections.retain(|sec| sec.id != id));
    crate::undo::offer(s, t!("rune.sections.removed"), crate::undo::Undoable::Sections(Box::new(before)));
}

/// Undo on a removed section's toast: the sections as they were, less tabs closed since.
pub fn put_back(s: &mut App, mut doc: Doc) {
    let open: Vec<String> = s.tabs.iter().map(Tab::uid).collect();
    doc.retain_open(&open);
    update(s, |d| *d = doc);
}

pub fn close_tabs(s: &mut App, uids: &[String], tx: &mut Tx) {
    clear_selection(s);
    for uid in uids {
        s.close_tab(uid, tx);
    }
}

/// The sessions behind `uids` (drafts have none): server and id.
fn sessions_of(s: &App, uids: &[String]) -> Vec<(ServerKey, String)> {
    uids.iter().filter_map(|u| s.tab(u)).filter(|t| !t.is_draft()).map(|t| (t.server.clone(), t.id.clone())).collect()
}

pub fn session_count(s: &App, uids: &[String]) -> usize {
    sessions_of(s, uids).len()
}

/// Archive each session (each tab closes; the last archive's toast offers its session back).
pub fn archive(s: &mut App, uids: &[String], tx: &mut Tx) {
    clear_selection(s);
    for (server, id) in sessions_of(s, uids) {
        crate::session_menu::SessionMenuHost::archive_session(s, &server, &id, tx);
    }
}

/// Delete each session, once confirmed: they and their subagents leave the list at once and one
/// toast offers them all back; the deletes go only when it does (as one Delete does,
/// `session_menu::delete`).
pub fn delete(s: &mut App, uids: &[String]) {
    clear_selection(s);
    let mut gone = Vec::new();
    for (server, id) in sessions_of(s, uids) {
        let hidden = crate::servers::runtime::with_server(s, &server, |s| {
            let mut tree = vec![id.clone()];
            let mut at = 0;
            while at < tree.len() {
                let parent = tree[at].clone();
                tree.extend(s.sessions.iter().filter(|x| x.parent.as_deref() == Some(parent.as_str())).map(|x| x.id.clone()));
                at += 1;
            }
            let hidden: Vec<_> = s.sessions.iter().filter(|x| tree.contains(&x.id)).cloned().collect();
            s.sessions.retain(|x| !tree.contains(&x.id));
            hidden
        });
        gone.push((server, id, hidden.unwrap_or_default()));
    }
    if gone.is_empty() {
        return;
    }
    let n = gone.len();
    crate::undo::offer(s, &t!("rune.undo.sessionsDeleted", count = n), crate::undo::Undoable::DeletedMany(gone));
}

/// A press on tab `uid` (its click): ⌘ toggles it in the selection, ⇧ takes the run to it, a
/// plain click opens it and drops the selection.
pub fn press(s: &mut App, uid: &str, tx: &mut Tx) {
    if drag::step(&mut state_mut(s).drag, drag::Event::Press) != drag::Effect::Click {
        return;
    }
    let live = s.kernel.slots.untracked(&LIVE);
    let (cmd, shift) = (live.cmd.get(), live.shift.get());
    // A pick washes in and the bar rises on their springs; a plain click opens at once.
    if shift {
        let order = visible_uids(s).unwrap_or_default();
        let from = s.current_tab();
        state_mut(s).selection.step(select::Event::Range { to: uid, order: &order, from: from.as_deref() });
        return;
    }
    if cmd {
        state_mut(s).selection.step(select::Event::Toggle(uid));
        return;
    }
    tx.instant();
    clear_selection(s);
    if s.current_tab().as_deref() != Some(uid) {
        s.select_tab(uid, tx);
    }
}

/// A drag on tab `uid` (at `index` in the window's tabs).
pub fn tab_drag(s: &mut App, uid: &str, index: usize, d: &mut Drag, tx: &mut Tx) {
    let live = s.kernel.slots.untracked(&LIVE);
    if d.phase == Phase::Down && (live.cmd.get() || live.shift.get()) {
        // A ⌘- or ⇧-click picks; it never opens or drags.
        return;
    }
    let picked = state(s).selection.contains(uid);
    // A press opens its tab at once, as the strip's does, unless it carries the selection.
    if d.phase == Phase::Down && !picked && !session_view::style::touch_controls() && s.current_tab().as_deref() != Some(uid) {
        s.select_tab(uid, tx);
    }
    if crate::tabs::groups::handle_tab_tear(s, uid, index, d, tx) {
        state_mut(s).drag = drag::DragState::Idle;
        return;
    }
    let event = match d.phase {
        Phase::Down => {
            let sel = state(s).selection.uids().to_vec();
            let uids = if picked && sel.len() > 1 { in_list_order(s, &sel) } else { vec![uid.to_owned()] };
            drag::Event::Down { lead: uid.to_owned(), uids }
        }
        Phase::Move => {
            let carried = match &state(s).drag {
                drag::DragState::Pending { uids, .. } | drag::DragState::Dragging { uids, .. } => uids.clone(),
                _ => vec![uid.to_owned()],
            };
            drag::Event::Move { dy: d.dy, drop: hit(s, d.start_y + d.dy, &carried) }
        }
        Phase::Up if d.cancelled() => drag::Event::Cancel,
        Phase::Up => drag::Event::Up,
    };
    match drag::step(&mut state_mut(s).drag, event) {
        drag::Effect::None | drag::Effect::Click => {}
        drag::Effect::Redraw => tx.animated(),
        drag::Effect::Commit { uids, drop } => {
            tx.animated();
            land(s, &uids, &drop);
        }
    }
}

/// Where a pointer at window `y` drops `carried`: a header takes it in, a tab's top half lands it
/// before that tab and its bottom half after, below them all it goes to the end.
fn hit(s: &App, y: f32, carried: &[String]) -> Option<Drop> {
    let rects = s.kernel.slots.untracked(&LIVE).rects.borrow();
    let l = super::laid(s);
    let mut last: Option<[f32; 4]> = None;
    for (sec, tabs) in &l.sections {
        if let Some(r) = rects.get(&format!("head:{}", sec.id))
            && y >= r[1]
            && y < r[1] + r[3]
        {
            return Some(Drop::Into(sec.id.clone()));
        }
        if sec.collapsed {
            continue;
        }
        for &i in tabs {
            if let Some(d) = row_hit(s, &rects, i, y, carried, &mut last) {
                return Some(d);
            }
        }
    }
    for &i in &l.ungrouped {
        if let Some(d) = row_hit(s, &rects, i, y, carried, &mut last) {
            return Some(d);
        }
    }
    last.filter(|r| y >= r[1] + r[3]).map(|_| Drop::End)
}

fn row_hit(s: &App, rects: &HashMap<String, [f32; 4]>, i: usize, y: f32, carried: &[String], last: &mut Option<[f32; 4]>) -> Option<Drop> {
    let uid = s.tabs.get(i)?.uid();
    let r = *rects.get(&format!("row:{uid}"))?;
    *last = Some(r);
    if carried.contains(&uid) || y < r[1] || y >= r[1] + r[3] {
        return None;
    }
    Some(if y < r[1] + r[3] / 2.0 { Drop::Before(uid) } else { Drop::After(uid) })
}

/// `uids` land at `drop`: in a section, or among the ungrouped tabs in a new tab order.
pub fn land(s: &mut App, uids: &[String], drop: &Drop) {
    let order: Vec<String> = s.tabs.iter().map(Tab::uid).collect();
    let mut tabs = None;
    update(s, |d| tabs = model::land(d, uids, drop, &order));
    if let Some(next) = tabs {
        let rank: HashMap<&str, usize> = next.iter().enumerate().map(|(i, u)| (u.as_str(), i)).collect();
        let mut sorted: Vec<Tab> = s.tabs.to_vec();
        sorted.sort_by_key(|t| rank.get(t.uid().as_str()).copied().unwrap_or(usize::MAX));
        if sorted != *s.tabs {
            *s.tabs = sorted;
            s.save_tabs();
        }
    }
}
