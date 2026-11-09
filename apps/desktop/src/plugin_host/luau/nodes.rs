//! The contract's `ui.*` nodes beyond rune-luau's own (`row`, `col`, `box`, `text`, `spacer`,
//! `virtualList`): the app's controls and content, built by the same code the app draws its own
//! with, so a plugin's button or settings row is the app's to the pixel. Native views later
//! packages bring (terminal, browser, code, diff, tree, UI outlines) draw a note naming them.

use rune::{El, TextStyle, Tx, el, row, text};
use rune_kit::{ButtonVariant, Loader};
use rune_luau::{Data, HostNode, NodeArgs};

use super::cx;
use crate::App;
use crate::palette::{Pal, kit, pal};

pub fn all() -> Vec<HostNode> {
    vec![
        HostNode::new("divider", divider),
        HostNode::new("icon", icon),
        HostNode::new("image", |_| Ok(el())),
        HostNode::new("metric", metric),
        HostNode::new("shimmer", shimmer),
        HostNode::new("keybind", keybind),
        HostNode::new("button", button),
        HostNode::new("iconButton", icon_button),
        HostNode::new("switch", switch),
        HostNode::new("radio", radio),
        HostNode::new("field", field),
        HostNode::new("loader", |a| Ok(Loader::new().size(a.number("size").unwrap_or(16.0) as f32).build(kit(&p(a))))),
        HostNode::new("serverIndicator", server_indicator),
        HostNode::new("pill", pill),
        HostNode::new("settingsRow", settings_row),
        HostNode::new("settingsList", settings_list),
        HostNode::new("flyoutRow", flyout_row),
        HostNode::new("markdown", markdown),
        HostNode::new("progressCircle", progress_circle),
        pending("terminalSurface", "LU-6"),
        pending("browserSurface", "LU-7"),
        pending("codeView", "LU-6"),
        pending("diffView", "LU-6"),
        pending("fileTree", "LU-6"),
        pending("panelSidebar", "LU-6"),
        pending("openInButton", "LU-6"),
        HostNode::new("genUi", gen_ui),
    ]
}

/// `ui.genUi`: a ```` ```ui ```` outline as parts (`rune_parts`). In a reply's code block it is
/// the timeline's own view of that block, with Find's marks, selection and the reply's width;
/// anywhere else it parses `source` itself at the content width.
fn gen_ui(a: &mut NodeArgs) -> Result<El, String> {
    if let Some(native) = super::surfaces::take_native() {
        return Ok(native);
    }
    let width = a.number("w").map_or_else(session_view::style::content_w, |w| w as f32);
    Ok(session_view::genui::standalone::<App>(&p(a), a.string("source").unwrap_or_default(), width, a.flag("streaming")).unwrap_or_else(el))
}

/// Icon `name`: the rendering plugin's own (`icons/<name>.svg`), else the host's or the app's.
fn named_icon(name: &str) -> &'static rune::Icon {
    crate::plugin_host::icons::resolve(cx::rendering().as_deref().unwrap_or_default(), name)
}

fn p(a: &NodeArgs) -> Pal {
    *pal(a.dark())
}

fn pending(name: &'static str, package: &'static str) -> HostNode {
    HostNode::new(name, move |a| Ok(text(format!("ui.{name} arrives with {package}")).font(crate::view::UI).color(p(a).faint)))
}

/// The text style the props describe, from `base`.
fn style(a: &NodeArgs, base: TextStyle) -> TextStyle {
    let mut s = base;
    if let Some(n) = a.number("size") {
        s.size = n as f32;
    }
    s.weight = match a.string("weight") {
        Some("regular") => 440.0,
        Some("medium") => 530.0,
        Some("semibold") => 600.0,
        Some("bold") => 700.0,
        _ => a.number("weight").map_or(s.weight, |w| w as f32),
    };
    if let Some(n) = a.number("lineHeight") {
        s = s.lh(n as f32);
    }
    if let Some(n) = a.number("tracking") {
        s = s.tracking(n as f32);
    }
    s
}

fn divider(a: &mut NodeArgs) -> Result<El, String> {
    let c = a.color("color").unwrap_or(p(a).border_muted);
    Ok(if a.flag("vertical") { el().w(1.0).h_full().fill(c) } else { el().w_full().h(1.0).fill(c) })
}

