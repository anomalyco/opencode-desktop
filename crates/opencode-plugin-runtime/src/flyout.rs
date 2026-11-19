//! The nested popover a popover's `ui.flyoutRow` opens beside its row (Session details'
//! Extensions lists), as Desktop's `flyout-machine.ts`: the pointer coming onto a row opens a
//! closed flyout, and moving across the rows switches the list at once, except while the
//! pointer heads for the open list across another row (inside the safe triangle from its last
//! step to the list's near edge): then the row under it takes over only once the pointer rests
//! there for [`AIM_TIMEOUT`].

use rune_luau::Data;

/// How long a pointer may rest inside the safe triangle before the row under it takes over
/// (`AIM_TIMEOUT_MS`).
pub const AIM_TIMEOUT: f64 = 0.1;

/// A row and the list it opens: popover `id` with `data`, hung on `side` of row key `row`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub row: String,
    pub side: rune::Side,
    pub data: Data,
}

/// `FlyoutState`. Each aim at another row takes the next attempt number, so a timer whose
/// attempt the state no longer holds is stale; `issued` is the last one handed out.
#[derive(Clone, Debug, PartialEq)]
pub enum Flyout {
    Closed { issued: u32 },
    Open { list: Row, issued: u32 },
    /// The pointer heads for `list` across row `target`, which takes over at `due`.
    Aiming { list: Row, target: Row, attempt: u32, due: f64 },
}

impl Default for Flyout {
    fn default() -> Self {
        Flyout::Closed { issued: 0 }
    }
}

pub enum FlyoutEvent {
    /// The pointer came onto a row (`row.entered`): opens a closed flyout.
    Entered(Row),
    /// A click on a row (`row.activated`): its list shows at once.
    Activated(Row),
    /// The pointer moved over a row (`row.moved`) at `now`; `aiming`: it heads for the open list.
    Moved { row: Row, aiming: bool, now: f64 },
    /// The pointer left the rows or reached the list (`aim.cancelled`, a step between rows).
    Settled,
    /// The aim's timer came due (`aim.due`).
    AimDue { attempt: u32 },
    /// A footer action, Escape, or the popover closing (`close`, `hidden`).
    Close,
}

/// What a step asks for: whether anything shown changed, and when to wake for the aim's timer.
#[derive(Default)]
pub struct FlyoutEffect {
    pub changed: bool,
    pub wake_at: Option<f64>,
}

impl Flyout {
    /// `transitionFlyout`.
    pub fn step(self, event: FlyoutEvent) -> (Flyout, FlyoutEffect) {
        let shown = self.row().map(str::to_owned);
        let (next, wake_at) = match (self, event) {
            (Flyout::Closed { issued }, FlyoutEvent::Entered(list) | FlyoutEvent::Activated(list)) => (Flyout::Open { list, issued }, None),
            (state @ Flyout::Closed { .. }, _) => (state, None),
            (state, FlyoutEvent::Close) => (Flyout::Closed { issued: state.issued() }, None),
            (state, FlyoutEvent::Settled) => (state.settle(), None),
            (state, FlyoutEvent::Entered(_)) => (state, None),
            (state, FlyoutEvent::Activated(row)) => (state.show(row), None),
            (Flyout::Aiming { target, attempt, .. }, FlyoutEvent::AimDue { attempt: due }) if attempt == due => (Flyout::Open { list: target, issued: attempt }, None),
            (state, FlyoutEvent::AimDue { .. }) => (state, None),
            (state, FlyoutEvent::Moved { row, .. }) if state.row() == Some(row.row.as_str()) => (state.settle(), None),
            (state, FlyoutEvent::Moved { row, aiming: true, now }) => {
                let due = now + AIM_TIMEOUT;
                (state.aim(row, due), Some(due))
            }
            (state, FlyoutEvent::Moved { row, aiming: false, .. }) => (state.show(row), None),
        };
        let changed = next.row() != shown.as_deref();
        (next, FlyoutEffect { changed, wake_at })
    }

    /// The row whose list shows.
    pub fn row(&self) -> Option<&str> {
        self.list().map(|l| l.row.as_str())
    }

    /// The list on show.
    pub fn list(&self) -> Option<&Row> {
        match self {
            Flyout::Open { list, .. } | Flyout::Aiming { list, .. } => Some(list),
            Flyout::Closed { .. } => None,
        }
    }

    /// When an aim's timer comes due, with its attempt.
    pub fn due(&self) -> Option<(u32, f64)> {
        match self {
            Flyout::Aiming { attempt, due, .. } => Some((*attempt, *due)),
            _ => None,
        }
    }

    fn issued(&self) -> u32 {
        match self {
            Flyout::Closed { issued } | Flyout::Open { issued, .. } => *issued,
            Flyout::Aiming { attempt, .. } => *attempt,
        }
    }

    /// The pointer gave up its aim: the list on show stays.
    fn settle(self) -> Flyout {
        match self {
            Flyout::Aiming { list, attempt, .. } => Flyout::Open { list, issued: attempt },
            state => state,
        }
    }

    fn show(self, row: Row) -> Flyout {
        let issued = self.issued();
        Flyout::Open { list: row, issued }
    }

