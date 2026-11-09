//! `ui.segmented`: the app's segmented control (`segmented-control-v2`): a 28 pt track on
//! `layer-01` under a 0.5 pt `border-base` ring, each item 40 × 28 with its 16 pt icon, the picked
//! one on `bg-base` with a 0.5 pt `border-strong` ring. An item's label is its tooltip.

use rune::{Color, Cursor, El, Icon, el, icon, row};
use rune_luau::{Data, NodeArgs};

use super::p;
use crate::palette::Pal;

pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let pal = p(a);
    let value = a.string("value").unwrap_or_default().to_owned();
    let Some(Data::List(items)) = a.data("items") else { return Err("ui.segmented needs items".into()) };
    let mut track = row().h(28.0).center_y().radius(6.0).fill(pal.layer1).box_shadow(Pal::ring_out(pal.border));
    for item in items {
        let id = item.get("id").as_str().unwrap_or_default().to_owned();
        let label = item.get("label").as_str().unwrap_or_default().to_owned();
        let glyph = super::icon(item.get("icon").as_str().unwrap_or_default());
        let key = item.get("key").as_str().map_or_else(|| id.clone(), str::to_owned);
        let on = id == value;
        let b = button(&pal, key, glyph, on, label);
        track = track.child(if on { b } else { a.on_press_with(b, "onChange", vec![("value", Data::String(id))]) });
    }
    Ok(track)
}

fn button(p: &Pal, key: String, glyph: &'static Icon, active: bool, help: String) -> El {
    el().key(key)
        .size(40.0, 28.0)
        .radius(6.0)
        .center()
        .cursor(Cursor::Pointer)
        .fill(if active { p.bg_base } else { Color::TRANSPARENT })
        .box_shadow(Pal::ring_out(if active { p.q.border_strong } else { Color::TRANSPARENT }))
        .role("button")
        .label(help.clone())
        .help(help)
        .child(icon(glyph, 16.0).color(if active { p.icon } else { p.icon_muted }).group_color(p.icon))
        .hover_group()
}
