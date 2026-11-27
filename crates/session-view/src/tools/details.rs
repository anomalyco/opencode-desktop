//! Execute's View code dialog (`ExecuteDetails`): the script Code Mode ran, numbered and coloured
//! as JavaScript, over its result (coloured as JSON when it is JSON) or its error (numbered
//! too, in the danger colour), each with a copy button. Escape or a click outside closes it.

use opencode_kit::model::{Message, Part, ToolStatus};
use rune::{Cx, El, Transition, Tx, col, el, icon, row, text};

use super::Call;
use crate::TimelineHost;
use crate::palette::{Elevate as _, Pal};
use crate::style::{INSTANT, UI_MED};

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
    status: ToolStatus,
}

/// The open dialog, over the whole window: a scrim, and the dialog.
pub fn overlay<S: TimelineHost>(s: &S, p: &Pal, cx: &Cx<S>) -> Vec<El> {
    let Some(open) = s.expanded().iter().find(|k| k.starts_with("execute-details:")).cloned() else { return Vec::new() };
    let Some((session, id)) = open.trim_start_matches("execute-details:").split_once(':') else { return Vec::new() };
    let Some(d) = s.with_transcript(session, |t| find(t, id)).flatten() else { return Vec::new() };
    let [win_w, win_h] = cx.layout_size("window").unwrap_or([1280.0, 820.0]);
    // `<Dialog size="large" fit>` (`tool-renderer.tsx`): as tall as its sections up to the large
    // height, growing down from its top edge.
    let size = crate::dialog::DialogSize::Large;
    let w = size.width(win_w);
    let close = move |s: &mut S| {
        s.expanded_mut().retain(|k| !k.starts_with("execute-details:"));
    };
    let lines = |t: &str| t.split('\n').count();
    // Code and output or error share one number gutter, sized to the longer block, so their
    // content starts on the same column (`numberDigits`, opencode#66295).
    let under = match &d.error {
        Some(e) => lines(e),
        None if !d.output.is_empty() => lines(&d.output),
        None => 0,
    };
    let digits = lines(&d.code).max(under).to_string().len();
    let mut sections = col().key("execute-details").w_full().gap(16.0).px(16.0).pb(16.0);
    sections = sections.child(section(s, p, &d.id, "code", opencode_i18n::t!("ui.tool.execute.code"), &d.code, opencode_i18n::t!("ui.tool.execute.copyCode"), super::diff::code(p, &d.code, Some("js"), digits, None)));
    match &d.error {
        Some(e) => {
            // The error is a numbered text file like the code, its lines in the danger colour
            // (`data-tone="danger"`, `--opencode-diffs-fg`; opencode#66295).
            let view = super::diff::code(p, e, None, digits, Some(p.danger));
            sections = sections.child(section(s, p, &d.id, "error", opencode_i18n::t!("ui.tool.execute.error"), e, opencode_i18n::t!("ui.toolErrorCard.copyError"), view));
        }
        None if !d.output.is_empty() => {
            let view = if d.output.len() > 4096 {
                super::console::output_view::<S>(p,session,id,&d.output,true)
            } else {
                let json = serde_json::from_str::<serde_json::Value>(&d.output).is_ok();
                super::diff::code(p, &d.output, json.then_some("json"), digits, None)
            };
            sections = sections.child(section(s, p, &d.id, "output", opencode_i18n::t!("ui.tool.execute.output"), &d.output, opencode_i18n::t!("ui.tool.execute.copyOutput"), view));
        }
        None => {}
    }
    let status = match (&d.error, &d.status) {
        (Some(_), _) => opencode_i18n::t!("ui.toolErrorCard.failed"),
        (None, ToolStatus::Running | ToolStatus::Pending) => opencode_i18n::t!("ui.tool.execute.running"),
        _ => opencode_i18n::t!("ui.tool.execute.completed"),
    };
    let header = row()
        .key("header")
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .center_y()
        .child(text(opencode_i18n::t!("ui.tool.execute")).key("title").font(rune::TextStyle::new(15.0, 530.0).lh(20.0).tracking(-0.13)).color(p.text))
        // The status is a `Badge`, danger for a failed run; the call count is gone (opencode#66295).
        .child(crate::activity::tag(p, status, d.error.is_some()))
        .child(el().grow(1.0))
        .child(
            row()
                .key("close")
                .role("button")
                .label_static(opencode_i18n::t!("ui.common.close"))
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
        .label_static(opencode_i18n::t!("ui.tool.execute"))
        .w(w)
        .max_h(size.height(win_h))
        // Large dialogs sit on bg-base with 12 pt corners (`dialog.css`).
        .radius(12.0)
        .fill(p.bg_base)
        .elevate(p.overlay)
        .clip_rounded(12.0)
        .transition(Transition::none())
        .modal(true, close)
        .child(header)
        // `DialogBody` with `overflow-y-auto`: a plain box's bar (`tool-renderer.tsx:2024`).
        .child(crate::scroller::scroller("body").build::<S>(p, col().w_full().shrink().min_h(0.0), sections));
    let scrim = el().key("execute-scrim").absolute(0.0, 0.0).w_full().h_full().fill(p.scrim).transition(Transition::none());
    let modal = col().key("execute-modal").absolute(0.0, 0.0).w_full().h_full().items_center().pt(size.top(win_h)).transition(Transition::none()).child(dialog).raise();
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
                Some(Details { id: id.to_owned(), code, output, error, status: status.clone() })
            }
            _ => None,
        }),
        _ => None,
    })
}

/// One section: its label, a copy button at the end, and its source in a rounded box at most
/// 360 pt tall that scrolls the rest.
#[allow(clippy::too_many_arguments)]
fn section<S: TimelineHost>(s: &S, p: &Pal, id: &str, part: &str, label: &'static str, copy: &str, copy_label: &'static str, source: El) -> El {
    let copy_key = format!("copy-execute-{id}-{part}");
    let done = crate::copied::is_copied(s, &copy_key);
    let tip = if done { opencode_i18n::t!("ui.message.copied") } else { copy_label };
    let copied = copy.to_owned();
    let button = row()
        .key("copy")
        .role("button")
        .label(tip)
        .help(tip)
        .help_open(done)
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
    let head = row().key("head").w_full().h(24.0).gap(8.0).center_y().child(text(label).font(UI_MED).color(p.muted)).child(el().grow(1.0)).child(button);
    // `execute-details-source`: a plain `overflow: auto` box's bar (`message-part.css:1150-1153`).
    let boxed = crate::scroller::scroller("source").build::<S>(p, col().w_full().max_h(360.0).padding(0.5).radius(8.0).border(0.5, p.border).clip_rounded(8.0), source);
    col().key(part.to_owned()).w_full().gap(6.0).child(head).child(boxed)
}
