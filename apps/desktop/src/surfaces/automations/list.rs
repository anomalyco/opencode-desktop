//! The list beside the inspector: every automation under its status, Completed folded away
//! until asked for (packages/app/src/automations/route.tsx, view.tsx:AutomationListRow).

use opencode_i18n::t;
use opencode_kit::model::{AutomationOutcome, AutomationStatus};
use opencode_ui::palette::Pal;
use opencode_ui::widgets::BODY;
use rune::{Color, Direction, El, TextStyle, Tx, col, row, spacer, text};
use session_view::focus::FocusExt as _;

use crate::App;

use super::AUTOMATIONS;
use super::format::relative_time;
use super::source::Entry;

const GROUP_LABEL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const ROW_TITLE: TextStyle = TextStyle::new(13.0, 530.0).lh(20.0).tracking(-0.04);
const ROW_NOTE: TextStyle = TextStyle::new(12.0, 440.0).lh(20.0);
const ROW_SUB: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);

const STATUSES: [AutomationStatus; 3] = [AutomationStatus::Active, AutomationStatus::Paused, AutomationStatus::Done];

/// The list in `viewport`, which the page sizes: a column beside the inspector, or a band over it.
pub(super) fn pane(s: &App, p: &Pal, viewport: El, filtered: &[Entry], selected: Option<&Entry>, context: impl Fn(&Entry) -> Vec<String>) -> El {
    let st = s.kernel.slots.get(&AUTOMATIONS);
    let has_query = !st.query.text.trim().is_empty();
    let mut content = col().key("automations-list").w_full().padding(8.0).gap(12.0);

    if filtered.is_empty() {
        content = content.child(
            row()
                .w_full()
                .px(12.0)
                .py(32.0)
                .justify_center()
                .child(text(t!("automations.empty.filtered")).font(BODY).color(p.faint).wrap()),
        );
    }
    for status in STATUSES {
        let items: Vec<&Entry> = filtered.iter().filter(|e| e.row.status == status).collect();
        if items.is_empty() {
            continue;
        }
        let done = status == AutomationStatus::Done;
        let open = !done || st.show_done || has_query;
        let label = match status {
            AutomationStatus::Active => t!("automations.status.active"),
            AutomationStatus::Paused => t!("automations.status.paused"),
            AutomationStatus::Done => t!("automations.status.done"),
        };
        let mut section = col().key(format!("auto-group-{}", status.as_str())).w_full().gap(2.0);
        section = section.child(group_header(p, label, items.len(), done.then_some(open)));
        if open {
            for entry in items {
                let is_selected = selected.is_some_and(|cur| cur.key == entry.key);
                section = section.child(list_row(s, p, entry, is_selected, context(entry)));
            }
        }
        content = content.child(section);
    }

    session_view::scroller::scroller("automations-list-scroll").build::<App>(p, viewport, content)
}

/// A group's name and count. `fold` is whether a group that folds (Completed) is open: its
/// header is then the button that shows or hides it.
fn group_header(p: &Pal, label: &'static str, count: usize, fold: Option<bool>) -> El {
    let head = row()
        .w_full()
        .h(28.0)
        .px(10.0)
        .gap(6.0)
        .center_y()
        .child(text(label).font(GROUP_LABEL).color(p.faint).group_color(p.muted))
        .child(text(count.to_string()).font(GROUP_LABEL.features("\"tnum\"")).color(p.faint).group_color(p.muted));
    let Some(open) = fold else { return head };
    let word = if open { t!("automations.group.hide") } else { t!("automations.group.show") };
    head.key("auto-group-done-toggle")
        .role("button")
        .label(format!("{label} {count}"))
        .expanded(open)
        .radius(6.0)
        .focus_inset(p)
        .hover_group()
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            let st = s.kernel.slots.get_mut(&AUTOMATIONS);
            st.show_done = !st.show_done;
        })
        .child(spacer())
        .child(text(word).font(GROUP_LABEL).color(p.faint).group_color(p.muted))
}

fn list_row(s: &App, p: &Pal, entry: &Entry, selected: bool, mut context: Vec<String>) -> El {
    let info = &entry.row;
    let failed = info.status == AutomationStatus::Paused && info.last_outcome == Some(AutomationOutcome::Failed);
    let note = if failed {
        Some(t!("automations.note.failed").to_owned())
    } else if info.status == AutomationStatus::Done && info.last_outcome == Some(AutomationOutcome::Expired) {
        Some(t!("automations.status.expired").to_owned())
    } else if info.status == AutomationStatus::Active {
        info.next_due_at.map(|at| relative_time(at, *s.now_ms))
    } else {
        None
    };
    context.push(entry.trigger.clone());
    // A finished automation steps back: its name in the muted ink.
    let name_ink = if info.status == AutomationStatus::Done { p.muted } else { p.text };
    let note_ink = if failed { p.danger } else { p.muted };

    let key = entry.key.clone();
    let top = row()
        .w_full()
        .min_w(0.0)
        .gap(12.0)
        .center_y()
        .child(text(info.name.clone()).font(ROW_TITLE).color(name_ink).grow(1.0).shrink().min_w(0.0).truncate().dir(Direction::Auto))
        .children(note.map(|n| text(n).font(ROW_NOTE.features("\"tnum\"")).color(note_ink)));

    col()
        .key(format!("auto-row-{}", info.id))
        .role("button")
        .label(info.name.clone())
        .focus_selected(selected)
        .w_full()
        .px(10.0)
        .py(8.0)
        .gap(2.0)
        .radius(7.0)
        .focus_inset(p)
        .fill(if selected { p.layer2 } else { Color::TRANSPARENT })
        .hover(if selected { Color::TRANSPARENT } else { p.hover })
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.kernel.slots.get_mut(&AUTOMATIONS).selected = Some(key.clone());
        })
        .child(top)
        .child(text(context.join(" · ")).font(ROW_SUB).color(p.faint).w_full().truncate())
}
