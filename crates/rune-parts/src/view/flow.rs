//! How a `row` shares its column (t1197). Each pane is as wide as its words want; when they don't
//! all fit, the tallest pane gets room first so the row's shared height stays low; and when even
//! that would wrap a sentence past two lines, the row stacks instead of crushing its words.

use std::cell::RefCell;
use std::collections::HashMap;

use rune::TextStyle;

use super::{ARROW_SIZE, BODY, CHIP, DETAIL_GAP, FRAME_X, FRAME_Y, LABEL, MARK, PHONE, QUIET, ROW_GAP, TITLE, WHO, gap_between, measure, ruled, widest_word};
use crate::outline::Node;

/// A pane wanting more than this wraps rather than crowding the row.
pub const PANE_MAX: f32 = 320.0;
/// The narrowest a pane gets, however short its words.
pub const PANE_MIN: f32 = 96.0;
/// The most lines a sentence in a pane wraps to before the row stacks.
const MAX_LINES: f32 = 2.0;
/// Widths are tried this far apart.
const STEP: f32 = 4.0;

/// One pane of a row: the width that sets it on one line (`want`), the least it takes before a
/// sentence wraps past two lines (`floor`), the most it grows to when the row has room to spare
/// (a phone stays phone-sized; the rest share the column), and its height at each width
/// from `floor` to `want`, [`STEP`] apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub want: f32,
    pub floor: f32,
    pub max: f32,
    pub heights: Vec<f32>,
}

impl Pane {
    /// The narrowest width that keeps this pane within `h` tall.
    fn width_for(&self, h: f32) -> f32 {
        self.heights.iter().position(|&x| x <= h + 0.5).map_or(self.want, |i| (self.floor + i as f32 * STEP).min(self.want))
    }
}

pub(super) fn pane(n: &Node) -> Pane {
    let phone = n.part.name == "phone";
    let (want, word) = natural(n);
    let want = want.min(if phone { PHONE } else { PANE_MAX });
    let mut floor = PANE_MIN.max(word).min(want);
    while floor < want && shape(n, floor).1 > MAX_LINES {
        floor += STEP;
    }
    let floor = floor.min(want);
    let mut heights = Vec::new();
    let mut w = floor;
    while w < want {
        heights.push(shape(n, w).0);
        w += STEP;
    }
    heights.push(shape(n, want).0);
    Pane { want, floor, max: if phone { PHONE } else { f32::INFINITY }, heights }
}

/// Each pane's width when the row fits `width` side by side, or `None` when it must stack.
pub fn plan_row(panes: &[Pane], arrows: usize, width: f32) -> Option<Vec<f32>> {
    let chrome = arrows as f32 * ARROW_SIZE + (panes.len() + arrows).saturating_sub(1) as f32 * ROW_GAP;
    let room = width - chrome;
    if panes.is_empty() || panes.iter().map(|p| p.floor).sum::<f32>() > room + 0.5 {
        return None;
    }
    if panes.iter().map(|p| p.want).sum::<f32>() <= room {
        let caps: Vec<f32> = panes.iter().map(|p| p.max).collect();
        return Some(spread(panes.iter().map(|p| p.want).collect(), &caps, room));
    }
    // The lowest shared height the room allows, then the spare shared out.
    let mut tall: Vec<f32> = panes.iter().flat_map(|p| p.heights.iter().copied()).collect();
    tall.sort_by(f32::total_cmp);
    let caps: Vec<f32> = panes.iter().map(|p| p.want).collect();
    let fits = tall.iter().map(|&h| panes.iter().map(|p| p.width_for(h)).collect::<Vec<_>>()).find(|w| w.iter().sum::<f32>() <= room);
    let w = fits.unwrap_or_else(|| panes.iter().map(|p| p.floor).collect());
    let mut w: Vec<f32> = spread(w, &caps, room).into_iter().map(f32::floor).collect();
    let short = (room - w.iter().sum::<f32>()).max(0.0);
    if let Some(last) = w.last_mut() {
        *last += short;
    }
    Some(w)
}

