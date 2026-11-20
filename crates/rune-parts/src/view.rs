//! Outlines drawn as Rune elements. Each app passes its colours ([`PartsTheme`]) and how it draws
//! rich words and glyphs ([`PartsResolver`]); nothing here knows about a specific host app.

use rune::{Color, El, Icon, TextStyle, col, el, icon, row, spacer, text};

use crate::catalog::Tone;
use crate::outline::Node;

mod flow;
pub use flow::{PANE_MAX, PANE_MIN, Pane, plan_row};

/// Below this width a `row` stacks its kids and its arrows point down.
pub const NARROW: f32 = 480.0;

/// A surface's colours for parts.
#[derive(Clone, Copy, Debug)]
pub struct PartsTheme {
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    /// Hairlines and frame edges.
    pub ring: Color,
    /// The panel behind a frame (a toggle's knob).
    pub bg: Color,
    /// Your own chat bubble.
    pub bubble: Color,
    pub done: Color,
    pub risk: Color,
    pub proposal: Color,
    pub working: Color,
    pub accent: Color,
}

impl PartsTheme {
    pub fn tone(&self, t: Tone) -> Color {
        match t {
            Tone::Done => self.done,
            Tone::Risk => self.risk,
            Tone::Proposal => self.proposal,
            Tone::Working => self.working,
            Tone::Accent => self.accent,
            Tone::Quiet => self.muted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Arrow,
    ArrowDown,
    Voice,
}

macro_rules! stroke {
    ($name:literal, $($d:literal),+) => {
        Icon { name: $name, view: 24.0, body: concat!($("<path d=\"", $d, "\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"),+) }
    };
}

static ARROW: Icon = stroke!("rune-parts-arrow", "M5 12h14", "m12 5 7 7-7 7");
static ARROW_DOWN: Icon = stroke!("rune-parts-arrow-down", "M12 5v14", "m19 12-7 7-7-7");
static VOICE: Icon = stroke!("rune-parts-voice", "M12 19v3", "M19 10v2a7 7 0 0 1-14 0v-2", "M12 2a3 3 0 0 1 3 3v7a3 3 0 0 1-6 0V5a3 3 0 0 1 3-3Z");

/// What a surface draws its own way: words with chips in them, glyphs, and who "you" are.
pub trait PartsResolver {
    /// Words that may hold Markdown, PR and card references.
    fn inline(&self, src: &str, font: TextStyle, color: Color) -> El {
        text(src.to_owned()).font(font).color(color).wrap()
    }

    /// Vector strokes in the text colour; override only to use a surface's own icon set.
    fn glyph(&self, g: Glyph, size: f32, color: Color) -> El {
        let i = match g {
            Glyph::Arrow => &ARROW,
            Glyph::ArrowDown => &ARROW_DOWN,
            Glyph::Voice => &VOICE,
        };
        icon(i, size).color(color)
    }

    /// Chat lines from you draw on the right, in your bubble.
    fn is_you(&self, who: &str) -> bool {
        matches!(who.to_ascii_lowercase().as_str(), "kit" | "you" | "me")
    }
}

/// Plain words and the default glyphs, for tests and surfaces without chips.
pub struct Plain;
impl PartsResolver for Plain {}

const BODY: TextStyle = TextStyle::new(14.0, 420.0).lh(21.0).tracking(-0.1);
const TITLE: TextStyle = TextStyle::new(14.0, 600.0).lh(21.0).tracking(-0.1);
const WHO: TextStyle = TextStyle::new(12.5, 560.0).lh(18.0);
const QUIET: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0);
/// A frame's label: sentence case as written, faint, no tracking.
const LABEL: TextStyle = TextStyle::new(12.5, 500.0).lh(18.0);
const CHIP: TextStyle = TextStyle::new(11.5, 440.0).mono().lh(16.0);
const MARK: f32 = 7.0;
const BULLET: f32 = 4.0;
/// Between words and their quiet ` · detail`.
const DETAIL_GAP: f32 = 8.0;
/// Between the panes of a row and its arrows.
const ROW_GAP: f32 = 6.0;
const ARROW_SIZE: f32 = 12.0;
const PHONE: f32 = 250.0;
/// A frame's fill: the text colour this faint, so it reads on any surface.
const FILL: f32 = 0.045;
/// A frame's padding: its one fill is the only edge it has.
const FRAME_X: f32 = 10.0;
const FRAME_Y: f32 = 10.0;
const RADIUS: f32 = 10.0;

struct Ctx<'a> {
    t: &'a PartsTheme,
    r: &'a dyn PartsResolver,
}

