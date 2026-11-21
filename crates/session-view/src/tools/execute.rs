//! Execute: "Execute" and its latest (or failed) nested call; open, a row per nested call
//! (`ExecuteCallRow`), its result (`ExecuteOutputRow`), and a runtime failure under the call that
//! raised it. A View code button beside the trigger opens the script (`ExecuteDetails`).

use opencode_kit::model::ToolStatus;
use rune::{El, Tx, col, el, icon, row, text};
use serde_json::Value;

use super::console::{capped, console};
use super::plain::{linked, one_line, strip_ansi};
use super::{Call, Ctx, Display, Press, Trigger, body, disclosure, field, output, sub_key, trigger};
use crate::TimelineHost;
use crate::focus::FocusExt;
use crate::palette::Pal;
use crate::style::{INSTANT, TOOL_CODE, readable};

pub fn execute<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let calls = calls(c.metadata);
    let error = error(c);
    let out = if error.is_some() { "" } else { output(c).trim() };
    let opens = !calls.is_empty() || error.is_some() || !out.is_empty();
    let d = Display::of(s, cx, c, opens);
    let subtitle = (!d.open()).then(|| subtitle(&calls)).flatten();
    let t = Trigger { title: opencode_i18n::t!("ui.tool.execute").into(), compact: true, subtitle, code: true, ..Default::default() };
    let press = if opens { Press::Toggle(super::key(c.id)) } else { Press::None };
    let code = field(c.input, "code").map(str::trim_end).filter(|code| !code.is_empty());
    // The script is whole once the model has written the call, so it can be read while it runs.
    let view_code = code.filter(|_| *c.status != ToolStatus::Pending).map(|_| view_code_button::<S>(p, cx.session, c.id));
    disclosure(s, trigger(s, p, cx, c, d, t, press, view_code), d, || {
        let owner = calls.iter().rposition(|v| v["status"] == "error");
        let mut list = col().key("calls").w_full().gap(6.0);
        for (n, call) in calls.iter().enumerate() {
            let err = if Some(n) == owner { error.as_deref() } else { None };
            list = list.child(call_row(s, p, cx, c.id, n, Some(call), err));
        }
        if let (None, Some(e)) = (owner, error.as_deref()) {
            list = list.child(call_row(s, p, cx, c.id, calls.len(), None, Some(e)));
        }
        if !out.is_empty() {
            list = list.child(output_row(s, p, cx.session, c.id, out));
        }
        body(list, false, 4.0)
    })
}

/// The nested calls Code Mode reported (`metadata.toolCalls`).
pub fn calls(metadata: &Value) -> Vec<&Value> {
    metadata["toolCalls"].as_array().map(|a| a.iter().filter(|v| v["tool"].is_string()).collect()).unwrap_or_default()
}

/// A runtime failure's message, without its "Error:" prefix; "Failed" when it says nothing.
pub fn error(c: &Call) -> Option<String> {
    let failed = *c.status == ToolStatus::Error || c.metadata["error"] == Value::Bool(true);
    failed.then(|| {
        let raw = opencode_kit::model::tool_error(c.metadata).filter(|e| !e.is_empty() && *c.status == ToolStatus::Error).unwrap_or(output(c));
        let text = strip_ansi(raw);
        let text = text.trim_start().strip_prefix("Error:").unwrap_or(&text).trim().to_owned();
        if text.is_empty() { opencode_i18n::t!("ui.toolErrorCard.failed").to_owned() } else { text }
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
                Value::String(s) => one_line(s),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some(format!("{k}={v}"))
        })
        .collect()
}

/// Execute's `›` marker and arrow: the code font's own glyph, turned down while open.
fn chevron(p: &Pal, open: bool) -> El {
    let g = text("›").font(TOOL_CODE).color(p.faint);
    el().key("arrow").child(g.rotate(if open { std::f32::consts::FRAC_PI_2 } else { 0.0 }).rotate_motion(INSTANT))
}

