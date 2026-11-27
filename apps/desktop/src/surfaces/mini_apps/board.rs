//! The dashboard's tiles and its edit mode, as one explicit machine: viewing (the widgets take
//! input), editing (tiles move and resize, the widgets don't take input), and within editing a
//! tile moving or resizing under the pointer. While a tile moves or resizes, the others make
//! room for it live ([`grid::arrange`], from where they were when it was picked up), so the
//! gesture can come back and they go back; on release it snaps into its target, and Escape puts
//! all back.

use super::grid::{self, Metrics, Tile};

/// The dashboard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// Where each tile is (during a gesture: where each is to be, the moving one at its target).
    pub tiles: Vec<Tile>,
    pub mode: Mode,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Mode {
    /// The widgets take input.
    #[default]
    Viewing,
    /// Tiles can be moved and resized; the widgets don't take input.
    Editing,
    /// Editing, tile `key` under the pointer.
    Moving(Gesture),
    Resizing(Gesture),
}

/// A tile picked up: what it and the others were, and how far the pointer went.
#[derive(Clone, Debug, PartialEq)]
pub struct Gesture {
    pub key: String,
    /// The tile as it was picked up.
    pub from: Tile,
    /// Every tile as it was then.
    pub base: Vec<Tile>,
    /// How far the pointer went since, in points.
    pub d: [f32; 2],
}

impl Mode {
    pub fn editing(&self) -> bool {
        !matches!(self, Mode::Viewing)
    }

    /// The tile under the pointer, if any.
    pub fn gesture(&self) -> Option<&Gesture> {
        match self {
            Mode::Moving(g) | Mode::Resizing(g) => Some(g),
            Mode::Viewing | Mode::Editing => None,
        }
    }
}

pub enum Event {
    /// The layout read from storage, or tiles rekeyed to the widgets that replaced theirs.
    Load(Vec<Tile>),
    /// Edit.
    Edit,
    /// Done: leaves edit mode, keeping the layout.
    Done,
    /// The pointer went down on tile `key`'s body (move) or its corner (resize).
    Press { key: String, resize: bool },
    /// The pointer moved `d` from where it went down, over a grid of `metrics`.
    Drag { d: [f32; 2], metrics: Metrics },
    /// The pointer came up: the tile snaps into its target.
    Release,
    /// Escape, or the pointer taken away: all back as before the gesture.
    Cancel,
    /// A widget added (from the list, or "Move to dashboard"), at the first free spot.
    Add(String),
    Remove(String),
}

/// What the host does after a step.
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// The layout changed for good: keep it.
    Save,
}

pub fn step(b: &mut Board, ev: Event) -> Option<Effect> {
    match (std::mem::take(&mut b.mode), ev) {
        (mode, Event::Load(tiles)) => {
            b.tiles = tiles;
            b.mode = match mode {
                Mode::Moving(_) | Mode::Resizing(_) | Mode::Editing => Mode::Editing,
                Mode::Viewing => Mode::Viewing,
            };
            None
        }
        (Mode::Viewing, Event::Edit) => {
            b.mode = Mode::Editing;
            None
        }
        // Done mid-gesture: the tile lands where it was going.
        (Mode::Editing | Mode::Moving(_) | Mode::Resizing(_), Event::Done) => Some(Effect::Save),
        (Mode::Editing, Event::Press { key, resize }) => {
            let Some(from) = b.tiles.iter().find(|t| t.key == key).cloned() else {
                b.mode = Mode::Editing;
                return None;
            };
            let g = Gesture { key, from, base: b.tiles.clone(), d: [0.0, 0.0] };
            b.mode = if resize { Mode::Resizing(g) } else { Mode::Moving(g) };
            None
        }
        (Mode::Moving(mut g), Event::Drag { d, metrics }) => {
            g.d = d;
            b.tiles = grid::arrange(&g.base, &metrics.moved(&g.from, d));
            b.mode = Mode::Moving(g);
            None
        }
        (Mode::Resizing(mut g), Event::Drag { d, metrics }) => {
            g.d = d;
            b.tiles = grid::arrange(&g.base, &metrics.resized(&g.from, d));
            b.mode = Mode::Resizing(g);
            None
        }
        (Mode::Moving(g) | Mode::Resizing(g), Event::Release) => {
            b.mode = Mode::Editing;
            (b.tiles != g.base).then_some(Effect::Save)
        }
        (Mode::Moving(g) | Mode::Resizing(g), Event::Cancel) => {
            b.tiles = g.base;
            b.mode = Mode::Editing;
            None
        }
        (mode, Event::Add(key)) => {
            b.mode = mode;
            if b.tiles.iter().any(|t| t.key == key) {
                return None;
            }
            b.tiles = grid::add(&b.tiles, &key);
            Some(Effect::Save)
        }
        (mode, Event::Remove(key)) => {
            // Removing the tile under the pointer ends its gesture.
            b.mode = match mode {
                Mode::Moving(g) | Mode::Resizing(g) if g.key == key => Mode::Editing,
                mode => mode,
            };
            let before = b.tiles.len();
            b.tiles = grid::remove(&b.tiles, &key);
            (b.tiles.len() != before).then_some(Effect::Save)
        }
        (mode, _) => {
            b.mode = mode;
            None
        }
    }
}
