//! Vector box-drawing, block-element, and underline `Shape` primitives for terminal rows.

use rune::{Color, LineCap, Shape};
use terminal::UnderlineKind;

pub fn draw_special_underline(
    mut shape: Shape,
    kind: UnderlineKind,
    x: f32,
    w: f32,
    h: f32,
    color: Color,
) -> Shape {
    let y = (h - 2.0).max(1.0);
    match kind {
        UnderlineKind::Double => shape
            .line([x, y - 2.0], [x + w, y - 2.0], 1.0, LineCap::Butt, color)
            .line([x, y], [x + w, y], 1.0, LineCap::Butt, color),
        UnderlineKind::Curly => {
            let steps = ((w / 3.0).ceil() as usize).max(2);
            let mut pts = Vec::with_capacity(steps + 1);
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let py = y - 1.0 + (t * std::f32::consts::TAU).sin() * 1.3;
                pts.push([x + w * t, py]);
            }
            shape.stroke_with(pts, false, 1.1, LineCap::Round, color)
        }
        UnderlineKind::Dotted => {
            let mut cx = x;
            while cx + 1.5 <= x + w {
                shape = shape.line([cx, y], [cx + 1.5, y], 1.2, LineCap::Butt, color);
                cx += 3.5;
            }
            shape
        }
        UnderlineKind::Dashed => {
            shape.line([x + 0.5, y], [(x + w - 1.0).max(x + 1.0), y], 1.2, LineCap::Butt, color)
        }
        UnderlineKind::None | UnderlineKind::Single => shape,
    }
}

/// Seamless vector box-drawing and block elements (`U+2500..=U+259F`) so TUI borders never show gaps.
pub fn draw_box_or_block(
    shape: &mut Shape,
    ch: char,
    x: f32,
    w: f32,
    h: f32,
    color: Color,
) -> bool {
    let (xm, ym, x1, y1) = (x + w * 0.5, h * 0.5, x + w, h);
    let rect = |s: &mut Shape, rx0: f32, ry0: f32, rx1: f32, ry1: f32, c: Color| {
        *s = std::mem::take(s).fill(vec![[rx0, ry0], [rx1, ry0], [rx1, ry1], [rx0, ry1]], c);
    };
    let seg = |s: &mut Shape, a: [f32; 2], b: [f32; 2], sw: f32| {
        *s = std::mem::take(s).line(a, b, sw, LineCap::Butt, color);
    };

    let arms: Option<(f32, f32, f32, f32)> = match ch {
        '─' | '╌' | '┄' | '┈' => Some((1.0, 1.0, 0.0, 0.0)),
        '━' | '═' => Some((2.0, 2.0, 0.0, 0.0)),
        '│' | '╎' | '┆' | '┊' => Some((0.0, 0.0, 1.0, 1.0)),
        '┃' | '║' => Some((0.0, 0.0, 2.0, 2.0)),
        '┌' | '╭' => Some((0.0, 1.0, 0.0, 1.0)),
        '┐' | '╮' => Some((1.0, 0.0, 0.0, 1.0)),
        '└' | '╰' => Some((0.0, 1.0, 1.0, 0.0)),
        '┘' | '╯' => Some((1.0, 0.0, 1.0, 0.0)),
        '├' => Some((0.0, 1.0, 1.0, 1.0)),
        '┤' => Some((1.0, 0.0, 1.0, 1.0)),
        '┬' => Some((1.0, 1.0, 0.0, 1.0)),
        '┴' => Some((1.0, 1.0, 1.0, 0.0)),
        '┼' => Some((1.0, 1.0, 1.0, 1.0)),
        _ => None,
    };
    if let Some((l, r, u, d)) = arms {
        if l > 0.0 && r > 0.0 && (l - r).abs() < 0.01 {
            seg(shape, [x, ym], [x1, ym], l);
        } else {
            if l > 0.0 {
                seg(shape, [x, ym], [xm, ym], l);
            }
            if r > 0.0 {
                seg(shape, [xm, ym], [x1, ym], r);
            }
        }
        if u > 0.0 && d > 0.0 && (u - d).abs() < 0.01 {
            seg(shape, [xm, 0.0], [xm, y1], u);
        } else {
            if u > 0.0 {
                seg(shape, [xm, 0.0], [xm, ym], u);
            }
            if d > 0.0 {
                seg(shape, [xm, ym], [xm, y1], d);
            }
        }
        return true;
    }

    match ch {
        '█' => rect(shape, x, 0.0, x1, y1, color),
        '▀' => rect(shape, x, 0.0, x1, ym, color),
        '▄' => rect(shape, x, ym, x1, y1, color),
        '▌' => rect(shape, x, 0.0, xm, y1, color),
        '▐' => rect(shape, xm, 0.0, x1, y1, color),
        '░' => rect(shape, x, 0.0, x1, y1, color.alpha(0.25)),
        '▒' => rect(shape, x, 0.0, x1, y1, color.alpha(0.50)),
        '▓' => rect(shape, x, 0.0, x1, y1, color.alpha(0.75)),
        '❯' => {
            *shape = std::mem::take(shape).stroke_with(
                vec![
                    [x + w * 0.22, h * 0.29],
                    [x + w * 0.72, ym],
                    [x + w * 0.22, h * 0.71],
                ],
                false,
                1.6,
                LineCap::Round,
                color,
            );
        }
        _ => return false,
    }
    true
}
