//! Draw commands into Rune elements. Vector work (fills, strokes, paths, under any transform)
//! batches into `canvas(Shape)` layers; what a shape can't carry breaks the batch into its own
//! element at its place in the order: a gradient fill (an element's linear or radial gradient,
//! where the transform keeps axes upright), text, and images through the host's picture cache.
//! A gradient a rotated or skewed transform turns, or on a path, fills with its middle color.

use opencode_widget::Command;
use opencode_widget::draw::{Cap, Color as DrawColor, Fill, GradientShape, Paint, Segment, TextAlign, TextOptions, Token};
use rune::{Color, El, Image, LineCap, Path, Shape, SurfaceFit, canvas, el, text};
use rune_kit::Kit;

/// A 2D affine transform: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f32; 6]);

impl Affine {
    const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn apply(&self, x: f32, y: f32) -> [f32; 2] {
        let [a, b, c, d, e, f] = self.0;
        [a * x + c * y + e, b * x + d * y + f]
    }

    fn then(&self, [a2, b2, c2, d2, e2, f2]: [f32; 6]) -> Affine {
        // self ∘ other: other applies first.
        let [a, b, c, d, e, f] = self.0;
        Affine([a * a2 + c * b2, b * a2 + d * b2, a * c2 + c * d2, b * c2 + d * d2, a * e2 + c * f2 + e, b * e2 + d * f2 + f])
    }

    /// How much it scales lengths, on average.
    fn stretch(&self) -> f32 {
        let [a, b, c, d, ..] = self.0;
        (a * d - b * c).abs().sqrt()
    }

    /// Axes stay upright (no rotation or skew): boxes stay boxes.
    fn upright(&self) -> bool {
        self.0[1].abs() < 1e-6 && self.0[2].abs() < 1e-6 && self.0[0] > 0.0 && self.0[3] > 0.0
    }
}

/// sRGB bytes to Rune's linear colors, decoded once.
fn decode(v: u8) -> f32 {
    static TABLE: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, out) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *out = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        t
    })[v as usize]
}

pub(crate) fn color(k: &Kit, c: DrawColor) -> Color {
    let s = &k.c;
    match c {
        DrawColor::Rgba([r, g, b, a]) => Color { r: decode(r), g: decode(g), b: decode(b), a: f32::from(a) / 255.0 },
        DrawColor::Token(t) => match t {
            Token::Text => s.text_base,
            Token::Muted => s.text_muted,
            Token::Accent => s.text_accent,
            Token::Bg => s.bg_base,
            Token::Surface => s.surface_raised_base,
            Token::Border => s.border_base,
            Token::Success => s.success.icon,
            Token::Warning => s.warning.icon,
            Token::Error => s.danger.icon,
        },
    }
}

struct Painter<'a> {
    k: &'a Kit,
    image: &'a dyn Fn(&str) -> Option<Image>,
    /// Device pixels per logical pixel: how finely curves tessellate.
    scale: f32,
    size: [f32; 2],
    at: Affine,
    alpha: f32,
    stack: Vec<(Affine, f32)>,
    shape: Shape,
    out: Vec<El>,
}

