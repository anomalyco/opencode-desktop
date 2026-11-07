//! The Context tab (`tab.tsx`): the session's figures in a two-column grid (one column under
//! 32rem), its system prompt, and its raw messages, each opening to its JSON, with Export
//! session. The messages are the server's own (`GET /api/session/:id/message`), read again
//! whenever the session changes.

use opencode_plugin::prelude::*;
use serde_json::Value;

use crate::format::{date_time, number, usd};
use crate::json;
use crate::Context;

/// An export in flight: the session's info and the pages of messages read so far.
pub struct Export {
    session: String,
    info: String,
    messages: Vec<String>,
}

/// One raw message: its text as the server wrote it and what the tab reads off it.
struct Raw<'a> {
    text: &'a str,
    value: Value,
}

impl Raw<'_> {
    fn kind(&self) -> &str {
        self.value["type"].as_str().unwrap_or_default()
    }
    fn id(&self) -> &str {
        self.value["id"].as_str().unwrap_or_default()
    }
    fn created(&self) -> i64 {
        self.value["time"]["created"].as_i64().unwrap_or(0)
    }
}

fn messages_path(session: &str) -> String {
    format!("/api/session/{session}/message?limit=200&order=asc")
}

pub fn panel(s: &Context, cx: &mut ViewCx<Context>, props: &Props) -> El {
    let sid = props.session_id.clone();
    let session = cx.session(&sid);
    let (res, generation) = cx.resource(&messages_path(&sid), session.as_ref().map_or(0, |s| s.version));
    let body = match res {
        Resource::Ready(b) | Resource::Loading(Some(b)) => b,
        _ => String::new(),
    };
    let wide = props.width >= 512.0;
    let exporting = s.export.as_ref().is_some_and(|e| e.session == sid);
    let version = (session.as_ref().map(|s| s.version), generation, s.open.clone(), exporting, wide, cx.dark(), props.width.round() as i64);
    cx.memo(&format!("panel-{sid}"), version, |cx| {
        let raws: Vec<Raw> = json::member(&body, "data")
            .map(json::items)
            .unwrap_or_default()
            .into_iter()
            .map(|text| Raw { text, value: serde_json::from_str(text).unwrap_or(Value::Null) })
            .collect();
        let content = col()
            .key("session-usage-content")
            .w_full()
            .pl(24.0)
            .pr(32.0)
            .pt(16.0)
            .pb(40.0)
            .gap(40.0)
            .child(stats(cx, session.as_ref(), &sid, &raws, wide))
            .children(system_prompt(cx, &raws))
            .child(raw_messages(s, cx, &sid, &raws, exporting));
        col().key("context-tab").w_full().h_full().pt(8.0).child(col().key("context-scroll").w_full().grow(1.0).min_h(0.0).scroll_y().child(content))
    })
}

/// The last step with tokens, after the last finished compaction (`lastAssistantWithUsage`).
fn last_with_usage<'a>(raws: &'a [Raw]) -> Option<&'a Raw<'a>> {
    let after = raws.iter().rposition(|m| m.kind() == "compaction" && m.value["status"] == "completed").map_or(0, |i| i + 1);
    raws[after..].iter().rev().find(|m| m.kind() == "assistant" && m.value.get("tokens").is_some_and(|t| !t.is_null()))
}

