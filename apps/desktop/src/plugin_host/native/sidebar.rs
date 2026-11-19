//! `ui.panelSidebar`: the frame Review and Files share (`SessionReviewV2`, `SessionFilePanelV2`):
//! the file sidebar at the panel's start, resized by a 5 pt handle on its end edge (a double
//! click resets it), the panel's content beside it, and the tree toggle pinned at the panel's
//! top left. Whether the sidebar shows and how wide it is are one preference every panel shares,
//! as Desktop's `PanelSidebar` (`createPanelSidebar`, `review-panel-v2`), which ⌘\ toggles.

use rune::{Cursor, Drag, El, KeyEvent, Phase, Tx, col, el, row};
use rune_luau::NodeArgs;

use super::p;
use crate::App;
use crate::keymap::Bind;
use crate::review::chrome;

/// `SESSION_REVIEW_V2_SIDEBAR_WIDTH_*`.
pub const DEFAULT_W: f32 = 240.0;
pub const MIN_W: f32 = 200.0;
pub const MAX_W: f32 = 480.0;

/// The resize handle's gesture.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Resize {
    #[default]
    Idle,
    /// Dragging, from the width the sidebar had and what the user had set then.
    Dragging { from: f32, was: Option<f32> },
}

/// What moves the handle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResizeEvent {
    Down { width: f32 },
    Move { dx: f32, min: f32, max: f32 },
    Up { cancelled: bool },
    Reset,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelSidebar {
    pub opened: bool,
    /// The width the user dragged it to; the default until then.
    pub width: Option<f32>,
    pub resize: Resize,
}

impl Default for PanelSidebar {
    fn default() -> Self {
        Self { opened: true, width: None, resize: Resize::Idle }
    }
}

impl PanelSidebar {
    /// The preference kept across launches (Desktop's `Persist.global("review-panel-v2")`:
    /// `sidebarOpened`, and `sidebarWidth` within its bounds).
    pub fn load() -> Self {
        let v = crate::storage::json("opencode.global.dat", "review-panel-v2").unwrap_or_default();
        Self {
            opened: v["sidebarOpened"].as_bool().unwrap_or(true),
            width: v["sidebarWidth"].as_f64().map(|w| (w as f32).clamp(MIN_W, MAX_W)),
            resize: Resize::Idle,
        }
    }

    fn save(&self) {
        if !crate::headless() {
            crate::storage::set_json("opencode.global.dat", "review-panel-v2", &serde_json::json!({ "sidebarOpened": self.opened, "sidebarWidth": self.width.unwrap_or(DEFAULT_W) }));
        }
    }

    pub fn toggle(&mut self) {
        self.opened = !self.opened;
        self.save();
    }

    /// Steps the resize gesture; a width it settles on is kept.
    pub fn step(&mut self, event: ResizeEvent) {
        let before = self.width;
        self.step_resize(event);
        if self.resize == Resize::Idle && self.width != before || matches!(event, ResizeEvent::Up { cancelled: false }) {
            self.save();
        }
    }

    fn step_resize(&mut self, event: ResizeEvent) {
        self.resize = match (self.resize, event) {
            (_, ResizeEvent::Down { width }) => Resize::Dragging { from: width, was: self.width },
            (Resize::Dragging { from, was }, ResizeEvent::Move { dx, min, max }) => {
                self.width = Some((from + dx).clamp(min, max));
                Resize::Dragging { from, was }
            }
            (Resize::Dragging { was, .. }, ResizeEvent::Up { cancelled: true }) => {
                self.width = was;
                Resize::Idle
            }
            (_, ResizeEvent::Reset) => {
                self.width = None;
                Resize::Idle
            }
            (_, ResizeEvent::Up { .. }) | (Resize::Idle, ResizeEvent::Move { .. }) => Resize::Idle,
        };
    }
}

fn toggle(s: &mut App, tx: &mut Tx) {
    tx.instant();
    s.workspace_panes.panel_sidebar.toggle();
    crate::plugin_host::bump();
}

