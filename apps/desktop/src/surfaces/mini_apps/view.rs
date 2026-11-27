//! The Mini apps page: its header (the view, Edit and Done), the dashboard's grid and the list of
//! all the server's widgets. Every widget draws through the widget glue's own frame
//! (`opencode_feature_widgets::instance_view`), as the timeline draws it.

use std::rc::Rc;

use opencode_i18n::t;
use opencode_kit::model::{WidgetInstance, WidgetStatus};
use opencode_ui::widgets::{BODY, LABEL};
use opencode_ui::scroller::scroller;
use rune::{Drag, El, Phase, Spring, TextStyle, Tx, col, el, icon, row, text};
use rune_kit::{ButtonVariant, SegmentedControl};
use rune_theme_opencode::icons as ic;

use super::board::{self, Mode};
use super::grid::{self, Metrics, Tile};
use super::{MINI_APPS, View};
use crate::App;
use crate::palette::{Pal, kit};
use crate::session::widgets::WidgetsHost as _;

/// Tiles move to their places on this spring; the one under the pointer follows it at once.
const PLACE: Spring = Spring::new(0.28, 0.0);
/// A tile's caption above its widget.
const CAPTION: f32 = 24.0;
/// How tall a widget shows in the list, at most.
const PREVIEW_H: f32 = 320.0;
/// The list's width, at most (the timeline's measure).
const LIST_W: f32 = 760.0;
const SMALL: TextStyle = TextStyle::new(12.0, 440.0).lh(16.0).tracking(-0.04);

/// The page, filling the panel.
pub fn page(s: &App, p: &Pal) -> El {
    let view = super::shown(s).unwrap_or(View::Dashboard);
    col()
        .key("mini-apps-page")
        .role("region")
        .label(t!("rune.miniApps.title").to_owned())
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(header(s, p, view))
        .child(match view {
            View::Dashboard => dashboard(s, p),
            View::All => list(s, p),
        })
}

fn header(s: &App, p: &Pal, view: View) -> El {
    let editing = s.kernel.slots.get(&MINI_APPS).board.mode.editing();
    let views = [t!("rune.miniApps.dashboard"), t!("rune.miniApps.all")];
    let picker = SegmentedControl::new(&views, Some(if view == View::Dashboard { 0 } else { 1 }))
        .label(t!("rune.miniApps.title"))
        .as_tabs()
        .fit()
        .on_pick(|s: &mut App, i, tx: &mut Tx| {
            super::open(s, if i == 0 { View::Dashboard } else { View::All });
            tx.settle();
        })
        .build(kit(p));
    let edit = (view == View::Dashboard && !s.kernel.slots.get(&MINI_APPS).board.tiles.is_empty()).then(|| {
        let (label, variant) = if editing { (t!("rune.miniApps.done"), ButtonVariant::Contrast) } else { (t!("rune.miniApps.edit"), ButtonVariant::Neutral) };
        crate::widgets::button(label, variant)
            .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                super::step(s, if editing { board::Event::Done } else { board::Event::Edit });
                tx.settle();
            })
            .build(kit(p))
            .key("mini-apps-edit")
    });
    row()
        .key("mini-apps-header")
        .w_full()
        .h(48.0)
        .px(20.0)
        .gap(16.0)
        .center_y()
        .window_drag()
        .child(text(t!("rune.miniApps.title")).font(TextStyle::new(15.0, 600.0).lh(20.0).tracking(-0.1)).color(p.text))
        .child(picker)
        .child(el().grow(1.0))
        .children(edit)
}

/// Words in the middle of the page, with an action under them.
fn notice(p: &Pal, key: &str, title: &str, body: Option<&str>, action: Option<El>) -> El {
    col()
        .key(key.to_owned())
        .w_full()
        .grow(1.0)
        .center()
        .gap(8.0)
        .role("status")
        .child(text(title.to_owned()).font(LABEL).color(p.text))
        .children(body.map(|b| text(b.to_owned()).font(BODY).color(p.muted).wrap().max_w(420.0).align(rune::Align::Center)))
        .children(action.map(|a| el().pt(8.0).child(a)))
}

