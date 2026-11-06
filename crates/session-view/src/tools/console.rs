//! Shell and Execute: a console card of the command over its output (`ConsoleOutput`), and
//! Execute's nested calls (`ExecuteCallRow`, `ExecuteOutputRow`).

use rune::{Color, El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use super::{Call, Display, Press, Title, Trigger, body, disclosure, field, output, sub_key, trigger};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{CODE, INSTANT, chevron, readable};

/// Desktop caps a console, a call's input and an error at 240 px and scrolls the rest.
pub const MAX_H: f32 = 240.0;

/// A shell call: "Shell" and its command; open, the console. It opens while it runs, its output
/// streaming in.
pub fn shell<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let d = Display::of(s, c, true);
    let command = field(c.input, "command").or_else(|| field(c.metadata, "command")).unwrap_or("");
    // While open the command shows in the console, so the trigger drops it.
    let subtitle = match (d.open(), command.is_empty(), d.phase) {
        (true, _, _) => None,
        (false, true, super::Phase::Pending) => Some("Writing command.".to_owned()),
        (false, _, _) => Some(first_line(command)),
    };
    let t = Trigger { title: Title::Fixed("Shell"), subtitle, code: true, ..Default::default() };
    let out = clean(output(c));
    disclosure(trigger(s, p, c, d, t, Press::Toggle(super::key(c.id))), d, || {
        body(console(s, p, c.id, (!command.is_empty()).then_some(command), &out), false, 8.0)
    })
}

/// An execute call: "Execute" and its latest (or failed) nested call; open, a row per nested
/// call and its output.
pub fn execute<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let calls: Vec<&Value> = c.metadata["toolCalls"].as_array().map(|a| a.iter().filter(|v| v["tool"].is_string()).collect()).unwrap_or_default();
    let failed = *c.status == opencode_kit::model::ToolStatus::Error || c.metadata["error"] == Value::Bool(true);
    let error = failed.then(|| {
        let raw = opencode_kit::model::tool_error(c.metadata).filter(|e| !e.is_empty()).unwrap_or(output(c));
        let text = strip_ansi(raw).trim_start_matches("Error:").trim().to_owned();
        if text.is_empty() { "Failed".to_owned() } else { text }
    });
    let out = if error.is_some() { String::new() } else { clean(output(c)).trim().to_owned() };
    let opens = !calls.is_empty() || error.is_some() || !out.is_empty();
    let d = Display::of(s, c, opens);
    let subtitle = (!d.open()).then(|| subtitle(&calls)).flatten();
    let t = Trigger { title: Title::Fixed("Execute"), subtitle, code: true, ..Default::default() };
    let press = if opens { Press::Toggle(super::key(c.id)) } else { Press::None };
    disclosure(trigger(s, p, c, d, t, press), d, || {
        let owner = calls.iter().rposition(|v| v["status"] == "error");
        let mut list = col().key("calls").w_full().gap(6.0);
        for (n, call) in calls.iter().enumerate() {
            let err = if Some(n) == owner { error.as_deref() } else { None };
            list = list.child(call_row(s, p, c.id, n, Some(call), err));
        }
        if let (None, Some(e)) = (owner, error.as_deref()) {
            list = list.child(call_row(s, p, c.id, calls.len(), None, Some(e)));
        }
        if !out.is_empty() {
            list = list.child(output_row(s, p, c.id, &out));
        }
        body(list, false, 4.0)
    })
}

/// "read [path=ci/toolchain.env]": the failed call, else the latest.
fn subtitle(calls: &[&Value]) -> Option<String> {
    let call = calls.iter().find(|v| v["status"] == "error").or(calls.last())?;
    let tool = call["tool"].as_str()?;
    let args = call_args(call);
    Some(if args.is_empty() { tool.to_owned() } else { format!("{tool} [{}]", args.join(", ")) })
}

fn call_args(call: &Value) -> Vec<String> {
    call["input"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| {
            let v = match v {
                Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some(format!("{k}={v}"))
        })
        .collect()
}

/// One nested call: a marker, its tool and args; open, its input and its error.
fn call_row<S: TimelineHost>(s: &S, p: &Pal, id: &str, n: usize, call: Option<&Value>, error: Option<&str>) -> El {
    let failed = call.is_none_or(|c| c["status"] == "error");
    let input: Vec<(String, String)> = call
        .and_then(|c| c["input"].as_object())
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| serde_json::to_string_pretty(v).unwrap_or_default(), str::to_owned)))
        .collect();
    let opens = !input.is_empty() || error.is_some();
    let k = sub_key(id, format!("call-{n}"));
    let open = opens && s.expanded().contains(&k);
    let marker = if failed {
        icon(&ic::WARNING, 14.0).color(p.danger)
    } else {
        text("›").font(CODE.lh(20.0)).color(p.faint)
    };
    let mut line = row().key(format!("call-{n}")).w_full().gap(6.0).center_y().child(el().w(14.0).center().child(marker));
    if let Some(c) = call {
        let running = c["status"] == "running";
        line = line.child(text(c["tool"].as_str().unwrap_or("").to_owned()).font(CODE.lh(20.0)).color(if running { p.muted } else { p.text }));
    }
    let summary = match call {
        Some(c) => Some(call_args(c).join(", ")).filter(|a| !a.is_empty()).map(|a| format!("[{a}]")),
        None => error.and_then(|e| e.lines().next()).map(str::to_owned),
    };
    if let Some(sum) = summary.filter(|_| !open) {
        line = line.child(text(sum).font(CODE.lh(20.0)).color(p.muted).truncate_fade().shrink().min_w(0.0));
    }
    if error.is_some() {
        line = line.child(icon(chevron(open), 14.0).color(p.icon_faint));
    }
    if opens {
        line = line.role("button").hover_group().hover_motion(INSTANT, INSTANT).on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{id}")));
    }
    let mut c = col().key(format!("call-item-{n}")).w_full().child(line);
    if open {
        if !input.is_empty() {
            let mut entries = col().key("input").w_full().gap(2.0).pl(13.0).ml(4.0).mt(4.0);
            for (key, value) in input {
                entries = entries.child(
                    row().w_full().gap(6.0).child(text(format!("{key}:")).font(CODE.lh(20.0)).color(p.muted))
                        .child(readable(p, text(value).font(CODE.lh(20.0)).color(p.text).wrap_anywhere()).shrink().min_w(0.0)),
                );
            }
            c = c.child(row().w_full().child(el().w(0.5).h_full().fill(p.border).ml(4.0)).child(capped(entries)));
        }
        if let Some(e) = error {
            c = c.child(capped(col().key("error").w_full().pl(20.0).mt(4.0).child(readable(p, text(e.to_owned()).font(CODE.lh(20.0)).color(p.danger).wrap_anywhere()))));
        }
    }
    c
}

