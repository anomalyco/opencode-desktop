//! Shell: "Shell" and its command; open, Desktop's console card (`ConsoleOutput`) of the command
//! over its output, which Execute's output row opens too.

use rune::{Color, El, ScrollFade, Tx, col, el, row, text};

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
        (false, true) => Some(opencode_i18n::t!("ui.tool.shell.writingCommand").to_owned()),
        (false, false) => Some(one_line(command)),
    };
    let t = Trigger { title: opencode_i18n::t!("ui.tool.shell").into(), compact: true, subtitle, code: !writing, ..Default::default() };
    // A shell its result doesn't finish shows the output its host read from it (`viewShellRun`).
    let streamed = follow(c, |_| false).and_then(|(id, _)| s.shell_output(id)).filter(|o| !o.is_empty());
    let out = streamed.unwrap_or_else(|| output(c));
    disclosure(trigger(s, p, cx, c, d, t, Press::Toggle(super::key(c.id)), None), d, || {
        body(console(s, p, c.id, (!command.is_empty()).then_some(command), &clean(out)), false, 8.0)
    })
}

/// The shell a shell call's console reads its output from, and whether it reads it again each
/// second, as Desktop's `run-machine.ts` decides: a call the model is still writing, or whose
/// result carries the shell's final output (`completed`, `exited`, `timeout`, `killed`), reads
/// nothing; one running, or whose shell the server still lists as running (`running`), polls;
/// any other reads once. None for a call with no shell.
pub fn follow<'a>(c: &Call<'a>, running: impl Fn(&str) -> bool) -> Option<(&'a str, bool)> {
    let id = field(c.metadata, "shellID")?;
    let saved = c.metadata["status"].as_str().is_some_and(|s| matches!(s, "completed" | "exited" | "timeout" | "killed"));
    if saved || *c.status == opencode_kit::model::ToolStatus::Pending {
        return None;
    }
    Some((id, *c.status == opencode_kit::model::ToolStatus::Running || running(id)))
}

/// Whether shell call `c`'s console shows: Desktop reads a shell's output only while its
/// console is mounted, as its card is open.
pub fn console_open<S: TimelineHost>(s: &S, c: &Call) -> bool {
    crate::disclosure::is_open(s, &super::key(c.id), super::default_open(s, c))
}

/// A console's command scrolls past this, eight lines and its padding.
const COMMAND_MAX_H: f32 = 8.0 * 20.0 + 2.0 * PAD;
/// The inset of a section's text, kept while it scrolls.
const PAD: f32 = 12.0;
const RADIUS: f32 = 6.0;
/// The ring's width: the sections sit inside it, rounded concentrically.
const RING: f32 = 0.5;

/// Desktop's console card: the command over its result, each with a copy button. Each section
/// scrolls on its own past its cap, its text held 12 pt in from every edge and fading where
/// it's cut; the ring stays whole round all four corners, the sections inside it. The card shows
/// as much as Desktop's does: its sections stand at most [`MAX_H`] together (`bash-scroll`'s
/// `max-height`, which takes the command and the output in one region), the output giving way.
pub fn console<S: TimelineHost>(s: &S, p: &Pal, id: &str, command: Option<&str>, out: &str) -> El {
    // A terminal reads left to right in any window; its Copy buttons stay at its right.
    let mut card = col().key("console").dir(rune::Direction::Ltr).w_full().max_h(MAX_H + 2.0 * RING).radius(RADIUS).padding(RING).border(RING, p.ring).fill(p.bg_base).hover_group()
        // Its Copy fades in and out over 0.15 s ease (`message-part.css:796-800`'s `bash-copy`).
        .hover_motion(crate::style::HOVER_FADE, crate::style::HOVER_FADE);
    let inner = RADIUS - RING;
    let result = !out.is_empty();
    if let Some(cmd) = command {
        let words = text(cmd.to_owned()).font(TOOL_CODE).color(p.text).wrap_anywhere();
        let corners = [inner, inner, if result { 0.0 } else { inner }, if result { 0.0 } else { inner }];
        card = card.child(section(s, p, id, Part { name: "command", copy: cmd, fill: p.bg_base, label: opencode_i18n::t!("ui.message.copy"), corners, cap: COMMAND_MAX_H, rule: false }, words));
    }
    if result {
        // A line break that ends the output opens no empty line, as in a `pre`.
        let words = linked::<S>(p, out.strip_suffix('\n').unwrap_or(out), TOOL_CODE, p.muted);
        let top = if command.is_some() { 0.0 } else { inner };
        let corners = [top, top, inner, inner];
        card = card.child(section(s, p, id, Part { name: "result", copy: out, fill: p.layer1, label: opencode_i18n::t!("ui.message.copyOutput"), corners, cap: MAX_H, rule: command.is_some() }, words));
    }
    card
}

/// `content` in a region at most [`MAX_H`] tall that scrolls the rest, with no scroll bar, as Desktop hides it
/// (`message-part.css:746-755`).
pub fn capped(content: El) -> El {
    crate::scroller::scroller("scroll").hidden(col().w_full().max_h(MAX_H), content)
}

/// One section of a console.
struct Part<'a> {
    name: &'static str,
    /// What its copy button copies.
    copy: &'a str,
    fill: Color,
    label: &'static str,
    /// `[top_left, top_right, bottom_right, bottom_left]`, concentric with the ring.
    corners: [f32; 4],
    /// Its height, past which its text scrolls.
    cap: f32,
    /// A hairline over it, as it follows the command.
    rule: bool,
}

/// One section of the console: its text on its fill, 12 pt in from each edge, in a viewport
/// capped at its height that scrolls the rest and feathers its text out into the padding where
/// it's cut, and its copy button 4 pt in from the top right, still while the text scrolls. A
/// backdrop of the section's fill sits behind the button, fading out to its left, top and
/// bottom, so text under it softens rather than being cut.
fn section<S: TimelineHost>(s: &S, p: &Pal, id: &str, part: Part, words: El) -> El {
    let copy_key = format!("copy-tool-{id}-{}", part.name);
    let (copied, fill, label) = (part.copy.to_owned(), part.fill, part.label);
    let done = crate::copied::is_copied(s, &copy_key);
    let button = row()
        .key("copy")
        .role("button")
        .label(if done { opencode_i18n::t!("ui.message.copied") } else { label })
        .help(if done { opencode_i18n::t!("ui.message.copied") } else { label })
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
    let holder = crate::style::hover_reveal(el().key("copy-holder").size(24.0, 24.0).pin(None, Some(4.0), Some(4.0), None)).child(backdrop).child(button);
    let inset = col().key("text").w_full().padding(PAD).child(readable(p, words).w_full());
    // `bash-scroll`: no bar, as Desktop hides it (`message-part.css:815-824`).
    let viewport = crate::scroller::scroller("scroll").hidden(col().w_full().max_h(part.cap).shrink().min_h(0.0).scroll_fade(ScrollFade::feather(PAD + 4.0)), inset);
    let mut sec = col().key(part.name).w_full().fill(fill).corners(part.corners).clip_corners(part.corners).child(viewport).child(holder);
    // The output gives way to the command within the card's cap.
    if part.name == "result" {
        sec = sec.shrink().min_h(0.0);
    }
    // `border-block-start: 0.5px`, drawn after the copy button's backdrop, which stops below it.
    if part.rule {
        sec = sec.child(el().key("rule").absolute(0.0, 0.0).w_full().h(RING).fill(p.ring));
    }
    sec
}