fn stats(cx: &mut ViewCx<Context>, session: Option<&Session>, sid: &str, raws: &[Raw], wide: bool) -> El {
    let p = cx.pal();
    let weak = cx.token("text-weak").unwrap_or(p.faint);
    let strong = cx.token("text-strong").unwrap_or(p.text);
    let last = last_with_usage(raws);
    // The step's tokens and model: off the server's messages, or, before they're read (a run
    // without a server), as the app holds the session.
    let step = match last {
        Some(m) => {
            let t = &m.value["tokens"];
            let n = |v: &Value| v.as_u64().unwrap_or(0);
            let tokens = opencode_plugin::abi::TokensAbi { input: n(&t["input"]), output: n(&t["output"]), reasoning: n(&t["reasoning"]), cache_read: n(&t["cache"]["read"]), cache_write: n(&t["cache"]["write"]) };
            let s = |v: &Value| v.as_str().unwrap_or_default().to_owned();
            Some((tokens, s(&m.value["model"]["providerID"]), s(&m.value["model"]["id"])))
        }
        None if raws.is_empty() => session.and_then(|s| Some((s.usage?, s.usage_provider_id.clone(), s.usage_model_id.clone()))),
        None => None,
    };
    let tokens = |k: &str| {
        step.as_ref().map(|(t, _, _)| match k {
            "input" => t.input,
            "output" => t.output,
            "reasoning" => t.reasoning,
            "cache.read" => t.cache_read,
            _ => t.cache_write,
        })
    };
    let model = step.as_ref().map(|(_, provider, id)| (provider.clone(), id.clone()));
    let catalog = model.as_ref().map(|(provider, id)| cx.model(provider, id));
    let total = step.as_ref().map(|(t, _, _)| t.input + t.output + t.reasoning + t.cache_read + t.cache_write);
    let limit = catalog.as_ref().and_then(|c| c.context_limit);
    let usage = total.zip(limit).map(|(t, l)| (t as f64 / l as f64 * 100.0).round() as u64);
    let dash = || "-".to_owned();
    let num = |v: Option<u64>| v.map_or_else(dash, number);
    let count = |k: &str| raws.iter().filter(|m| m.kind() == k).count() as u64;
    let offset = |ms: i64| cx.utc_offset(ms);
    let time = |ms: Option<i64>| ms.filter(|&ms| ms > 0).map_or_else(dash, |ms| date_time(ms, offset(ms)));
    let provider = match (&model, &catalog) {
        (Some((id, _)), Some(c)) => c.provider_name.clone().unwrap_or_else(|| id.clone()),
        _ => dash(),
    };
    let model_name = match (&model, &catalog) {
        (Some((_, id)), Some(c)) => c.name.clone().unwrap_or_else(|| id.clone()),
        _ => dash(),
    };
    let rows = [
        ("Session", session.map_or_else(|| if sid.is_empty() { dash() } else { sid.to_owned() }, |s| s.title.clone())),
        ("Messages", number(if raws.is_empty() { session.map_or(0, |s| u64::from(s.messages)) } else { raws.len() as u64 })),
        ("Provider", provider),
        ("Model", model_name),
        ("Context Limit", num(limit)),
        ("Total Tokens", num(total)),
        ("Usage", usage.map_or_else(dash, |u| format!("{}%", number(u)))),
        ("Input Tokens", num(tokens("input"))),
        ("Output Tokens", num(tokens("output"))),
        ("Reasoning Tokens", num(tokens("reasoning"))),
        ("Cache Tokens (read/write)", format!("{} / {}", num(tokens("cache.read")), num(tokens("cache.write")))),
        ("User Messages", number(count("user"))),
        ("Assistant Messages", number(count("assistant"))),
        ("Total Cost", usd(session.map_or(0.0, |s| s.cost))),
        ("Session Created", time(session.map(|s| s.created_ms))),
        ("Last Activity", time(last.map(Raw::created))),
    ];
    let stat = |(label, value): (&'static str, String)| {
        col()
            .gap(4.0)
            .w(0.0)
            .grow(1.0)
            .min_w(0.0)
            .child(text(label).font(13.0, 440.0).line_height(19.0).color(weak))
            // Two 19.5 lines on whole pixels: a row is 59 tall, as Desktop's.
            .child(text(value).font(13.0, 530.0).line_height(20.0).color(strong).selectable())
    };
    let per_row = if wide { 2 } else { 1 };
    let mut grid = col().key("context-stats").w_full().gap(16.0);
    for pair in rows.chunks(per_row) {
        grid = grid.child(row().w_full().gap(16.0).children(pair.iter().cloned().map(stat)));
    }
    grid
}