/// "Output · 3 lines"; open, a console of the script's result.
fn output_row<S: TimelineHost>(s: &S, p: &Pal, id: &str, out: &str) -> El {
    let k = sub_key(id, "output");
    let open = s.expanded().contains(&k);
    let lines = out.split('\n').count();
    let line = row()
        .key("output-row")
        .w_full()
        .gap(6.0)
        .center_y()
        .role("button")
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{id}")))
        .child(el().w(14.0).center().child(icon(chevron(open), 12.0).color(p.faint)))
        .child(text("Output").font(CODE.lh(20.0)).color(p.text))
        .child(text(format!("{lines} {}", if lines == 1 { "line" } else { "lines" })).font(CODE.lh(20.0)).color(p.muted));
    let c = col().key("output-item").w_full().child(line);
    if open { c.child(col().w_full().pl(20.0).mt(4.0).child(console(s, p, id, None, out))) } else { c }
}

/// Desktop's console card: the command over its result, each with a copy button on hover, in
/// one region that scrolls past [`MAX_H`].
pub fn console<S: TimelineHost>(s: &S, p: &Pal, id: &str, command: Option<&str>, out: &str) -> El {
    let mut inner = col().key("console-text").w_full();
    if let Some(cmd) = command {
        inner = inner.child(section(s, p, id, "command", cmd, p.bg_base, p.text, "Copy", false));
    }
    if !out.is_empty() {
        inner = inner.child(section(s, p, id, "result", out, p.layer1, p.muted, "Copy output", command.is_some()));
    }
    col()
        .key("console")
        .w_full()
        .radius(6.0)
        .border(0.5, p.border)
        .fill(p.bg_base)
        .clip_rounded(6.0)
        .hover_group()
        .child(capped(inner))
}

/// `content` in a region at most [`MAX_H`] tall that scrolls the rest.
fn capped(content: El) -> El {
    col().key("scroll").w_full().max_h(MAX_H).scroll(ScrollSpec { stick_bottom: false }).child(content)
}

#[allow(clippy::too_many_arguments)]
fn section<S: TimelineHost>(s: &S, p: &Pal, id: &str, part: &str, body: &str, fill: Color, ink: Color, label: &'static str, rule: bool) -> El {
    let copy_key = format!("copy-tool-{id}-{part}");
    let copied = body.to_owned();
    let button = row()
        .key("copy")
        .role("button")
        .label(label)
        .help(label)
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .fill(fill)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.copy(copied.clone());
            crate::copied::mark_copied(s, copy_key.clone(), tx);
        })
        .child(crate::copied::glyph(s, &format!("copy-tool-{id}-{part}"), 14.0, p.icon_muted));
    let mut sec = col()
        .key(part.to_owned())
        .w_full()
        .fill(fill)
        .padding(12.0)
        .child(readable(p, text(body.to_owned()).font(CODE).color(ink).wrap_anywhere()).w_full())
        .child(button.pin(None, Some(4.0), Some(4.0), None));
    if rule {
        sec = sec.child(el().key("rule").absolute(0.0, 0.0).w_full().h(0.5).fill(p.border));
    }
    sec
}

/// A shell's output as Desktop shows it: no ANSI codes, `\n` line ends.
fn clean(out: &str) -> String {
    strip_ansi(out).replace("\r\n", "\n").replace('\r', "\n")
}

fn first_line(s: &str) -> String {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_owned()
}

/// `text` without ANSI escapes: CSI sequences (colours, cursor moves) and OSC sequences ending
/// in BEL or ESC \, as Desktop's `stripAnsi` (ansi-regex) removes them.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' && ch != '\u{9b}' {
            out.push(ch);
            continue;
        }
        if ch == '\u{1b}' && chars.peek() == Some(&']') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '\u{7}' || c == '\u{9c}' || (c == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                    break;
                }
            }
            continue;
        }
        // CSI and its kin: intermediates and parameters, then one final byte.
        while chars.next_if(|c| "[]()#;?:0123456789".contains(*c)).is_some() {}
        chars.next_if(|c| c.is_ascii_alphabetic() || "=><~".contains(*c));
    }
    out
}