/// `w` plus an even share of what's left of `room`, none past its cap.
fn spread(mut w: Vec<f32>, caps: &[f32], room: f32) -> Vec<f32> {
    for _ in 0..w.len() {
        let spare = room - w.iter().sum::<f32>();
        let open: Vec<usize> = (0..w.len()).filter(|&i| w[i] < caps[i] - 0.01).collect();
        if open.is_empty() || spare < 0.5 {
            break;
        }
        let each = spare / open.len() as f32;
        for i in open {
            w[i] = (w[i] + each).min(caps[i]);
        }
    }
    w
}

thread_local! {
    static PLANS: RefCell<HashMap<(u64, u32, u64), Option<Vec<f32>>>> = RefCell::new(HashMap::new());
}

/// [`plan_row`] for a row node, kept per outline and width: a row is re-planned only when its
/// lines, the width or the fonts change.
pub(super) fn planned(n: &Node, width: f32) -> Option<Vec<f32>> {
    let key = (tree_hash(n), width.to_bits(), rune::text::font_epoch());
    if let Some(p) = PLANS.with_borrow(|m| m.get(&key).cloned()) {
        return p;
    }
    let panes: Vec<Pane> = n.kids.iter().filter(|k| k.part.name != "arrow").map(pane).collect();
    let plan = plan_row(&panes, n.kids.len() - panes.len(), width);
    PLANS.with_borrow_mut(|m| {
        if m.len() > 256 {
            m.clear();
        }
        m.insert(key, plan.clone());
    });
    plan
}

fn tree_hash(n: &Node) -> u64 {
    n.kids.iter().fold(n.hash ^ (n.line as u64).rotate_left(32), |h, k| (h ^ tree_hash(k)).wrapping_mul(0x0100_0000_01b3))
}

/// What a part wants on one line, and its widest word, in points.
fn natural(n: &Node) -> (f32, f32) {
    let p = &n.part;
    let line = |s: &str, f: TextStyle| (measure(s, f), widest_word(s, f));
    let side = |a: (f32, f32), extra: f32| (a.0 + extra, a.1 + extra);
    // Chips and Markdown draw a touch wider than their raw words.
    let slack = |a: (f32, f32)| (a.0.ceil() + 2.0, a.1.ceil());
    let Some(spec) = n.spec() else {
        return if n.kids.is_empty() { slack(line(&n.words(), QUIET)) } else { framed(n, &p.name) };
    };
    match spec.name {
        "row" => {
            let ps: Vec<Pane> = n.kids.iter().filter(|k| k.part.name != "arrow").map(pane).collect();
            let chrome = (n.kids.len() - ps.len()) as f32 * ARROW_SIZE + n.kids.len().saturating_sub(1) as f32 * ROW_GAP;
            (ps.iter().map(|p| p.want).sum::<f32>() + chrome, ps.iter().map(|p| p.floor).fold(0.0, f32::max))
        }
        "col" => n.kids.iter().map(natural).fold((0.0, 0.0), |a, b| (a.0.max(b.0), a.1.max(b.1))),
        "box" | "screen" | "phone" => framed(n, &n.words()),
        _ => slack(match spec.name {
            "arrow" => (ARROW_SIZE, ARROW_SIZE),
            "title" => line(&p.text, TITLE),
            "text" => line(&p.text, BODY),
            "item" => side(line(&p.text, BODY), MARK + 8.0 + if p.detail.is_empty() { 0.0 } else { measure(&p.detail, QUIET) + DETAIL_GAP }),
            "kv" => {
                let v = line(&p.text, BODY);
                let mark = if p.tone.is_some() { MARK + 8.0 } else { 0.0 };
                (measure(&p.key, BODY).min(120.0) + 12.0 + mark + v.0, v.1.max(widest_word(&p.key, BODY)))
            }
            "say" => {
                let b = line(&p.text, BODY);
                (b.0.min(380.0) + 26.0, b.1 + 26.0)
            }
            "tools" => line(&format!("Used {}", p.text), QUIET),
            "from" => side(line(&p.text, WHO), measure("From", QUIET) + 24.0),
            "toggle" => side(line(&p.text, BODY), 42.0),
            "button" => side(line(&p.text, WHO), 24.0),
            "field" => (160.0, 100.0),
            "chips" => {
                let ws: Vec<f32> = p.text.split(',').map(str::trim).filter(|w| !w.is_empty()).map(|w| measure(w, CHIP) + 10.0).collect();
                (ws.iter().sum::<f32>() + 6.0 * ws.len().saturating_sub(1) as f32, ws.iter().copied().fold(0.0, f32::max))
            }
            _ => (0.0, 0.0),
        }),
    }
}

