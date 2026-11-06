//! Read, List, Glob, Grep, Webfetch, Web Search, Skill, and any other tool: their triggers and,
//! where Desktop shows one, their results.

use rune::{El, Tx, col, icon, row, text};
use rune_theme_opencode::icons as ic;
use serde_json::Value;

use super::{Call, Display, Press, Title, Trigger, body, directory, disclosure, field, file_name, output, path, sub_key, trigger};
use crate::TimelineHost;
use crate::palette::Pal;
use crate::style::{INSTANT, UI, UI_MED, readable};

/// A search's results scroll past 240 px, as a console's do.
const MAX_H: f32 = super::console::MAX_H;

/// Web Search lists this many links, then "+N more".
const LINKS: usize = 10;

/// A read: "Read", the file's name and its `offset=`/`limit=`. Desktop never shows a read's text;
/// an image it loaded shows under the group, and instructions it loaded say "Loaded …".
pub fn read<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let d = Display::of(s, c, false);
    let t = Trigger { title: Title::Fixed("Read"), subtitle: path(c.input).map(|x| file_name(x).to_owned()), args: read_args(c.input), ..Default::default() };
    let mut out = col().w_full().child(trigger(s, p, c, d, t, Press::None));
    if *c.status == opencode_kit::model::ToolStatus::Completed {
        let dir = s.active_session_id().and_then(|id| s.session_directory(id));
        let loaded: Vec<&str> = c.metadata["loaded"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|x| super::relative(x, dir)).collect();
        if !loaded.is_empty() {
            out = out.child(loaded_item(p, "Loaded", &loaded.join(", "), None));
        }
    }
    out
}

/// Plain reads in a row of the group: "Read a.ts offset=11 limit=20, b.ts" on one line, which a
/// press wraps whole when it doesn't fit (`CurrentReadToolGroup`).
pub fn read_group<S: TimelineHost>(s: &S, p: &Pal, calls: &[Call]) -> El {
    let Some(first) = calls.first() else { return col() };
    let files: Vec<String> = calls
        .iter()
        .filter_map(|c| {
            let name = file_name(path(c.input)?);
            let args = read_args(c.input);
            Some(if args.is_empty() { name.to_owned() } else { format!("{name} {}", args.join(" ")) })
        })
        .collect();
    let list = files.join(", ");
    let busy = calls.iter().any(|c| matches!(c.status, opencode_kit::model::ToolStatus::Pending | opencode_kit::model::ToolStatus::Running));
    // Wider than the column shows it in one line: Desktop measures the clip, this counts it.
    let long = rune::text::measure(&list, &[], UI, None).0 > crate::style::CONTENT_W - 80.0;
    let k = super::key(first.id);
    let open = long && s.expanded().contains(&k);
    let title = if busy { crate::working::shimmer(s, p, "Read", UI_MED, 0.0).key("title") } else { text("Read").key("title").font(UI_MED).color(p.text) };
    let words = text(list).key("subtitle").font(UI).color(p.muted);
    let words = if open { words.wrap() } else { words.truncate_fade() };
    let mut r = row()
        .key(format!("tool-{}", first.id))
        .w_full()
        .min_h(28.0)
        .gap(6.0)
        .hover_group()
        .hover_motion(INSTANT, INSTANT)
        .child(row().h(28.0).center_y().child(title))
        .child(row().min_h(28.0).center_y().shrink().min_w(0.0).child(readable(p, words).shrink().min_w(0.0)));
    if open {
        r = r.items_start().py(4.0);
    } else {
        r = r.center_y();
    }
    if long {
        let arrow = icon(crate::style::chevron(open), 14.0).key("chevron").color(p.icon_faint);
        r = r
            .child(row().h(28.0).center_y().child(if open { arrow } else { arrow.group_opacity(0.0, 1.0) }))
            .role("button")
            .label("Read")
            .on_press_tx(crate::disclosure::toggle::<S>(k, format!("tool-{}", first.id)));
    }
    r
}

fn read_args(input: &Value) -> Vec<String> {
    ["offset", "limit"].iter().filter_map(|k| input[*k].as_i64().map(|n| format!("{k}={n}"))).collect()
}

/// List, Glob and Grep: the tool, the directory it looked in, its pattern and how many it
/// found; open, what it returned.
pub fn search<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let out = output(c).trim_end();
    let d = Display::of(s, c, !out.is_empty() && !matches!(c.status, opencode_kit::model::ToolStatus::Pending | opencode_kit::model::ToolStatus::Running));
    let (title, mut args, count) = match c.name {
        "list" => (Title::Fixed("List"), Vec::new(), None),
        "glob" => (Title::Fixed("Glob"), Vec::new(), c.metadata["count"].as_u64()),
        _ => (Title::Fixed("Grep"), Vec::new(), c.metadata["matches"].as_u64()),
    };
    if c.name != "list" {
        args.extend(field(c.input, "pattern").map(|x| format!("pattern={x}")));
    }
    if c.name == "grep" {
        args.extend(field(c.input, "include").map(|x| format!("include={x}")));
    }
    let matches = count.filter(|n| *n > 0).map(|n| format!("({n} {})", if n == 1 { "match" } else { "matches" }));
    let t = Trigger { title, subtitle: Some(directory(s, path(c.input).unwrap_or("/"))), args, matches, ..Default::default() };
    let press = if out.is_empty() { Press::None } else { Press::Toggle(super::key(c.id)) };
    disclosure(trigger(s, p, c, d, t, press), d, || {
        let words = readable(p, text(out.to_owned()).font(UI.lh(20.0)).color(p.text).wrap_anywhere()).w_full();
        body(col().key("output").w_full().max_h(MAX_H).scroll(rune::ScrollSpec { stick_bottom: false }).child(words), true, 4.0)
    })
}

