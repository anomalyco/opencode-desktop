//! The background shells in the running-work list (`activity-chips.tsx`): each shell's row, and
//! the panel a row opens in place (`session-activity-shell-*`, `activity-chips.css:135-236`).

use rune::{El, Tx, col, icon, row, text};
use rune_knobs::Knobs;
use rune_theme_opencode::icons as ic;
use session_view::palette::{Elevate as _, Pal, kit};
use session_view::style::{INSTANT, UI};

use super::tail;
use super::{Shell, dispatch};
use crate::activity::{End, EndStatus, Event, Phase};
use crate::{ELAPSED, PANEL_PAD, SubagentsHost, elapsed, panel_row, run_clock, run_time};

/// The command in a row and the panel's header: the code face at 13/16 (`session-activity-shell-command`).
pub const COMMAND: rune::TextStyle = session_view::style::TOOL_CODE.lh(16.0);
/// The shell panel's width (`min(560px, 100vw - 56px)`) and height cap (`min(320px, …)`).
const WIDE: f32 = 560.0;
const MAX_H: f32 = 320.0;
/// The tail shows its last 14 lines (`TAIL_LINES`).
const TAIL_LINES: usize = 14;

/// A shell's row in the list: the console mark, its command across the agent and task columns,
/// and its run time, which hover swaps for a stop button.
pub fn row_of<H: SubagentsHost + 'static>(host: &H, p: &Pal, sh: &Shell, span: f32, time_w: f32) -> El {
    let open = sh.clone();
    let end_el = end(host, p, &sh.id, false, (sh.started > 0).then(|| run_clock(host) - sh.started), time_w);
    build_row_shell(p, sh, span, end_el).on_press_tx(move |s: &mut H, tx: &mut Tx| select_shell(s, open.clone(), tx))
}

/// A row's end (`session-activity-end`): its run time, and the stop button over it, shown under
/// the pointer or while its Stop is out.
pub fn end<H: SubagentsHost + 'static>(host: &H, p: &Pal, key: &str, subagent: bool, ms: Option<i64>, time_w: f32) -> El {
    let stopping = host.shells().activity.stopping.contains_key(key);
    let target = key.to_owned();
    let stop = build_stop_button_shell(p, subagent, stopping).on_press_tx(move |s: &mut H, tx: &mut Tx| request_stop(s, target.clone(), subagent, tx));
    build_end_layout(p, key, ms, time_w, stopping, stop)
}

/// The panel of the shell the list opened: back, its command, run time and Stop (or how it
/// ended) over the last lines of its output, on the code face over the page's fill.
pub fn panel<H: SubagentsHost + 'static>(host: &H, p: &Pal) -> Option<El> {
    let (sh, phase, read, back_lit) = host.shells().activity.shown()?;
    let back = build_back_button_shell(p, back_lit)
        .on_hover_change(|s: &mut H, inside, _: &mut Tx| back_hover_changed(s, inside))
        .on_press_tx(|s: &mut H, tx: &mut Tx| back_pressed(s, tx));
    let status = match phase {
        Phase::Ended(e) => build_ended_status(p, e),
        phase => {
            let stopping = host.shells().activity.stopping.contains_key(&sh.id);
            let disabled = stopping || phase != Phase::Running;
            let stop = build_panel_stop_shell(p, stopping, disabled);
            if disabled {
                stop
            } else {
                let id = sh.id.clone();
                stop.on_press_tx(move |s: &mut H, tx: &mut Tx| request_stop(s, id.clone(), false, tx))
            }
        }
    };
    let (header, output_box, output_inner) = build_panel_sections(host, p, sh, phase, read, back, status);
    // `session-activity-shell-output`: a plain `overflow-y: auto` box's bar (`activity-chips.css:207-212`).
    let output = session_view::scroller::scroller("shell-output").stick_bottom().build::<H>(p, output_box, output_inner);
    Some(build_panel_frame(host.knobs(), p, header, output).on_press(|_: &mut H| {}))
}

fn select_shell(host: &mut dyn SubagentsHost, shell: Shell, tx: &mut Tx) {
    tx.instant();
    dispatch(host, Event::Selected(shell));
}

fn request_stop(host: &mut dyn SubagentsHost, key: String, subagent: bool, tx: &mut Tx) {
    tx.instant();
    dispatch(host, Event::StopRequested { key, subagent });
}

fn back_hover_changed(host: &mut dyn SubagentsHost, inside: bool) {
    if !inside {
        dispatch(host, Event::BackLeft);
    }
}

fn back_pressed(host: &mut dyn SubagentsHost, tx: &mut Tx) {
    tx.instant();
    dispatch(host, Event::Back);
}

