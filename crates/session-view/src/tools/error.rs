//! A failed call: Desktop's `ToolErrorCard`. Shut, the tool's name, what it was working on, and
//! the error's gist in red; open, the whole error and a Copy error button.

use rune::{El, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;

use super::{Call, Display, directory, field, file_name, files_count, path};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{CODE, INSTANT, UI, UI_MED, chevron, readable};

pub fn card<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let d = Display::of(s, c, true);
    let raw = opencode_kit::model::tool_error(c.metadata).unwrap_or("");
    let cleaned = raw.trim_start_matches("Error:").trim().to_owned();
    let k = super::key(c.id);
    let id = format!("tool-{}", c.id);
    let mut r = row()
        .key(id.clone())
        .w_full()
        .h(28.0)
        .gap(6.0)
        .center_y()
        .role("button")
        .label(format!("{} failed", name(c)))
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(crate::disclosure::toggle::<S>(k, id))
        .child(icon(&ic::OUTLINE_HEXAGONAL_WARNING, 16.0).key("icon").color(p.danger))
        .child(text(name(c)).key("title").font(UI_MED).color(p.text));
    if let Some(sub) = subtitle(s, c) {
        let font = if matches!(c.name, "shell" | "bash") { CODE.lh(16.0) } else { UI };
        r = r.child(readable(p, text(sub).key("subtitle").font(font).color(p.muted).truncate_fade()).shrink().min_w(0.0));
    }
    if !d.open() {
        r = r.child(text(summary(c.name, &cleaned)).key("summary").font(UI).color(p.danger).truncate_fade().shrink().min_w(0.0));
    }
    if let Some(action) = s.tool_row_action(p, c.id, c.name, c.status, c.input, c.metadata) {
        r = r.child(row().key("action").group_opacity(0.0, 1.0).child(action));
    }
    r = r.child(icon(chevron(d.open()), 14.0).key("chevron").color(p.icon_faint));
    let card = col().key("error-card").w_full().hover_group().child(r);
    if !d.open() {
        return card;
    }
    let copy_key = format!("copy-tool-{}-error", c.id);
    let copied = cleaned.clone();
    let copy = row()
        .key("copy")
        .role("button")
        .label("Copy error")
        .help("Copy error")
        .size(24.0, 24.0)
        .center()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .hover_motion(INSTANT, INSTANT)
        .group_opacity(0.0, 1.0)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.copy(copied.clone());
            crate::copied::mark_copied(s, copy_key.clone(), tx);
        })
        .child(crate::copied::glyph(s, &format!("copy-tool-{}-error", c.id), 14.0, p.icon_muted));
    card.child(
        col()
            .key("error")
            .w_full()
            .pl(24.0)
            .pr(40.0)
            .pb(4.0)
            .child(readable(p, text(cleaned).font(UI.lh(20.0)).color(p.danger).wrap_anywhere()).w_full())
            .child(copy.pin(None, Some(0.0), Some(0.0), None)),
    )
}

/// The tool's name, as its trigger says it.
fn name(c: &Call) -> String {
    let fixed = match c.name {
        "read" => "Read",
        "list" => "List",
        "glob" => "Glob",
        "grep" => "Grep",
        "subagent" | "task" => "Agent",
        "webfetch" => "Webfetch",
        "shell" | "bash" => "Shell",
        "execute" => "Execute",
        "edit" | "multiedit" => "Edit",
        "write" => "Write",
        "skill" => "Skill",
        "patch" | "apply_patch" => "Patch",
        "question" => "Questions",
        "websearch" => {
            return match field(c.metadata, "provider") {
                Some(provider) => format!("{} Web Search", opencode_kit::model::capitalize(provider)),
                None => "Web Search".to_owned(),
            };
        }
        other => return other.to_owned(),
    };
    fixed.to_owned()
}

/// What the call was working on, as its own trigger's subtitle says it (`toolErrorSubtitle`).
fn subtitle<S: TimelineHost>(s: &S, c: &Call) -> Option<String> {
    let text = match c.name {
        "shell" | "bash" => field(c.input, "command").or_else(|| field(c.metadata, "command"))?.to_owned(),
        "read" | "edit" | "multiedit" | "write" => file_name(path(c.input)?).to_owned(),
        "list" | "glob" | "grep" => directory(s, path(c.input).unwrap_or("/")),
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
        "subagent" | "task" => field(c.input, "description")?.to_owned(),
        _ => return None,
    };
    Some(text.lines().next().unwrap_or("").to_owned())
}

/// The error's gist: what comes before its first ": ", capitalised, without the tool's own name
/// in front; "Failed" when there's nothing.
pub fn summary(tool: &str, cleaned: &str) -> String {
    let tail = cleaned.strip_prefix(&format!("{tool} ")).unwrap_or(cleaned);
    let head = tail.split(": ").next().unwrap_or("").trim();
    let head = head.lines().next().unwrap_or("").trim();
    let mut chars = head.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Failed".to_owned(),
    }
}