/// A web fetch: "Webfetch" and its URL, which opens in the browser. It has nothing to open.
pub fn webfetch<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let d = Display::of(s, c, false);
    let url = field(c.input, "url").filter(|_| *c.status != opencode_kit::model::ToolStatus::Pending);
    let t = Trigger { title: Title::Fixed("Webfetch"), link: url.map(|u| link::<S>(p, u).key("subtitle")), ..Default::default() };
    trigger(s, p, c, d, t, Press::None)
}

/// A URL that opens in the browser, with an arrow out.
fn link<S: TimelineHost>(p: &Pal, url: &str) -> El {
    let to = url.to_owned();
    row()
        .gap(4.0)
        .center_y()
        .shrink()
        .min_w(0.0)
        .role("link")
        .label(url.to_owned())
        .help(url.to_owned())
        .hover_motion(INSTANT, INSTANT)
        .on_press_tx(move |s: &mut S, tx: &mut Tx| {
            tx.instant();
            s.open_link(&to, tx);
        })
        .child(text(url.to_owned()).font(UI).color(p.muted).truncate_fade().shrink().min_w(0.0))
        .child(icon(&ic::ARROW_UP, 12.0).color(p.icon_faint).rotate(std::f32::consts::FRAC_PI_4))
}

/// A web search: "{Provider} Web Search" and its query; open, the links it found.
pub fn websearch<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let links = urls(output(c));
    let d = Display::of(s, c, !links.is_empty());
    let title = match field(c.metadata, "provider") {
        Some(provider) => Title::Named(format!("{} Web Search", provider_name(provider))),
        None => Title::Fixed("Web Search"),
    };
    let t = Trigger { title, subtitle: field(c.input, "query").map(str::to_owned), ..Default::default() };
    let press = if links.is_empty() { Press::None } else { Press::Toggle(super::key(c.id)) };
    disclosure(trigger(s, p, c, d, t, press), d, || {
        let more_key = sub_key(c.id, "more");
        let all = s.expanded().contains(&more_key);
        let shown = if all || links.len() <= LINKS { links.len() } else { LINKS };
        let mut list = col().key("links").w_full().gap(8.0);
        for (n, u) in links.iter().take(shown).enumerate() {
            list = list.child(row().key(format!("link-{n}")).child(link::<S>(p, u)));
        }
        if shown < links.len() {
            list = list.child(
                text(format!("+{} more", links.len() - shown))
                    .key("more")
                    .font(UI)
                    .color(p.faint)
                    .role("button")
                    .on_press_tx(crate::disclosure::toggle::<S>(more_key, format!("tool-{}", c.id))),
            );
        }
        body(list, true, 4.0)
    })
}

/// "OpenCode", "TinyFish", or the provider's id capitalised.
fn provider_name(id: &str) -> String {
    match id {
        "opencode" => "OpenCode".to_owned(),
        "tinyfish" => "TinyFish".to_owned(),
        _ => opencode_kit::model::capitalize(id),
    }
}

/// The distinct `http(s)` URLs in a search's result, in order, without trailing punctuation.
fn urls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        let tail = &rest[at..];
        if !(tail.starts_with("http://") || tail.starts_with("https://")) {
            rest = &tail[4..];
            continue;
        }
        let end = tail.find(|c: char| c.is_whitespace() || "<>\"'`)]".contains(c)).unwrap_or(tail.len());
        let url = tail[..end].trim_end_matches(|c: char| "),.;:!?".contains(c));
        if !out.iter().any(|u| u == url) {
            out.push(url.to_owned());
        }
        rest = &tail[end..];
    }
    out
}

/// A skill: "Loaded {name} skill", or "Skill" until its name is known.
pub fn skill<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    let name = field(c.metadata, "name").or_else(|| field(c.input, "id")).or_else(|| field(c.input, "name"));
    match name {
        Some(n) => row().key(format!("tool-{}", c.id)).w_full().h(28.0).center_y().child(loaded_item(p, "Loaded", n, Some("skill"))),
        None => {
            let d = Display::of(s, c, false);
            trigger(s, p, c, d, Trigger { title: Title::Fixed("Skill"), ..Default::default() }, Press::None)
        }
    }
}

/// "Loaded {value} {kind}": Desktop's `tool-loaded-item`.
fn loaded_item(p: &Pal, label: &str, value: &str, kind: Option<&str>) -> El {
    row()
        .key("loaded")
        .w_full()
        .min_h(28.0)
        .gap(6.0)
        .center_y()
        .child(text(label.to_owned()).font(UI_MED).color(p.text))
        .child(readable(p, text(value.to_owned()).font(UI).color(p.text).truncate_fade()).shrink().min_w(0.0))
        .children(kind.map(|k| text(k.to_owned()).font(UI).color(p.muted).ml(-2.0)))
}

/// Any other tool, an MCP server's: "Called `tool`", the first of its description, query, URL,
/// path, pattern or name, and up to three more of its plain inputs.
pub fn generic<S: TimelineHost>(s: &S, p: &Pal, c: &Call) -> El {
    const LABELS: [&str; 6] = ["description", "query", "url", "path", "pattern", "name"];
    let d = Display::of(s, c, false);
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
    let t = Trigger { title: Title::Named(format!("Called `{}`", c.name)), subtitle, args, ..Default::default() };
    trigger(s, p, c, d, t, Press::None)
}
