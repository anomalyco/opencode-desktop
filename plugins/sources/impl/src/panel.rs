//! A turn's Sources tab (`panel.tsx`, `session-ui/src/sources/sources-panel.tsx`, `sources.css`):
//! its searches, each opening to the results it found, and the pages it opened, in the order
//! they ran; a result or page opens in the browser. Its label is the globe, Sources and the count.

use opencode_plugin::prelude::*;
use serde_json::Value;

use crate::Sources;
use crate::turn::{Source, Step, StepStatus, TurnSources, turn_messages, turn_sources};

/// The project avatar colours, so a site's tile reads like the app's other letter tiles.
const COLORS: [&str; 8] = ["orange", "yellow", "cyan", "green", "red", "pink", "blue", "purple"];

/// The session's messages as the server has them, read again whenever the session changes.
fn sources_of(cx: &mut ViewCx<Sources>, session: &str, from: &str) -> (TurnSources, u64) {
    let version = cx.session(session).map_or(0, |s| s.version);
    let (res, generation) = cx.resource(&format!("/api/session/{session}/message?limit=200&order=asc"), version);
    let body = match res {
        Resource::Ready(b) | Resource::Loading(Some(b)) => b,
        _ => String::new(),
    };
    let v: Value = serde_json::from_str(&body).unwrap_or_default();
    let messages = v["data"].as_array().cloned().unwrap_or_default();
    (turn_sources(&turn_messages(&messages, from)), u64::from(generation) ^ version)
}

pub fn label(_: &Sources, cx: &mut ViewCx<Sources>, props: &Props) -> El {
    let (sources, _) = sources_of(cx, &props.session_id, &props.path);
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let count = sources.sources.len();
    let mut r = row()
        .shrink()
        .min_w(0.0)
        .gap(6.0)
        .center_y()
        .child(icon("globe", 16.0).color(p.muted))
        .child(text("Sources").font(13.0, 440.0).line_height(20.0).color(p.muted).truncate().shrink().min_w(0.0));
    if count > 0 {
        r = r.child(text("\u{b7}").font(13.0, 440.0).line_height(20.0).color(faint)).child(text(count.to_string()).font(13.0, 440.0).line_height(20.0).tabular().color(faint));
    }
    r
}

pub fn render(s: &Sources, cx: &mut ViewCx<Sources>, props: &Props) -> El {
    if !crate::enabled() {
        return cx.nothing();
    }
    let (sources, version) = sources_of(cx, &props.session_id, &props.path);
    let open: Vec<String> = sources.steps.iter().filter_map(|st| match st {
        Step::Search { id, .. } if s.open.contains(id) => Some(id.clone()),
        _ => None,
    }).collect();
    cx.memo(&format!("sources-{}-{}", props.session_id, props.path), (version, &open, cx.dark()), |cx| {
        let p = cx.pal();
        let list = col().key("sources-panel").w_full().gap(2.0).padding(8.0).children(sources.steps.iter().map(|st| match st {
            Step::Search { id, query, status, results, error } => search(cx, id, query, *status, results, error.as_deref(), open.contains(id)),
            Step::Fetch { id, source, path, status, error } => fetch(cx, id, source, path, *status, error.as_deref()),
        }));
        col()
            .key("session-sources-panel")
            .w_full()
            .h_full()
            .fill(p.bg_base)
            .child(col().key("sources-scroll").w_full().grow(1.0).min_h(0.0).scroll_y().child(list))
    })
}

/// A row's three columns (`grid-template-columns: 16px minmax(0, 1fr) auto`, 10 apart, 8 in).
fn step_row(cx: &mut ViewCx<Sources>, key: String, glyph: &'static str, words: El, count: Option<El>, hover: bool) -> El {
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let mut r = row()
        .key(key)
        .role("button")
        .w_full()
        .padding(8.0)
        .gap(10.0)
        .radius(8.0)
        .child(row().w(16.0).h(16.0).center().child(icon(glyph, 14.0).color(faint)))
        .child(words.grow(1.0).min_w(0.0).shrink());
    if hover {
        r = r.hover(p.hover).press(p.pressed).instant_hover();
    }
    r.children(count)
}

fn label_text(cx: &ViewCx<Sources>, words: &'static str) -> El {
    let faint = cx.token("text-text-faint").unwrap_or(cx.pal().faint);
    text(words).font(12.0, 440.0).line_height(16.0).tracking(0.05).color(faint)
}

fn error_text(cx: &ViewCx<Sources>, error: Option<&str>) -> Option<El> {
    let danger = cx.token("state-fg-danger").unwrap_or(cx.pal().danger);
    error.map(|e| text(e.to_owned()).font(12.0, 440.0).line_height(16.0).color(danger).wrap())
}