fn icon(a: &mut NodeArgs) -> Result<El, String> {
    let name = a.content().unwrap_or_default().to_owned();
    let size = a.number("size").unwrap_or(16.0) as f32;
    let pal = p(a);
    let color = a.color("color").unwrap_or(match a.string("tone") {
        Some("success") => pal.success,
        Some("warning") => pal.warning,
        Some("danger") => pal.danger,
        _ => pal.icon,
    });
    Ok(rune::icon(named_icon(&name), size).color(color))
}

/// A number whose changed digits roll (`session_view::odometer`), or plain text.
fn metric(a: &mut NodeArgs) -> Result<El, String> {
    let value = a.number("value").unwrap_or(0.0);
    let words = a.string("text").unwrap_or_default().to_owned();
    let mut font = style(a, TextStyle::new(13.0, 440.0));
    if a.get("tabular").is_none() || a.flag("tabular") {
        font = font.features("\"tnum\"");
    }
    let color = a.color("color").unwrap_or(p(a).text);
    let animate = a.get("animate").is_none() || a.flag("animate");
    if !animate {
        return Ok(text(words).font(font).color(color));
    }
    let key = a.string("key").unwrap_or("metric").to_owned();
    Ok(session_view::odometer::odometer_end(format!("luau:{key}"), words, value, font, color))
}

fn shimmer(a: &mut NodeArgs) -> Result<El, String> {
    let words = a.content().unwrap_or_default().to_owned();
    let font = style(a, TextStyle::new(13.0, 440.0));
    let pal = p(a);
    let app = cx::app().ok_or("ui.shimmer draws only in a render")?;
    Ok(session_view::working::shimmer(app, &pal, &words, font, app.clock))
}

fn keybind(a: &mut NodeArgs) -> Result<El, String> {
    let label = match (a.string("keys"), a.string("command")) {
        (Some(keys), _) => keys.parse::<rune::Keybind>().ok().and_then(|k| k.chords().first().copied()).map(crate::keymap::label).unwrap_or_else(|| keys.to_owned()),
        (None, Some(command)) => crate::keymap::overrides::command(command)
            .and_then(|k| k.chords().first().copied())
            .or_else(|| cx::app().and_then(|app| app.catalog().into_iter().find(|c| c.id == command).and_then(|c| c.chord)))
            .map(crate::keymap::label)
            .unwrap_or_default(),
        _ => String::new(),
    };
    Ok(crate::keymap::view::chips(&p(a), vec![label]))
}

fn variant(name: Option<&str>) -> ButtonVariant {
    match name {
        Some("contrast") => ButtonVariant::Contrast,
        Some("primary") => ButtonVariant::Submit,
        Some("ghost") => ButtonVariant::Ghost,
        Some("ghost-muted") => ButtonVariant::GhostMuted,
        Some("outline") => ButtonVariant::Outline,
        Some("danger") => ButtonVariant::Danger,
        _ => ButtonVariant::Neutral,
    }
}

#[allow(deprecated)]
fn button(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let k = kit(&pal);
    let words = a.string("text").unwrap_or_default().to_owned();
    let (disabled, loading) = (a.flag("disabled"), a.flag("loading"));
    let small = a.string("size") == Some("small");
    let size = if small { rune_kit::ButtonSize::Small } else { rune_kit::ButtonSize::Normal };
    let v = if loading && !small { ButtonVariant::Loading } else { variant(a.string("variant")) };
    let mut b = crate::widgets::button(words.clone(), v).size(size).disabled(disabled);
    if let Some(lead) = a.string("icon") {
        b = b.icon(named_icon(lead));
    }
    if loading && small {
        // `<Loader class="size-3.5" />` before the label, in the label's color.
        let spinner = Loader::new().size(14.0).build(k);
        b = b.label_with(move |style, ink| row().gap(4.0).center_y().child(spinner).child(text(words).font(style).color(ink)));
    } else if loading {
        // Desktop's `variant="loading"` button holds only `<Loader />`.
        let spinner = Loader::new().size(16.0).build(k);
        b = b.label_with(move |_, _| spinner);
    }
    let el = b.build(k);
    Ok(if disabled || loading { el } else { a.on_press(el, "onPress") })
}

fn icon_button(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let name = a.string("icon").unwrap_or_default().to_owned();
    let small = a.string("size") == Some("small");
    let side = if small { 24.0 } else { 28.0 };
    let pressed = a.flag("pressed");
    let mut b = row().role("button").size(side, side).center().radius(6.0).hover(pal.hover).press(pal.pressed).hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT);
    if pressed {
        b = b.fill(pal.pressed);
    }
    let b = b.child(rune::icon(named_icon(&name), 16.0).color(if a.string("variant") == Some("ghost-muted") { pal.icon_muted } else { pal.icon }));
    Ok(if a.flag("disabled") { b.layer_opacity(0.5) } else { a.on_press(b, "onPress") })
}

