//! Flights in the motion log. Each `Tx::launch` the app makes ([`LAUNCHES`]) is followed from
//! the frame its destination mounts, in the frame its source's words leave, to the frame it
//! rests, and logged when it starts away from its source (`wrong-start`), starts off the window
//! (`wrong-side`), heads away from where it lands or overshoots it (`reversal`), ends away from
//! its rest (`end-snap`) or flies at another size than it rests at (`size`).
//!
//! A frame with no flight in the air costs a check of two empty lists. A flight's frames go in
//! a buffer taken from a pool, so following one allocates nothing per frame.

use serde_json::{Value, json};

use super::{Events, Sample, iso};
use rune::motion_gate::short;

/// The app's `Tx::launch` pairs (`send.rs`, `arrivals.rs`): a destination selector with `{q}`
/// for the prompt's id, and the sources it may leave from, the first one seen winning.
const LAUNCHES: &[(&str, &[&str])] = &[
    ("u-{q}/bubble", &["queued-{q}/label", "prompt"]),
    ("queued-{q}", &["prompt"]),
];
/// Destinations mounted but not yet drawn, and flights in the air, at most.
const PENDING: usize = 8;
const LIVE: usize = 8;
/// Frames a destination may take to show before it's forgotten.
const WAIT: u32 = 4;
/// Frames a flight keeps, 4 s at 120 Hz; one still moving after that is dropped, never logged.
const LONGEST: usize = 480;
/// How far outside its source's box a flight may start, in points at zoom 1.
pub const NEAR: f32 = 48.0;
/// How far its last flying frame may be from where it rests, in points.
pub const SNAP: f32 = 1.0;
/// How far it may move away from where it lands, in points.
pub const AWAY: f32 = 2.0;
/// How far past where it lands it may swing: this many points, or this share of its travel.
pub const OVERSHOOT: (f32, f32) = (4.0, 0.15);
/// How much its flying size may differ from its resting size.
pub const SIZE: f32 = 0.01;

/// The window a frame was drawn in.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub size: (f32, f32),
    pub zoom: f32,
    /// The open session's timeline shows its end.
    pub at_end: bool,
}

impl Window {
}

struct Pending {
    path: String,
    sources: Vec<String>,
    waited: u32,
}

struct Flight {
    path: String,
    key: String,
    source: Option<(String, [f32; 4])>,
    /// Each frame's rect and wall-clock milliseconds.
    frames: Vec<([f32; 4], f64)>,
    /// Frames since its rect last changed.
    still: usize,
    input: bool,
    scrolled: bool,
    at_end: bool,
}

#[derive(Default)]
pub struct Flights {
    pending: Vec<Pending>,
    live: Vec<Flight>,
    pool: Vec<Vec<([f32; 4], f64)>>,
    /// Flights followed to rest, logged or not.
    pub landed: u64,
}

impl Flights {
    /// An element seen for the first time: if it's a launch destination, it's watched for the
    /// next few frames.
    pub fn mount(&mut self, path: &str, key: Option<&str>) {
        let Some(key) = key else { return };
        if self.pending.len() >= PENDING { return; }
        for (dest, sources) in LAUNCHES {
            if let Some(q) = capture(dest, path, key) {
                let sources = sources.iter().map(|s| s.replace("{q}", &q)).collect();
                self.pending.push(Pending { path: path.to_owned(), sources, waited: 0 });
                return;
            }
        }
    }

