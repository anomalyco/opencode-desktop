//! Shell: "Shell" and its command; open, Desktop's console card (`ConsoleOutput`) of the command
//! over its output, which Execute's output row opens too.

use rune::{Color, El, ScrollSpec, Tx, col, el, row, text};

use super::plain::{clean, linked, one_line};
use super::{Call, Ctx, Display, Phase, Press, Trigger, body, disclosure, field, output, trigger};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, TOOL_CODE, readable};

/// Desktop caps a console, a call's input and an error at 240 px and scrolls the rest.
pub const MAX_H: f32 = 240.0;

/// A shell call: "Shell" and its command on one line; open, the console. It opens while it
/// runs, its output streaming in.
pub fn shell<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let d = Display::of(s, cx, c, true);
    let command = field(c.input, "command").or_else(|| field(c.metadata, "command")).unwrap_or("");
    // While open the command shows in the console, so the trigger drops it.
    let writing = command.is_empty() && d.phase == Phase::Pending;
    let subtitle = match (d.open(), writing) {
        (true, _) => None,
        (false, true) => Some("Writing command…".to_owned()),
        (false, false) => Some(one_line(command)),
    };
    let t = Trigger { title: "Shell".into(), compact: true, subtitle, code: !writing, ..Default::default() };
    disclosure(trigger(s, p, cx, c, d, t, Press::Toggle(super::key(c.id)), None), d, || {
        body(console(s, p, c.id, (!command.is_empty()).then_some(command), &clean(output(c))), false, 8.0)
    })
}

/// Desktop's console card: the command over its result, each with a copy button that stays in
/// view while its section scrolls, in one region that scrolls past [`MAX_H`].
pub fn console<S: TimelineHost>(s: &S, p: &Pal, id: &str, command: Option<&str>, out: &str) -> El {
    let mut inner = col().key("console-text").w_full();
    if let Some(cmd) = command {
        inner = inner.child(section(s, p, id, "command", text(cmd.to_owned()).font(TOOL_CODE).color(p.text).wrap_anywhere(), cmd, p.bg_base, "Copy", false));
    }
    if !out.is_empty() {
        // A line break that ends the output opens no empty line, as in a `pre`.
        let words = linked::<S>(p, out.strip_suffix('\n').unwrap_or(out), TOOL_CODE, p.muted);
        inner = inner.child(section(s, p, id, "result", words, out, p.layer1, "Copy output", command.is_some()));
    }
    col()
        .key("console")
        .w_full()
        .radius(6.0)
        .padding(0.5)
        .border(0.5, p.border)
        .fill(p.bg_base)
        .clip_rounded(6.0)
        .hover_group()
        .child(capped(inner))
}

/// `content` in a region at most [`MAX_H`] tall that scrolls the rest, with no scroll bar, as Desktop hides it.
pub fn capped(content: El) -> El {
    col().key("scroll").w_full().max_h(MAX_H).scroll(ScrollSpec { stick_bottom: false }).child(content)
}

/// One section of the console: its text on `fill`, a hairline over it when it follows the
/// command, and its copy button 4 pt in from the top right, held there while the section
/// scrolls (`position: sticky; top: 4px`). A backdrop of the section's fill sits behind the
/// button, fading out to its left, top and bottom, so text under it softens rather than being cut.
#[allow(clippy::too_many_arguments)]
fn section<S: TimelineHost>(s: &S, p: &Pal, id: &str, part: &str, words: El, copy: &str, fill: Color, label: &'static str, rule: bool) -> El {
    let copy_key = format!("copy-tool-{id}-{part}");
    let copied = copy.to_owned();
    let done = crate::copied::is_copied(s, &copy_key);
    let button = row()
        .key("copy")
        .role("button")
        .label(if done { "Copied" } else { label })
        .help(if done { "Copied" } else { label })
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
    let backdrop = el().key("backdrop").absolute(-24.0, -4.0).size(52.0, 36.0).clip_fade_linear(24.0, 8.0, 0.0, 8.0).child(el().w_full().h_full().fill(fill));
    let holder = el()
        .key("copy-holder")
        .size(24.0, 24.0)
        .pin(None, Some(4.0), Some(4.0), None)
        .sticky_top(4.0)
        .group_opacity(0.0, 1.0)
        .child(backdrop)
        .child(button);
    let mut sec = col().key(part.to_owned()).w_full().fill(fill).padding(12.0).child(readable(p, words).w_full()).child(holder);
    if rule {
        sec = sec.child(el().key("rule").absolute(0.0, 0.0).w_full().h(0.5).fill(p.border));
    }
    sec
}