fn framed(n: &Node, title: &str) -> (f32, f32) {
    let label = if n.part.name == "screen" { measure(title, QUIET) + 45.0 } else { measure(title, LABEL) };
    let (w, word) = n.kids.iter().map(natural).fold((label, widest_word(title, LABEL)), |a, b| (a.0.max(b.0), a.1.max(b.1)));
    (w + 2.0 * FRAME_X, word + 2.0 * FRAME_X)
}

/// About how tall a part draws `w` points wide, and the most lines any sentence in it wraps to.
fn shape(n: &Node, w: f32) -> (f32, f32) {
    let p = &n.part;
    let wrap = |s: &str, f: TextStyle, w: f32| {
        let h = if s.is_empty() { 0.0 } else { rune::text::measure(s, &[], f, Some(w.max(1.0))).1 };
        (h, (h / f.line_height).round())
    };
    let Some(spec) = n.spec() else {
        return if n.kids.is_empty() { wrap(&n.words(), QUIET, w) } else { frame(n, &p.name, w) };
    };
    match spec.name {
        "box" | "screen" | "phone" => frame(n, &n.words(), w),
        "col" => stacked(&n.kids, w, 0.0),
        "row" => (n.kids.iter().map(|k| shape(k, w / n.kids.len().max(1) as f32).0).fold(0.0, f32::max), 0.0),
        "title" => wrap(&p.text, TITLE, w),
        "text" => wrap(&p.text, BODY, w),
        "item" => {
            let room = w - MARK - 8.0;
            if p.detail.is_empty() || measure(&p.text, BODY) + DETAIL_GAP + measure(&p.detail, QUIET) <= room {
                wrap(&p.text, BODY, room)
            } else {
                let (a, b) = (wrap(&p.text, BODY, room), wrap(&p.detail, QUIET, room));
                (a.0 + b.0, a.1.max(b.1))
            }
        }
        "kv" => (wrap(&p.text, BODY, w - 132.0).0 + 4.0, 0.0),
        "say" => {
            let b = wrap(&p.text, BODY, (w * 0.78).min(380.0) - 26.0);
            (b.0 + 37.0 + if n.kids.is_empty() { 0.0 } else { 21.0 }, b.1)
        }
        "toggle" => wrap(&p.text, BODY, w - 42.0),
        "tools" | "from" | "chips" => (18.0, 0.0),
        "button" => (26.0, 0.0),
        "field" => (30.0, 0.0),
        "arrow" => (ARROW_SIZE, 0.0),
        _ => (0.5, 0.0),
    }
}

fn frame(n: &Node, title: &str, w: f32) -> (f32, f32) {
    let inner = w - 2.0 * FRAME_X;
    let top = match (n.part.name.as_str(), title.is_empty()) {
        ("screen", _) => 17.0,
        (_, true) => 0.0,
        _ => rune::text::measure(title, &[], LABEL, Some(inner.max(1.0))).1 + 4.0,
    };
    let (h, l) = stacked(&n.kids, inner, top);
    (h + 2.0 * FRAME_Y, l)
}

/// Kids one under another, spaced as a frame spaces them.
fn stacked(kids: &[Node], w: f32, top: f32) -> (f32, f32) {
    let mut prev = None;
    kids.iter().fold((top, 0.0f32), |(h, l), k| {
        let (kh, kl) = shape(k, w);
        let gap = gap_between(prev, &k.part.name) * if ruled(prev, &k.part.name) { 2.0 } else { 1.0 };
        prev = Some(k.part.name.as_str());
        (h + gap + kh, l.max(kl))
    })
}