impl Painter<'_> {
    fn ink(&self, c: DrawColor) -> Color {
        let c = color(self.k, c);
        c.alpha(c.a * self.alpha)
    }

    fn flush(&mut self) {
        if !self.shape.is_empty() {
            let shape = std::mem::take(&mut self.shape);
            self.out.push(canvas(shape).absolute(0.0, 0.0).size(self.size[0], self.size[1]).inert());
        }
    }

    fn push(&mut self, e: El) {
        self.flush();
        self.out.push(e.inert());
    }

    /// Fills and strokes the closed outline `points` (logical, before the transform), or a
    /// gradient box over `bounds` with corner radius `radius` where the transform allows.
    fn closed(&mut self, points: &[[f32; 2]], bounds: [f32; 4], radius: f32, paint: &Paint) {
        let pts: Vec<[f32; 2]> = points.iter().map(|p| self.at.apply(p[0], p[1])).collect();
        match &paint.fill {
            Some(Fill::Color(c)) => self.shape = std::mem::take(&mut self.shape).fill(pts.clone(), self.ink(*c)),
            Some(Fill::Gradient(g)) if self.at.upright() && !g.stops.is_empty() => {
                let [x, y, w, h] = bounds;
                let [x0, y0] = self.at.apply(x, y);
                let [x1, y1] = self.at.apply(x + w, y + h);
                let (bw, bh) = ((x1 - x0).max(0.01), (y1 - y0).max(0.01));
                let stops: Vec<(f32, Color)> = g.stops.iter().map(|(o, c)| (*o, self.ink(*c))).collect();
                let mut e = el().absolute(x0, y0).size(bw, bh).radius(radius * self.at.stretch());
                e = match g.shape {
                    GradientShape::Linear([gx1, gy1, gx2, gy2]) => {
                        let [p1x, p1y] = self.at.apply(gx1, gy1);
                        let [p2x, p2y] = self.at.apply(gx2, gy2);
                        let (dx, dy) = (p2x - p1x, p2y - p1y);
                        // CSS's gradient line runs through the box's center at `angle` (0 up,
                        // clockwise), as long as the box's extent along it; map each stop's point
                        // on the command's line on to it.
                        let angle = dx.atan2(-dy);
                        let (u, v) = (angle.sin(), -angle.cos());
                        let len = (bw * u).abs() + (bh * v).abs();
                        let (cx, cy) = (x0 + bw / 2.0, y0 + bh / 2.0);
                        let at = |o: f32| ((p1x + o * dx - cx) * u + (p1y + o * dy - cy) * v) / len.max(0.01) + 0.5;
                        let css: Vec<(f32, Color)> = stops.iter().map(|(o, c)| (at(*o), *c)).collect();
                        e.linear_gradient(angle, css)
                    }
                    GradientShape::Radial([cx, cy, r]) => {
                        let [px, py] = self.at.apply(cx, cy);
                        e.radial_gradient([(px - x0) / bw, (py - y0) / bh], r * self.at.stretch(), stops)
                    }
                };
                self.push(e);
            }
            Some(Fill::Gradient(g)) => {
                if let Some((_, c)) = g.stops.get(g.stops.len() / 2) {
                    self.shape = std::mem::take(&mut self.shape).fill(pts.clone(), self.ink(*c));
                }
            }
            None => {}
        }
        if let Some(stroke) = paint.stroke {
            let width = paint.width * self.at.stretch();
            self.shape = std::mem::take(&mut self.shape).stroke_with(pts, true, width, cap(paint.cap), self.ink(stroke));
        }
    }

    fn text(&mut self, x: f32, y: f32, words: &str, style: &TextOptions) {
        let s = self.at.stretch();
        let size = style.size * s;
        let mut font = if style.mono { self.k.ty.mono } else { self.k.ty.ui };
        font.size = size;
        font.weight = f32::from(style.weight);
        let font = font.lh(size * 1.25);
        let [ax, ay] = self.at.apply(x, y);
        // `y` is the baseline: a line box 1.25 em tall puts it about 0.98 em under its top.
        let top = ay - size * 0.98;
        let ink = self.ink(style.color.unwrap_or(DrawColor::Token(Token::Text)));
        let t = text(words.to_owned()).font(font).color(ink).single_line();
        const WIDE: f32 = 4096.0;
        let e = match style.align {
            TextAlign::Left => el().absolute(ax, top).child(t),
            TextAlign::Center => el().absolute(ax - WIDE / 2.0, top).w(WIDE).justify_center().child(t),
            TextAlign::Right => el().absolute(ax - WIDE, top).w(WIDE).justify_end().child(t),
        };
        self.push(e);
    }

    fn command(&mut self, c: &Command) {
        match c {
            Command::Save => self.stack.push((self.at, self.alpha)),
            Command::Restore => {
                if let Some((at, alpha)) = self.stack.pop() {
                    self.at = at;
                    self.alpha = alpha;
                }
            }
            Command::Translate(x, y) => self.at = self.at.then([1.0, 0.0, 0.0, 1.0, *x, *y]),
            Command::Rotate(r) => {
                let (s, c) = r.sin_cos();
                self.at = self.at.then([c, s, -s, c, 0.0, 0.0]);
            }
            Command::Scale(x, y) => self.at = self.at.then([*x, 0.0, 0.0, *y, 0.0, 0.0]),
            Command::Alpha(a) => self.alpha *= a,
            Command::Rect { x, y, w, h, paint } => {
                let pts = [[*x, *y], [x + w, *y], [x + w, y + h], [*x, y + h]];
                self.closed(&pts, [*x, *y, *w, *h], 0.0, paint);
            }
            Command::RRect { x, y, w, h, r, paint } => {
                let pts = rounded(*x, *y, *w, *h, *r, self.scale * self.at.stretch());
                self.closed(&pts, [*x, *y, *w, *h], r.min(w / 2.0).min(h / 2.0).max(0.0), paint);
            }
            Command::Circle { cx, cy, r, paint } => {
                let pts = ellipse(*cx, *cy, *r, self.scale * self.at.stretch());
                self.closed(&pts, [cx - r, cy - r, r * 2.0, r * 2.0], *r, paint);
            }
            Command::Line { x1, y1, x2, y2, paint } => {
                let ink = paint.stroke.or(match &paint.fill {
                    Some(Fill::Color(c)) => Some(*c),
                    _ => None,
                });
                if let Some(ink) = ink {
                    let (a, b) = (self.at.apply(*x1, *y1), self.at.apply(*x2, *y2));
                    let width = paint.width * self.at.stretch();
                    self.shape = std::mem::take(&mut self.shape).line(a, b, width, cap(paint.cap), self.ink(ink));
                }
            }
            Command::Path { segments, paint } => self.path(segments, paint),
            Command::Text { x, y, text, style } => self.text(*x, *y, text, style),
            Command::Image { src, x, y, w, h } => {
                let [x0, y0] = self.at.apply(*x, *y);
                let s = self.at.stretch();
                let tile = el().absolute(x0, y0).size(w * s, h * s).opacity(self.alpha);
                if let Some(img) = (self.image)(src).filter(|i| i.error().is_none()) {
                    self.push(tile.pixels_fit(img.pixels(), SurfaceFit::Fill));
                }
            }
        }
    }

    fn path(&mut self, segments: &[Segment], paint: &Paint) {
        let mut path = Path::new();
        for s in segments {
            let p = |x: f32, y: f32| self.at.apply(x, y);
            path = match *s {
                Segment::M(x, y) => path.move_to(p(x, y)),
                Segment::L(x, y) => path.line_to(p(x, y)),
                Segment::Q(cx, cy, x, y) => path.quad_to(p(cx, cy), p(x, y)),
                Segment::C(c1x, c1y, c2x, c2y, x, y) => path.cubic_to(p(c1x, c1y), p(c2x, c2y), p(x, y)),
                Segment::Z => path.close(),
            };
        }
        let fill = match &paint.fill {
            Some(Fill::Color(c)) => Some(*c),
            Some(Fill::Gradient(g)) => g.stops.get(g.stops.len() / 2).map(|(_, c)| *c),
            None => None,
        };
        if let Some(c) = fill {
            self.shape = std::mem::take(&mut self.shape).fill_path(&path, self.ink(c));
        }
        if let Some(stroke) = paint.stroke {
            let width = paint.width * self.at.stretch();
            self.shape = std::mem::take(&mut self.shape).stroke_path(&path, width, cap(paint.cap), self.ink(stroke));
        }
    }
}

