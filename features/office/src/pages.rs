//! `ui.officePages`: a Word document's pages or a deck's slides as Desktop's `OfficePages` lays
//! them out (`pages.tsx`, `pages-machine.ts`, `document-surface.css`, `presentation.tsx`): a
//! scrolling column centred in the pane, each page on its paper surface fit to the pane's width
//! between the gutters, times the zoom, with a slide's Hidden badge and speaker notes under it,
//! and the zoom controls over the corner. Each page draws its Vello scene in an effect slot
//! (`compositor.rs`); only pages a viewport either side of the screen ask for theirs.

use rune::{El, ScrollPos, TextStyle, Tx, col, el, row, text};
use opencode_ui::palette::{Elevate as _, Pal};

use super::registry::{self, Snapshot};

pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 2.0;
pub const ZOOM_STEP: f32 = 1.25;

/// `document-surface.css`: the column's inline padding, 12 in a pane at most 600 wide.
fn gutter(width: f32) -> f32 {
    if width <= 600.0 { 12.0 } else { 20.0 }
}

/// The scrollbar's room the scrollport keeps at its end (`scrollbar-gutter: stable`).
const SCROLLBAR: f32 = 8.0;

/// A page's scale in a pane `width` wide (`pageFit`): it fits between the gutters, times the zoom.
pub fn fit(width: f32, zoom: f32, page_width: f64) -> f32 {
    ((width - SCROLLBAR - gutter(width) * 2.0).max(0.0) / page_width.max(1.0) as f32) * zoom
}

const NOTE: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0);
const BADGE: TextStyle = TextStyle::new(11.0, 440.0).lh(16.0);

/// The words a view shows, in the plugin's language.
#[derive(Clone, Default)]
pub struct Labels {
    /// `Page {{number}}` or `Slide {{number}}`.
    pub page: String,
    pub hidden: String,
    pub failed: String,
    pub notes: String,
    pub zoom_in: String,
    pub zoom_out: String,
    pub fit: String,
    pub controls: String,
}

