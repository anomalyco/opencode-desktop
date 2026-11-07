//! The Extensions card's rows and the flyout beside it (`server-panel.tsx`): the location's MCP
//! servers with their status dots and switches, its plugins and its skills, each read from the
//! server when the popover opens, with the footer's Configuration file, Manage MCP servers and
//! setup rows.

use opencode_plugin::abi::{ALIGN_START, SHADOW_FLOATING, SIDE_LEFT};
use opencode_plugin::prelude::*;
use serde_json::Value;

use crate::{Details, POPOVER};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Service {
    Mcp,
    Plugins,
    Skills,
}

impl Service {
    pub const ALL: [Service; 3] = [Service::Mcp, Service::Plugins, Service::Skills];

    fn label(self) -> &'static str {
        match self {
            Service::Mcp => "MCP",
            Service::Plugins => "Plugins",
            Service::Skills => "Skills",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Service::Mcp => "mcp",
            Service::Plugins => "cube",
            Service::Skills => "graduation-cap",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Service::Mcp => "mcp",
            Service::Plugins => "plugins",
            Service::Skills => "skills",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Service::Mcp => "/api/mcp",
            Service::Plugins => "/api/plugin",
            Service::Skills => "/api/skill",
        }
    }

    fn empty(self) -> &'static str {
        match self {
            Service::Mcp => "No MCP servers configured",
            Service::Plugins => "No plugins configured",
            Service::Skills => "No skills configured",
        }
    }

    fn setup(self) -> (&'static str, &'static str) {
        match self {
            Service::Mcp => (
                "Add a server with OpenCode",
                "Help me add an MCP server to my global OpenCode config. Ask me which service I want to connect, then set it up and check that it connects.",
            ),
            Service::Plugins => (
                "Add a plugin with OpenCode",
                "Help me add a plugin to my global OpenCode config. Ask me what I want it to do, suggest a plugin, then add it.",
            ),
            Service::Skills => (
                "New skill with OpenCode",
                "Help me write a new OpenCode skill in my global config. Ask me what it should teach you, then write its SKILL.md.",
            ),
        }
    }
}

/// One row of a list: its name, status, error and (a skill's) file.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    name: String,
    status: Option<String>,
    error: Option<String>,
    path: Option<String>,
}

/// A list's load: loading, failed, or its items (sorted by name).
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    Loading,
    Failed,
    Ready(Vec<Item>),
}

/// The three lists, as read for this open.
pub struct Lists {
    loads: [Load; 3],
    generation: u64,
}

impl Lists {
    pub fn read(cx: &ViewCx<Details>, directory: &str, opens: u64) -> Lists {
        let mut generation = 0u64;
        let loads = Service::ALL.map(|service| {
            let (res, g) = cx.resource(&format!("{}{}", service.path(), location(directory)), opens);
            generation = generation.wrapping_mul(31).wrapping_add(u64::from(g));
            match res {
                Resource::Ready(body) => Load::Ready(parse(service, &body)),
                Resource::Failed(_) => Load::Failed,
                Resource::Loading(_) => Load::Loading,
            }
        });
        Lists { loads, generation }
    }

    fn get(&self, service: Service) -> &Load {
        &self.loads[service as usize]
    }
}

/// `?location[directory]=…`, or nothing for the server's default location.
fn location(directory: &str) -> String {
    if directory.is_empty() {
        return String::new();
    }
    format!("?location%5Bdirectory%5D={}", encode(directory))
}

fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn list(v: &Value) -> Vec<Value> {
    v["data"].as_array().or_else(|| v.as_array()).cloned().unwrap_or_default()
}

fn parse(service: Service, body: &str) -> Vec<Item> {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let s = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let mut items: Vec<Item> = list(&v)
        .iter()
        .filter_map(|m| match service {
            Service::Mcp => Some(Item { name: s(&m["name"])?, status: s(&m["status"]["status"]), error: s(&m["status"]["error"]), path: None }),
            Service::Plugins => {
                let source = &m["source"];
                if source["type"] == "builtin" {
                    return None;
                }
                let name = s(&m["id"]).or_else(|| s(&source["target"])).or_else(|| s(&source["path"])).or_else(|| s(&source["type"]))?;
                let status = s(&m["state"]["status"]);
                let error = if status.as_deref() == Some("failed") { s(&m["state"]["error"]) } else { None };
                Some(Item { name, status, error, path: None })
            }
            Service::Skills => Some(Item { name: s(&m["name"])?, status: None, error: None, path: s(&m["path"]) }),
        })
        .collect();
    items.sort_by_key(|i| i.name.to_lowercase());
    items
}