/// An outline `width` points wide. Lines that aren't in a frame (chat lines, controls) share one
/// quiet panel, so a mockup reads as a picture of an interface, apart from the note's own text.
pub fn outline(nodes: &[Node], width: f32, t: &PartsTheme, r: &dyn PartsResolver) -> El {
    let cx = Ctx { t, r };
    let mut out = Vec::new();
    let mut loose: Vec<&Node> = Vec::new();
    for n in nodes {
        if framed(n) {
            if !loose.is_empty() {
                out.push(cx.panel(&std::mem::take(&mut loose), width));
            }
            out.push(cx.node(n, width));
        } else {
            loose.push(n);
        }
    }
    if !loose.is_empty() {
        out.push(cx.panel(&loose, width));
    }
    col().key("ui").w_full().gap(12.0).children(out)
}

/// Whether a top-level line draws its own frame (or lays frames out), so it needs no panel.
fn framed(n: &Node) -> bool {
    match n.spec() {
        Some(s) => matches!(s.name, "row" | "col" | "box" | "screen" | "phone"),
        None => !n.kids.is_empty(),
    }
}

/// The space above a kid in a frame, after `prev`: list lines close, chat lines apart.
fn gap_between(prev: Option<&str>, name: &str) -> f32 {
    match (prev, name) {
        (None, _) => 0.0,
        (Some("item"), "item") => 4.0,
        (Some("kv"), "kv") => 5.0,
        (Some("say"), "say") => 14.0,
        _ => 10.0,
    }
}

/// Facts (`kv` rows) get a hairline between them.
fn ruled(prev: Option<&str>, name: &str) -> bool {
    prev == Some("kv") && name == "kv"
}

fn mark(c: Color) -> El {
    el().key("mark").w(MARK).h(MARK).radius(MARK * 0.25).fill(c)
}

/// A mark beside the first line of `font` text, its middle on the line's x-height; with no tone,
/// a small bullet in the same slot.
fn mark_at(c: Option<Color>, font: TextStyle, t: &PartsTheme) -> El {
    let mid = rune::text::ink("x", font).map_or(font.line_height * 0.55, |i| i.centre()[1]);
    let Some(c) = c else {
        let dot = el().w(BULLET).h(BULLET).radius(BULLET / 2.0).fill(t.muted);
        return row().key("mark").w(MARK).justify_center().mt((mid - BULLET / 2.0).max(0.0)).child(dot);
    };
    mark(c).mt((mid - MARK / 2.0).max(0.0))
}

fn measure(s: &str, font: TextStyle) -> f32 {
    if s.is_empty() { 0.0 } else { rune::text::measure(s, &[], font, None).0 }
}

/// The widest word: what a line can't wrap below.
fn widest_word(s: &str, font: TextStyle) -> f32 {
    s.split_whitespace().map(|w| measure(w, font)).fold(0.0, f32::max)
}

fn tone_of(t: &PartsTheme, word: Option<&str>) -> Option<Color> {
    word.map(|w| t.tone(Tone::of(w)))
}