fn system_prompt(cx: &mut ViewCx<Context>, raws: &[Raw]) -> Option<El> {
    let prompt = raws.iter().rev().find(|m| m.kind() == "system")?.value["text"].as_str()?.trim().to_owned();
    if prompt.is_empty() {
        return None;
    }
    let p = cx.pal();
    let weak = cx.token("text-weak").unwrap_or(p.faint);
    let surface = cx.token("surface-base").unwrap_or(p.layer1);
    Some(
        col()
            .key("context-system")
            .w_full()
            .gap(8.0)
            .child(text("System Prompt").font(13.0, 440.0).line_height(19.5).color(weak))
            .child(
                col()
                    .w_full()
                    .px(12.0)
                    .py(8.0)
                    .radius(6.0)
                    .border(1.0, p.border)
                    .fill(surface)
                    .child(text(prompt).font(13.0, 440.0).line_height(19.5).wrap().selectable().color(p.text)),
            ),
    )
}

fn raw_messages(s: &Context, cx: &mut ViewCx<Context>, sid: &str, raws: &[Raw], exporting: bool) -> El {
    let p = cx.pal();
    let weak = cx.token("text-weak").unwrap_or(p.faint);
    let base = cx.token("text-base").unwrap_or(p.muted);
    let background = cx.token("background-base").unwrap_or(p.bg_base);
    let session = sid.to_owned();
    let export = row()
        .key("context-export")
        .role("button")
        .label("Export session")
        .h(24.0)
        .px(8.0)
        .gap(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .press(p.pressed)
        .instant_hover()
        .hover_group()
        .opacity(if exporting || sid.is_empty() { 0.5 } else { 1.0 })
        .child(icon("download", 14.0).color(weak).group_color(p.text))
        .child(text("Export session").font(13.0, 530.0).line_height(20.0).tracking(-0.04).tabular().color(weak).group_color(p.text));
    let export = if exporting || sid.is_empty() { export } else { export.on_press(cx.handler(move |s: &mut Context, cx| start_export(s, cx, &session))) };
    let head = row().w_full().center_y().justify_between().child(text("Raw messages").font(13.0, 440.0).line_height(19.5).color(weak)).child(export);
    let last = raws.len().saturating_sub(1);
    let mut list = col().key("context-raw").w_full();
    for (i, m) in raws.iter().enumerate() {
        let id = m.id().to_owned();
        let open = s.open.iter().any(|o| *o == id);
        let lg = 8.0;
        let top = if i == 0 { lg } else { 0.0 };
        let bottom = if i == last && !open { lg } else { 0.0 };
        let offset = cx.utc_offset(m.created());
        let toggle = id.clone();
        let trigger = row()
            .key(format!("raw-{id}"))
            .role("button")
            .label(format!("{} {id}", m.kind()))
            .w_full()
            .h(32.0)
            .px(12.0)
            .gap(8.0)
            .center_y()
            .corners([top, top, bottom, bottom])
            .border(1.0, p.border)
            .fill(p.bg_base)
            .hover(p.hover)
            .instant_hover()
            .child(
                row()
                    .shrink()
                    .min_w(0.0)
                    .center_y()
                    .child(text(format!("{} ", m.kind())).font(13.0, 440.0).line_height(19.5).color(p.text).no_wrap())
                    .child(text(format!("\u{2022} {id}")).font(13.0, 440.0).line_height(19.5).color(base).truncate().shrink()),
            )
            .child(spacer())
            .child(
                row()
                    .gap(12.0)
                    .center_y()
                    .child(text(date_time(m.created(), offset)).font(13.0, 440.0).line_height(19.5).color(weak).no_wrap())
                    .child(icon("chevron-grabber-vertical", 14.0).color(weak)),
            )
            .on_press(cx.handler(move |s: &mut Context, _| match s.open.iter().position(|o| *o == toggle) {
                Some(at) => {
                    s.open.remove(at);
                }
                None => s.open.push(toggle.clone()),
            }));
        let trigger = if i > 0 { trigger.mt(-1.0) } else { trigger };
        list = list.child(trigger);
        if open {
            let content = col()
                .key(format!("raw-body-{id}"))
                .w_full()
                .padding(12.0)
                .corners([0.0, 0.0, bottom.max(if i == last { lg } else { 0.0 }), if i == last { lg } else { 0.0 }])
                .border(1.0, p.border)
                .mt(-1.0)
                .fill(background)
                .child(text(json::pretty(m.text)).font(12.0, 440.0).line_height(18.0).mono().wrap().selectable().color(p.text));
            list = list.child(content);
        }
    }
    col().key("context-raw-section").w_full().gap(8.0).child(head).child(list)
}

fn start_export(s: &mut Context, cx: &mut ActionCx, session: &str) {
    s.export = Some(Export { session: session.to_owned(), info: String::new(), messages: Vec::new() });
    let sid = session.to_owned();
    cx.server_request("GET", &format!("/api/session/{session}"), "", move |s: &mut Context, cx| {
        if failed(s, cx) {
            return;
        }
        let body = cx.input().to_owned();
        if let Some(e) = s.export.as_mut() {
            e.info = json::member(&body, "data").unwrap_or(&body).to_owned();
        }
        next_page(cx, &sid, None);
    });
}

/// Reads the next page of 200 messages, oldest first, then the next, then saves.
fn next_page(cx: &mut ActionCx, session: &str, cursor: Option<&str>) {
    let mut path = messages_path(session);
    if let Some(c) = cursor {
        path.push_str(&format!("&cursor={}", encode(c)));
    }
    let sid = session.to_owned();
    cx.server_request("GET", &path, "", move |s: &mut Context, cx| {
        if failed(s, cx) {
            return;
        }
        let body = cx.input().to_owned();
        let Some(e) = s.export.as_mut() else { return };
        e.messages.extend(json::member(&body, "data").map(json::items).unwrap_or_default().into_iter().map(str::to_owned));
        let next = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["cursor"]["next"].as_str().map(str::to_owned));
        match next {
            Some(c) => next_page(cx, &sid, Some(&c)),
            None => save(s, cx),
        }
    });
}

