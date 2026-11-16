//! Dragging side, split and bottom tabs between panes (`runtime/plugin/pane-drag-machine.ts`,
//! `pane-drag.tsx`). A plain state machine: a press on a tab arms it, 4 pt of travel drags it,
//! and the release drops it on the zone under the pointer: between two tabs of a strip, on a
//! pane, on the lower half of a lone side pane (which splits it), or on the bottom quarter of the
//! chat (which opens the bottom pane). Escape, a lost window and a session switch cancel it.

use std::cell::RefCell;

use rune::{El, Tx, el, text};

use super::{SESSION_PANES, SessionPane};
use crate::App;
use crate::palette::{Elevate as _, Pal};

/// How far the pointer moves from the press before the tab drags; a shorter press is a click.
pub const DRAG_THRESHOLD: f32 = 4.0;

/// `[left, top, width, height]` in window points.
pub type Rect = [f32; 4];

/// Where a dragged tab lands: before a tab of a pane's strip (its end without `before`), or in
/// a pane. `rect` is what the layer draws: the strip's insertion bar or the pane's zone.
#[derive(Clone, Debug, PartialEq)]
pub struct DropTarget {
    pub pane: SessionPane,
    pub before: Option<String>,
    pub strip: bool,
    pub rect: Rect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaneDragState {
    #[default]
    Idle,
    /// The button is down on a tab, but hasn't moved far enough to drag it.
    Pressed { session: String, key: String, title: String, origin: [f32; 2] },
    Dragging { session: String, key: String, title: String, at: [f32; 2], target: Option<DropTarget> },
}

#[derive(Clone, Debug)]
pub enum PaneDragEvent {
    Down { session: String, key: String, title: String, at: [f32; 2] },
    Move { at: [f32; 2], target: Option<DropTarget> },
    Up,
    /// Escape, the window losing focus, or the pointer taken away.
    Cancel,
    /// The screen shows another session, whose tabs the release must not move.
    SessionChanged,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaneDragEffect {
    /// Swallows the click the release over a strip would land on the tab under it.
    SuppressClick,
    Drop { key: String, target: DropTarget },
}

impl PaneDragState {
    pub fn transition(&self, event: PaneDragEvent) -> (PaneDragState, Vec<PaneDragEffect>) {
        match (self, event) {
            (Self::Idle, PaneDragEvent::Down { session, key, title, at }) => (Self::Pressed { session, key, title, origin: at }, vec![]),
            (Self::Pressed { session, key, title, origin }, PaneDragEvent::Move { at, target }) => {
                if (at[0] - origin[0]).hypot(at[1] - origin[1]) < DRAG_THRESHOLD {
                    return (self.clone(), vec![]);
                }
                (Self::Dragging { session: session.clone(), key: key.clone(), title: title.clone(), at, target }, vec![])
            }
            (Self::Pressed { .. }, PaneDragEvent::Up | PaneDragEvent::Cancel | PaneDragEvent::SessionChanged) => (Self::Idle, vec![]),
            (Self::Dragging { session, key, title, .. }, PaneDragEvent::Move { at, target }) => {
                (Self::Dragging { session: session.clone(), key: key.clone(), title: title.clone(), at, target }, vec![])
            }
            (Self::Dragging { key, target, .. }, PaneDragEvent::Up) => {
                let mut effects = vec![PaneDragEffect::SuppressClick];
                if let Some(target) = target {
                    effects.push(PaneDragEffect::Drop { key: key.clone(), target: target.clone() });
                }
                (Self::Idle, effects)
            }
            (Self::Dragging { .. }, PaneDragEvent::Cancel | PaneDragEvent::SessionChanged) => (Self::Idle, vec![]),
            (state, _) => (state.clone(), vec![]),
        }
    }

    /// The dragged tab's key while a drag shows.
    pub fn dragging(&self) -> Option<&str> {
        match self {
            Self::Dragging { key, .. } => Some(key),
            _ => None,
        }
    }

    /// The tab a move hit-tests drop zones for: the pressed or dragged one.
    fn key(&self) -> Option<&str> {
        match self {
            Self::Pressed { key, .. } | Self::Dragging { key, .. } => Some(key),
            Self::Idle => None,
        }
    }

