//! The debug bar (`bar.tsx`, inline): across the window's foot in the mono face, the close
//! button, the latest turn's TIME and TPS, MEMLOG in debug builds, and at the far end the motion
//! tools of local and Beta builds: COMPONENTS, SPEED, SLOW and KNOBS.

use opencode_plugin::prelude::*;
use opencode_plugin::{Handler, system};

use crate::Debug;
use crate::turn::{Turn, latest};

const CLOSE: &str = r#"<path d="M12.4446 3.55469L3.55566 12.4436M3.55566 3.55469L12.4446 12.4436" stroke="currentColor" stroke-linejoin="round" fill="none"/>"#;

/// The speed cell steps through the common slow-downs, then back to full speed.
fn next_speed(speed: f32) -> f32 {
    [0.5, 0.25, 1.0].into_iter().find(|step| *step < speed - 1e-4).unwrap_or(1.0)
}

pub fn render(s: &Debug, cx: &mut ViewCx<Debug>, props: &Props) -> El {
    let view = s.view();
    if !view.open {
        return cx.nothing();
    }
    let dark = cx.dark();
    let turn = turn(cx, &props.session_id).unwrap_or_default();
    let ink = if dark { [1.0, 1.0, 1.0, 0.936] } else { [0x17 as f32 / 255.0, 0x17 as f32 / 255.0, 0x17 as f32 / 255.0, 1.0] };
    // `surface-raised-base`: a pressed cell, and any cell under the pointer.
    let raised = if dark { [1.0, 1.0, 1.0, 0.059] } else { [0.0, 0.0, 0.0, 0.031] };
    let na = "N/A";
    let mut cells = row().key("debug-bar-cells").w_full().gap(9.0).center_y().no_wrap();
    if view.closable {
        let p = cx.pal();
        let muted = cx.token("icon-icon-muted").unwrap_or(p.icon_muted);
        let hover = cx.token("overlay-simple-overlay-hover").unwrap_or(p.hover);
        let pressed = cx.token("overlay-simple-overlay-pressed").unwrap_or(p.pressed);
        cells = cells.child(
            row()
                .key("debug-bar-close")
                .role("button")
                .label("Hide debug bar")
                .help("Hide debug bar. Bring it back from the command palette: Toggle debug bar.")
                .help_above()
                .size(20.0, 20.0)
                .radius(4.0)
                .center()
                .hover(hover)
                .press(pressed)
                .child(icon_id(intern_icon("debug-close", CLOSE, 16.0), 16.0).color(muted))
                .on_press(cx.handler(|s: &mut Debug, cx| s.hide(cx))),
        );
    }
    let tps = turn.tps.map(|t| format!("{t:.1}"));
    cells = cells
        .child(cell("debug-bar-time", "TIME", "Time from the prompt that started the latest turn until its response completed.", turn.duration.as_deref().unwrap_or(na), turn.duration.is_none(), ink))
        .child(cell("debug-bar-tps", "TPS", "Output and reasoning tokens per second across the latest turn's model requests, excluding tool time.", tps.as_deref().unwrap_or(na), tps.is_none(), ink));
    if s.debug_build {
        let (on, env) = (s.memlog.on(), s.memlog.env());
        let tip = if env {
            "Memory diagnostics are on for this launch: OPENCODE_DESKTOP_MEMORY_LOG is set."
        } else {
            "Memory diagnostics: log app memory to the desktop logs every 30 seconds. Stays on across restarts until turned off; the bar stays open while it runs."
        };
        let value = if env { "ENV" } else if on { "ON" } else { "OFF" };
        let press = cx.handler(|s: &mut Debug, cx| {
            let now = cx.now_ms();
            s.memlog.toggle(now);
            s.schedule(cx);
        });
        cells = cells.child(toggle_cell("debug-bar-memlog", "MEMLOG", tip, value, on, ink, raised, press));
    }
    if s.tools {
        let speed = system::dev("speed").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let slow = system::dev("slow").as_deref() == Some("on");
        let knobs = system::dev("knobs").as_deref() == Some("on");
        let components = row()
            .key("debug-bar-components")
            .role("link")
            .label("COMPONENTS")
            .help("Open the components lab: every story, with live controls and its states")
            .help_above()
            .min_h(20.0)
            .px(6.0)
            .pt(4.0)
            .pb(6.0)
            .radius(4.0)
            .hover(raised)
            .instant_hover()
            .child(label("COMPONENTS", ink))
            .on_press(cx.handler(|_: &mut Debug, cx| cx.dev("components")));
        let speed_cell = toggle_cell(
            "debug-bar-speed",
            "SPEED",
            "Global animation speed. Click to step through 0.5x and 0.25x back to 1x.",
            // Plex Mono draws an x between figures as a multiplication sign.
            &format!("{speed:.2}\u{d7}"),
            (speed - 1.0).abs() > 1e-4,
            ink,
            raised,
            cx.handler(move |_: &mut Debug, cx| cx.dev(&format!("speed:{}", next_speed(speed)))),
        );
        let slow_cell = toggle_cell(
            "debug-bar-slow",
            "SLOW",
            "Run every tuned animation at a quarter speed.",
            if slow { "ON" } else { "OFF" },
            slow,
            ink,
            raised,
            cx.handler(move |_: &mut Debug, cx| cx.dev(if slow { "slow:off" } else { "slow:on" })),
        );
        let knobs_cell = toggle_cell("debug-bar-knobs", "KNOBS", "Tune live. Changes persist on this device.", if knobs { "ON" } else { "OFF" }, knobs, ink, raised, cx.handler(|s: &mut Debug, cx| s.toggle_knobs(cx)));
        cells = cells.child(spacer()).child(row().key("debug-bar-motion").gap(9.0).center_y().child(components).child(speed_cell).child(slow_cell).child(knobs_cell));
    }
    col().key("debug-bar").role("complementary").label("Provider performance diagnostics").w_full().mt(-6.0).px(12.0).py(4.0).clip().child(cells)
}

