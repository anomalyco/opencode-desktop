//! Layout and content nodes: column, row, text, markdown, image, card, divider; the controls
//! and the carousel live in their own modules.

use opencode_widget::{Align, Node, TextStyle, Tone, WidgetEvent};
use rune::{Color, El, SurfaceFit, Tx, col, el, icon, row, text};
use rune_kit::Kit;

use crate::{Cx, carousel, controls, gap};

/// Below this width a row of cards stacks into a column, as a narrow window or a phone shows them.
const CARDS_STACK_BELOW: f32 = 440.0;

/// Corner radius of cards and images.
pub(crate) const RADIUS: f32 = 10.0;

pub(crate) fn node<S: 'static>(cx: &Cx<S>, n: &Node, path: &str) -> El {
    let k = cx.look.kit;
    let e = match n {
        Node::Column { children, gap: g, align } => {
            let c = col().w_full().gap(gap(*g));
            let c = match align {
                Align::Start => c.items_start(),
                Align::Center => c.items_center(),
                Align::End => c.items_end(),
                Align::Stretch => c.items_stretch(),
            };
            c.children(kids(cx, children, path, Share::No))
        }
        Node::Row { children, gap: g, align, wrap } => row_node(cx, children, *g, *align, *wrap, path),
        Node::Text { text: t, style, tone } => text_node(k, t, *style, *tone),
        Node::Markdown { text: t } => col().w_full().child((cx.look.markdown)(t)),
        Node::Image { src, alt, aspect } => image(cx, src, alt.as_deref(), *aspect),
        Node::Carousel { id, children, .. } => carousel::view(cx, id.as_deref(), children, path),
        Node::Card { id, selected, children } => card(cx, id.as_deref(), *selected, children, path),
        Node::Button { id, label, variant, disabled } => controls::button(cx, id, label, *variant, *disabled),
        Node::Checkbox { id, label, checked, disabled } => controls::checkbox(cx, id, label, *checked, *disabled),
        Node::Slider { id, label, value, min, max, step } => controls::slider(cx, id, label.as_deref(), *value, *min, *max, *step),
        Node::Select { id, label, options, value, multiple } => controls::select(cx, id, label.as_deref(), options, value.as_ref(), *multiple),
        Node::Input { id, label, placeholder, multiline, .. } => controls::input(cx, id, label.as_deref(), placeholder.as_deref(), *multiline),
        Node::Divider => rune::rule(1.0, k.c.border_weak).w_full(),
        Node::Unknown => {
            #[cfg(debug_assertions)]
            eprintln!("widget: a node of a type this client doesn't know was drawn as nothing ({path})");
            el()
        }
    };
    e.key(path.to_owned())
}

/// How cards in a row take its width.
#[derive(Clone, Copy, PartialEq)]
enum Share {
    /// Not in a row: each takes the column's width.
    No,
    /// Side by side on one line, sharing it equally.
    Line,
    /// Wrapping: each at least [`CARD_MIN`] wide, the line's rest shared.
    Wrap,
}

/// A wrapping row's narrowest card.
const CARD_MIN: f32 = 180.0;

fn kids<S: 'static>(cx: &Cx<S>, children: &[Node], path: &str, share: Share) -> Vec<El> {
    children
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let e = node(cx, c, &format!("{path}.{i}"));
            match (share, c) {
                (Share::Line, Node::Card { .. }) => e.w(0.0).grow(1.0).min_w(0.0),
                (Share::Wrap, Node::Card { .. }) => e.w(CARD_MIN).grow(1.0),
                _ => e,
            }
        })
        .collect()
}

fn row_node<S: 'static>(cx: &Cx<S>, children: &[Node], g: Option<u8>, align: Option<Align>, wrap: bool, path: &str) -> El {
    let cards = children.iter().filter(|c| matches!(c, Node::Card { .. })).count();
    // Cards on one line need room; in a narrow column they read one under the other.
    if cards >= 2 && !wrap && cx.look.width < CARDS_STACK_BELOW {
        return col().w_full().gap(gap(g)).children(kids(cx, children, path, Share::No));
    }
    let r = row().w_full().gap(gap(g));
    let r = match align {
        Some(Align::Center) => r.items_center(),
        Some(Align::End) => r.items_end(),
        // A row of cards keeps them the same height; anything else sits at the top.
        _ if cards > 0 => r.items_stretch(),
        _ => r.items_start(),
    };
    let r = if wrap { r.flex_wrap() } else { r };
    r.children(kids(cx, children, path, if wrap { Share::Wrap } else { Share::Line }))
}

