//! A session's usage in its header: the context ring, the last step's context window and the
//! session's spend, each number an odometer keyed by its session.

use std::sync::OnceLock;

use opencode_kit::model::{Message, Transcript, tokens_short};
use rune::{El, Icon, el, icon, row};

use crate::odometer::{SPRING, odometer_end};
use crate::palette::Pal;
use crate::style::UI_SMALL;

/// The last finished step's context window, when one says.
pub fn context(t: &Transcript) -> Option<u64> {
    t.messages.iter().rev().find_map(|m| match m {
        Message::Assistant(a) if a.tokens.context() > 0 => Some(a.tokens.context()),
        _ => None,
    })
}

/// What the transcript's steps cost, summed.
pub fn spent(t: &Transcript) -> f64 {
    t.messages.iter().map(|m| if let Message::Assistant(a) = m { a.cost } else { 0.0 }).sum()
}

/// The token count and dollars as the header writes them.
pub fn labels(ctx: u64, cost: f64) -> (String, String) {
    (tokens_short(ctx), format!("${cost:.2}"))
}

/// The usage row for session `sid`: the ring (`ctx` of `limit`) and the token count when asked,
/// then the cost. Its numbers widen to the left; the row slides on their spring, so they roll in
/// place, while `ride_trailing` keeps it locked to its parent's right edge.
pub fn row_for(p: &Pal, sid: &str, ctx: u64, cost: f64, limit: f32, show_ring: bool, show_tokens: bool) -> El {
    let (tokens, spend) = labels(ctx, cost);
    row()
        .key("usage")
        .gap(6.0)
        .center_y()
        .motion(SPRING)
        .width_leads()
        .ride_trailing()
        .children(show_ring.then(|| ring(p, (ctx as f32 / limit.max(1.0)).min(1.0))))
        .children(show_tokens.then(|| odometer_end(format!("ctx-{sid}"), tokens, ctx as f64, UI_SMALL, p.muted)))
        .child(odometer_end(format!("cost-{sid}"), spend, (cost * 100.0).round(), UI_SMALL, p.faint))
}

/// Context ring: a track circle and an arc, both icons built once per 1/32 step.
pub fn ring(p: &Pal, frac: f32) -> El {
    static ARCS: OnceLock<Vec<Icon>> = OnceLock::new();
    static TRACK: Icon = Icon { name: "ring-track", view: 16.0, body: r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"# };
    let arcs = ARCS.get_or_init(|| {
        (0..=32)
            .map(|k| {
                let f = k as f32 / 32.0;
                let body = if k == 0 {
                    String::new()
                } else if k == 32 {
                    r#"<circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="2"/>"#.to_owned()
                } else {
                    let a = f * std::f32::consts::TAU;
                    let (x, y) = (8.0 + 6.0 * a.sin(), 8.0 - 6.0 * a.cos());
                    format!(
                        r#"<path d="M8 2A6 6 0 {} 1 {x:.3} {y:.3}" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>"#,
                        u8::from(f > 0.5)
                    )
                };
                Icon { name: "ring-arc", view: 16.0, body: Box::leak(body.into_boxed_str()) }
            })
            .collect()
    });
    let k = (frac * 32.0).round().clamp(0.0, 32.0) as usize;
    el().key("ring")
        .size(16.0, 16.0)
        .child(icon(&TRACK, 16.0).color(p.layer3).absolute(0.0, 0.0))
        .child(icon(&arcs[k], 16.0).color(p.icon).absolute(0.0, 0.0))
}
