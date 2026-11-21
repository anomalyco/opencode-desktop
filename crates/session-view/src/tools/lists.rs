//! Read, List, Glob, Grep, Webfetch, Web Search, Skill, and any other tool: their triggers and,
//! where they have one, their results.

use rune::{El, Span, Tx, col, el, icon, row, text};
use serde_json::Value;

use super::{Call, Ctx, Display, Press, Trigger, body, busy, directory, disclosure, field, file_name, output, path, still, sub_key, trigger};
use crate::TimelineHost;
use crate::focus::FocusExt;
use crate::palette::Pal;
use crate::style::{HOVER_FADE, INSTANT, UI, arrow, readable};

/// Web Search lists this many links, then "+N more".
const LINKS: usize = 10;

/// The width of session `session`'s timeline column, as laid out.
pub fn column(session: &str) -> f32 {
    rune::memo::layout_with(&format!("scroll-{session}"), |r| (r[2] - crate::style::sides()).clamp(0.0, crate::style::content_w())).unwrap_or(crate::style::content_w())
}

/// A read that doesn't share a line with others (an image, a failed one, or one that loaded
/// instructions): "Read", the file's name and its `offset=`/`limit=`; "Loaded …" under it for
/// the instructions it loaded. An image it returned shows under the group.
pub fn read<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let d = Display::of(s, cx, c, false);
    let t = Trigger { title: opencode_i18n::t!("ui.tool.read").into(), subtitle: path(c.input).map(|x| file_name(x).to_owned()), args: read_args(c.input), ..Default::default() };
    let mut out = still(trigger(s, p, cx, c, d, t, Press::None, None));
    if *c.status == opencode_kit::model::ToolStatus::Completed {
        let loaded: Vec<&str> = c.metadata["loaded"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|x| super::session_relative(s, cx.session, x)).collect();
        if !loaded.is_empty() {
            out = out.child(loaded_item(s, p, opencode_i18n::t!("ui.tool.loaded"), &[(loaded.join(", "), false)], None));
        }
    }
    out
}

/// Plain reads in a row of the group: "Read a.ts offset=11 limit=20, b.ts" on one line, which a
/// press wraps whole when it doesn't fit (`CurrentReadToolGroup`).
pub fn read_group<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, calls: &[Call]) -> El {
    let Some(first) = calls.first() else { return col() };
    let mut list = String::new();
    let mut args: Vec<Span> = Vec::new();
    for (n, c) in calls.iter().enumerate() {
        let Some(name) = path(c.input).map(file_name).filter(|n| !n.is_empty()) else { continue };
        if !list.is_empty() {
            list.push(' ');
        }
        list.push_str(name);
        for a in read_args(c.input) {
            // Args carry a leading space; their 2 pt margin completes the 6 pt gap.
            let at = list.len();
            list.push_str("  ");
            list.push_str(&a);
            args.push(Span::new(at, list.len()).color(p.muted));
        }
        if n + 1 < calls.len() {
            list.push(',');
        }
    }
    let pending = calls.iter().any(|c| busy(c.status, cx.live));
    let title_w = rune::text::measure(opencode_i18n::t!("ui.tool.read"), &[], UI, None).0;
    // Wider than the row shows it in one line: Desktop measures the clip, this measures the words.
    let long = rune::text::measure(&list, &[], UI, None).0 > column(cx.session) - 24.0 - 24.0 - title_w - 8.0;
    let k = super::key(first.id);
    let open = long && s.expanded().contains(&k);
    let title = if pending { crate::working::shimmer(s, p, opencode_i18n::t!("ui.tool.read"), UI, 0.0).key("title") } else { text(opencode_i18n::t!("ui.tool.read")).key("title").font(UI).color(p.muted) };
    let words = text(list).key("subtitle").font(if open { UI.lh(20.0) } else { UI }).color(p.text).spans(args);
    let words = if open { words.wrap() } else { words.truncate_fade() };
    let mut r = row()
        .key(format!("tool-{}", first.id))
        .w_full()
        .min_h(28.0)
        .gap(8.0)
        .hover_group_focus()
        .hover_motion(HOVER_FADE, HOVER_FADE)
        .child(row().h(if open { 20.0 } else { 28.0 }).center_y().child(title))
        .child(row().shrink().min_w(0.0).center_y().child(readable(p, words).shrink().min_w(0.0)));
    r = if open { r.items_start().py(4.0) } else { r.center_y() };
    if long {
        // The group's arrow turns to the line's start right to left (`message-part.css:1864`).
        let a = el().key("arrow").w(16.0).h(20.0).center().child(arrow(p, open).mirror_in_rtl());
        r = r
            .child(if open || crate::style::hover_none() { a } else { a.group_opacity(0.0, 1.0) })
            .role("button")
            .label_static(opencode_i18n::t!("ui.tool.read"))
            .expanded(open)
            .radius(4.0)
            .cursor(rune::Cursor::Pointer)
            .focus_default(p)
            .on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{}", first.id)));
    }
    r
}