fn switch(a: &mut NodeArgs) -> Result<El, String> {
    let checked = a.flag("checked");
    let key = a.string("key").unwrap_or("switch").to_owned();
    let s = crate::settings::switch(&p(a), &key, checked);
    if a.flag("disabled") {
        return Ok(s.disabled(true).layer_opacity(0.5));
    }
    Ok(a.on_press_with(s, "onChange", vec![("checked", Data::Bool(!checked))]))
}

fn radio(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let checked = a.flag("checked");
    // A description's tone is Desktop's `state-fg-*` (`data-tone` on a status line).
    let tone = match a.string("tone") {
        Some(t @ ("success" | "warning" | "danger")) => session_view::palette::token(pal.dark, &format!("state-fg-{t}")),
        _ => None,
    };
    let r = crate::plugin_host::widgets::radio(kit(&pal), a.string("text").unwrap_or_default(), a.string("description").unwrap_or_default(), checked, a.flag("disabled"), tone, a.number("lineHeight").unwrap_or(20.0) as f32);
    Ok(if a.flag("disabled") { r } else { a.on_press_with(r, "onChange", vec![("checked", Data::Bool(true))]) })
}

/// Desktop's `TextInput` over the plugin's editor (`ctx:CreateEditor`). Enter fires the
/// editor's `Submitted` signal.
fn field(a: &mut NodeArgs) -> Result<El, String> {
    let editor = a.data("editor").ok_or("ui.field needs an editor from ctx:CreateEditor")?;
    let (Some(plugin), Some(index)) = (editor.get("plugin").as_str().map(str::to_owned), editor.get("id").as_i64()) else { return Err("ui.field needs an editor from ctx:CreateEditor".into()) };
    let index = index as usize;
    // The field draws its editor's text, caret and focus: the render runs again as they change.
    cx::read(cx::Dep::Editor(plugin.as_str().into(), index));
    let app = cx::app().ok_or("ui.field draws only in a render")?;
    let editors = app.plugins.luau.editors.borrow();
    let Some(edit) = editors.get(&(plugin.clone(), index)) else { return Ok(el()) };
    let edit_plugin = plugin.clone();
    let mut input = rune_kit::TextInput::new(edit)
        .placeholder(a.string("placeholder").unwrap_or_default().to_owned())
        .invalid(a.flag("invalid"))
        .disabled(a.flag("disabled"))
        .on_edit(move |app: &mut App| super::editors::get_mut(app, &edit_plugin, index));
    if a.string("size") == Some("large") {
        input = input.large();
    }
    if a.flag("secure") {
        input = input.secure();
    }
    if let Some(w) = a.number("w").filter(|w| *w > 0.0) {
        input = input.width(Some(w as f32));
    } else if a.string("w") == Some("full") {
        input = input.width(None);
    }
    input = input.on_submit(move |app: &mut App, tx: &mut Tx| super::submitted(app, &plugin, index, tx));
    Ok(input.build(kit(&p(a))))
}

/// Desktop's `ServerHealthIndicator` (`servers/registry/row.tsx`) as a plugin row's
/// `ServerRow.Indicator`: a lock while it waits for sign-in, a spinner while it connects, a
/// warning when incompatible, else a 6-point dot (green once healthy, red when unreachable,
/// faint when unknown).
fn server_indicator(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let health = a.data("health");
    let field = |k: &str| health.as_ref().map(|h| h.get(k).clone()).unwrap_or(Data::Nil);
    let token = |name: &str, fallback| session_view::palette::token(pal.dark, name).unwrap_or(fallback);
    let slot = || row().w(6.0).h(14.0).center();
    if a.flag("auth") {
        return Ok(slot().role("status").child(rune::icon(named_icon("lock"), 12.0).color(pal.icon_muted)));
    }
    if a.flag("connecting") || field("checking") == Data::Bool(true) {
        return Ok(slot().role("status").child(Loader::new().size(12.0).build(kit(&pal))));
    }
    if field("incompatible") == Data::Bool(true) {
        return Ok(rune::icon(named_icon("warning"), 16.0).color(token("icon-warning-base", pal.warning)));
    }
    let color = match field("healthy") {
        Data::Bool(true) => token("icon-success-base", pal.success),
        Data::Bool(false) => token("icon-critical-base", pal.danger),
        _ => pal.icon_faint,
    };
    Ok(el().size(6.0, 6.0).my(3.5).radius(3.0).fill(color))
}

