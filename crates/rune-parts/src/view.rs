//! Outlines drawn as Rune elements. Each app passes its colours ([`PartsTheme`]) and how it draws
//! rich words and glyphs ([`PartsResolver`]); nothing here knows about Seer or OpenCode.

use rune::{Color, El, TextStyle, col, el, row, spacer, text};

use crate::catalog::Tone;
use crate::outline::Node;

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

/// What a surface draws its own way: words with chips in them, glyphs, and who "you" are.
pub trait PartsResolver {
    /// Words that may hold Markdown, PR and card references.
    fn inline(&self, src: &str, font: TextStyle, color: Color) -> El {
        text(src.to_owned()).font(font).color(color).wrap()
    }

    fn glyph(&self, g: Glyph, size: f32, color: Color) -> El {
        let s = match g {
            Glyph::Arrow => "→",
            Glyph::ArrowDown => "↓",
            Glyph::Voice => "◉",
        };
        text(s).font(TextStyle::new(size, 500.0).lh(size + 4.0)).color(color)
    }

    /// Chat lines from you draw on the right, in your bubble.
    fn is_you(&self, who: &str) -> bool {
        matches!(who.to_ascii_lowercase().as_str(), "kit" | "you" | "me")
    }
}

/// Plain words and text glyphs, for tests and surfaces without chips.
pub struct Plain;
impl PartsResolver for Plain {}

const BODY: TextStyle = TextStyle::new(14.0, 420.0).lh(21.0).tracking(-0.1);
const TITLE: TextStyle = TextStyle::new(14.0, 600.0).lh(21.0).tracking(-0.1);
const WHO: TextStyle = TextStyle::new(12.5, 560.0).lh(18.0);
const QUIET: TextStyle = TextStyle::new(12.0, 440.0).lh(18.0);
const CAPS: TextStyle = TextStyle::new(11.0, 560.0).lh(20.0).tracking(0.6);
const CHIP: TextStyle = TextStyle::new(11.5, 440.0).mono().lh(16.0);
const MARK: f32 = 8.0;
const PAD: f32 = 10.0;

struct Ctx<'a> {
    t: &'a PartsTheme,
    r: &'a dyn PartsResolver,
}

/// An outline `width` points wide.
pub fn outline(nodes: &[Node], width: f32, t: &PartsTheme, r: &dyn PartsResolver) -> El {
    let cx = Ctx { t, r };
    col().key("ui").w_full().gap(12.0).children(nodes.iter().map(|n| cx.node(n, width)))
}

fn mark(c: Color) -> El {
    el().key("mark").w(MARK).h(MARK).radius(MARK * 0.2).fill(c)
}

fn tone_of(t: &PartsTheme, word: Option<&str>) -> Option<Color> {
    word.map(|w| t.tone(Tone::of(w)))
}

impl Ctx<'_> {
    fn kids(&self, n: &Node, width: f32) -> Vec<El> {
        n.kids.iter().map(|k| self.node(k, width)).collect()
    }

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
            "col" => col().key(key).grow(1.0).shrink().min_w(0.0).gap(10.0).children(self.kids(n, width)),
            "box" => self.frame(n, width, false, "box"),
            "screen" => self.frame(n, width, false, "screen"),
            "phone" => self.frame(n, width.min(250.0), true, "phone"),
            "arrow" => row().key(key).self_center().child(self.r.glyph(if width < NARROW { Glyph::ArrowDown } else { Glyph::Arrow }, 16.0, t.faint)),
            "title" => self.r.inline(&p.text, TITLE, t.text).key(key),
            "text" => self.r.inline(&p.text, BODY, t.muted).key(key),
            "item" => {
                let mut r = row().key(key).w_full().gap(8.0).items_start().px(PAD).py(6.0).radius(8.0).fill(t.text.alpha(0.04));
                if let Some(c) = tone_of(t, p.tone.as_deref()) {
                    r = r.child(row().h(21.0).center_y().child(mark(c)));
                }
                r = r.child(col().grow(1.0).shrink().min_w(0.0).child(self.r.inline(&p.text, BODY, t.text)));
                if !p.detail.is_empty() {
                    r = r.child(text(p.detail.clone()).font(QUIET).color(t.faint).pt(2.0).shrink().min_w(0.0).truncate());
                }
                r
            }
            "kv" => {
                let mut v = row().grow(1.0).shrink().min_w(0.0).gap(8.0).items_start();
                if let Some(c) = tone_of(t, p.tone.as_deref()) {
                    v = v.child(row().h(21.0).center_y().child(mark(c)));
                }
                v = v.child(col().grow(1.0).shrink().min_w(0.0).child(self.r.inline(&n.part.text, BODY, t.text)));
                row().key(key).w_full().gap(12.0).items_start().px(PAD).py(6.0).radius(8.0).fill(t.text.alpha(0.04))
                    .child(text(p.key.clone()).font(BODY).color(t.muted).w((width * 0.22).clamp(64.0, 120.0)).shrink().truncate())
                    .child(v)
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

    /// Side by side when there's room, stacked below [`NARROW`].
    fn row(&self, n: &Node, width: f32) -> El {
        let gap = 16.0;
        if width < NARROW {
            return col().key(n.key()).w_full().gap(10.0).children(self.kids(n, width));
        }
        let panes = n.kids.iter().filter(|k| k.part.name != "arrow").count().max(1) as f32;
        let arrows = n.kids.len() as f32 - panes;
        let each = (width - arrows * 16.0 - gap * (n.kids.len().max(1) - 1) as f32) / panes;
        row().key(n.key()).w_full().gap(gap).items_start().children(n.kids.iter().map(|k| {
            match k.part.name.as_str() {
                "arrow" => self.node(k, width).self_center(),
                "phone" => self.node(k, each),
                _ => self.node(k, each).grow(1.0).shrink().min_w(0.0),
            }
        }))
    }

    fn frame(&self, n: &Node, width: f32, phone: bool, kind: &str) -> El {
        let t = self.t;
        let title = if kind.is_empty() { n.part.name.clone() } else { n.part.text.clone() };
        let inner = width - 32.0;
        let mut c = col().key(n.key()).px(16.0).py(12.0).gap(12.0).radius(if phone { 22.0 } else { 12.0 }).border(0.5, t.ring);
        if kind == "screen" {
            let dot = || el().w(7.0).h(7.0).radius(3.5).fill(t.text.alpha(0.12));
            let mut bar = row().key("bar").w_full().gap(5.0).center_y().child(dot()).child(dot()).child(dot());
            if !title.is_empty() {
                bar = bar.child(text(title).font(QUIET).color(t.faint).pl(6.0).truncate().shrink().min_w(0.0));
            }
            c = c.child(bar);
        } else if !title.is_empty() {
            c = c.child(text(title.to_uppercase()).key("title").font(CAPS).color(t.faint));
        }
        c = c.children(n.kids.iter().map(|k| self.node(k, inner)));
        if phone { c.w(width).shrink() } else { c.w_full() }
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