fn build_row_shell(p: &Pal, sh: &Shell, span: f32, end_el: El) -> El {
    row()
        .key(format!("shell-{}", sh.id))
        .role("menuitem")
        .label(sh.command.clone())
        .w_full()
        .h(panel_row())
        .px(4.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .hover_group()
        .child(icon(&ic::CONSOLE, 16.0).key("mark").color(p.icon_muted))
        .child(text(sh.command.clone()).key("command").font(COMMAND).color(p.text).truncate_fade().w(span).grow(1.0).shrink().min_w(0.0))
        .child(end_el)
}

fn build_stop_button_shell(p: &Pal, subagent: bool, stopping: bool) -> El {
    let label = if subagent { opencode_i18n::t!("session.running.stop.subagent") } else { opencode_i18n::t!("session.running.stop.shell") };
    let mark = if stopping { rune_kit::Loader::new().size(14.0).build(kit(p)) } else { icon(&ic::OUTLINE_XMARK, 14.0).color(p.icon_muted) };
    row()
        .key("stop")
        .role("button")
        .label(label)
        .size(20.0, 20.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .hover_motion(INSTANT, INSTANT)
        .hover_alone()
        .child(mark)
}

fn build_end_layout(p: &Pal, key: &str, ms: Option<i64>, time_w: f32, stopping: bool, stop: El) -> El {
    let time = row().key("time").w_full().h_full().justify_end().center_y().children(ms.map(|ms| run_time(format!("time-{key}"), ms, ELAPSED, p.muted)));
    if session_view::style::hover_none() {
        return row().key("end").h(44.0).gap(2.0).ml(8.0).center_y().child(time.w(time_w)).child(stop.size(44.0, 44.0));
    }
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

fn build_back_button_shell(p: &Pal, back_lit: bool) -> El {
    row()
        .key("back")
        .role("button")
        .label_static(opencode_i18n::t!("session.activity.shell.back"))
        .size(28.0, 28.0)
        .center()
        .radius(4.0)
        .fill(if back_lit { p.hover } else { p.hover.alpha(0.0) })
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .instant_changes()
        .child(icon(&ic::ARROW_LEFT, 16.0).color(p.icon_muted))
}

fn build_ended_status(p: &Pal, e: End) -> El {
    let (words, failed) = match (e.status, e.exit) {
        (EndStatus::Timeout, _) => (opencode_i18n::t!("session.activity.shell.timeout").to_owned(), true),
        (EndStatus::Killed, _) => (opencode_i18n::t!("session.activity.shell.killed").to_owned(), false),
        (EndStatus::Exited, None) => (opencode_i18n::t!("session.activity.shell.exited").to_owned(), false),
        (EndStatus::Exited, Some(code)) => (opencode_i18n::t!("session.activity.shell.exitCode", code = code.to_string()).to_string(), code != 0),
    };
    text(words).key("status").role("status").font(UI).color(if failed { p.danger } else { p.muted }).px(6.0)
}

fn build_panel_stop_shell(p: &Pal, stopping: bool, disabled: bool) -> El {
    let words = if stopping { opencode_i18n::t!("session.activity.shell.stopping") } else { opencode_i18n::t!("session.activity.shell.stop") };
    let stop = row()
        .key("shell-stop")
        .role("button")
        .label(words)
        .h(28.0)
        .px(6.0)
        .center_y()
        .radius(4.0)
        .child(text(words).font(UI).color(if disabled { p.muted } else { p.text }));
    if disabled { stop } else { stop.hover(p.hover).press(p.pressed).hover_motion(INSTANT, INSTANT) }
}

fn build_panel_sections(host: &dyn SubagentsHost, p: &Pal, sh: &Shell, phase: Phase, read: &tail::Text, back: El, status: El) -> (El, El, El) {
    let ms = match phase {
        Phase::Ended(e) => Some(e.completed - sh.started),
        _ => Some(run_clock(host) - sh.started),
    }
    .filter(|_| sh.started > 0);
    let time = ms.map(|ms| match phase {
        Phase::Ended(_) => text(elapsed(ms)).key("time").font(ELAPSED).color(p.muted),
        _ => run_time("time".into(), ms, ELAPSED, p.muted),
    });
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
    let output_box = col().key("shell-output").w_full().h(out_h).mt(4.0).radius(6.0).fill(p.bg_base).instant_changes();
    let output_inner = col().w_full().py(8.0).px(10.0).child(body);
    (header, output_box, output_inner)
}

fn build_panel_frame(knobs: &Knobs, p: &Pal, header: El, output: El) -> El {
    col()
        .key("activity-panel")
        .role("menu")
        .label_static(opencode_i18n::t!("session.activity.section.shells"))
        .above("activity-chip", 8.0)
        .w(WIDE)
        .padding(PANEL_PAD)
        .radius(10.0)
        .fill(p.layer1)
        .elevate(p.floating)
        .clip_soft(0.0)
        .transition(session_view::popover::motion::transition(knobs, rune::Side::Top, rune::Align::Start))
        .child(header)
        .child(output)
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
                        if n == '\u{1b}' {
                            it.next();
                        }
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