fn pill(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let app = cx::app().ok_or("ui.pill draws only in a render")?;
    let words = a.string("text").unwrap_or_default().to_owned();
    let key = a.string("key").unwrap_or("pill").to_owned();
    let mut chip = crate::subagents::chip(app, &pal, &key, words.clone(), a.flag("pressed")).shrink().min_w(0.0);
    if let Some(name) = a.string("icon") {
        let ink = match a.string("tone") {
            Some(t @ ("success" | "warning" | "danger")) => session_view::palette::token(pal.dark, &format!("state-fg-{t}")).unwrap_or(pal.icon_muted),
            _ => pal.icon_muted,
        };
        chip = chip.child(rune::icon(named_icon(name), 14.0).color(ink));
    }
    chip = chip.child(text(words).font(crate::view::UI).color(pal.text).truncate().max_w(260.0).shrink().min_w(0.0));
    Ok(a.on_press(chip, "onPress"))
}

fn settings_row(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let id = a.string("id").unwrap_or_default().to_owned();
    let control = if a.children.is_empty() { el() } else { a.children.remove(0) };
    let row = crate::settings::setting(&pal, &id, a.string("title").unwrap_or_default(), a.string("description").unwrap_or_default(), control);
    let lit = cx::app().and_then(|app| crate::settings::search::reveal_of(app, &id));
    Ok(crate::settings::pages::revealed(&pal, lit, 20.0, row))
}

fn settings_list(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let key = a.string("key").unwrap_or("plugin-settings").to_owned();
    let rows = std::mem::take(&mut a.children);
    // Desktop's `SettingsList`: the app's own card, its rows split by hairlines.
    let card = crate::settings::list(&pal, &key, rows);
    Ok(match a.string("title") {
        Some(title) => crate::settings::pages::section(&pal, &format!("plugin-section-{title}"), Some(title), card),
        None => card,
    })
}

/// A popover row that opens popover `popover` beside itself, as Session details' Extensions rows
/// do (`session-summary-row session-service-trigger`, details.css): its icon, label, a danger dot
/// (`alert`), the count (`detail`) and the chevron, lit while its list shows. The pointer
/// coming onto it, or a click, shows its list ([`super::Flyout`]).
fn flyout_row(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let words = a.string("text").unwrap_or_default().to_owned();
    let key = a.string("key").unwrap_or("flyout").to_owned();
    let plugin = cx::rendering();
    let expanded = plugin.as_ref().is_some_and(|plugin| {
        cx::read(cx::Dep::Flyout(plugin.clone()));
        cx::app().and_then(|app| app.plugins.luau.overlay.flyout_row(plugin)) == Some(key.as_str())
    });
    let mut r = row()
        .role("button")
        .label(words.clone())
        .w_full()
        .min_h(28.0)
        .pl(12.0)
        .pr(8.0)
        .py(4.0)
        .gap(8.0)
        .center_y()
        .radius(4.0)
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT);
    // An open row's fill is the hover fill (`[data-expanded]`), which the pointer doesn't add to.
    r = if expanded { r.fill(pal.hover) } else { r.hover(pal.hover) };
    if let Some(name) = a.string("icon") {
        r = r.child(rune::icon(named_icon(name), 16.0).color(pal.icon_muted));
    }
    r = r.child(text(words).font(TextStyle::new(13.0, 440.0).lh(16.0)).color(pal.text).truncate().grow(1.0).shrink().min_w(0.0));
    if a.flag("alert") {
        r = r.child(el().w(6.0).h(16.0).center().child(el().size(6.0, 6.0).radius(3.0).fill(pal.danger)));
    }
    if let Some(detail) = a.string("detail").filter(|d| !d.is_empty()) {
        r = r.child(text(detail.to_owned()).font(TextStyle::new(12.0, 440.0).lh(16.0).features("\"tnum\"")).color(pal.faint).single_line());
    }
    r = r.child(rune::icon(crate::plugin_host::icons::resolve("", "chevron-right"), 16.0).color(pal.icon_muted).ml(4.0));
    let (Some(plugin), Some(popover)) = (plugin, a.string("popover").map(str::to_owned)) else { return Ok(r) };
    let side = match a.string("side") {
        Some("left") => rune::Side::Left,
        Some("top") => rune::Side::Top,
        Some("bottom") => rune::Side::Bottom,
        _ => rune::Side::Right,
    };
    let data = a.data("props").unwrap_or_default();
    let show = move |app: &mut App, tx: &mut Tx| {
        let event = super::FlyoutEvent::Row { id: popover.clone(), row: key.clone(), side, data: data.clone() };
        cx::ask(&plugin, cx::Request::Flyout(event));
        super::requests::carry_out(app, tx);
    };
    let show = std::rc::Rc::new(show);
    let hover = show.clone();
    Ok(r.key(a.string("key").unwrap_or("flyout").to_owned())
        .on_hover_change(move |app: &mut App, inside: bool, tx: &mut Tx| {
            if inside {
                hover(app, tx);
            }
        })
        .on_press_tx(move |app: &mut App, tx: &mut Tx| show(app, tx)))
}