    /// The latest frame is the ring's last. Returns a line for each rule a flight that came to
    /// rest in it broke.
    pub fn step(&mut self, ring: &std::collections::VecDeque<Sample>, window: Window, events: &Events) -> Vec<Value> {
        if self.pending.is_empty() && self.live.is_empty() { return Vec::new(); }
        let Some(cur) = ring.back() else { return Vec::new() };
        let prev = ring.len().checked_sub(2).map(|i| &ring[i]);
        let mut k = 0;
        while k < self.pending.len() {
            let p = &mut self.pending[k];
            let Some(e) = cur.get(&p.path) else {
                p.waited += 1;
                if p.waited > WAIT { self.pending.swap_remove(k); } else { k += 1; }
                continue;
            };
            let p = self.pending.swap_remove(k);
            // A launch: its source showed words in the frame before, and they left in this one.
            let source = prev.and_then(|prev| p.sources.iter().find_map(|sel| {
                let s = prev.els.iter().find(|s| selects(sel, &s.path, s.key.as_deref()) && s.text.is_some())?;
                let left = cur.get(&s.path).is_none_or(|now| now.exiting || now.text != s.text);
                left.then(|| (s.key.clone().unwrap_or_default(), s.rect))
            }));
            if source.is_none() || self.live.len() >= LIVE || self.live.iter().any(|f| f.path == p.path) { continue; }
            let mut frames = self.pool.pop().unwrap_or_else(|| Vec::with_capacity(LONGEST));
            frames.clear();
            frames.push((e.rect, cur.ms));
            self.live.push(Flight {
                key: e.key.clone().unwrap_or_default(),
                path: p.path,
                source,
                frames,
                still: 0,
                input: false,
                scrolled: e.scrolling,
                at_end: window.at_end,
            });
        }
        let mut lines = Vec::new();
        let mut k = 0;
        while k < self.live.len() {
            let f = &mut self.live[k];
            let Some(e) = cur.get(&f.path) else { self.drop_at(k); continue };
            if f.frames.len() > 1 { f.input |= cur.input; }
            f.scrolled |= e.scrolling;
            let last = f.frames.last().unwrap().0;
            // A spring creeps in steps far under a point before it settles exactly.
            f.still = if (0..4).all(|a| (e.rect[a] - last[a]).abs() < 1e-4) { f.still + 1 } else { 0 };
            f.frames.push((e.rect, cur.ms));
            if f.still >= 2 {
                let f = self.live.swap_remove(k);
                self.landed += 1;
                lines.extend(f.lines(window, events));
                self.pool.push(f.frames);
            } else if f.frames.len() >= LONGEST {
                self.drop_at(k);
            } else {
                k += 1;
            }
        }
        lines
    }

    fn drop_at(&mut self, k: usize) {
        let f = self.live.swap_remove(k);
        self.pool.push(f.frames);
    }

}

impl Flight {
    /// Each rule it broke, with the detail in numbers.
    fn verdicts(&self, window: Window) -> Vec<(&'static str, String)> {
        let r: Vec<[f32; 4]> = self.frames.iter().map(|f| f.0).collect();
        let rest_at = r.len() - 1 - self.still;
        if rest_at == 0 { return Vec::new(); }
        let rest = r[rest_at];
        let flying = &r[..rest_at];
        let start = r[0];
        let mut out = Vec::new();
        if let Some((key, s)) = &self.source {
            let near = NEAR * window.zoom;
            let off = |v: f32, lo: f32, len: f32| if v < lo - near { v - lo } else if v > lo + len + near { v - (lo + len) } else { 0.0 };
            let (dx, dy) = (off(start[0], s[0], s[2]), off(start[1], s[1], s[3]));
            if dx != 0.0 || dy != 0.0 {
                out.push(("wrong-start", format!(
                    "starts at ({:.1}, {:.1}), {dx:.1}, {dy:.1} pt outside its source {key} at ({:.1}, {:.1}, {:.1}, {:.1}) (allowed {near:.0} pt)",
                    start[0], start[1], s[0], s[1], s[2], s[3],
                )));
            }
        }
        let (w, h) = window.size;
        if start[0] < -1.0 || start[1] < -1.0 || start[0] > w - 1.0 || start[1] > h - 1.0 {
            out.push(("wrong-side", format!("starts off the window at ({:.1}, {:.1}); the window is {w:.0} x {h:.0}", start[0], start[1])));
        }
        for (a, axis) in [(0, "x"), (1, "y")] {
            let d: Vec<f32> = flying.iter().map(|f| f[a] - rest[a]).collect();
            let travel = d[0].abs();
            let side = if travel < AWAY { 0.0 } else { d[0].signum() };
            let (mut low, mut away, mut over) = (travel, (0.0f32, 0usize), (0.0f32, 0usize));
            for (k, &v) in d.iter().enumerate().skip(1) {
                if side == 0.0 || v.signum() == side {
                    if v.abs() - low > away.0 { away = (v.abs() - low, k); }
                    low = low.min(v.abs());
                } else if v.abs() > over.0 {
                    over = (v.abs(), k);
                }
            }
            if away.0 > AWAY {
                out.push(("reversal", format!(
                    "{axis} moves {:.1} pt away from where it lands by frame {}, then back; it starts {:.1} pt off and rests at {:.1}",
                    away.0, away.1, d[0], rest[a],
                )));
            }
            if side != 0.0 && over.0 > OVERSHOOT.0.max(OVERSHOOT.1 * travel) {
                out.push(("reversal", format!(
                    "{axis} swings {:.1} pt past where it lands at frame {}, then back, on a {travel:.1} pt flight",
                    over.0, over.1,
                )));
            }
        }
        let last = flying[flying.len() - 1];
        let (dx, dy, dw, dh) = (last[0] - rest[0], last[1] - rest[1], last[2] - rest[2], last[3] - rest[3]);
        if dx.abs().max(dy.abs()) > SNAP || dw.abs().max(dh.abs()) > 0.5 {
            out.push(("end-snap", format!(
                "its last flying frame is {dx:.1}, {dy:.1} pt from where it rests, and {dw:.1} x {dh:.1} pt in size",
            )));
        }
        let same = flying.iter().all(|f| (f[2] - start[2]).abs() <= 0.005 * start[2].max(1.0) && (f[3] - start[3]).abs() <= 0.005 * start[3].max(1.0));
        let (sw, sh) = ((start[2] - rest[2]) / rest[2].max(1.0), (start[3] - rest[3]) / rest[3].max(1.0));
        if same && sw.abs().max(sh.abs()) > SIZE {
            out.push(("size", format!(
                "flies at {:.1} x {:.1} and rests at {:.1} x {:.1} ({:+.1}%, {:+.1}%)",
                start[2], start[3], rest[2], rest[3], sw * 100.0, sh * 100.0,
            )));
        }
        out
    }

