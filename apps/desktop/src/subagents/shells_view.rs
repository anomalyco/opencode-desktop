//! The background shells in the running-work list (`activity-chips.tsx`): each shell's row, and
//! the panel a row opens in place (`session-activity-shell-*`, `activity-chips.css:135-236`).

use rune::{El, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::activity::{EndStatus, Event, Phase};
use super::shells::{self, Shell};
use super::{ELAPSED, PANEL_PAD, PANEL_ROW, elapsed, run_time};
use crate::App;
use crate::palette::{Elevate as _, Pal};
use crate::view::{INSTANT, UI};

/// The command in a row and the panel's header: the code face at 13/16 (`session-activity-shell-command`).
pub const COMMAND: rune::TextStyle = session_view::style::TOOL_CODE.lh(16.0);
/// The shell panel's width (`min(560px, 100vw - 56px)`) and height cap (`min(320px, …)`).
const WIDE: f32 = 560.0;
const MAX_H: f32 = 320.0;
/// The tail shows its last 14 lines (`TAIL_LINES`).
const TAIL_LINES: usize = 14;

/// A shell's row in the list: the console mark, its command across the agent and task columns,
/// and its run time, which hover swaps for a stop button.
pub fn row_of(s: &App, p: &Pal, sh: &Shell, span: f32, time_w: f32) -> El {
    let open = sh.clone();
    row()
        .key(format!("shell-{}", sh.id))
        .role("menuitem")
        .label(sh.command.clone())
        .w_full()
        .h(PANEL_ROW)
        .px(6.0)
        .gap(8.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .hover_group()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            shells::dispatch(s, Event::Selected(open.clone()));
        })
        .child(icon(&ic::CONSOLE, 16.0).key("mark").color(p.icon_muted))
        .child(text(sh.command.clone()).key("command").font(COMMAND).color(p.text).truncate_fade().w(span).grow(1.0).shrink().min_w(0.0))
        .child(end(s, p, &sh.id, false, (sh.started > 0).then(|| s.run_clock() - sh.started), time_w))
}

/// A row's end (`session-activity-end`): its run time, and the stop button over it, shown under
/// the pointer or while its Stop is out.
pub fn end(s: &App, p: &Pal, key: &str, subagent: bool, ms: Option<i64>, time_w: f32) -> El {
    let stopping = s.shells.activity.stopping.contains_key(key);
    let time = row().key("time").w_full().h_full().justify_end().center_y().children(ms.map(|ms| run_time(format!("time-{key}"), ms, ELAPSED, p.muted)));
    let target = key.to_owned();
    let label = if subagent { opencode_i18n::t!("session.running.stop.subagent") } else { opencode_i18n::t!("session.running.stop.shell") };
    let mark = if stopping { rune_kit::Loader::new().size(14.0).build(crate::palette::kit(p)) } else { icon(&ic::OUTLINE_XMARK, 14.0).color(p.icon_muted) };
    let stop = row()
        .key("stop")
        .role("button")
        .label(label)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .hover_alone()
        .on_press_tx(move |s: &mut App, tx: &mut Tx| {
            tx.instant();
            shells::dispatch(s, Event::StopRequested { key: target.clone(), subagent });
        })
        .child(mark);
    let (time, stop) = if stopping { (time.opacity(0.0), stop) } else { (time.group_opacity(1.0, 0.0), stop.group_opacity(0.0, 1.0)) };
    row()
        .key("end")
        .w(time_w)
        .h(20.0)
        .ml(8.0)
        .justify_end()
        .center_y()
        .child(time)
        .child(stop.pin(None, Some(0.0), Some(0.0), None))
}

