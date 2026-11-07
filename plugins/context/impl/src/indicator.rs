//! `SessionContextUsage` (`indicator.tsx`): the session header's button (its context ring, its
//! token count and, when anything costs, its spend), which opens the Context tab; in a narrow
//! header only the ring, which opens the usage menu; and the ring that leads the tab's title.

use opencode_plugin::abi::SHADOW_FLOATING;
use opencode_plugin::prelude::*;

use crate::format::{compact, number, usd};
use crate::{Context, PANEL};

/// The figures the header shows for a session.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    /// The last step's tokens, all kinds summed.
    pub total: u64,
    /// That as a share of its model's context window, when the catalog gives the window.
    pub percent: Option<u64>,
    pub cost: f64,
    pub show_cost: bool,
}

impl Usage {
    pub fn of(cx: &ViewCx<Context>, s: &Session) -> Usage {
        let t = s.usage.unwrap_or_default();
        let total = t.input + t.output + t.reasoning + t.cache_read + t.cache_write;
        let limit = s.usage.and_then(|_| cx.model(&s.usage_provider_id, &s.usage_model_id).context_limit);
        let percent = limit.map(|l| (total as f64 / l as f64 * 100.0).round() as u64);
        // Spend anywhere in the family shows the cost, even on a free model; an unknown model too.
        let (provider, model) = if s.model_id.is_empty() { (&s.usage_provider_id, &s.usage_model_id) } else { (&s.provider_id, &s.model_id) };
        let info = (!model.is_empty()).then(|| cx.model(provider, model));
        let show_cost = s.cost > 0.0 || info.is_none_or(|m| m.name.is_none() || m.paid);
        Usage { total, percent, cost: s.cost, show_cost }
    }

    fn version(&self) -> (u64, Option<u64>, u64, bool) {
        (self.total, self.percent, self.cost.to_bits(), self.show_cost)
    }
}

/// `ProgressCircle`: Desktop's two looks of the ring.
#[derive(Clone, Copy)]
enum Look {
    /// 14-unit box drawn at 16, 1.5 stroke: the header's.
    Compact,
    /// 16-unit box, 2 stroke, a pressed overlay over its track: the tab's.
    Indicator,
}