fn read_args(input: &Value) -> Vec<String> {
    ["offset", "limit"].iter().filter_map(|k| input[*k].as_i64().map(|n| format!("{k}={n}"))).collect()
}

/// List, Glob and Grep: the tool, the directory it looked in, its pattern and how many it
/// found. Desktop's grouped row is a trigger alone; this one opens to what the search returned,
/// as Desktop's separate-mode renderer does, an intentional Rune design.
pub fn search<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    // A search shows what was searched and how many matches it found, never the raw list it
    // returned, so it has nothing to open (`timeline-next/tools/explore.tsx`, `SearchItem`).
    let d = Display::of(s, cx, c, false);
    let (title, count) = match c.name {
        "list" => (opencode_i18n::t!("ui.tool.list"), None),
        "glob" => (opencode_i18n::t!("ui.tool.glob"), c.metadata["count"].as_u64()),
        _ => (opencode_i18n::t!("ui.tool.grep"), c.metadata["matches"].as_u64()),
    };
    let mut args = Vec::new();
    if c.name != "list" {
        args.extend(field(c.input, "pattern").map(|x| format!("pattern={x}")));
    }
    if c.name == "grep" {
        args.extend(field(c.input, "include").map(|x| format!("include={x}")));
    }
    let tag = count.filter(|n| *n > 0).map(|n| (opencode_i18n::plural!("ui.messagePart.context.matchCount", n), false));
    let searched = path(c.input).or_else(|| s.session_directory(cx.session)).unwrap_or("/");
    let t = Trigger { title: title.into(), subtitle: directory(s, cx.session, searched), args, tag, ..Default::default() };
    still(trigger(s, p, cx, c, d, t, Press::None, None))
}

/// A web fetch: "Webfetch" and its URL, a link that opens in the browser. It has nothing to open.
pub fn webfetch<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let d = Display::of(s, cx, c, false);
    let url = field(c.input, "url").filter(|_| d.phase != super::Phase::Pending);
    let t = Trigger { title: opencode_i18n::t!("ui.tool.fetch").into(), link: url.map(|u| webfetch_link::<S>(p, u).key("subtitle")), ..Default::default() };
    still(trigger(s, p, cx, c, d, t, Press::None, None))
}

/// `.webfetch-link` in New timeline (`timeline-next.css:195-210`): `text-base` ink with a faint
/// 40% underline that brightens to full ink on hover, and no external arrow icon.
fn webfetch_link<S: TimelineHost>(p: &Pal, url: &str) -> El {
    let to = url.to_owned();
    el().key("words")
        .shrink()
        .min_w(0.0)
        .radius(4.0)
        .role("link")
        .label(url.to_owned())
        .help(url.to_owned())
        .cursor(rune::Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.open_link(&to, tx);
        })
        .child(text(url.to_owned()).font(UI).color(p.text).truncate_fade())
        .child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(1.0)).h(1.0).fill(p.text).group_opacity(0.4, 1.0))
}