/// A card row (`session-summary-row session-service-trigger`): its icon, label, a failed dot,
/// the count once read, and the chevron; hovering it opens its list beside the card.
pub fn trigger(s: &Details, cx: &mut ViewCx<Details>, lists: &Lists, service: Service) -> El {
    let p = cx.pal();
    let load = lists.get(service);
    let expanded = s.flyout == Some(service);
    let critical = cx.token("icon-critical-base").unwrap_or(p.danger);
    let (failed, count) = match load {
        Load::Ready(items) => (items.iter().any(|i| i.status.as_deref() == Some("failed")), items.len()),
        _ => (false, 0),
    };
    let mut r = row()
        .key(format!("details-{}", service.key()))
        .role("button")
        .label(service.label())
        .w_full()
        .min_h(28.0)
        .pl(12.0)
        .pr(8.0)
        .py(4.0)
        .gap(8.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .instant_hover()
        .child(icon(service.icon(), 16.0).color(p.icon_muted))
        .child(text(service.label()).font(13.0, 440.0).line_height(16.0).color(p.text).truncate().grow(1.0).shrink().min_w(0.0));
    if failed {
        r = r.child(dot(critical, false));
    }
    if count > 0 {
        r = r.child(text(count.to_string()).font(12.0, 440.0).line_height(16.0).tabular().color(p.faint));
    }
    r = r.child(icon("chevron-right", 16.0).color(p.icon_muted).ml(4.0));
    if expanded {
        r = r.fill(p.hover);
    }
    r.on_hover(cx.handler(move |s: &mut Details, cx| {
        if cx.event().x > 0.5 && s.flyout != Some(service) {
            s.flyout = Some(service);
        } else {
            cx.unchanged();
        }
    }))
    .on_press(cx.handler(move |s: &mut Details, _| s.flyout = if s.flyout == Some(service) { None } else { Some(service) }))
}

/// A 16 box with a 6 dot: filled, or a hollow ring for a disabled server.
fn dot(color: Rgba, hollow: bool) -> El {
    let d = el().size(6.0, 6.0).radius(3.0);
    let d = if hollow { d.border(1.0, color) } else { d.fill(color) };
    el().size(16.0, 16.0).center().child(d)
}

/// The list beside the card: `service`'s rows, then its footer.
pub fn flyout(s: &Details, cx: &mut ViewCx<Details>, lists: &Lists, service: Service, directory: &str) -> El {
    let p = cx.pal();
    let version = (service, s.opens, lists.generation, s.toggling.clone(), s.acting, cx.dark(), cx.server_local());
    let load = lists.get(service).clone();
    let directory = directory.to_owned();
    cx.memo(&format!("flyout-{}", service.key()), version, |cx| {
        let body = match &load {
            Load::Loading => vec![message(&p, "Loading…")],
            Load::Failed => vec![
                message(&p, "Request failed"),
                config_row(&p, "details-retry", "", "Retry").on_press(cx.handler(|_: &mut Details, cx| cx.invalidate("/api/"))),
            ],
            Load::Ready(items) if items.is_empty() => {
                vec![col().w_full().child(text(service.empty()).font(13.0, 440.0).line_height(16.0).color(p.faint).px(12.0).py(8.0))]
            }
            Load::Ready(items) => items.iter().enumerate().map(|(i, item)| item_row(s, cx, service, item, &directory).mt(if i > 0 { 2.0 } else { 0.0 })).collect(),
        };
        let footer = matches!(load, Load::Ready(_)).then(|| footer(cx, service, matches!(&load, Load::Ready(items) if items.is_empty()), &directory));
        let pane = col()
            .key(format!("details-pane-{}", service.key()))
            .role("group")
            .label(service.label())
            .w_full()
            .max_h(480.0)
            .child(col().w_full().px(2.0).shrink().min_h(0.0).scroll_y().children(body))
            .children(footer);
        col()
            .key("details-flyout")
            .w(280.0)
            .py(2.0)
            .radius(6.0)
            .fill(p.layer1)
            .shadow(SHADOW_FLOATING)
            .mt(-2.0)
            .anchored(format!("details-{}", service.key()), SIDE_LEFT, ALIGN_START, 2.0)
            .child(pane)
    })
}

fn message(p: &Palette, words: &'static str) -> El {
    text(words).font(13.0, 440.0).line_height(16.0).color(p.faint).px(12.0).py(6.0)
}

fn item_row(s: &Details, cx: &mut ViewCx<Details>, service: Service, item: &Item, directory: &str) -> El {
    let p = cx.pal();
    let status = item.status.as_deref();
    let color = |token: &str, or: Rgba| cx.token(token).unwrap_or(or);
    let ink = match status {
        Some("connected" | "active") => color("icon-success-base", p.success),
        Some("failed") => color("icon-critical-base", p.danger),
        Some("needs_auth") => color("icon-warning-base", p.warning),
        _ => p.icon_faint,
    };
    let name = |muted: bool| text(item.name.clone()).font(13.0, 440.0).line_height(16.0).color(if muted { p.muted } else { p.text }).truncate().shrink().min_w(0.0);
    let base = row().key(format!("details-item-{}", item.name)).w_full().min_h(28.0).pl(12.0).py(4.0).gap(8.0).center_y().radius(4.0);
    match service {
        Service::Mcp => {
            let on = status != Some("disabled");
            let busy = s.toggling.iter().any(|n| *n == item.name);
            let (name_owned, dir) = (item.name.clone(), directory.to_owned());
            let failure = item.error.clone().filter(|_| status == Some("failed"));
            // Desktop's `follow`: a press asks for the switch's other side, or signs in.
            let enable = status == Some("needs_auth") || !on;
            let switch = ui::switch(on).opacity(if busy { 0.5 } else { 1.0 });
            let mut dot_el = el().size(6.0, 6.0).radius(3.0);
            dot_el = if status == Some("disabled") { dot_el.border(1.0, ink) } else { dot_el.fill(ink) };
            if let Some(error) = &failure {
                dot_el = dot_el.help(error.clone());
            }
            let row = base
                .pr(6.0)
                .hover(p.hover)
                .instant_hover()
                .child(row().gap(8.0).center_y().grow(1.0).shrink().min_w(0.0).child(dot_el).child(name(status == Some("disabled"))))
                .child(row().ml(8.0).gap(8.0).center_y().child(switch));
            if busy {
                return row;
            }
            row.on_press(cx.handler(move |s: &mut Details, cx| {
                if failure.is_some() {
                    cx.close_popover();
                    cx.run_command("mcp.toggle");
                    return;
                }
                toggle_mcp(s, cx, &name_owned, &dir, enable);
            }))
        }
        Service::Plugins => {
            let mut row = base.pr(12.0);
            if let Some(st) = status {
                let mut d = dot(ink, st == "disabled");
                if let Some(error) = &item.error {
                    d = d.help(error.clone());
                }
                row = row.child(d);
            }
            row.child(name(status == Some("disabled")))
        }
        Service::Skills => {
            let path = item.path.clone().unwrap_or_default();
            base.pr(12.0)
                .role("button")
                .hover(p.hover)
                .instant_hover()
                .child(icon("file-document", 16.0).color(p.icon_muted))
                .child(name(false))
                .on_press(cx.handler(move |_: &mut Details, cx| {
                    cx.close_popover();
                    cx.open_link(&path);
                }))
        }
    }
}

fn toggle_mcp(s: &mut Details, cx: &mut ActionCx, name: &str, directory: &str, enable: bool) {
    if !cx.can_write() {
        cx.toast_error("Request failed", "This window is read-only. Relaunch without --read-only to act.");
        return;
    }
    s.toggling.push(name.to_owned());
    let verb = if enable { "connect" } else { "disconnect" };
    let owned = name.to_owned();
    let path = format!("/api/experimental/mcp/{}/{verb}{}", encode(name), location(directory));
    cx.server_request("POST", &path, "", move |s: &mut Details, cx| {
        s.toggling.retain(|n| *n != owned);
        let error = cx.input_extra().to_owned();
        if !error.is_empty() {
            cx.toast_error("Request failed", &error);
        }
        cx.invalidate("/api/mcp");
    });
}

/// `session-service-config`: an icon and a label, as a menu row.
fn config_row(p: &Palette, key: &str, glyph: &'static str, label: &str) -> El {
    let mut r = row().key(key.to_owned()).role("button").label(label.to_owned()).w_full().min_h(28.0).px(12.0).py(4.0).gap(8.0).center_y().radius(4.0).hover(p.hover).instant_hover();
    if !glyph.is_empty() {
        r = r.child(icon(glyph, 16.0).color(p.icon_muted));
    }
    r.child(text(label.to_owned()).font(13.0, 440.0).line_height(16.0).color(p.text).truncate().shrink().min_w(0.0))
}

/// Under the list: a rule, then (when empty, or for skills) the setup row, the configuration
/// file, and MCP's Manage MCP servers.
fn footer(cx: &mut ViewCx<Details>, service: Service, empty: bool, directory: &str) -> El {
    let p = cx.pal();
    let local = cx.server_local();
    let rule = el().w_full().h(1.0).fill(p.border_muted).py(0.0);
    let mut f = col().key(format!("details-footer-{}", service.key())).w_full().pt(6.0).px(2.0).child(col().w_full().mx(-2.0).my(2.0).child(rule));
    if empty || service == Service::Skills {
        let (label, prompt) = service.setup();
        let dir = directory.to_owned();
        f = f.child(config_row(&p, &format!("details-setup-{}", service.key()), "plus", label).on_press(cx.handler(move |s: &mut Details, cx| config_action(s, cx, &dir, service, Some(prompt), local))));
    }
    let (glyph, label) = if local { ("settings-gear", "Configuration file") } else { ("outline-copy", "Copy configuration file path") };
    let dir = directory.to_owned();
    f = f.child(config_row(&p, &format!("details-config-{}", service.key()), glyph, label).on_press(cx.handler(move |s: &mut Details, cx| config_action(s, cx, &dir, service, None, local))));
    if service == Service::Mcp && !empty {
        f = f.child(config_row(&p, "details-manage-mcp", "mcp", "Manage MCP servers").on_press(cx.handler(|_: &mut Details, cx| {
            cx.close_popover();
            cx.run_command("mcp.toggle");
        })));
    }
    f
}

/// Reads the location's configuration, then opens (or copies) the file holding `service`, or with
/// `prompt` starts a draft in the global configuration's folder that sets it up. A file on this
/// machine opens in a side tab; a remote server's path is copied.
fn config_action(s: &mut Details, cx: &mut ActionCx, directory: &str, service: Service, prompt: Option<&'static str>, local: bool) {
    if s.acting {
        return;
    }
    s.acting = true;
    let dir = directory.to_owned();
    cx.server_request("GET", &format!("/api/config{}", location(directory)), "", move |s: &mut Details, cx| {
        s.acting = false;
        let error = cx.input_extra().to_owned();
        if !error.is_empty() {
            cx.toast_error("Request failed", &error);
            return;
        }
        let v: Value = serde_json::from_str(cx.input()).unwrap_or(Value::Null);
        let entries = list(&v);
        if let Some(prompt) = prompt {
            cx.close_popover();
            cx.new_draft(&global_directory(&entries, &dir), prompt);
            return;
        }
        let Some(path) = config_file(&entries, service) else {
            cx.toast_error("Request failed", "No configuration file found");
            return;
        };
        if local {
            if cx.popover_open(POPOVER) {
                cx.close_popover();
            }
            cx.open_link(&path);
        } else {
            cx.copy_text(&path);
            cx.toast_with("Copied", &path);
        }
    });
}

/// `serviceConfigFile`: the last document listing any of the service's entries, else the last
/// setting it, else the first.
fn config_file(entries: &[Value], service: Service) -> Option<String> {
    let docs: Vec<&Value> = entries
        .iter()
        .filter(|e| e["type"] == "document" && e["path"].as_str().is_some_and(|p| p.ends_with(".json") || p.ends_with(".jsonc")))
        .collect();
    let key = match service {
        Service::Mcp => "mcp",
        Service::Plugins => "plugins",
        Service::Skills => "skills",
    };
    let lists = |d: &&Value| match service {
        Service::Mcp => d["info"]["mcp"]["servers"].as_object().is_some_and(|o| !o.is_empty()),
        _ => d["info"][key].as_array().is_some_and(|a| !a.is_empty()),
    };
    let doc = docs.iter().rev().find(|d| lists(d)).or_else(|| docs.iter().rev().find(|d| !d["info"][key].is_null())).or_else(|| docs.first());
    doc.and_then(|d| d["path"].as_str()).map(str::to_owned)
}

/// `globalConfigDirectory`: the first config root unless it's a project's `.opencode`, else the
/// location itself.
fn global_directory(entries: &[Value], directory: &str) -> String {
    let root = entries.iter().find(|e| e["type"] == "directory").and_then(|e| e["path"].as_str());
    match root {
        Some(r) if !r.trim_end_matches(['/', '\\']).ends_with(".opencode") => r.trim_end_matches(['/', '\\']).to_owned(),
        _ => directory.to_owned(),
    }
}