    fn session(&self) -> Option<&str> {
        match self {
            Self::Pressed { session, .. } | Self::Dragging { session, .. } => Some(session),
            Self::Idle => None,
        }
    }
}

/// The drop zones the view laid out last, for the drag's hit tests (its `data-pane-strip`,
/// `data-pane-tab`, `data-pane-content` and `data-pane-row` marks).
#[derive(Clone, Debug, Default)]
pub struct Zones {
    /// Each pane's strip on screen, and its tabs in order.
    pub strips: Vec<(SessionPane, Rect, Vec<(String, Rect)>)>,
    pub contents: Vec<(SessionPane, Rect)>,
    pub chat: Option<Rect>,
    pub row: Option<Rect>,
    /// The panes that hold tabs; a pane without them is made by dropping on its zone.
    pub shown: Vec<SessionPane>,
}

/// The machine and the zones it hit-tests.
#[derive(Default)]
pub struct PaneDrag {
    pub state: PaneDragState,
    pub zones: RefCell<Zones>,
    suppress_click: bool,
}

impl PaneDrag {
    pub fn dragging(&self) -> Option<&str> {
        self.state.dragging()
    }

    pub fn active(&self) -> bool {
        !matches!(self.state, PaneDragState::Idle)
    }

    /// Whether the press that just landed ends a drag rather than clicking; clears it.
    pub fn take_suppressed(&mut self) -> bool {
        std::mem::take(&mut self.suppress_click)
    }
}

fn inside(r: Rect, [x, y]: [f32; 2]) -> bool {
    r[2] > 0.0 && r[3] > 0.0 && x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3]
}

/// The zone under `at` for tab `key` (`hit` in `pane-drag.tsx`).
fn hit(z: &Zones, at: [f32; 2], key: &str) -> Option<DropTarget> {
    if let Some((pane, bounds, tabs)) = z.strips.iter().find(|(_, r, _)| inside(*r, at)) {
        let tabs: Vec<&(String, Rect)> = tabs.iter().filter(|(k, _)| k != key).collect();
        let next = tabs.iter().find(|(_, r)| at[0] < r[0] + r[2] / 2.0);
        let anchor = next.or(tabs.last()).map(|(_, r)| *r);
        let rect = match anchor {
            Some(a) => {
                let edge = if next.is_some() { a[0] } else { a[0] + a[2] };
                [(edge - 1.0).max(bounds[0]).min(bounds[0] + bounds[2] - 2.0), a[1] + 2.0, 2.0, a[3] - 4.0]
            }
            None => [bounds[0] + 12.0, bounds[1] + 12.0, 2.0, bounds[3] - 24.0],
        };
        return Some(DropTarget { pane: *pane, before: next.map(|(k, _)| k.clone()), strip: true, rect });
    }
    if let Some((pane, r)) = z.contents.iter().find(|(_, r)| inside(*r, at)) {
        if *pane == SessionPane::Side && !z.shown.contains(&SessionPane::Split) && at[1] > r[1] + r[3] / 2.0 {
            return Some(DropTarget { pane: SessionPane::Split, before: None, strip: false, rect: [r[0], r[1] + r[3] / 2.0, r[2], r[3] / 2.0] });
        }
        return Some(DropTarget { pane: *pane, before: None, strip: false, rect: *r });
    }
    let (chat, row) = (z.chat?, z.row?);
    if z.shown.contains(&SessionPane::Bottom) || !inside(chat, at) || at[1] < chat[1] + chat[3] - chat[3] / 4.0 {
        return None;
    }
    let h = 280.0f32.min(row[3] / 3.0);
    Some(DropTarget { pane: SessionPane::Bottom, before: None, strip: false, rect: [row[0], row[1] + row[3] - h, row[2], h] })
}

impl App {
    /// Steps the machine; true when what it draws changed (a drag shows, moves, drops or ends).
    fn dispatch_pane_drag(&mut self, event: PaneDragEvent, tx: &mut Tx) -> bool {
        let was = self.side.pane_drag.state.dragging().is_some();
        let (next, effects) = self.side.pane_drag.state.transition(event);
        self.side.pane_drag.state = next;
        let drawn = was || self.side.pane_drag.state.dragging().is_some() || !effects.is_empty();
        for effect in effects {
            match effect {
                PaneDragEffect::SuppressClick => self.side.pane_drag.suppress_click = true,
                PaneDragEffect::Drop { key, target } => {
                    self.move_side_tab(&key, target.pane, target.before.as_deref(), tx);
                    self.side.active_pane_focus = target.pane;
                }
            }
        }
        if drawn {
            tx.instant();
        }
        drawn
    }