/// A search result URL that opens in the browser (`ExaOutput`): accent text, underlined under
/// the pointer, with the way-out icon then too.
fn link<S: TimelineHost>(p: &Pal, url: &str) -> El {
    let to = url.to_owned();
    let words = el()
        .key("words")
        .shrink()
        .min_w(0.0)
        .child(text(url.to_owned()).font(UI).color(p.accent).truncate_fade())
        .child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(1.0)).h(1.0).fill(p.accent).group_opacity(0.0, 1.0));
    row()
        .gap(8.0)
        .center_y()
        .shrink()
        .min_w(0.0)
        .radius(4.0)
        .role("link")
        .label(url.to_owned())
        .help(url.to_owned())
        .cursor(rune::Cursor::Pointer)
        .focus_default(p)
        .hover_group_focus()
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.open_link(&to, tx);
        })
        .child(words)
        .child(icon(&rune_theme_opencode::icons::OUTLINE_SQUARE_ARROW, 16.0).key("out").color(p.accent).group_opacity(0.0, 1.0))
}

/// A web search: "{Provider} Web Search" and its query; open, the links it found. It opens once
/// it has finished.
pub fn websearch<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    let out = output(c);
    // Very large search reports use the same virtual result reader as a shell, with links
    // recognised on mounted lines only. Never scan or allocate their URLs in a shut row.
    let large = out.len() > 8192;
    let opens = !busy(c.status, cx.live) && (large || !super::plain::urls(out).is_empty());
    let d = Display::of(s, cx, c, opens);
    // Named by what it did rather than the provider (`WebSearchTool`).
    let t = Trigger { title: opencode_i18n::t!("ui.tool.searchWeb").into(), subtitle: field(c.input, "query").map(str::to_owned), ..Default::default() };
    let press = if opens { Press::Toggle(super::key(c.id)) } else { Press::None };
    disclosure(s, trigger(s, p, cx, c, d, t, press, None), d, || {
        if large {
            return body(super::console::capped(super::console::output_view::<S>(p,cx.session,c.id,out,true)),true,4.0);
        }
        let mut links: Vec<&str> = Vec::new();
        for r in super::plain::urls(out) {
            if !links.contains(&&out[r.clone()]) {
                links.push(&out[r]);
            }
        }
        let more_key = sub_key(c.id, "more");
        let all = s.expanded().contains(&more_key);
        let shown = if all || links.len() <= LINKS { links.len() } else { LINKS };
        let mut list = col().key("links").w_full().gap(8.0);
        for (n, u) in links.iter().take(shown).enumerate() {
            list = list.child(row().key(format!("link-{n}")).w_full().child(link::<S>(p, u)));
        }
        if shown < links.len() {
            let more = opencode_i18n::plural!("ui.common.moreCount", links.len() - shown);
            let words = el()
                .key("more")
                .role("button")
                .label(more.clone())
                .radius(4.0)
                .cursor(rune::Cursor::Pointer)
                .focus_default(p)
                .hover_group_focus()
                .on_press_tx(crate::disclosure::toggle::<S>(more_key, format!("tool-{}", c.id)))
                .child(text(more.clone()).font(UI).color(p.faint).group_color(p.muted))
                .child(el().key("underline").pin(Some(0.0), None, Some(0.0), Some(1.0)).h(1.0).fill(p.muted).group_opacity(0.0, 1.0));
            list = list.child(row().key("more-row").w_full().child(words));
        }
        body(list, true, 4.0)
    })
}