    fn lines(&self, window: Window, events: &Events) -> Vec<Value> {
        let verdicts = self.verdicts(window);
        if verdicts.is_empty() { return Vec::new(); }
        let rest_at = self.frames.len() - 1 - self.still;
        let t0 = self.frames[0].1;
        let at = |label: &str, k: usize| json!({
            "at": label,
            "frame": k,
            "ms": ((self.frames[k].1 - t0) * 10.0).round() / 10.0,
            "rect": self.frames[k].0,
        });
        let mut frames = vec![at("start", 0)];
        let mids = rest_at.saturating_sub(2);
        for i in 1..=mids.min(4) {
            frames.push(at("mid", i * (rest_at - 1) / (mids.min(4) + 1)));
        }
        if rest_at > 1 { frames.push(at("end", rest_at - 1)); }
        frames.push(at("rest", rest_at));
        let (w, h) = window.size;
        verdicts.into_iter().map(|(rule, detail)| json!({
            "time": iso(self.frames[rest_at].1),
            "rule": rule,
            "keys": [self.key],
            "element": short(&self.path),
            "detail": detail,
            "flight": true,
            "source": self.source.as_ref().map(|(key, rect)| json!({"key": key, "rect": rect})),
            "zoom": window.zoom,
            "window": [w, h],
            "at_end": self.at_end,
            "input": self.input,
            "scrolled": self.scrolled,
            "frames": frames,
            "events": events.to_json(),
        })).collect()
    }
}

/// The `{q}` a destination selector captures for an element, if it names it. A selector names an
/// element by its key, scoped by its keyed ancestors from the nearest up, as Rune's flights do.
fn capture(selector: &str, path: &str, key: &str) -> Option<String> {
    let above = path.strip_suffix(key)?;
    let above = above.strip_suffix('/').unwrap_or(above);
    let mut keyed = std::iter::once(key).chain(above.rsplit('/').filter(|s| !s.starts_with('#') && !s.is_empty()));
    let mut q: Option<String> = None;
    for seg in selector.rsplit('/') {
        let got = keyed.next()?;
        match seg.split_once("{q}") {
            Some((pre, post)) => {
                let mid = got.strip_prefix(pre)?.strip_suffix(post)?;
                if mid.is_empty() || q.as_deref().is_some_and(|q| q != mid) { return None; }
                q = Some(mid.to_owned());
            }
            None if seg == got => {}
            None => return None,
        }
    }
    q
}

/// Rune's `flight::selects`: `selector` names the element at `path` keyed `key`.
fn selects(selector: &str, path: &str, key: Option<&str>) -> bool {
    let Some(key) = key else { return false };
    if selector == key { return true; }
    let Some(scope) = selector.strip_suffix(key).and_then(|s| s.strip_suffix('/')) else { return false };
    let Some(above) = path.strip_suffix(key).and_then(|s| s.strip_suffix('/')) else { return false };
    let mut keyed = above.rsplit('/').filter(|seg| !seg.starts_with('#'));
    scope.rsplit('/').all(|seg| keyed.next() == Some(seg))
}

