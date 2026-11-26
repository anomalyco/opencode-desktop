//! Experimental › Sidebar sections, drawn: side tabs under quiet section headers ("Whisper
//! headers": a label, no box), a hairline, then the tabs in no section. Selected tabs wear a soft
//! accent wash, and while any are selected a small bar along the sidebar's foot counts them and
//! offers Group (⌘G) and Clear.

mod bar;
mod header;
mod menu;
mod row;
mod story;

use super::*;
use crate::tabs::sections::{self as sec, Drop, LIVE};
use opencode_i18n::t;

/// The space the selection bar takes at the list's foot.
const BAR_ROOM: f32 = 52.0;

/// The side tabs in sections, with `lead` (Consolidated's pinned area) above them.
pub(super) fn list(s: &App, p: &Pal, lead: Option<El>) -> El {
    let laid = sec::laid(s);
    let st = sec::state(s);
    let nav_order = s.nav_order();
    let show_hints = crate::tab_hints::shown(s);
    let mut content = col()
        .key("sidebar-sections")
        .role("tablist")
        .label_static(t!("settings.general.row.interfaceMode.side"))
        .w_full()
        .children(lead)
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            tx.live_only();
            s.kernel.slots.untracked(&LIVE).rects.borrow_mut().insert("content".into(), r);
            s.strip_laid_out(r);
        });
    for (n, (section, tabs)) in laid.sections.iter().enumerate() {
        content = content.child(header::header(s, p, section, tabs, n == 0));
        if let Some(alts) = header::alternates(s, p, section) {
            content = content.child(alts);
        }
        if !section.collapsed {
            let rows = tabs.iter().filter_map(|&i| s.tabs.get(i).map(|t| row::tab_row(s, p, t, i, &nav_order, show_hints)));
            content = content.child(col().key(format!("sec-rows-{}", section.id)).w_full().gap(2.0).pt(2.0).children(rows));
        }
    }
    if !laid.sections.is_empty() && !laid.ungrouped.is_empty() {
        content = content.child(el().key("sections-divider").mx(8.0).my(8.0).h(1.0).fill(p.border_muted).a11y_hidden());
    }
    let rows = laid.ungrouped.iter().filter_map(|&i| s.tabs.get(i).map(|t| row::tab_row(s, p, t, i, &nav_order, show_hints)));
    content = content.child(col().key("sec-rows-ungrouped").w_full().gap(2.0).children(rows));
    if let Some(line) = indicator(s, p, st.drag.drop()) {
        content = content.child(line);
    }
    let picked = !st.selection.is_empty();
    if picked {
        content = content.pb(BAR_ROOM);
    }
    let viewport = col().grow(1.0).shrink().min_h(0.0).focus_inset(p);
    let scroll = session_view::scroller::scroller("sidebar-sections-scroll").build::<App>(p, viewport, content);
    col().key("sidebar-sections-list").w_full().grow(1.0).shrink().min_h(0.0).child(scroll).children(picked.then(|| bar::bar(s, p)))
}

/// The line a dragged tab would land at: above or below a tab, or under the last.
fn indicator(s: &App, p: &Pal, drop: Option<&Drop>) -> Option<El> {
    let rects = s.kernel.slots.untracked(&LIVE).rects.borrow();
    let top = rects.get("content")?[1];
    let y = match drop? {
        Drop::Before(u) => rects.get(&format!("row:{u}"))?[1] - 1.0,
        Drop::After(u) => {
            let r = rects.get(&format!("row:{u}"))?;
            r[1] + r[3] + 1.0
        }
        Drop::End => {
            let last = sec::laid(s).ungrouped.last().and_then(|&i| s.tabs.get(i)).map(crate::tabs::Tab::uid)?;
            let r = rects.get(&format!("row:{last}"))?;
            r[1] + r[3] + 1.0
        }
        Drop::Into(_) => return None,
    };
    Some(el().key("sections-drop").pin(Some(8.0), Some(y - top - 1.0), Some(0.0), None).h(2.0).radius(1.0).fill(p.accent).raise().a11y_hidden())
}
