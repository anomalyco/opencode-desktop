//! Changed files (Settings › Preferences › Timeline, `session-timeline-row.tsx`'s `TurnDiff`):
//! once a run ends, the files it changed, under "N Changed files" and the sum of their lines
//! added and removed, as the patch renderer's card of files, each opening to its diff. The host
//! loads the run's diff (`TimelineHost::turn_diff`); until then the header shows alone, and a
//! failure says Diff unavailable beside Retry. Past five files, Show all and Show less.

use rune::{El, TextStyle, Tx, col, row, text};
use serde_json::Value;

use crate::TimelineHost;
use crate::focus::FocusExt;
use crate::palette::Pal;

/// What the host has of a run's diff.
pub enum State<'a> {
    Loading,
    /// The server's `FileDiff.Info` list.
    Ready(&'a [Value]),
    Failed,
}

/// How many files show until Show all (`turnDiffFileLimit`).
const LIMIT: usize = 5;
/// "N Changed files": 14 at 530, tabular (`text-14-medium`).
const TITLE: TextStyle = TextStyle::new(14.0, 530.0).lh(21.0);
/// Retry and Show all: 14 at 440 in the accent.
const ACTION: TextStyle = TextStyle::new(14.0, 440.0).lh(21.0);
/// `diff-changes` compact: 11 at 440, 0.05 tracking, on one line.
const COUNT: TextStyle = TextStyle::new(11.0, 440.0).lh(11.0).tracking(0.05);
/// Diff unavailable: 13 at 440 on the compact line.
const NOTE: TextStyle = TextStyle::new(13.0, 440.0).lh(16.0);

/// The run that ended at message `id`, from prompt `from`, which changed `files`, `pt` under
/// the row above.
pub fn view<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, from: &str, files: &[String], pt: f32) -> El {
    let state = s.turn_diff(session, from);
    let diffs = match state {
        State::Ready(d) => Some(d),
        _ => None,
    };
    let count = diffs.map_or(files.len(), <[Value]>::len);
    if count == 0 {
        return col();
    }
    let show_all = format!("turn-diff:{id}");
    let all = s.expanded().contains(&show_all);
    let failed = matches!(state, State::Failed);
    let (adds, dels) = diffs.into_iter().flatten().fold((0, 0), |(a, d), f| (a + f["additions"].as_u64().unwrap_or(0), d + f["deletions"].as_u64().unwrap_or(0)));
    let mut head = row().key("turn-diff-head").w_full().h(24.0).gap(8.0).center_y()
        .child(text(opencode_i18n::plural!("ui.sessionTurn.diffs.changed", count)).font(TITLE.features("'tnum' 1")).color(p.text));
    if adds + dels > 0 {
        head = head.child(row().key("changes").gap(8.0).center_y()
            .child(text(format!("+{adds}")).dir(rune::Direction::Ltr).font(COUNT).color(p.success))
            .child(text(format!("-{dels}")).dir(rune::Direction::Ltr).font(COUNT).color(p.danger)));
    }
    if failed {
        head = head.child(text(opencode_i18n::t!("ui.sessionTurn.diffs.failed")).key("turn-diff-error").font(NOTE).color(p.faint).truncate().shrink().min_w(0.0));
    }
    let overflow = count > LIMIT;
    if failed || overflow {
        let (session, from) = (session.to_owned(), from.to_owned());
        let retry = failed.then(|| text(opencode_i18n::t!("ui.sessionTurn.diffs.retry")).key("turn-diff-retry").role("button").label_static(opencode_i18n::t!("ui.sessionTurn.diffs.retry")).font(ACTION).color(p.accent).radius(4.0).focus_default(p)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
                s.retry_turn_diff(&session, &from);
            }));
        let more = overflow.then(|| text(if all { opencode_i18n::t!("ui.sessionTurn.diffs.showLess") } else { opencode_i18n::t!("ui.sessionTurn.diffs.showAll") }).key("turn-diff-all").role("button").font(ACTION).color(p.accent).radius(4.0).focus_default(p)
            .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                tx.instant();
                crate::disclosure::toggle_expanded(s, &show_all);
            }));
        head = head.child(rune::spacer()).child(row().gap(12.0).center_y().children(retry).children(more));
    }
    let mut c = col().key("session-turn-diffs").w_full().gap(8.0).pt(pt).child(head);
    if let Some(d) = diffs {
        let shown: Vec<Value> = if all { d.to_vec() } else { d.iter().take(LIMIT).cloned().collect() };
        let metadata = serde_json::json!({ "files": shown });
        let call_id = format!("turn-diff-{id}");
        let call = crate::tools::Call { id: &call_id, name: "patch", status: &opencode_kit::model::ToolStatus::Completed, input: &Value::Null, metadata: &metadata };
        let cx = crate::tools::Ctx { session, live: false, sticky: crate::style::sticky_top() };
        c = c.child(crate::tools::shut_group(s, p, cx, &[call]));
    }
    c
}