fn save(s: &mut Context, cx: &mut ActionCx) {
    let Some(e) = s.export.as_ref() else { return };
    let raw = format!("{{\"info\":{},\"messages\":[{}]}}", if e.info.is_empty() { "{}" } else { &e.info }, e.messages.join(","));
    let info: Value = serde_json::from_str(&e.info).unwrap_or(Value::Null);
    let name = filename(&info, &e.session);
    let done = name.clone();
    cx.save_file(&name, &json::pretty(&raw), move |s: &mut Context, cx| {
        s.export = None;
        let error = cx.input_extra().to_owned();
        match error.as_str() {
            "" => cx.toast_with("Session exported", &format!("Saved session to {done}")),
            error => cx.toast_error("Failed to export session", error),
        }
    });
}

/// Ends the export with Desktop's failure toast when the request failed.
fn failed(s: &mut Context, cx: &mut ActionCx) -> bool {
    if cx.input_extra().is_empty() {
        return false;
    }
    s.export = None;
    let error = cx.input_extra().to_owned();
    cx.toast_error("Failed to export session", if error.is_empty() { "An error occurred while exporting the session" } else { &error });
    true
}

/// `sessionExportFilename`: the title (or slug, or id) lowercased, runs of anything but letters,
/// digits, `_` and `-` as one `-`, at most 250, trimmed of `-`, plus `.json`.
fn filename(info: &Value, id: &str) -> String {
    let name = ["title", "slug"].iter().filter_map(|k| info[*k].as_str()).find(|s| !s.is_empty()).unwrap_or(id);
    let mut clean = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            clean.push(c);
        } else if !clean.ends_with('-') {
            clean.push('-');
        }
    }
    let clean: String = clean.chars().take(250).collect();
    let clean = clean.trim_matches('-');
    format!("{}.json", if clean.is_empty() { id } else { clean })
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}