/// Why there's nothing to list or show yet: loading, the plugin isn't there, or none.
fn waiting(s: &App, p: &Pal) -> Option<El> {
    let store = &s.widgets().instances;
    if store.read() {
        return None;
    }
    Some(match &store.fetch {
        opencode_feature_widgets::instances::Fetch::Unavailable => notice(p, "mini-apps-unavailable", t!("rune.miniApps.unavailable"), None, None),
        opencode_feature_widgets::instances::Fetch::Failed(m) => notice(p, "mini-apps-failed", t!("rune.miniApps.failed"), Some(m), None),
        _ => col().key("mini-apps-loading").w_full().grow(1.0).center().child(opencode_ui::widgets::status_row(p, "mini-apps-loading-row", t!("rune.miniApps.loading"))),
    })
}

// ---------------------------------------------------------------------------------------------
// The dashboard.

fn dashboard(s: &App, p: &Pal) -> El {
    let m = s.kernel.slots.get(&MINI_APPS);
    if m.board.tiles.is_empty() {
        let all = crate::widgets::button(t!("rune.miniApps.all"), ButtonVariant::Neutral)
            .on_press_tx(|s: &mut App, tx: &mut Tx| {
                super::open(s, View::All);
                tx.settle();
            })
            .build(kit(p))
            .key("mini-apps-empty-all");
        return notice(p, "mini-apps-empty", t!("rune.miniApps.empty.title"), Some(t!("rune.miniApps.empty.body")), Some(all));
    }
    let width = if m.width > 0.0 { m.width } else { 960.0 };
    let metrics = Metrics::new(width);
    let gesture = m.board.mode.gesture();
    // The tile under the pointer is drawn where the pointer has it, its target as a placeholder.
    let target = gesture.and_then(|g| m.board.tiles.iter().find(|t| t.key == g.key)).map(|t| {
        let [x, y, w, h] = metrics.rect(t);
        el().key("mini-apps-placeholder").pin(Some(x), Some(y), None, None).size(w, h).radius(14.0).fill(p.hover).border(1.0, p.border).motion(PLACE)
    });
    let height = metrics.height(&m.board.tiles).max(gesture.map_or(0.0, |g| {
        let [_, y, _, h] = held(&metrics, g, &m.board.mode);
        y + h
    }));
    let grid = el()
        .key("mini-apps-grid")
        .w_full()
        .h(height + 24.0)
        // Scrolling moves it without changing its width: only a new width builds again.
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            let m = s.kernel.slots.get_mut(&MINI_APPS);
            if (m.width - r[2]).abs() > 0.5 {
                m.width = r[2];
            } else {
                tx.live_only();
            }
        })
        .children(target)
        .children(m.board.tiles.iter().map(|t| tile(s, p, &metrics, t)));
    let viewport = col().w_full().grow(1.0).min_h(0.0);
    scroller("mini-apps-dashboard").build::<App>(p, viewport, col().key("mini-apps-pad").w_full().px(20.0).pt(4.0).child(grid))
}

/// Where the tile under the pointer draws: picked up and moved by the pointer's travel, or its
/// corner dragged, at least the smallest tile.
fn held(metrics: &Metrics, g: &board::Gesture, mode: &Mode) -> [f32; 4] {
    let [x, y, w, h] = metrics.rect(&g.from);
    let min = metrics.rect(&Tile { w: grid::MIN_W, h: grid::MIN_H, ..g.from.clone() });
    match mode {
        Mode::Resizing(_) => [x, y, (w + g.d[0]).max(min[2]), (h + g.d[1]).max(min[3])],
        _ => [(x + g.d[0]).max(0.0), (y + g.d[1]).max(0.0), w, h],
    }
}

