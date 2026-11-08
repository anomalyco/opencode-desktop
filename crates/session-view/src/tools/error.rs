//! A failed call: Desktop's `ToolErrorCard`. Shut, a 24 pt trigger of the warning mark, the
//! tool's name, what it was working on, the error's gist in red and an arrow; open, the whole
//! error and a Copy error button, with a subtitle the trigger had to cut moved above it.

use rune::{El, Tx, col, el, icon, row, text};

use super::{Call, directory, field, file_name, files_count, path};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, TOOL_CODE, UI, UI_MED, arrow, readable};

/// The card of failed call `c` in session `session`.
pub fn card<S: TimelineHost>(s: &S, p: &Pal, session: &str, c: &Call) -> El {
    let k = super::key(c.id);
    let open = crate::disclosure::is_open(s, &k, super::default_open(s, c));
    let raw = opencode_kit::model::tool_error(c.metadata).unwrap_or("");
    let cleaned = raw.trim_start().strip_prefix("Error:").unwrap_or(raw).trim().to_owned();
    let id = format!("tool-{}", c.id);
    let title = name(c);
    let shell = matches!(c.name, "shell" | "bash");
    let sub_font = if shell { TOOL_CODE.lh(16.0) } else { UI };
    let sub = subtitle(s, session, c);
    let gist = summary(c.name, &cleaned);
    // Measured as the shut row lays it out: a subtitle it can't show whole moves into the body.
    let clipped = sub.as_deref().is_some_and(|t| {
        let room = super::lists::column(session) - 24.0 - 16.0 - 8.0 - rune::text::measure(&title, &[], UI_MED, None).0 - 12.0 - 20.0;
        t.contains('\n') || rune::text::measure(t, &[], sub_font, None).0 + rune::text::measure(&gist, &[], UI, None).0 > room
    });
    let mut words = row().key("words").gap(6.0).center_y().shrink().min_w(0.0).child(text(title.clone()).key("title").font(UI_MED).color(p.text));
    if let Some(t) = sub.as_deref().filter(|_| !(open && clipped)) {
        let first = t.lines().next().unwrap_or("").to_owned();
        words = words.child(readable(p, text(first).key("subtitle").font(sub_font).color(p.muted).truncate_fade()).shrink().min_w(0.0));
    }
    if !open {
        words = words.child(text(gist).key("summary").font(UI).color(p.danger).truncate_fade().shrink().min_w(0.0));
    }
    let button = row()
        .key(id.clone())
        .h(24.0)
        .gap(8.0)
        .center_y()
        .grow(1.0)
        .shrink()
        .min_w(0.0)
        .role("button")
        .label(opencode_i18n::t!("rune.tool.failed", tool = title))
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(crate::disclosure::set::<S>(k, open, id))
        .child(el().key("icon").size(16.0, 16.0).center().child(icon(&rune_theme_opencode::icons::OUTLINE_HEXAGONAL_WARNING, 16.0).color(p.danger)))
        .child(words)
        .child(el().key("arrow").size(14.0, 14.0).ml(6.0).center().child(arrow(p, open).color(p.faint)));
    let mut line = row().key("line").w_full().min_h(28.0).gap(2.0).center_y().child(button);
    if let Some(action) = s.tool_row_action(p, session, c, None) {
        line = line.child(row().key("actions").child(action));
    }
    let card = col().key("error-card").w_full().hover_group().child(line);
    if !open {
        return card;
    }
    let copy_key = format!("copy-tool-{}-error", c.id);
    let done = crate::copied::is_copied(s, &copy_key);
    let label = if done { opencode_i18n::t!("ui.message.copied") } else { opencode_i18n::t!("ui.toolErrorCard.copyError") };
    let copied = cleaned.clone();
    let copy = row()
        .key("copy")
        .role("button")
        .label(label)
        .help(label)
        .size(24.0, 24.0)
        .center()
        .radius(6.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .group_opacity(0.0, 1.0)
        .on_press_tx({
            let copy_key = copy_key.clone();
            move |s: &mut S, tx: &mut Tx| {
                tx.copy(copied.clone());
                crate::copied::mark_copied(s, copy_key.clone(), tx);
            }
        })
        .child(crate::copied::glyph_of(s, &copy_key, 16.0, p.text, &rune_theme_opencode::icons::COPY));
    let mut content = col().key("error").w_full().pl(24.0).pr(40.0);
    if let Some(t) = sub.filter(|_| clipped) {
        let full = col().key("subtitle-full").w_full().max_h(super::console::MAX_H).mb(4.0).scroll(rune::ScrollSpec { stick_bottom: false });
        content = content.child(full.child(readable(p, text(t).font(if shell { TOOL_CODE } else { UI.lh(20.0) }).color(p.muted).wrap_anywhere()).w_full()));
    }
    content = content.child(readable(p, text(cleaned).font(UI.lh(20.0)).color(p.danger).wrap_anywhere()).w_full()).child(copy.pin(None, Some(0.0), Some(0.0), None));
    card.child(content)
}

/// The tool's name, as its trigger says it.
fn name(c: &Call) -> String {
    match c.name {
        "websearch" => super::lists::web_search_title(c.metadata),
        "multiedit" => crate::turns::tool_title("edit").to_owned(),
        other => crate::turns::tool_title(other).to_owned(),
    }
}

/// What the call was working on, as its own trigger's subtitle says it (`toolErrorSubtitle`),
/// whole: the trigger shows its first line.
fn subtitle<S: TimelineHost>(s: &S, session: &str, c: &Call) -> Option<String> {
    let text = match c.name {
        "shell" | "bash" => field(c.input, "command").or_else(|| field(c.metadata, "command"))?.to_owned(),
        "read" | "edit" | "multiedit" | "write" => file_name(path(c.input)?).to_owned(),
        "list" | "glob" | "grep" => directory(s, session, path(c.input).unwrap_or(s.session_directory(session).unwrap_or("/")))?,
        "webfetch" => field(c.input, "url")?.to_owned(),
        "websearch" => field(c.input, "query")?.to_owned(),
        "skill" => field(c.metadata, "name").or_else(|| field(c.input, "id")).or_else(|| field(c.input, "name"))?.to_owned(),
        "patch" | "apply_patch" => {
            let n = c.metadata["files"].as_array().map_or(0, Vec::len);
            if n == 0 {
                return None;
            }
            files_count(n)
        }
        "question" => match c.input["questions"].as_array().map_or(0, Vec::len) {
            0 => return None,
            n => format!("{n} {}", opencode_i18n::I18N.plural(opencode_i18n::plural_key!("ui.common.question"), n as u64)),
        },
        "subagent" | "task" => field(c.input, "description")?.to_owned(),
        _ => return None,
    };
    Some(text).filter(|t| !t.is_empty())
}

/// The error's gist: what comes before its first ": ", capitalised, without the tool's own name
/// in front; "Failed" when there's nothing.
pub fn summary(tool: &str, cleaned: &str) -> String {
    let tool = if tool == "bash" { "shell" } else { tool };
    let tail = cleaned.strip_prefix(&format!("{tool} ")).unwrap_or(cleaned);
    let head = tail.split(": ").next().unwrap_or("").trim();
    let head = head.lines().next().unwrap_or("").trim();
    let mut chars = head.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => opencode_i18n::t!("ui.toolErrorCard.failed").to_owned(),
    }
}
