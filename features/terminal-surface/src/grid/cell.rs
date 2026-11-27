//! Which terminal cell a pointer point is over. Plain `std` only, so a table program outside the
//! repo can include this file by path.

/// The point (x, y) in an element's own coordinates, where the terminal's text starts at
/// `origin`: its offset into the text (never negative) and the cell under it, both clamped to the
/// last of `cap` (columns, rows). The viewport passes its padding as `origin`; the screen text,
/// which starts at its own (0, 0), passes zero, so one point maps to one cell from either.
pub fn cell_at(x: f32, y: f32, origin: (f32, f32), cell: (f32, f32), cap: (u16, u16)) -> (f32, f32, u16, u16) {
    let (lx, ly) = ((x - origin.0).max(0.0), (y - origin.1).max(0.0));
    let col = ((lx / cell.0).floor() as u16).min(cap.0.saturating_sub(1));
    let row = ((ly / cell.1).floor() as u16).min(cap.1.saturating_sub(1));
    (lx, ly, col, row)
}