/// One nested call: a marker, its tool (shimmering while it runs) and args; open, its input and
/// its error. Its name and args brighten under the pointer when it opens.
fn call_row<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, id: &str, n: usize, call: Option<&Value>, error: Option<&str>) -> El {
    let failed = call.is_none_or(|c| c["status"] == "error");
    let entries = call.and_then(|c| c["input"].as_object());
    let opens = entries.is_some_and(|e| !e.is_empty()) || error.is_some();
    let k = sub_key(id, format!("call-{n}"));
    let open = opens && s.expanded().contains(&k);
    let marker = if failed {
        icon(&rune_theme_opencode::icons::OUTLINE_HEXAGONAL_WARNING, 14.0).color(p.danger).offset(0.0, 1.0)
    } else {
        text("›").font(TOOL_CODE).color(p.faint)
    };
    let mut line = row().key(format!("call-{n}")).w_full().gap(6.0).center_y().child(el().w(14.0).center().child(marker));
    if let Some(c) = call {
        let name = c["tool"].as_str().unwrap_or("");
        let running = c["status"] == "running";
        let shown = if running && cx.live {
            crate::working::shimmer(s, p, name, TOOL_CODE, 0.0)
        } else {
            text(name.to_owned()).font(TOOL_CODE).color(p.text)
        };
        line = line.child(shown.key("name"));
    }
    let summary = match call {
        Some(c) => Some(call_args(c).join(", ")).filter(|a| !a.is_empty()).map(|a| format!("[{a}]")),
        None => error.and_then(|e| e.lines().next()).map(str::to_owned),
    };
    if let Some(sum) = summary.filter(|_| !open) {
        let args = text(sum).key("args").font(TOOL_CODE).color(if call.is_some() { p.muted } else { p.danger }).truncate_fade().shrink().min_w(0.0);
        line = line.child(if opens && call.is_some() { args.group_color(p.text) } else { args });
    }
    if error.is_some() {
        line = line.child(chevron(p, open));
    }
    if opens {
        line = line
            .role("button")
            .expanded(open)
            .radius(4.0)
            .cursor(rune::Cursor::Pointer)
            .focus_default(p)
            .hover_group_focus()
            .hover_motion(INSTANT, INSTANT)
            .on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{id}")));
    }
    let mut item = col().key(format!("call-item-{n}")).w_full().child(line);
    if !open {
        return item;
    }
    if let Some(entries) = entries.filter(|e| !e.is_empty()) {
        let mut list = col().key("input").w_full().gap(2.0).pl(13.0);
        for (key, value) in entries {
            let value = value.as_str().map_or_else(|| serde_json::to_string_pretty(value).unwrap_or_default(), str::to_owned);
            list = list.child(
                row().w_full().gap(6.0).child(text(format!("{key}:")).font(TOOL_CODE).color(p.muted))
                    .child(readable(p, text(value).font(TOOL_CODE).color(p.text).wrap_anywhere()).shrink().min_w(0.0)),
            );
        }
        // A hairline down the input's left, 4 pt in from the marker (`margin: 4px 0 2px 4px`).
        item = item.child(crate::disclosure::reveal(
            s.knobs(),
            row().key("input-box").w_full().mt(4.0).mb(2.0).pl(4.0)
                .child(el().key("rule").w(0.5).self_stretch().fill(p.border))
                .child(capped(list).grow(1.0).min_w(0.0)),
        ));
    }
    if let Some(e) = error {
        item = item.child(crate::disclosure::reveal(s.knobs(), capped(col().key("error").w_full().pl(20.0).mt(4.0).child(readable(p, linked::<S>(p, e, TOOL_CODE, p.danger))))));
    }
    item
}

/// "Output · 3 lines"; open, a console of the script's result.
fn output_row<S: TimelineHost>(s: &S, p: &Pal, session: &str, id: &str, out: &str) -> El {
    let k = sub_key(id, "output");
    let open = s.expanded().contains(&k);
    let lines = out.split('\n').count();
    let line = row()
        .key("output-row")
        .w_full()
        .gap(6.0)
        .center_y()
        .role("button")
        .expanded(open)
        .radius(4.0)
        .cursor(rune::Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{id}")))
        .child(el().w(14.0).center().child(chevron(p, open)))
        .child(text(opencode_i18n::t!("ui.tool.execute.output")).key("name").font(TOOL_CODE).color(p.text))
        .child(text(opencode_i18n::plural!("ui.tool.execute.lines", lines)).key("args").font(TOOL_CODE).color(p.muted).group_color(p.text));
    let c = col().key("output-item").w_full().child(line);
    if open { c.child(crate::disclosure::reveal(s.knobs(), col().key("output").w_full().pl(20.0).mt(4.0).child(console(s, p, session, id, None, out)))) } else { c }
}

/// View code: a 24 pt ghost icon button that opens the script and its result in a dialog.
fn view_code_button<S: TimelineHost>(p: &Pal, session: &str, id: &str) -> El {
    let open = super::details::key(session, id);
    row()
        .key("view-code")
        .role("button")
        .label_static(opencode_i18n::t!("ui.tool.execute.viewCode"))
        .help(opencode_i18n::t!("ui.tool.execute.viewCode"))
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .cursor(rune::Cursor::Pointer)
        .focus_default(p)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.expanded_mut().insert(open.clone());
        })
        .child(crate::style::hover_reveal(icon(&rune_theme_opencode::icons::CODE_SLASH, 14.0).color(p.text)))
}