/// Skills in a row of the group: "Loaded release-notes skill", or "Loaded a, b skills" for named
/// skills one after another, each name shimmering while it loads; "Skill" until its name is known.
pub fn skill_group<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, calls: &[Call]) -> El {
    let Some(first) = calls.first() else { return col() };
    let names: Vec<(String, bool)> = calls.iter().filter_map(|c| skill_name(c).map(|n| (n.to_owned(), busy(c.status, cx.live)))).collect();
    if names.is_empty() {
        let d = Display::of(s, cx, first, false);
        let title = if d.busy() { crate::working::shimmer(s, p, opencode_i18n::t!("ui.tool.skill"), UI, 0.0) } else { text(opencode_i18n::t!("ui.tool.skill")).font(UI).color(p.muted) };
        return row().key(format!("tool-{}", first.id)).w_full().h(28.0).center_y().child(title.key("title"));
    }
    // `ui.tool.loadedSkills` around its name: "Loaded" before it and "skill" or "skills" after.
    let template = opencode_i18n::plural_text(opencode_i18n::plural_key!("ui.tool.loadedSkills"), names.len() as u64);
    let (before, after) = template.split_once("{{name}}").unwrap_or((opencode_i18n::t!("ui.tool.loaded"), ""));
    let after = after.replace("{{count}}", &names.len().to_string());
    loaded_item(s, p, before.trim(), &names, Some(after.trim()).filter(|a| !a.is_empty())).key(format!("tool-{}", first.id))
}

/// A skill call's name: its metadata's, else what the model asked for.
pub fn skill_name<'a>(c: &Call<'a>) -> Option<&'a str> {
    field(c.metadata, "name").or_else(|| field(c.input, "id")).or_else(|| field(c.input, "name"))
}

/// "Loaded {values} {kind}": Desktop's `tool-loaded-item`, a value that is still loading
/// shimmering.
fn loaded_item<S: TimelineHost>(s: &S, p: &Pal, label: &str, values: &[(String, bool)], kind: Option<&str>) -> El {
    let value = if values.iter().any(|v| v.1) {
        let mut r = row().key("value").shrink().min_w(0.0).center_y();
        for (i, (v, loading)) in values.iter().enumerate() {
            r = r.child(if *loading { crate::working::shimmer(s, p, v, UI, 0.0) } else { text(v.clone()).font(UI).color(p.text) });
            if i + 1 < values.len() {
                r = r.child(text(", ").font(UI).color(p.text));
            }
        }
        r
    } else {
        let joined = values.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>().join(", ");
        readable(p, text(joined).key("value").font(UI).color(p.text).truncate_fade()).shrink().min_w(0.0)
    };
    row()
        .key("loaded")
        .w_full()
        .min_h(28.0)
        .gap(6.0)
        .center_y()
        .child(text(label.to_owned()).font(UI).color(p.muted))
        .child(value)
        .children(kind.map(|k| text(k.to_owned()).font(UI).color(p.muted).ml(-2.0)))
}

/// Any other tool, an MCP server's: "Called `tool`", the first of its description, query, URL,
/// path, pattern or name, and up to three more of its plain inputs.
pub fn generic<S: TimelineHost>(s: &S, p: &Pal, cx: Ctx, c: &Call) -> El {
    const LABELS: [&str; 6] = ["description", "query", "url", "path", "pattern", "name"];
    let d = Display::of(s, cx, c, false);
    let subtitle = LABELS.iter().find_map(|k| field(c.input, k)).map(str::to_owned);
    let args = c.input.as_object().into_iter().flatten()
        .filter(|(k, _)| !LABELS.contains(&k.as_str()))
        .filter_map(|(k, v)| match v {
            Value::String(x) => Some(format!("{k}={x}")),
            Value::Number(n) => Some(format!("{k}={n}")),
            Value::Bool(b) => Some(format!("{k}={b}")),
            _ => None,
        })
        .take(3)
        .collect();
    let t = Trigger { title: opencode_i18n::t!("ui.basicTool.called", tool = c.name).into(), subtitle, args, ..Default::default() };
    still(trigger(s, p, cx, c, d, t, Press::None, None))
}