    /// Each move keeps the aim alive for another timeout; a new target takes a new attempt.
    fn aim(self, target: Row, due: f64) -> Flyout {
        match self {
            Flyout::Aiming { list, target: t, attempt, .. } if t.row == target.row => Flyout::Aiming { list, target, attempt, due },
            Flyout::Aiming { list, attempt, .. } => Flyout::Aiming { list, target, attempt: attempt + 1, due },
            Flyout::Open { list, issued } => Flyout::Aiming { list, target, attempt: issued + 1, due },
            closed => closed,
        }
    }
}

/// Whether `point` lies inside the triangle `a`, `b`, `c` (`insideTriangle`).
pub fn inside_triangle(point: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let side = |p: [f32; 2], q: [f32; 2]| (q[0] - p[0]) * (point[1] - p[1]) - (q[1] - p[1]) * (point[0] - p[0]);
    let sides = [side(a, b), side(b, c), side(c, a)];
    sides.iter().all(|v| *v >= 0.0) || sides.iter().all(|v| *v <= 0.0)
}

/// The pointer's two latest positions at least 2 points apart, which give its heading, and
/// where the rows and the open list were laid out, all in layout points (the app's zoom is a
/// visual scale above them). None of it is drawn, so none of it runs a view.
#[derive(Default)]
pub struct Aim {
    pub trail: std::cell::Cell<[f32; 4]>,
    pub list: std::cell::Cell<Option<[f32; 4]>>,
    pub rows: std::cell::RefCell<std::collections::HashMap<String, [f32; 4]>>,
}

impl Aim {
    /// The point `local` inside row `row`, in layout points.
    pub fn point(&self, row: &str, local: [f32; 2]) -> Option<[f32; 2]> {
        self.rows.borrow().get(row).map(|r| [r[0] + local[0], r[1] + local[1]])
    }

    /// Notes a pointer step to `point` (window points).
    pub fn step(&self, point: [f32; 2]) {
        let [x, y, _, _] = self.trail.get();
        if (point[0] - x).hypot(point[1] - y) >= 2.0 {
            self.trail.set([point[0], point[1], x, y]);
        }
    }

    /// Whether the pointer at `point` heads for the open list, hung on `side` of its row: inside
    /// the triangle from its last step to the list's near edge, 8 past its top and its foot.
    pub fn heading(&self, point: [f32; 2], side: rune::Side) -> bool {
        let Some([x, y, w, h]) = self.list.get() else { return false };
        let [_, _, px, py] = self.trail.get();
        let edge = if side == rune::Side::Left { x + w } else { x };
        inside_triangle(point, [px, py], [edge, y - 8.0], [edge, y + h + 8.0])
    }
}

/// What a plugin shows over the window: nothing, a popover it placed (with the list a
/// `ui.flyoutRow` in it opened beside it), or a composer pill's menu (under the host's heading,
/// above its pill). One at a time: opening one closes the other.
#[derive(Default)]
pub enum Overlay {
    #[default]
    Closed,
    Popover {
        plugin: std::rc::Rc<str>,
        id: String,
        anchor: String,
        side: rune::Side,
        align: rune::Align,
        gap: f32,
        data: Data,
        flyout: Box<Flyout>,
        back: Option<String>,
    },
    PillMenu {
        plugin: std::rc::Rc<str>,
        id: String,
        anchor: String,
        data: Data,
    },
}

impl Overlay {
    /// Whether plugin `plugin`'s popover `id` is open, on its own or as a row's list.
    pub fn popover_open(&self, plugin: &str, id: &str) -> bool {
        match self {
            Overlay::Popover { plugin: p, id: i, flyout, .. } if &**p == plugin => {
                i == id || flyout.list().is_some_and(|l| l.id == id)
            }
            _ => false,
        }
    }

    /// The row of plugin `plugin`'s open popover whose list shows beside it.
    pub fn flyout_row(&self, plugin: &str) -> Option<&str> {
        match self {
            Overlay::Popover { plugin: p, flyout, .. } if &**p == plugin => flyout.row(),
            _ => None,
        }
    }

    /// The list the open popover's flyout shows, whoever's popover it is.
    pub fn flyout_list(&self) -> Option<&Row> {
        match self {
            Overlay::Popover { flyout, .. } => flyout.list(),
            _ => None,
        }
    }

    /// The open popover's flyout waits on the pointer's aim.
    pub fn flyout_due(&self) -> bool {
        matches!(self, Overlay::Popover { flyout, .. } if flyout.due().is_some())
    }

    /// Whether plugin `plugin`'s pill `id` has its menu open.
    pub fn pill_open(&self, plugin: &str, id: &str) -> bool {
        matches!(self, Overlay::PillMenu { plugin: p, id: i, .. } if &**p == plugin && i == id)
    }

    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::Closed)
    }

    /// Closes what plugin `plugin` shows (it stopped).
    pub fn close_for(&mut self, plugin: &str) {
        if matches!(self, Overlay::Popover { plugin: p, .. } | Overlay::PillMenu { plugin: p, .. } if &**p == plugin) {
            *self = Overlay::Closed;
        }
    }
}