/// The panel of the shell the list opened: back, its command, run time and Stop (or how it
/// ended) over the last lines of its output, on the code face over the page's fill.
pub fn panel(s: &App, p: &Pal) -> Option<El> {
    let (sh, phase, read, back_lit) = s.shells.activity.shown()?;
    let back = row()
        .key("back")
        .role("button")
        .label(opencode_i18n::t!("session.activity.shell.back"))
        .size(28.0, 28.0)
        .center()
        .radius(4.0)
        .fill(if back_lit { p.hover } else { p.hover.alpha(0.0) })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .instant_changes()
        .on_hover_change(|s: &mut App, inside, _: &mut Tx| {
            if !inside { shells::dispatch(s, Event::BackLeft); }
        })
        .on_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            shells::dispatch(s, Event::Back);
        })
        .child(icon(&ic::ARROW_LEFT, 16.0).color(p.icon_muted));
    let ms = match phase {
        Phase::Ended(e) => Some(e.completed - sh.started),
        _ => Some(s.run_clock() - sh.started),
    }
    .filter(|_| sh.started > 0);
    let time = ms.map(|ms| match phase {
        Phase::Ended(_) => text(elapsed(ms)).key("time").font(ELAPSED).color(p.muted),
        _ => run_time("time".into(), ms, ELAPSED, p.muted),
    });
    let status = match phase {
        Phase::Ended(e) => {
            let (words, failed) = match (e.status, e.exit) {
                (EndStatus::Timeout, _) => (opencode_i18n::t!("session.activity.shell.timeout").to_owned(), true),
                (EndStatus::Killed, _) => (opencode_i18n::t!("session.activity.shell.killed").to_owned(), false),
                (EndStatus::Exited, None) => (opencode_i18n::t!("session.activity.shell.exited").to_owned(), false),
                (EndStatus::Exited, Some(code)) => (opencode_i18n::t!("session.activity.shell.exitCode", code = code.to_string()).to_string(), code != 0),
            };
            text(words).key("status").role("status").font(UI).color(if failed { p.danger } else { p.muted }).px(6.0)
        }
        phase => {
            let stopping = s.shells.activity.stopping.contains_key(&sh.id);
            let disabled = stopping || phase != Phase::Running;
            let words = if stopping { opencode_i18n::t!("session.activity.shell.stopping") } else { opencode_i18n::t!("session.activity.shell.stop") };
            let id = sh.id.clone();
            let stop = row().key("shell-stop").role("button").label(words).h(28.0).px(6.0).center_y().radius(4.0)
                .child(text(words).font(UI).color(if disabled { p.muted } else { p.text }));
            if disabled {
                stop
            } else {
                stop.hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT).on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    tx.instant();
                    shells::dispatch(s, Event::StopRequested { key: id.clone(), subagent: false });
                })
            }
        }
    };
    let header = row()
        .key("shell-header")
        .w_full()
        .min_h(28.0)
        .gap(8.0)
        .pr(2.0)
        .center_y()
        .child(back)
        .child(text(sh.command.clone()).key("command").font(COMMAND).color(p.text).truncate_fade().grow(1.0).shrink().min_w(0.0))
        .children(time)
        .child(status);
    // The output's last lines, escape codes gone and carriage returns as breaks.
    let clean = strip_ansi(&read.output).replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = clean.trim_end().split('\n').collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let inner = WIDE - 2.0 * PANEL_PAD - 20.0;
    let (body, body_h) = if tail.trim().is_empty() {
        (text(opencode_i18n::t!("session.activity.shell.empty")).key("empty").font(UI).color(p.muted), 16.0)
    } else {
        let h = rune::text::measure(&tail, &[], COMMAND, Some(inner)).1;
        (text(tail).key("tail").font(COMMAND).color(p.text).wrap_anywhere().w_full(), h)
    };
    let room = (MAX_H - 2.0 * PANEL_PAD - 28.0 - 4.0).max(0.0);
    let out_h = (body_h + 16.0).min(room);
    let output = col()
        .key("shell-output")
        .w_full()
        .h(out_h)
        .mt(4.0)
        .radius(6.0)
        .fill(p.bg_base)
        .scroll(rune::ScrollSpec { stick_bottom: true })
        .instant_changes()
        .child(col().w_full().py(8.0).px(10.0).child(body));
    let panel = col()
        .key("activity-panel")
        .role("menu")
        .label(opencode_i18n::t!("session.activity.section.shells"))
        .above("activity-chip", 8.0)
        .w(WIDE)
        .padding(PANEL_PAD)
        .radius(10.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(crate::popover::motion::transition(&s.knobs, rune::Side::Top, rune::Align::Start))
        .on_press(|_: &mut App| {})
        .child(header)
        .child(output);
    Some(panel)
}

/// `stripAnsi`: CSI and OSC sequences and lone escapes out.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                while let Some(&n) = it.peek() {
                    it.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(n) = it.next() {
                    if n == '\u{7}' || (n == '\u{1b}' && it.peek() == Some(&'\\')) {
                        if n == '\u{1b}' { it.next(); }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

