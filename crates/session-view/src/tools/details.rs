//! Execute's View code dialog (`ExecuteDetails`): the script Code Mode ran, numbered and coloured
//! as JavaScript, over its result (coloured as JSON when it is JSON) or its error, each with a
//! copy button. Escape or a click outside closes it.

use opencode_kit::model::{Message, Part, ToolStatus};
use rune::{Cx, El, ScrollSpec, Transition, Tx, col, el, icon, row, text};

use super::Call;
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{CODE, INSTANT, UI_MED, readable};

/// The key in [`TimelineHost::expanded`] that holds the dialog open on call `id` of `session`.
pub fn key(session: &str, id: &str) -> String {
    format!("execute-details:{session}:{id}")
}

/// What the dialog shows, read from the call.
struct Details {
    id: String,
    code: String,
    output: String,
    error: Option<String>,
    calls: usize,
    status: ToolStatus,
}

/// The open dialog, over the whole window: a scrim, and the dialog.
pub fn overlay<S: TimelineHost>(s: &S, p: &Pal, cx: &Cx<S>) -> Vec<El> {
    let Some(open) = s.expanded().iter().find(|k| k.starts_with("execute-details:")).cloned() else { return Vec::new() };
    let Some((session, id)) = open.trim_start_matches("execute-details:").split_once(':') else { return Vec::new() };
    let Some(d) = s.with_transcript(session, |t| find(t, id)).flatten() else { return Vec::new() };
    let [win_w, win_h] = cx.layout_size("window").unwrap_or([1280.0, 820.0]);
    let w = (win_w - 32.0).min(720.0);
    let close = move |s: &mut S| {
        s.expanded_mut().retain(|k| !k.starts_with("execute-details:"));
    };
    let lines = |t: &str| t.split('\n').count();
    // Code and output share one number gutter, so their code starts on the same column.
    let digits = lines(&d.code).max(if d.error.is_none() { lines(&d.output) } else { 0 }).to_string().len();
    let mut sections = col().key("execute-details").w_full().gap(16.0).px(16.0).pb(16.0);
    sections = sections.child(section(s, p, &d.id, "code", "Code", Some(plural(lines(&d.code), "line")), &d.code, "Copy code", super::diff::code(p, &d.code, Some("js"), digits)));
    match &d.error {
        Some(e) => {
            let pre = col().w_full().px(14.0).py(12.0).fill(p.layer1).child(readable(p, text(e.clone()).font(CODE).color(p.danger).wrap_anywhere()).w_full());
            sections = sections.child(section(s, p, &d.id, "error", "Error", None, e, "Copy error", pre));
        }
        None if !d.output.is_empty() => {
            let json = serde_json::from_str::<serde_json::Value>(&d.output).is_ok();
            let view = super::diff::code(p, &d.output, json.then_some("json"), digits);
            sections = sections.child(section(s, p, &d.id, "output", "Output", Some(plural(lines(&d.output), "line")), &d.output, "Copy output", view));
        }
        None => {}
    }
    let (status, status_ink) = match (&d.error, &d.status) {
        (Some(_), _) => ("Failed", p.danger),
        (None, ToolStatus::Running | ToolStatus::Pending) => ("Running", p.muted),
        _ => ("Completed", p.muted),
    };
    let header = row()
        .key("header")
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .center_y()
        .child(text("Execute").key("title").font(rune::TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13)).color(p.text))
        .child(row().key("status").px(6.0).radius(4.0).fill(p.layer2).child(text(status).font(rune::TextStyle::new(12.0, 440.0).lh(20.0)).color(status_ink)))
        .child(text(plural(d.calls, "call")).key("meta").font(rune::TextStyle::new(12.0, 440.0).lh(16.0)).color(p.faint))
        .child(el().grow(1.0))
        .child(
            row()
                .key("close")
                .role("button")
                .label("Close")
                .size(20.0, 20.0)
                .center()
                .radius(4.0)
                .hover(p.hover)
                .hover_motion(INSTANT, INSTANT)
                .on_press_tx(move |s: &mut S, tx: &mut Tx| {
                    tx.instant();
                    close(s);
                })
                .child(icon(&rune_theme_opencode::icons::CLOSE, 16.0).color(p.icon_muted)),
        );
    let dialog = col()
        .key("execute-dialog")
        .role("dialog")
        .label("Execute")
        .w(w)
        .max_h((win_h - 92.0).max(120.0))
        .radius(6.0)
        .fill(p.layer1)
        .border(0.5, p.ring)
        .shadow(16.0, 32.0, p.shadow_far)
        .shadow(8.0, 16.0, p.shadow)
        .clip_rounded(6.0)
        .transition(Transition::none())
        .modal(true, close)
        .child(header)
        .child(col().key("body").w_full().shrink().min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(sections));
    let scrim = el().key("execute-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let modal = el().key("execute-modal").absolute(0.0, 0.0).w_full().h_full().center().transition(Transition::none()).child(dialog).raise();
    vec![scrim, modal]
}

/// Call `id` of the transcript, as the dialog shows it.
fn find(t: &opencode_kit::model::Transcript, id: &str) -> Option<Details> {
    t.messages.iter().find_map(|m| match m {
        Message::Assistant(a) => a.parts.iter().find_map(|part| match part {
            Part::Tool { id: i, name, status, input, metadata, .. } if i == id => {
                let c = Call { id, name, status, input, metadata };
                let error = super::execute::error(&c);
                let output = if error.is_some() { String::new() } else { super::plain::clean(super::output(&c)).trim().to_owned() };
                let code = input["code"].as_str().unwrap_or("").trim_end().to_owned();
                Some(Details { id: id.to_owned(), code, output, error, calls: super::execute::calls(metadata).len(), status: status.clone() })
            }
            _ => None,
        }),
        _ => None,
    })
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// One section: its label, how long it is, a copy button at the end, and its source in a
/// rounded box at most 360 pt tall that scrolls the rest.
#[allow(clippy::too_many_arguments)]
fn section<S: TimelineHost>(s: &S, p: &Pal, id: &str, part: &str, label: &'static str, meta: Option<String>, copy: &str, copy_label: &'static str, source: El) -> El {
    let copy_key = format!("copy-execute-{id}-{part}");
    let done = crate::copied::is_copied(s, &copy_key);
    let tip = if done { "Copied" } else { copy_label };
    let copied = copy.to_owned();
    let button = row()
        .key("copy")
        .role("button")
        .label(tip)
        .help(tip)
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx({
            let copy_key = copy_key.clone();
            move |s: &mut S, tx: &mut Tx| {
                tx.copy(copied.clone());
                crate::copied::mark_copied(s, copy_key.clone(), tx);
            }
        })
        .child(crate::copied::glyph(s, &copy_key, 14.0, p.icon_muted));
    let mut head = row().key("head").w_full().h(24.0).gap(8.0).center_y().child(text(label).font(UI_MED).color(p.muted));
    if let Some(m) = meta {
        head = head.child(text(m).font(rune::TextStyle::new(12.0, 440.0).lh(16.0)).color(p.faint));
    }
    head = head.child(el().grow(1.0)).child(button);
    let boxed = col()
        .key("source")
        .w_full()
        .max_h(360.0)
        .padding(0.5)
        .radius(8.0)
        .border(0.5, p.border)
        .clip_rounded(8.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(source);
    col().key(part.to_owned()).w_full().gap(6.0).child(head).child(boxed)
}