fn markdown(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let source = a.string("source").unwrap_or_default().to_owned();
    let width = a.number("w").map_or_else(session_view::style::content_w, |w| w as f32);
    let ink = a.color("color").unwrap_or(pal.text);
    let mut md = session_view::style::markdown_style(&pal, ink, width);
    if let Some(lh) = a.number("lineHeight") {
        md = md.body_font(session_view::style::MD.lh(lh as f32));
    }
    let none: Vec<(&[std::ops::Range<usize>], rune_kit::Highlight)> = Vec::new();
    Ok(session_view::style::markdown_styled::<App, _>(&pal, &source, 0.0, a.flag("streaming"), md, none, None, None, &[]))
}

/// The app's `ProgressCircle`: a track and the arc from twelve o'clock, clockwise; `compact` a
/// 14-unit ring with a 1.5 stroke, `indicator` a 16-unit one with an overlay over its track.
fn progress_circle(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let indicator = a.string("appearance") == Some("indicator");
    let view: f32 = if indicator { 16.0 } else { 14.0 };
    let stroke = a.number("strokeWidth").map_or(if indicator { 3.0 } else { 1.5 }, |s| s as f32);
    let size = a.number("size").map_or(view, |s| s as f32);
    let percent = a.number("percentage").unwrap_or(0.0).clamp(0.0, 100.0).round() as u32;
    let (track, arc) = ring_icons(view, stroke, percent);
    let icon = |id: u32| crate::plugin_host::icons::icon(id);
    let place = |e: El| e.pin(Some(0.0), Some(0.0), None, None);
    let mut out = el().size(size, size);
    if let Some(track) = icon(track) {
        out = out.child(place(rune::icon(track, size).color(a.color("track").unwrap_or(pal.q.layer4))));
        if let (true, Some(overlay)) = (indicator, a.color("overlay")) {
            out = out.child(place(rune::icon(track, size).color(overlay)));
        }
    }
    if let Some(arc) = arc.and_then(icon) {
        out = out.child(place(rune::icon(arc, size).color(a.color("progress").unwrap_or(pal.icon))));
    }
    Ok(out)
}

/// The ring's track and arc icons (`None` for an empty arc), interned once per look and percent.
fn ring_icons(view: f32, stroke: f32, percent: u32) -> (u32, Option<u32>) {
    thread_local! {
        static RINGS: std::cell::RefCell<std::collections::HashMap<(u32, u32, u32), (u32, Option<u32>)>> = std::cell::RefCell::default();
    }
    let key = (view.to_bits(), stroke.to_bits(), percent);
    if let Some(found) = RINGS.with_borrow(|r| r.get(&key).copied()) {
        return found;
    }
    let (c, r) = (view / 2.0, view / 2.0 - stroke / 2.0);
    let circle = format!(r#"<circle cx="{c}" cy="{c}" r="{r}" stroke="currentColor" stroke-width="{stroke}"/>"#);
    let arc = match percent {
        0 => None,
        100 => Some(circle.clone()),
        _ => {
            let angle = percent as f32 / 100.0 * std::f32::consts::TAU;
            let (x, y) = (c + r * angle.sin(), c - r * angle.cos());
            Some(format!(r#"<path d="M{c} {top}A{r} {r} 0 {large} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="{stroke}"/>"#, top = c - r, large = u8::from(percent > 50)))
        }
    };
    let intern = crate::plugin_host::icons::intern_icon;
    let track = intern("luau", &format!("ring-track-{view}-{stroke}"), &circle, view);
    let arc = arc.map(|a| intern("luau", &format!("ring-arc-{view}-{stroke}-{percent}"), &a, view));
    RINGS.with_borrow_mut(|r| r.insert(key, (track, arc)));
    (track, arc)
}