impl Ctx<'_> {
    fn node(&self, n: &Node, width: f32) -> El {
        let (t, p) = (self.t, &n.part);
        let key = n.key();
        let Some(spec) = n.spec() else {
            if n.kids.is_empty() {
                let words = format!("{} {}", p.name, n.words());
                return text(words.trim().to_owned()).key(key).font(QUIET).color(t.faint).wrap();
            }
            return self.frame(n, width, false, "");
        };
        match spec.name {
            "row" => self.row(n, width),
            "col" => col().key(key).w_full().children(self.stack(&n.kids.iter().collect::<Vec<_>>(), width)),
            "box" => self.frame(n, width, false, "box"),
            "screen" => self.frame(n, width, false, "screen"),
            "phone" => self.frame(n, width.min(PHONE), true, "phone"),
            "arrow" => self.arrow(n, width < NARROW),
            "title" => self.r.inline(&p.text, TITLE, t.text).key(key),
            "text" => self.r.inline(&p.text, BODY, t.text).key(key),
            "item" => {
                let words = self.with_detail(&key, &p.text, &p.detail, width - MARK - 8.0);
                row().key(key).w_full().gap(8.0).items_start()
                    .child(mark_at(tone_of(t, p.tone.as_deref()), BODY, t))
                    .child(words)
            }
            "kv" => {
                let mut v = row().grow(1.0).shrink().min_w(0.0).gap(8.0).items_start();
                if let Some(c) = tone_of(t, p.tone.as_deref()) {
                    v = v.child(mark_at(Some(c), BODY, t));
                }
                let label_w = if width < NARROW { 0.0 } else { (width * 0.22).clamp(64.0, 120.0) + 12.0 };
                let mark_w = if p.tone.is_some() { MARK + 8.0 } else { 0.0 };
                v = v.child(self.with_detail(&key, &p.text, &p.detail, width - label_w - mark_w));
                let label = text(p.key.clone()).font(BODY).color(t.muted);
                let r = row().key(key).w_full().py(2.0);
                // Narrow, the label sits over its value rather than truncating beside it.
                if width < NARROW {
                    return r.child(col().grow(1.0).shrink().min_w(0.0).gap(2.0).child(label.font(QUIET).wrap()).child(v));
                }
                r.gap(12.0).items_start().child(label.w((width * 0.22).clamp(64.0, 120.0)).shrink().truncate()).child(v)
            }
            "say" => self.say(n, width),
            "tools" => text(format!("Used {}", p.text)).key(key).font(QUIET).color(t.faint).wrap(),
            "from" => row().key(key).gap(6.0).center_y()
                .child(text("From").font(QUIET).color(t.faint))
                .child(text(p.text.clone()).font(WHO).color(t.text))
                .child(text("›").font(WHO).color(t.faint)),
            "toggle" => {
                let on = p.tone.as_deref() == Some("on");
                let knob = el().w(14.0).h(14.0).radius(7.0).fill(if on { t.bg } else { t.muted });
                let track = row().w(30.0).h(18.0).radius(9.0).px(2.0).center_y().fill(if on { t.done } else { t.text.alpha(0.1) });
                let track = if on { track.child(spacer()).child(knob) } else { track.child(knob) };
                row().key(key).w_full().gap(12.0).center_y()
                    .child(col().grow(1.0).shrink().min_w(0.0).child(self.r.inline(&p.text, BODY, t.text)))
                    .child(track)
            }
            "button" => {
                let primary = p.tone.as_deref().is_some_and(|w| Tone::of(w) == Tone::Accent);
                row().key(key).self_start().h(26.0).px(12.0).radius(13.0).center_y()
                    .fill(if primary { t.accent } else { t.text.alpha(0.08) })
                    .child(text(p.text.clone()).font(WHO).color(if primary { t.bg } else { t.text }))
            }
            "field" => row().key(key).w_full().h(30.0).px(10.0).radius(8.0).center_y().border(0.5, t.ring)
                .child(text(p.text.clone()).font(BODY).color(t.faint).truncate()),
            "chips" => row().key(key).flex_wrap().gaps(6.0, 6.0).children(p.text.split(',').map(str::trim).filter(|w| !w.is_empty()).map(|w| {
                row().h(18.0).px(5.0).radius(4.0).center_y().fill(t.text.alpha(0.07)).child(text(w.to_owned()).font(CHIP).color(t.muted))
            })),
            "rule" => el().key(key).w_full().h(0.5).fill(t.ring),
            _ => text(n.words()).key(key).font(QUIET).color(t.faint).wrap(),
        }
    }

    /// Words and their quiet detail: the detail after the words when both fit on one line,
    /// else on its own line under them. They never share a line they'd overlap on.
    fn with_detail(&self, key: &str, words: &str, detail: &str, room: f32) -> El {
        let t = self.t;
        let main = self.r.inline(words, BODY, t.text).key(format!("{key}.words"));
        if detail.is_empty() {
            return col().grow(1.0).shrink().min_w(0.0).child(main);
        }
        let quiet = text(detail.to_owned()).key(format!("{key}.detail")).font(QUIET).color(t.faint);
        if measure(words, BODY) + DETAIL_GAP + measure(detail, QUIET) <= room {
            // The words keep their own width (a surface's words may fill whatever they're given),
            // and if the line comes out narrower than planned the detail wraps under them.
            let main = col().w(measure(words, BODY).ceil() + 1.0).shrink().child(main);
            return row().grow(1.0).shrink().flex_wrap().gaps(0.0, DETAIL_GAP).items_start().child(main).child(quiet.pt(2.0).wrap());
        }
        col().grow(1.0).shrink().min_w(0.0).child(main).child(quiet.wrap())
    }

    /// Kids one under another, spaced by what they are: list lines close, kv rows ruled like
    /// facts, chat lines apart.
    fn stack(&self, kids: &[&Node], width: f32) -> Vec<El> {
        let mut out = Vec::new();
        let mut prev: Option<&str> = None;
        for k in kids {
            let name = k.part.name.as_str();
            let gap = gap_between(prev, name);
            if ruled(prev, name) {
                out.push(el().w_full().h(0.5).mt(gap).fill(self.t.ring));
            }
            out.push(self.node(k, width).mt(gap));
            prev = Some(name);
        }
        out
    }

    fn arrow(&self, n: &Node, down: bool) -> El {
        let g = if down { Glyph::ArrowDown } else { Glyph::Arrow };
        row().key(n.key()).self_center().child(self.r.glyph(g, ARROW_SIZE, self.t.faint))
    }

    /// Side by side when every pane fits without crushing its words; otherwise stacked, its
    /// arrows pointing down. Side by side, panes are as wide as their words want (sharing what's
    /// left of the column), all as tall as the tallest, arrows on their middle.
    fn row(&self, n: &Node, width: f32) -> El {
        let Some(widths) = flow::planned(n, width) else {
            let kids = n.kids.iter().map(|k| match k.part.name.as_str() {
                "arrow" => self.arrow(k, true),
                "phone" => self.node(k, width).self_center(),
                _ => self.node(k, width),
            });
            return col().key(n.key()).w_full().gap(ROW_GAP).children(kids);
        };
        let mut widths = widths.into_iter();
        let kids = n.kids.iter().map(|k| {
            if k.part.name == "arrow" {
                return self.arrow(k, false);
            }
            // Grown by its planned width: exact at `width`, and in proportion if the column the
            // row lands in is a little wider or narrower than the one it was planned for.
            let w = widths.next().unwrap_or(0.0);
            self.node(k, w).grow(w).shrink()
        });
        row().key(n.key()).w_full().gap(ROW_GAP).items_stretch().children(kids)
    }

    /// The quiet panel loose lines share at the top of an outline.
    fn panel(&self, kids: &[&Node], width: f32) -> El {
        let key = kids.first().map_or_else(|| "panel".into(), |k| format!("panel{}", k.key()));
        col().key(key).w_full().px(FRAME_X).py(FRAME_Y + 2.0).radius(RADIUS).fill(self.t.text.alpha(FILL))
            .children(self.stack(kids, width - 2.0 * FRAME_X))
    }

    fn frame(&self, n: &Node, width: f32, phone: bool, kind: &str) -> El {
        let t = self.t;
        // `box 1 · You dictate` is all title: the ` · ` split is for leaves.
        let title = if kind.is_empty() { n.part.name.clone() } else { n.words() };
        let inner = width - 2.0 * FRAME_X;
        let mut c = col().key(n.key()).px(FRAME_X).py(FRAME_Y).radius(if phone { 22.0 } else { RADIUS }).fill(t.text.alpha(FILL));
        if kind == "screen" {
            let dot = || el().w(7.0).h(7.0).radius(3.5).fill(t.text.alpha(0.12));
            let mut bar = row().key("bar").w_full().gap(5.0).pb(10.0).center_y().child(dot()).child(dot()).child(dot());
            if !title.is_empty() {
                bar = bar.child(text(title).font(QUIET).color(t.faint).pl(6.0).truncate().shrink().min_w(0.0));
            }
            c = c.child(bar);
        } else if !title.is_empty() {
            c = c.child(text(title).key("title").font(LABEL).color(t.faint).pb(4.0).wrap());
        }
        let kids: Vec<&Node> = n.kids.iter().collect();
        c = c.children(self.stack(&kids, inner));
        if phone { c.w(width.min(PHONE)).shrink() } else { c.w_full() }
    }

    /// Your words on the right in your bubble; everyone else's on the left under their name.
    fn say(&self, n: &Node, width: f32) -> El {
        let (t, p) = (self.t, &n.part);
        let how = p.tone.clone().unwrap_or_default();
        let spoken = how.contains("voice") || how.contains("spoken");
        let tools: Vec<&str> = n.kids.iter().filter(|k| k.part.name == "tools").map(|k| k.part.text.as_str()).collect();
        if self.r.is_you(&p.key) {
            let mut tag = row().gap(5.0).center_y().pr(4.0).child(spacer());
            if spoken {
                tag = tag.child(self.r.glyph(Glyph::Voice, 11.0, t.faint));
            }
            if !how.is_empty() {
                tag = tag.child(text(how).font(QUIET).color(t.faint));
            }
            let bubble = col().max_w((width * 0.78).min(380.0)).shrink().min_w(0.0).px(13.0).py(8.0).radius(16.0).fill(t.bubble)
                .child(self.r.inline(&p.text, BODY, t.text));
            return col().key(n.key()).w_full().gap(3.0).child(tag).child(row().w_full().child(spacer()).child(bubble));
        }
        let mut head = row().gap(6.0).center_y()
            .child(if spoken { self.r.glyph(Glyph::Voice, 11.0, t.muted) } else { mark(t.muted) })
            .child(text(p.key.clone()).font(WHO).color(t.text));
        if !how.is_empty() {
            head = head.child(text(how).font(QUIET).color(t.faint).truncate().shrink().min_w(0.0));
        }
        let mut c = col().key(n.key()).w_full().gap(3.0).child(head);
        if !tools.is_empty() {
            c = c.child(text(format!("Used {}", tools.join(", "))).font(QUIET).color(t.faint).wrap());
        }
        c.child(self.r.inline(&p.text, BODY, if spoken { t.muted } else { t.text }))
    }
}