/// The latest turn of the routed session, read again as its messages change.
fn turn(cx: &mut ViewCx<Debug>, session: &str) -> Option<Turn> {
    if session.is_empty() {
        return None;
    }
    let info = cx.session(session)?;
    // A step's end changes its tokens, a new message the count, and the turn's end `busy`.
    let version = crate::hash(&(info.messages, info.busy, info.usage));
    let (res, _) = cx.resource(&format!("/api/session/{session}/message?limit=100&order=desc"), version);
    let body = match res {
        Resource::Ready(b) | Resource::Loading(Some(b)) => b,
        _ => return None,
    };
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    let mut messages = v["data"].as_array().cloned().unwrap_or_default();
    messages.reverse();
    latest(&messages)
}

/// A cell's name: 10 px black caps at 70%.
fn label(words: &'static str, ink: Rgba) -> El {
    text(words).mono().font(10.0, 900.0).line_height(10.0).tracking(0.4).color([ink[0], ink[1], ink[2], ink[3] * 0.7]).no_wrap()
}

/// A reading (`Cell`, inline): its name and value on one baseline.
fn cell(key: &'static str, name: &'static str, tip: &'static str, value: &str, dim: bool, ink: Rgba) -> El {
    let value_ink = if dim { [ink[0], ink[1], ink[2], ink[3] * 0.7] } else { ink };
    // The 10 px name sits 1.4 pt lower than the 12 pt value line so their baselines meet.
    let pair = row().gap(6.0).child(label(name, ink).mt(1.4)).child(text(value.to_uppercase()).mono().font(11.0, 700.0).line_height(12.0).tabular().color(value_ink).no_wrap());
    col().key(key).help(tip).help_above().min_h(20.0).px(6.0).pt(3.0).pb(5.0).child(pair)
}

/// A switch (`ToggleCell`, inline): pressed while on, as a cell under the pointer.
#[allow(clippy::too_many_arguments)]
fn toggle_cell(key: &'static str, name: &'static str, tip: &'static str, value: &str, active: bool, ink: Rgba, raised: Rgba, press: Handler) -> El {
    let pair = row().gap(6.0).child(label(name, ink).mt(0.9)).child(text(value.to_uppercase()).mono().font(11.0, 700.0).line_height(11.0).color(ink).no_wrap());
    let cell = col()
        .key(key)
        .role("button")
        .label(format!("{name}: {value}"))
        .help(tip)
        .help_above()
        .min_h(20.0)
        .px(6.0)
        .pt(3.5)
        .pb(5.5)
        .radius(4.0)
        .hover(raised)
        .instant_hover()
        .child(pair)
        .on_press(press);
    if active { cell.fill(raised) } else { cell }
}