#[allow(clippy::too_many_arguments)]
fn search(cx: &mut ViewCx<Sources>, id: &str, query: &str, status: StepStatus, results: &[Source], error: Option<&str>, open: bool) -> El {
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let expandable = !results.is_empty();
    let words = col()
        .gap(2.0)
        .child(label_text(cx, if status == StepStatus::Error { "Search failed" } else { "Searched web" }))
        .child(text(query.to_owned()).font(13.0, 440.0).line_height(16.0).tracking(-0.04).mono().color(p.text).no_wrap().truncate_fade())
        .children(error_text(cx, error));
    let count = (status == StepStatus::Completed).then(|| {
        let words = if expandable { results.len().to_string() } else { "No results".to_owned() };
        text(words).font(12.0, 440.0).line_height(16.0).tabular().color(if open && expandable { p.text } else { faint }).h(16.0)
    });
    let mut head = step_row(cx, format!("sources-step-{id}"), "magnifying-glass", words, count, expandable);
    if expandable {
        let toggle = id.to_owned();
        head = head.label(format!("Searched web {query}")).on_press(cx.handler(move |s: &mut Sources, _| {
            if !s.open.remove(&toggle) {
                s.open.insert(toggle.clone());
            }
        }));
    }
    let mut step = col().key(format!("sources-search-{id}")).w_full().child(head);
    if open && expandable {
        step = step.child(col().key(format!("sources-results-{id}")).w_full().clip().pt(2.0).pb(6.0).children(results.iter().map(|r| result(cx, r))));
    }
    step
}

/// One result: its site's tile, then its title over its site (`sources-result`).
fn result(cx: &mut ViewCx<Sources>, r: &Source) -> El {
    let p = cx.pal();
    let faint = cx.token("text-text-faint").unwrap_or(p.faint);
    let url = r.url.clone();
    row()
        .key(format!("sources-result-{}", r.url))
        .role("link")
        .label(r.title.clone().unwrap_or_else(|| r.url.clone()))
        .help(r.url.clone())
        .ml(26.0)
        .py(6.0)
        .px(8.0)
        .gap(10.0)
        .radius(8.0)
        .hover(p.hover)
        .press(p.pressed)
        .instant_hover()
        .on_press(cx.handler(move |_: &mut Sources, cx| cx.open_link(&url)))
        .child(site_icon(cx, &r.site))
        .child(
            col()
                .grow(1.0)
                .shrink()
                .min_w(0.0)
                .child(text(r.title.clone().unwrap_or_else(|| r.url.clone())).font(13.0, 440.0).line_height(16.0).color(p.text).no_wrap().truncate_fade())
                .child(text(r.site.clone()).font(12.0, 440.0).line_height(16.0).tracking(0.05).color(faint).no_wrap().truncate_fade()),
        )
}

fn fetch(cx: &mut ViewCx<Sources>, id: &str, source: &Source, path: &str, status: StepStatus, error: Option<&str>) -> El {
    let p = cx.pal();
    let detail = row()
        .min_w(0.0)
        .child(text(source.site.clone()).font(13.0, 440.0).line_height(16.0).tracking(-0.04).mono().color(p.text).no_wrap())
        .child(text(path.to_owned()).font(13.0, 440.0).line_height(16.0).tracking(-0.04).mono().color(p.muted).no_wrap().truncate_fade().shrink().min_w(0.0));
    let words = col()
        .gap(2.0)
        .child(label_text(cx, if status == StepStatus::Error { "Couldn\u{2019}t open page" } else { "Opened page" }))
        .child(detail)
        .children(error_text(cx, error));
    let url = source.url.clone();
    let row = step_row(cx, format!("sources-step-{id}"), "window-cursor", words, None, true)
        .label(format!("Opened page {}", source.url))
        .help(source.url.clone())
        .on_press(cx.handler(move |_: &mut Sources, cx| cx.open_link(&url)));
    col().key(format!("sources-fetch-{id}")).w_full().child(row)
}

/// A site's letter tile (`SiteIcon` in letters mode): 16 round, on the avatar colour its name
/// hashes to (FNV-1a), the letter at 9/530 in the avatar ink, a hairline inside.
pub fn site_icon(cx: &ViewCx<Sources>, site: &str) -> El {
    let p = cx.pal();
    let hash = site.chars().fold(2_166_136_261u32, |h, c| (h ^ c as u32).wrapping_mul(16_777_619));
    let color = COLORS[(hash % COLORS.len() as u32) as usize];
    let bg = cx.token(&format!("avatar-bg-{color}")).unwrap_or(p.layer2);
    let ink = cx.token("avatar-fg").unwrap_or(p.text);
    let ring = cx.token("alpha-dark-12").unwrap_or(p.border);
    let letter = site.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    row().size(16.0, 16.0).center().radius(8.0).fill(bg).border(0.5, ring).child(text(letter).font(9.0, 530.0).line_height(9.0).color(ink))
}