    /// A press on a side, split or bottom tab, from its `on_drag`. A mouse selects the tab as
    /// its button goes down (`Tabs.Trigger`'s Kobalte `onPointerDown`,
    /// `create-selectable-item.ts`), so the release's click is the press's and draws nothing; a
    /// finger selects on the tap. `shown`: the tab is already selected in the pane that has the
    /// keyboard, where a press changes nothing. A draggable tab's press also arms its drag.
    pub fn side_tab_gesture(&mut self, key: &str, title: &str, draggable: bool, shown: bool, d: &rune::Drag, tx: &mut Tx) {
        let at = d.window_point();
        let session = self.active_session_id().unwrap_or_default().to_owned();
        let drawn = match d.phase {
            rune::Phase::Down => {
                let selects = !session_view::style::touch_controls();
                self.side.pane_drag.suppress_click = selects;
                let drawn = draggable && self.dispatch_pane_drag(PaneDragEvent::Down { session, key: key.to_owned(), title: title.to_owned(), at }, tx);
                if selects && !shown {
                    self.select_side_tab(key, tx);
                }
                drawn || (selects && !shown)
            }
            rune::Phase::Move if !draggable => false,
            rune::Phase::Move if self.side.pane_drag.state.session().is_some_and(|s| s != session) => self.dispatch_pane_drag(PaneDragEvent::SessionChanged, tx),
            rune::Phase::Move => {
                let target = self.side.pane_drag.state.key().and_then(|k| hit(&self.side.pane_drag.zones.borrow(), at, k));
                self.dispatch_pane_drag(PaneDragEvent::Move { at, target }, tx)
            }
            rune::Phase::Up => {
                // Released off the tab or cancelled: no click follows to swallow.
                if d.cancelled() || !((0.0..=d.w).contains(&d.x) && (0.0..=d.h).contains(&d.y)) {
                    self.side.pane_drag.suppress_click = false;
                }
                let event = if d.cancelled() { PaneDragEvent::Cancel } else { PaneDragEvent::Up };
                draggable && self.dispatch_pane_drag(event, tx)
            }
        };
        if !drawn {
            tx.live_only();
        }
    }
}
/// Notes where the drop zones lie while a tab drags, from the last layout.
pub fn note_zones(s: &App) {
    if !s.side.pane_drag.active() {
        return;
    }
    let rect = |key: &str| rune::memo::layout(key);
    let reg = s.resolve_side_region();
    // The session row: the chat panel to the side region's end, 8 in from the window's.
    let chat = rect("panel");
    let row = chat.zip(rect("window")).map(|(c, w)| [c[0], c[1], w[0] + w[2] - 8.0 - c[0], c[3]]);
    let mut z = Zones { chat, row, ..Zones::default() };
    for pane in SESSION_PANES {
        let slug = pane.name();
        let entries = reg.as_ref().map(|r| r.pane_entries(pane)).unwrap_or_default();
        if !entries.is_empty() {
            z.shown.push(pane);
        }
        let visible = match pane {
            SessionPane::Bottom => s.bottom_open(),
            _ => s.side_open(),
        };
        if !visible {
            continue;
        }
        if let Some(strip) = rect(&format!("side-tabs-{slug}")) {
            let tabs = entries.iter().filter_map(|e| Some((e.key.clone(), rect(&format!("side-tab-{}", super::view::safe_id(&e.key)))?))).collect();
            z.strips.push((pane, strip, tabs));
        }
        if let Some(content) = rect(&format!("pane-{slug}")) {
            z.contents.push((pane, content));
        }
    }
    *s.side.pane_drag.zones.borrow_mut() = z;
}

/// The floating layer a drag draws (`pane-drag-layer`): the drop target's bar or zone, and the
/// dragged tab's title by the pointer.
pub fn layer(s: &App, p: &Pal) -> Option<El> {
    let PaneDragState::Dragging { title, at, target, .. } = &s.side.pane_drag.state else { return None };
    let accent = p.v2.icon_accent;
    let mut layer = el().key("pane-drag-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).inert();
    if let Some(t) = target {
        let [x, y, w, h] = t.rect;
        let mark = el().key(if t.strip { "pane-drop-indicator" } else { "pane-drop-zone" }).absolute(x, y).size(w, h);
        let mark = if t.strip {
            mark.radius(w.min(h) / 2.0).fill(accent)
        } else {
            mark.radius(10.0).border(1.0, accent).fill(accent.alpha(0.12))
        };
        layer = layer.child(mark);
    }
    let chip = el()
        .key("pane-drag-title")
        .absolute(at[0] + 12.0, at[1] + 12.0)
        .max_w(240.0)
        .h(28.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(p.bg_base)
        .elevate(p.raised)
        .opacity(0.9)
        .child(text(title.clone()).font(rune::TextStyle::new(13.0, 400.0).lh(16.0)).color(p.text).truncate());
    Some(layer.child(chip))
}