fn tile(s: &App, p: &Pal, metrics: &Metrics, t: &Tile) -> El {
    let m = s.kernel.slots.get(&MINI_APPS);
    let editing = m.board.mode.editing();
    let held_here = m.board.mode.gesture().filter(|g| g.key == t.key);
    let [x, y, w, h] = match held_here {
        Some(g) => held(metrics, g, &m.board.mode),
        None => metrics.rect(t),
    };
    let instance = s.widgets().instances.resolve(&t.key);
    let widget = instance.and_then(|_| opencode_feature_widgets::instance_view(s, p, &t.key, w)).unwrap_or_else(|| {
        let words = if s.widgets().instances.read() { t!("rune.miniApps.gone") } else { t!("rune.miniApps.loading") };
        text(words).font(BODY).color(p.muted).padding(16.0)
    });
    let body = scroller(format!("tile-body-{}", t.key)).build::<App>(p, col().w_full().grow(1.0).min_h(0.0), widget);
    let body = if editing { body.inert() } else { body };
    let mut tile = col()
        .key(format!("tile-{}", t.key))
        .pin(Some(x), Some(y), None, None)
        .size(w, h)
        .gap(4.0)
        .motion(if held_here.is_some() { Spring::NONE } else { PLACE })
        .child(caption(p, instance, &t.key, editing))
        .child(body);
    if editing {
        tile = tile.radius(14.0).outline(1.0, 2.0, if held_here.is_some() { p.accent } else { p.border });
        // The whole tile is the handle; its corner resizes it; the cross takes it off.
        let key = t.key.clone();
        tile = tile.child(el().key(format!("move-{key}")).pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).role("button").label(t!("rune.miniApps.move").to_owned()).on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| gesture(s, &key, false, d, tx)));
        let key = t.key.clone();
        tile = tile.child(
            el().key(format!("resize-{key}"))
                .pin(None, None, Some(-4.0), Some(-4.0))
                .size(20.0, 20.0)
                .center()
                .role("button")
                .label(t!("rune.miniApps.resize").to_owned())
                .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| gesture(s, &key, true, d, tx))
                .child(el().size(10.0, 10.0).radius(2.0).border(1.5, p.icon).fill(p.bg_base)),
        );
        let key = t.key.clone();
        tile = tile.child(
            el().key(format!("remove-{key}"))
                .pin(None, Some(0.0), Some(0.0), None)
                .size(24.0, 24.0)
                .radius(6.0)
                .center()
                .hover(p.hover)
                .press(p.pressed)
                .role("button")
                .label(t!("rune.miniApps.remove").to_owned())
                .on_press_tx(move |s: &mut App, tx: &mut Tx| {
                    super::remove_from_dashboard(s, &key);
                    tx.settle();
                })
                .child(icon(&ic::CLOSE_SMALL, 16.0).color(p.icon)),
        );
    }
    if held_here.is_some() { tile.raise() } else { tile }
}

/// A tile's caption: the widget's name and its session's.
fn caption(p: &Pal, i: Option<&WidgetInstance>, key: &str, editing: bool) -> El {
    let (title, session) = match i {
        Some(i) => (i.state.label().to_owned(), i.session_title.clone().unwrap_or_default()),
        None => (key.split(':').next().unwrap_or(key).to_owned(), String::new()),
    };
    row()
        .key("caption")
        .w_full()
        .h(CAPTION)
        .gap(8.0)
        .center_y()
        .pl(4.0)
        .pr(if editing { 28.0 } else { 4.0 })
        .child(text(title).font(LABEL).color(p.text).truncate().shrink())
        .child(text(session).font(SMALL).color(p.faint).truncate().grow(1.0).min_w(0.0))
}

/// The pointer on tile `key`'s body (move) or corner (`resize`), in edit mode.
fn gesture(s: &mut App, key: &str, resize: bool, d: &mut Drag, tx: &mut Tx) {
    let width = s.kernel.slots.get(&MINI_APPS).width;
    let ev = match d.phase {
        Phase::Down => board::Event::Press { key: key.to_owned(), resize },
        Phase::Move => board::Event::Drag { d: [d.dx, d.dy], metrics: Metrics::new(width) },
        Phase::Up if d.cancelled() => board::Event::Cancel,
        Phase::Up => board::Event::Release,
    };
    super::step(s, ev);
    tx.animated();
}

// ---------------------------------------------------------------------------------------------
// All widgets.

