//! The Mini apps dashboard's grid, as a tiling window manager keeps it: tiles in whole cells of
//! [`COLS`] columns and rows of [`ROW`] points, never overlapping. A tile moved or resized takes
//! its target; every tile it overlaps goes down below it, and the rest rise as far as they can
//! (gravity), as react-grid-layout's vertical compaction does (`moveElement` + `compact`).

use serde::{Deserialize, Serialize};

/// Columns across the dashboard.
pub const COLS: i32 = 12;
/// A row's height and the gap between cells, in points.
pub const ROW: f32 = 36.0;
pub const GAP: f32 = 12.0;
/// The smallest a tile gets, in cells.
pub const MIN_W: i32 = 2;
pub const MIN_H: i32 = 2;
/// A new tile's size, in cells.
pub const NEW_W: i32 = 4;
pub const NEW_H: i32 = 8;

/// One widget on the dashboard: its key (`sessionID:messageID:callID`) and its cells.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub key: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Tile {
    fn overlaps(&self, o: &Tile) -> bool {
        self.key != o.key && self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// Kept inside the columns, at least the smallest size.
    pub fn clamped(mut self) -> Tile {
        self.w = self.w.clamp(MIN_W, COLS);
        self.h = self.h.max(MIN_H);
        self.x = self.x.clamp(0, COLS - self.w);
        self.y = self.y.max(0);
        self
    }
}

/// The grid's measures for a dashboard `width` points wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// A column's width, in points.
    pub col: f32,
}

impl Metrics {
    pub fn new(width: f32) -> Metrics {
        Metrics { col: ((width - GAP * (COLS - 1) as f32) / COLS as f32).max(8.0) }
    }

    /// Tile `t`'s rect, `[x, y, w, h]` in points.
    pub fn rect(&self, t: &Tile) -> [f32; 4] {
        let (cx, cy) = (self.col + GAP, ROW + GAP);
        [t.x as f32 * cx, t.y as f32 * cy, t.w as f32 * cx - GAP, t.h as f32 * cy - GAP]
    }

    /// Where `from` goes when its top-left moves `d` points: the nearest cells, kept inside.
    pub fn moved(&self, from: &Tile, d: [f32; 2]) -> Tile {
        let [x, y, ..] = self.rect(from);
        Tile { x: ((x + d[0]) / (self.col + GAP)).round() as i32, y: ((y + d[1]) / (ROW + GAP)).round() as i32, ..from.clone() }.clamped()
    }

    /// What `from` becomes when its bottom-right corner moves `d` points.
    pub fn resized(&self, from: &Tile, d: [f32; 2]) -> Tile {
        let [_, _, w, h] = self.rect(from);
        let w = ((w + d[0] + GAP) / (self.col + GAP)).round() as i32;
        let h = ((h + d[1] + GAP) / (ROW + GAP)).round() as i32;
        let t = Tile { w: w.clamp(MIN_W, COLS - from.x), h, ..from.clone() };
        t.clamped()
    }

    /// The height the tiles take, in points.
    pub fn height(&self, tiles: &[Tile]) -> f32 {
        let rows = tiles.iter().map(|t| t.y + t.h).max().unwrap_or(0);
        (rows as f32 * (ROW + GAP) - GAP).max(0.0)
    }
}

/// `tiles` with `moving` at its place: the tiles it overlaps go down below it, and every other
/// tile rises as far as it can without overlapping one placed before it, top to bottom.
pub fn arrange(tiles: &[Tile], moving: &Tile) -> Vec<Tile> {
    let mut placed = vec![moving.clone()];
    let mut rest: Vec<Tile> = tiles.iter().filter(|t| t.key != moving.key).cloned().collect();
    rest.sort_by_key(|t| (t.y, t.x));
    for t in rest {
        let t = settle(&placed, t);
        placed.push(t);
    }
    // In the order they came, so the view's keys keep their order.
    tiles.iter().filter_map(|t| placed.iter().find(|p| p.key == t.key).cloned()).collect()
}

/// Every tile risen as far as it can, top to bottom (gravity), none overlapping.
pub fn compact(tiles: &[Tile]) -> Vec<Tile> {
    let mut order: Vec<Tile> = tiles.to_vec();
    order.sort_by_key(|t| (t.y, t.x));
    let mut placed: Vec<Tile> = Vec::new();
    for t in order {
        let t = settle(&placed, t);
        placed.push(t);
    }
    tiles.iter().filter_map(|t| placed.iter().find(|p| p.key == t.key).cloned()).collect()
}

/// Tile `t` risen as far as it goes, then pushed below whatever of `placed` it still overlaps.
fn settle(placed: &[Tile], mut t: Tile) -> Tile {
    while t.y > 0 && !placed.iter().any(|p| Tile { y: t.y - 1, ..t.clone() }.overlaps(p)) {
        t.y -= 1;
    }
    while let Some(below) = placed.iter().filter(|p| t.overlaps(p)).map(|p| p.y + p.h).max() {
        t.y = below;
    }
    t
}

/// A new tile `key` at the first free spot, scanning rows from the top and each row from the
/// left, at the default size.
pub fn add(tiles: &[Tile], key: &str) -> Vec<Tile> {
    let mut t = Tile { key: key.to_owned(), x: 0, y: 0, w: NEW_W, h: NEW_H };
    'scan: for y in 0.. {
        for x in 0..=COLS - NEW_W {
            t.x = x;
            t.y = y;
            if !tiles.iter().any(|o| t.overlaps(o)) {
                break 'scan;
            }
        }
    }
    let mut out = tiles.to_vec();
    out.push(t);
    compact(&out)
}

/// `tiles` less `key`, the rest risen into its room.
pub fn remove(tiles: &[Tile], key: &str) -> Vec<Tile> {
    compact(&tiles.iter().filter(|t| t.key != key).cloned().collect::<Vec<_>>())
}