/// The ring at `percent`: a track and the arc from twelve o'clock, clockwise.
fn ring(p: &Palette, look: Look, percent: u64, overlay: Option<Rgba>) -> El {
    let (view, stroke) = match look {
        Look::Compact => (14.0_f32, 1.5_f32),
        Look::Indicator => (16.0, 2.0),
    };
    let (c, r) = (view / 2.0, view / 2.0 - stroke / 2.0);
    let circle = format!(r#"<circle cx="{c}" cy="{c}" r="{r}" stroke="currentColor" stroke-width="{stroke}"/>"#);
    let pct = percent.min(100);
    let arc = match pct {
        0 => String::new(),
        100 => circle.clone(),
        _ => {
            let a = pct as f32 / 100.0 * std::f32::consts::TAU;
            let (x, y) = (c + r * a.sin(), c - r * a.cos());
            format!(r#"<path d="M{c} {top}A{r} {r} 0 {large} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="{stroke}"/>"#, top = c - r, large = u8::from(pct > 50))
        }
    };
    let track = intern_icon(&format!("ring-track-{view}"), &circle, view);
    let mut box_ = el().size(16.0, 16.0).child(icon_id(track, 16.0).color(p.layer4).pin(Some(0.0), Some(0.0), None, None));
    if let Some(o) = overlay {
        box_ = box_.child(icon_id(track, 16.0).color(o).pin(Some(0.0), Some(0.0), None, None));
    }
    if !arc.is_empty() {
        let arc = intern_icon(&format!("ring-arc-{view}-{pct}"), &arc, view);
        box_ = box_.child(icon_id(arc, 16.0).color(p.icon).pin(Some(0.0), Some(0.0), None, None));
    }
    box_
}

/// The `session.header` slot: the usage button, or in a narrow header the ring alone.
pub fn header(_: &Context, cx: &mut ViewCx<Context>, props: &Props) -> El {
    let Some(session) = cx.session(&props.session_id).filter(|s| !s.id.is_empty()) else { return cx.nothing() };
    let usage = Usage::of(cx, &session);
    let open = cx.popover_open("usage");
    let sid = session.id.clone();
    cx.memo(&format!("header-{sid}"), (usage.version(), props.compact, open, cx.dark()), |cx| {
        let p = cx.pal();
        let base = row().key("context-usage").role("button").h(28.0).px(6.0).gap(8.0).center_y().radius(6.0).hover(p.hover).press(p.pressed).instant_hover();
        let circle = ring(&p, Look::Compact, usage.percent.unwrap_or(0), None);
        if props.compact {
            let base = if open { base.fill(p.pressed) } else { base };
            let button = base.label("View context usage").help("View context usage").child(circle).on_press(cx.handler(|_: &mut Context, cx| cx.toggle_popover("usage", "context-usage")));
            // The memo keys its root, so the button's own key sits a level down.
            return row().child(button);
        }
        let font = |e: El| e.font(13.0, 440.0).line_height(20.0).tracking(-0.04).color(p.faint);
        let mut button = base
            .label("Toggle session context")
            .help("Toggle session context")
            .child(circle)
            .child(font(odometer(format!("ctx-{sid}"), compact(usage.total), usage.total as f64)));
        if usage.show_cost {
            button = button
                .child(el().w(6.0).center().child(font(text("\u{b7}"))))
                .child(font(odometer(format!("cost-{sid}"), usd(usage.cost), (usage.cost * 100.0).round())));
        }
        row().child(button.on_press(cx.handler(|_: &mut Context, cx| cx.toggle_panel(PANEL))))
    })
}

/// The narrow header's usage menu: tokens, cost, context share, then View context usage.
pub fn menu(_: &Context, cx: &mut ViewCx<Context>, props: &Props) -> El {
    let Some(session) = cx.session(&props.session_id) else { return cx.nothing() };
    let usage = Usage::of(cx, &session);
    let p = cx.pal();
    let line = |name: &'static str, value: String| {
        row()
            .h(28.0)
            .px(12.0)
            .gap(16.0)
            .center_y()
            .child(text(name).font(13.0, 440.0).line_height(16.0).tracking(-0.04).color(p.muted).shrink())
            .child(spacer())
            .child(text(value).font(13.0, 440.0).line_height(16.0).tracking(-0.04).tabular().truncate().color(p.text))
    };
    let mut card = col().key("context-usage-menu").role("menu").label("View context usage").w(200.0).padding(2.0).radius(6.0).fill(p.layer1).shadow(SHADOW_FLOATING);
    card = card.child(line("Tokens", number(usage.total)));
    if usage.show_cost {
        card = card.child(line("Cost", usd(usage.cost)));
    }
    card = card.child(line("Context", format!("{}%", usage.percent.unwrap_or(0))));
    card = card.child(el().w_full().py(2.0).child(el().w_full().h(1.0).fill(p.border_muted)));
    let view = row()
        .key("context-usage-view")
        .role("menuitem")
        .h(28.0)
        .pl(12.0)
        .pr(6.0)
        .center_y()
        .radius(4.0)
        .hover(p.hover)
        .instant_hover()
        .child(text("View context usage").font(13.0, 440.0).line_height(16.0).tracking(-0.04).color(p.text))
        .on_press(cx.handler(|_: &mut Context, cx| {
            cx.close_popover();
            cx.open_panel(PANEL);
        }));
    card.child(view)
}

/// What leads the Context tab's title: the ring in its indicator look.
pub fn tab_label(_: &Context, cx: &mut ViewCx<Context>, props: &Props) -> El {
    let percent = cx.session(&props.session_id).map(|s| Usage::of(cx, &s).percent.unwrap_or(0)).unwrap_or(0);
    cx.memo("tab-label", (percent, cx.dark()), |cx| {
        let p = cx.pal();
        // Desktop's label spaces the ring 8 from the title; the strip leaves 6.
        ring(&p, Look::Indicator, percent, Some(p.pressed)).mr(2.0)
    })
}