fn list(s: &App, p: &Pal) -> El {
    if let Some(w) = waiting(s, p) {
        return w;
    }
    let store = &s.widgets().instances;
    if store.usable().next().is_none() {
        return notice(p, "mini-apps-none", t!("rune.miniApps.none"), None, None);
    }
    // Only the widgets one can still use: none dismissed, none with a recorded failure.
    let keys: Rc<Vec<String>> = Rc::new(store.usable().map(|i| i.state.instance_key()).collect());
    let (by_key, built) = (keys.clone(), keys.clone());
    let pal = *p;
    // The rows are the viewport's own child, as a virtual list needs; the column of `LIST_W`
    // centres by the padding either side of it (the page's width less the dashboard's 20 each side).
    let page = s.kernel.slots.get(&MINI_APPS).width + 40.0;
    let side = ((page - LIST_W) / 2.0).max(20.0);
    let rows = col()
        .key("mini-apps-rows")
        .w_full()
        .px(side)
        .growth(rune::GrowthSpec::default())
        .virtual_rows(keys.len(), PREVIEW_H + 72.0, move |_: &App, i| by_key[i].clone().into(), move |s: &App, i| list_row(s, &pal, &built[i]))
        .virtual_rows_version((store.revision(), s.kernel.slots.get(&MINI_APPS).board.tiles.len()));
    scroller("mini-apps-list").build::<App>(p, col().w_full().grow(1.0).min_h(0.0), rows)
}

fn list_row(s: &App, p: &Pal, key: &str) -> El {
    let Some(i) = s.widgets().instances.resolve(key) else { return el().key(key.to_owned()) };
    let on = super::on_dashboard(s, key);
    let toggle = {
        let id = format!("add-{key}");
        let key = key.to_owned();
        let (label, variant) = if on { (t!("rune.miniApps.added"), ButtonVariant::Ghost) } else { (t!("rune.miniApps.add"), ButtonVariant::Neutral) };
        #[allow(deprecated)]
        let b = crate::widgets::button(label, variant).size(rune_kit::ButtonSize::Small);
        b.on_press_tx(move |s: &mut App, tx: &mut Tx| {
            if super::on_dashboard(s, &key) {
                super::remove_from_dashboard(s, &key);
            } else {
                super::add_to_dashboard(s, &key);
            }
            tx.settle();
        })
        .build(kit(p))
        .key(id)
    };
    let status = match (&i.state.status, &i.state.failure) {
        (WidgetStatus::Open, Some(_)) => t!("rune.miniApps.status.failed"),
        (WidgetStatus::Open, None) => t!("rune.miniApps.status.open"),
        (WidgetStatus::Submitted(_), _) => t!("rune.miniApps.status.submitted"),
        (WidgetStatus::Dismissed, _) => t!("rune.miniApps.status.dismissed"),
        (WidgetStatus::Superseded, _) => t!("rune.miniApps.status.superseded"),
    };
    let age = crate::inbox_model::age(crate::inbox_model::InboxStatus::Failed, Some(i.updated), *s.now_ms).unwrap_or_default();
    let meta = [format!("{}@{}", i.state.name, &i.state.version[..i.state.version.len().min(8)]), i.session_title.clone().unwrap_or_default(), age, status.to_owned()]
        .into_iter()
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let width = s.kernel.slots.get(&MINI_APPS).width.clamp(320.0, LIST_W);
    let widget = opencode_feature_widgets::instance_view(s, p, key, width).unwrap_or_else(el);
    let seen = key.to_owned();
    col()
        .key(key.to_owned())
        .w_full()
        .pt(12.0)
        .pb(12.0)
        .gap(8.0)
        .on_visible(move |s: &mut App, on, tx: &mut Tx| {
            let m = s.kernel.slots.get_mut(&MINI_APPS);
            let moved = if on { m.seen.insert(seen.clone()) } else { m.seen.remove(&seen) };
            if moved {
                tx.wake_at(tx.clock());
            }
        })
        .on_layout_tx(|s: &mut App, r: [f32; 4], tx: &mut Tx| {
            // The dashboard's width stands in for the list's until the dashboard lays out.
            let m = s.kernel.slots.get_mut(&MINI_APPS);
            if m.width == 0.0 {
                m.width = r[2];
            } else {
                tx.live_only();
            }
        })
        .child(
            row()
                .key("head")
                .w_full()
                .gap(12.0)
                .center_y()
                .child(col().grow(1.0).min_w(0.0).gap(2.0).child(text(i.state.label().to_owned()).font(LABEL).color(p.text).truncate()).child(text(meta).font(SMALL).color(p.faint).truncate()))
                .child(toggle),
        )
        .child(el().key("preview").w_full().max_h(PREVIEW_H).clip_soft(0.0).child(widget))
}