/// ⌘\ while a panel with the sidebar shows (Review, a file).
pub fn key(app: &mut App, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if !Bind::TreeToggle.matches_tx(ev, tx) || !(app.is_review_active() || app.active_file_tab().is_some()) {
        return false;
    }
    toggle(app, tx);
    true
}

/// `ui.panelSidebar(props, { sidebar, main })`.
pub fn view(a: &mut NodeArgs) -> Result<El, String> {
    let app = crate::plugin_host::cx::app().ok_or("ui.panelSidebar draws only in a render")?;
    let pal = p(a);
    let id = a.string("id").unwrap_or("panel").to_owned();
    let min = a.number("minWidth").map_or(MIN_W, |n| n as f32);
    let max = a.number("maxWidth").map_or(MAX_W, |n| n as f32).max(min);
    let default = a.number("width").map_or(DEFAULT_W, |n| n as f32);
    let mut children = std::mem::take(&mut a.children).into_iter();
    let (side, main) = (children.next().unwrap_or_else(el), children.next().unwrap_or_else(el));
    let st = &app.workspace_panes.panel_sidebar;
    // Files' Open file launcher keeps the tree open, its toggle disabled.
    let locked = a.flag("locked");
    let opened = st.opened || locked;
    let w = st.width.unwrap_or(default).clamp(min, max);
    let mut body = row().key(format!("{id}-body")).w_full().grow(1.0).min_h(0.0);
    if opened {
        // `session-review-v2-sidebar`: its width holds its 1 pt `border-base` end edge, which takes
        // its room from the contents (`border-right`, border-box), so the tree ends inside it.
        body = body.child(col().key(format!("{id}-sidebar")).w(w).h_full().fill(pal.bg_base).border_sides([0.0, 1.0, 0.0, 0.0], pal.border).border_layout().child(side));
    }
    body = body.child(col().key(format!("{id}-main")).grow(1.0).min_w(0.0).h_full().child(main));
    // The resize handle lies over the edge, above the content beside it.
    if opened {
        let dragging = st.resize != Resize::Idle;
        let resize_inputs = ((pal.theme, pal.generation, pal.dark), w.to_bits(), min.to_bits(), max.to_bits(), dragging);
        body = body.child(rune::memo(format!("{id}-sidebar-resize"), resize_inputs, || handle(&pal, w, min, max, dragging)));
    }
    let toggle_key = crate::plugin_host::icons::intern(&format!("{id}-tree-toggle"));
    Ok(col()
        .key(format!("{id}-panel"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .fill(pal.bg_base)
        .child(body)
        .child(chrome::sidebar_toggle(&pal, toggle_key, opened, locked, toggle)))
}

/// Desktop's `ResizeHandle` over the sidebar's end edge: 8 pt wide, centred on the edge, its
/// 4 × 40 pill (`border-muted`, as the review sidebar colours it) showing under the pointer and
/// while dragging.
fn handle(p: &crate::palette::Pal, width: f32, min: f32, max: f32, dragging: bool) -> El {
    let pill = el().key("pill").size(4.0, 40.0).radius(2.0).fill(p.border_muted);
    let pill = if dragging { pill } else { pill.group_opacity(0.0, 1.0) };
    el().pin(Some(width - 4.0), Some(0.0), None, Some(0.0))
        .w(8.0)
        .center()
        .cursor(Cursor::ResizeColumn)
        .hover_group()
        .hover_motion(rune::Anim::INSTANT, rune::Anim::INSTANT)
        .on_double_press_tx(|s: &mut App, tx: &mut Tx| {
            tx.instant();
            s.workspace_panes.panel_sidebar.step(ResizeEvent::Reset);
        })
        .on_drag(move |s: &mut App, d: &mut Drag, tx: &mut Tx| {
            tx.instant();
            let event = match d.phase {
                Phase::Down => ResizeEvent::Down { width },
                Phase::Move => ResizeEvent::Move { dx: d.dx, min, max },
                Phase::Up => ResizeEvent::Up { cancelled: d.cancelled() },
            };
            s.workspace_panes.panel_sidebar.step(event);
        })
        .child(pill)
}