pub(crate) fn tone(k: &Kit, t: Tone) -> Color {
    let c = &k.c;
    match t {
        Tone::Default => c.text_base,
        Tone::Muted => c.text_muted,
        Tone::Accent => c.text_accent,
        Tone::Success => c.success.fg,
        Tone::Warning => c.warning.fg,
        Tone::Error => c.danger.fg,
    }
}

fn text_node(k: &Kit, t: &str, style: TextStyle, t_tone: Tone) -> El {
    let font = match style {
        TextStyle::Title => {
            let mut f = k.ty.body_medium;
            f.size = 15.0;
            f.weight = 600.0;
            f.lh(22.0)
        }
        TextStyle::Body => k.ty.ui,
        TextStyle::Caption => k.ty.caption(),
        TextStyle::Code => k.ty.mono,
    };
    let color = match (style, t_tone) {
        (TextStyle::Title, Tone::Default) => k.c.text_strong,
        (TextStyle::Caption, Tone::Default) => k.c.text_muted,
        _ => tone(k, t_tone),
    };
    let e = text(t.to_owned()).font(font).color(color).wrap().selectable(true);
    // It shrinks and wraps in a row, and a column stretches it.
    let e = if style == TextStyle::Title { e.role("heading").level(3) } else { e };
    e.shrink().min_w(0.0)
}

/// A picture at its aspect (4:3 until the template or the picture says otherwise), cropped to
/// fill; while it loads, the same room in a quiet fill; failed, its alt text in that room.
fn image<S: 'static>(cx: &Cx<S>, src: &str, alt: Option<&str>, aspect: Option<f64>) -> El {
    let k = cx.look.kit;
    let picture = (cx.look.image)(src);
    let natural = picture.as_ref().and_then(|p| p.size()).filter(|[w, h]| *w > 0 && *h > 0).map(|[w, h]| w as f32 / h as f32);
    let ratio = aspect.map(|a| a as f32).filter(|a| a.is_finite() && *a > 0.05 && *a < 20.0).or(natural).unwrap_or(4.0 / 3.0);
    let label = alt.map(str::to_owned).unwrap_or_else(|| file_name(src).to_owned());
    let tile = el().w_full().aspect_ratio(ratio).radius(RADIUS).clip_rounded(RADIUS).fill(k.c.surface_weak).role("img").label(label.clone());
    match picture {
        Some(p) if p.error().is_none() => tile.pixels_fit(p.pixels(), SurfaceFit::Fill),
        Some(_) => tile.center().child(text(label).font(k.ty.caption()).color(k.c.text_faint).truncate().px(8.0)),
        None => tile,
    }
}

fn file_name(src: &str) -> &str {
    if src.starts_with("data:") {
        return "image";
    }
    let path = src.split(['?', '#']).next().unwrap_or(src);
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(src)
}

/// A card: a raised surface; with an `id` it presses (`press`), and `selected` rings it in the
/// accent with a check in its corner.
fn card<S: 'static>(cx: &Cx<S>, id: Option<&str>, selected: bool, children: &[Node], path: &str) -> El {
    let k = cx.look.kit;
    let c = &k.c;
    let ring = if selected { (2.0, c.border_focus) } else { (1.0, c.border_weak) };
    let mut body = col()
        .w_full()
        .gap(8.0)
        .padding(8.0)
        .radius(RADIUS + 2.0)
        .fill(c.surface_raised_base)
        .border(ring.0, ring.1)
        .children(kids(cx, children, path, Share::No));
    if selected {
        let badge = el()
            .key("selected")
            .pin(None, Some(14.0), Some(14.0), None)
            .size(20.0, 20.0)
            .radius(10.0)
            .center()
            .fill(c.bg_accent)
            .a11y_hidden()
            .child(icon(k.glyph.check, 10.0).color(c.knob_on));
        body = body.child(badge);
    }
    let Some(id) = id else { return body };
    let body = body.role("button").checked(selected).disabled(!cx.live);
    let Some(hooks) = cx.act() else { return body };
    let id = id.to_owned();
    k.interactive(body.hover(c.surface_raised_hover).focus_ring(2.0, 2.0, c.border_focus).press_scale(0.99))
        .on_press_tx(move |s: &mut S, tx: &mut Tx| hooks.send(s, WidgetEvent::press(id.clone()), tx))
}