fn cap(c: Cap) -> LineCap {
    match c {
        Cap::Butt => LineCap::Butt,
        Cap::Round => LineCap::Round,
    }
}

/// Points on a circle's outline, finer the larger it shows.
fn ellipse(cx: f32, cy: f32, r: f32, px: f32) -> Vec<[f32; 2]> {
    let n = ((r * px).sqrt() * 4.0).clamp(12.0, 96.0) as usize;
    (0..n).map(|i| {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        [cx + r * a.cos(), cy + r * a.sin()]
    }).collect()
}

/// A rounded rectangle's outline: four quarter arcs.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32, px: f32) -> Vec<[f32; 2]> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.01 {
        return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
    }
    let n = ((r * px).sqrt() * 1.5).clamp(2.0, 16.0) as usize;
    let corners = [(x + w - r, y + r, -0.25), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 0.25), (x + r, y + r, 0.5)];
    let mut out = Vec::with_capacity(4 * (n + 1));
    for (cx, cy, start) in corners {
        for i in 0..=n {
            let a = (start + 0.25 * i as f32 / n as f32) * std::f32::consts::TAU;
            out.push([cx + r * a.cos(), cy + r * a.sin()]);
        }
    }
    out
}

/// Elements that draw `commands` in a `size` box, in order.
pub(crate) fn paint(k: &Kit, commands: &[Command], size: [f32; 2], scale: f32, image: &dyn Fn(&str) -> Option<Image>) -> Vec<El> {
    let mut p = Painter { k, image, scale, size, at: Affine::IDENTITY, alpha: 1.0, stack: Vec::new(), shape: Shape::new(), out: Vec::new() };
    for c in commands {
        p.command(c);
    }
    p.flush();
    p.out
}
