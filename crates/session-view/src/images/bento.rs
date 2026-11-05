//! Where a gallery's tiles go: one image in its own shape; several in a bento block of one
//! width, rows justified so each picture keeps close to its shape, three as a hero and a pair.

/// The block's width for two or more images, the gap between tiles and their corner radius.
pub const WIDTH: f32 = 360.0;
pub const GAP: f32 = 4.0;
pub const RADIUS: f32 = 10.0;

/// A tile's `[x, y, w, h]` in the block.
pub type Tile = [f32; 4];

/// Tiles for images whose own sizes (points, already fit) are `sizes`, and the block's size.
pub fn tiles(sizes: &[(f32, f32)]) -> ((f32, f32), Vec<Tile>) {
    let aspect = |&(w, h): &(f32, f32)| (w / h.max(1.0)).clamp(0.4, 2.5);
    let aspects: Vec<f32> = sizes.iter().map(aspect).collect();
    match sizes {
        [] => ((0.0, 0.0), Vec::new()),
        [(w, h)] => ((*w, *h), vec![[0.0, 0.0, *w, *h]]),
        [_, _, _] => hero(&aspects),
        _ => {
            let mut tiles = Vec::new();
            let mut y = 0.0;
            for (start, n) in rows(aspects.len()) {
                let h = justified(&aspects[start..start + n], y, &mut tiles);
                y += h + GAP;
            }
            ((WIDTH, y - GAP), tiles)
        }
    }
}

/// Row lengths for `n` images: threes, with a pair or two first so no row is left alone.
fn rows(n: usize) -> Vec<(usize, usize)> {
    let pairs = match n % 3 { _ if n <= 2 => 1, 0 => 0, 1 => 2, _ => 1 };
    let mut out = Vec::new();
    let mut at = 0;
    for _ in 0..pairs.min(n / 2) {
        out.push((at, 2));
        at += 2;
    }
    while at < n {
        let take = (n - at).min(3);
        out.push((at, take));
        at += take;
    }
    out
}

/// One row the block's width, its tiles one height and as wide as their shapes ask, so a tile
/// crops only when the row's height hits its floor or ceiling.
fn justified(aspects: &[f32], y: f32, out: &mut Vec<Tile>) -> f32 {
    let room = WIDTH - GAP * (aspects.len() - 1) as f32;
    let sum: f32 = aspects.iter().sum();
    let ceiling = if aspects.len() == 2 { 240.0 } else { 170.0 };
    let h = (room / sum).clamp(96.0, ceiling).round();
    let mut x = 0.0;
    for (i, a) in aspects.iter().enumerate() {
        let w = if i + 1 == aspects.len() { WIDTH - x } else { (room * a / sum).round() };
        out.push([x, y, w, h]);
        x += w + GAP;
    }
    h
}

/// Three: the first image large, the other two stacked beside it (a tall hero) or side by side
/// under it (a wide one).
fn hero(a: &[f32]) -> ((f32, f32), Vec<Tile>) {
    if a[0] < 1.0 {
        let h = 280.0;
        let big = (h * a[0]).clamp(WIDTH * 0.5, WIDTH * 0.62).round();
        let (x, w, half) = (big + GAP, WIDTH - big - GAP, ((h - GAP) / 2.0).round());
        ((WIDTH, h), vec![[0.0, 0.0, big, h], [x, 0.0, w, half], [x, half + GAP, w, h - half - GAP]])
    } else {
        let top = (WIDTH / a[0]).clamp(150.0, 220.0).round();
        let mut tiles = vec![[0.0, 0.0, WIDTH, top]];
        let below = justified(&a[1..], top + GAP, &mut tiles);
        ((WIDTH, top + GAP + below), tiles)
    }
}

#[cfg(test)]
#[path = "bento_tests.rs"]
mod tests;
