use super::{GAP, WIDTH, tiles};

const TALL: (f32, f32) = (216.0, 240.0);
const WIDE: (f32, f32) = (320.0, 200.0);
const SQUARE: (f32, f32) = (240.0, 240.0);

/// Every tile sits inside the block, no two overlap, and each row runs edge to edge.
fn tidy(sizes: &[(f32, f32)]) {
    let ((bw, bh), t) = tiles(sizes);
    assert_eq!(t.len(), sizes.len());
    for (i, a) in t.iter().enumerate() {
        assert!(a[0] >= 0.0 && a[1] >= 0.0 && a[0] + a[2] <= bw + 0.01 && a[1] + a[3] <= bh + 0.01, "{sizes:?}: tile {i} {a:?} outside {bw}×{bh}");
        for b in &t[i + 1..] {
            let apart = a[0] + a[2] + GAP <= b[0] + 0.01 || b[0] + b[2] + GAP <= a[0] + 0.01 || a[1] + a[3] + GAP <= b[1] + 0.01 || b[1] + b[3] + GAP <= a[1] + 0.01;
            assert!(apart, "{sizes:?}: {a:?} and {b:?} touch");
        }
        let right = t.iter().filter(|b| b[1] < a[1] + a[3] && a[1] < b[1] + b[3]).map(|b| b[0] + b[2]).fold(0.0, f32::max);
        assert!((right - bw).abs() < 0.01, "{sizes:?}: the row with {a:?} stops at {right}");
    }
}

#[test]
fn one_image_keeps_its_own_shape() {
    assert_eq!(tiles(&[TALL]), (TALL, vec![[0.0, 0.0, TALL.0, TALL.1]]));
}

#[test]
fn several_images_pack_into_one_block_edge_to_edge() {
    for n in 2..=9 {
        let sizes: Vec<_> = (0..n).map(|i| [TALL, WIDE, SQUARE][i % 3]).collect();
        tidy(&sizes);
        assert_eq!(tiles(&sizes).0 .0, WIDTH);
    }
    tidy(&[WIDE, TALL, TALL]);
    tidy(&[(320.0, 24.0), (24.0, 240.0)]);
}

/// Kit's two screenshots (t1378), a tall and a wide: one row, one height, each close to its shape.
#[test]
fn a_pair_shares_a_row_at_their_own_shapes() {
    let (_, t) = tiles(&[TALL, WIDE]);
    assert_eq!(t[0][3], t[1][3]);
    for (tile, (w, h)) in t.iter().zip([TALL, WIDE]) {
        let (got, want) = (tile[2] / tile[3], w / h);
        assert!((got - want).abs() / want < 0.03, "{tile:?} for {w}×{h}");
    }
}

#[test]
fn three_put_the_first_large() {
    let (_, t) = tiles(&[TALL, WIDE, WIDE]);
    assert_eq!(t[0][3], t[1][3] + GAP + t[2][3], "a tall hero beside a stacked pair");
    let (_, t) = tiles(&[WIDE, TALL, WIDE]);
    assert_eq!(t[0][2], WIDTH, "a wide hero over a pair");
}