pub fn view<H: 'static>(p: &Pal, id: u64, labels: &Labels) -> El {
    let snap = registry::snapshot(id).unwrap_or_default();
    let width = snap.width;
    let zoom = snap.zoom;
    let column = col()
        .key("office-column")
        .min_w(width.max(0.0))
        .pl(gutter(width))
        .pr(gutter(width) + SCROLLBAR)
        .pt(1.0)
        .pb(72.0)
        .gap(16.0)
        .items_center()
        .children(snap.pages.iter().enumerate().map(|(i, page)| page_row(p, id, i, page, &snap, labels, width)));
    let scroller = col()
        .key("office-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_xy()
        .on_layout_tx(move |_: &mut H, rect: [f32; 4], tx: &mut Tx| {
            if !registry::measure(id, rect[2]) {
                tx.live_only();
            }
        })
        .on_scroll_offset(move |at: ScrollPos| near(id, at))
        .child(column);
    if width > 0.0 {
        near(id, ScrollPos::default());
    }
    let disabled = width <= 0.0 || snap.pages.is_empty();
    let controls = row()
        .key("office-preview-controls")
        .role("group")
        .label(labels.controls.clone())
        .pin(None, None, Some(12.0), Some(12.0))
        .padding(4.0)
        .center_y()
        .radius(8.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .border(0.5, p.border)
        .child(control::<H>(p, id, "office-zoom-in", &labels.zoom_in, &rune_theme_opencode::icons::OUTLINE_MAGNIFIER_PLUS, disabled || zoom >= MAX_ZOOM, |z| (z * ZOOM_STEP).min(MAX_ZOOM)))
        .child(divider(p))
        .child(control::<H>(p, id, "office-zoom-out", &labels.zoom_out, &rune_theme_opencode::icons::OUTLINE_MAGNIFIER_MINUS, disabled || zoom <= MIN_ZOOM, |z| (z / ZOOM_STEP).max(MIN_ZOOM)))
        .child(divider(p))
        .child(control::<H>(p, id, "office-zoom-fit", &labels.fit, &rune_theme_opencode::icons::OUTLINE_EXPAND, disabled, |_| 1.0));
    // `artifact-stage`: the pages sit on the layer under the pane.
    el().key("office-stage").w_full().grow(1.0).min_h(0.0).fill(p.layer1).child(scroller).child(controls)
}

/// One page or slide: its surface, and under a slide its Hidden badge and notes.
fn page_row(p: &Pal, id: u64, i: usize, page: &office_preview::PageInfo, snap: &Snapshot, labels: &Labels, width: f32) -> El {
    let scale = fit(width, snap.zoom, page.width);
    let (w, h) = (page.width as f32 * scale, page.height as f32 * scale);
    let label = labels.page.replace("{{number}}", &(i + 1).to_string());
    let mut surface = el()
        .key(format!("office-page-{i}"))
        .role("img")
        .label(label)
        .size(w, h)
        .radius(8.0)
        .fill(rune::Color::WHITE)
        .elevate(p.raised)
        .clip_rounded(8.0)
        .child(el().w_full().h_full().effect(registry::page_mark(id, i)));
    // A hidden slide fades the way paper over it would, in every theme.
    if page.hidden {
        surface = surface.child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(rune::Color::WHITE.alpha(0.5)));
    }
    if snap.failed_pages.contains(&i) {
        surface = surface.child(
            row()
                .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .padding(16.0)
                .center()
                .child(text(labels.failed.clone()).font(NOTE).color(p.muted).role("status").fill(p.bg_base).radius(6.0).px(12.0).py(6.0).elevate(p.raised)),
        );
    }
    // The border paints above the page's pixels, as `document-surface.css`'s `::after` does.
    surface = surface.child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).radius(8.0).border(0.5, p.border));
    let mut entry = col().key(format!("office-row-{i}")).gap(12.0).items_center().child(surface);
    if page.hidden || !page.notes.is_empty() {
        let mut foot = col().w(w).gap(6.0).items_start();
        if page.hidden {
            foot = foot.child(text(labels.hidden.clone()).font(BADGE).color(p.muted).radius(4.0).border(1.0, p.border).px(6.0));
        }
        if !page.notes.is_empty() {
            foot = foot.child(text(page.notes.clone()).font(NOTE).color(p.muted).wrap().wrap_anywhere().selectable(true).role("note").label(labels.notes.clone()).w_full());
        }
        entry = entry.child(foot);
    }
    entry
}

/// The pages a viewport either side of the one shown (Desktop's observer, `rootMargin: 100%`).
fn near(id: u64, at: ScrollPos) {
    let Some(snap) = registry::snapshot(id) else { return };
    if snap.width <= 0.0 || snap.pages.is_empty() {
        return;
    }
    let viewport = if at.viewport[1] > 0.0 { at.viewport[1] } else { 900.0 };
    let (top, bottom) = (at.offset[1] - viewport, at.offset[1] + viewport * 2.0);
    let mut y = 1.0;
    let (mut from, mut to) = (usize::MAX, 0);
    for (i, page) in snap.pages.iter().enumerate() {
        let h = page.height as f32 * fit(snap.width, snap.zoom, page.width);
        if y + h >= top && y <= bottom {
            from = from.min(i);
            to = i + 1;
        }
        y += h + 16.0;
    }
    if from < to {
        registry::show_pages(id, from, to);
    }
}

/// A large ghost-muted `IconButton` with its tooltip above (`preview-controls.tsx`).
fn control<H: 'static>(p: &Pal, id: u64, key: &'static str, label: &str, icon: &'static rune::Icon, disabled: bool, to: fn(f32) -> f32) -> El {
    let target = if opencode_ui::style::touch_controls() { 44.0 } else { 28.0 };
    let b = row().key(key).role("button").label(label.to_owned()).help(label.to_owned()).help_above().size(target, target).radius(6.0).center().child(rune::icon(icon, 16.0).color(p.icon_muted));
    if disabled {
        return b.layer_opacity(0.5).disabled(true);
    }
    b.hover(p.hover).press(p.pressed).hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT).on_press_tx(move |_: &mut H, tx: &mut Tx| {
        tx.instant();
        registry::zoom(id, to);
    })
}

fn divider(p: &Pal) -> El {
    el().w(0.5).h(20.0).mx(4.0).fill(p.border)
}
