//! How a streaming reply shows each line it wraps onto (Timeline › New line reveal, p252).
//! Working sits its lane's gap under the reply; the reply's row opens each new line on its own
//! quick spring (Line grow), and nothing it holds paints below the row's moving edge:
//!
//! - Clip: the row clips its words at its edge, so the line shows top-down as the row opens.
//! - Grow, then show: the row opens first, and the line's words come once it has.
//! - Soft edge: Clip, its edge fading over a few points while it moves.
//! - Held room: the line of room under a streaming reply that #304 kept, for comparison.
//!
//! A reply with a fenced block in it (a diagram, an outline, code) climbs at the stream's pace
//! instead: one build can add many lines.

use std::collections::{HashMap, VecDeque};

use rune::{El, Spring, Tx, col};
use rune_knobs::{Knob, Knobs};

use crate::TimelineHost;

pub const NEW_LINE_REVEAL: Knob = Knob::choice("New line reveal", &["Clip", "Grow, then show", "Soft edge", "Held room"], 2)
    .about("How streamed text shows each new line")
    .hints(&["Row clips its words as it opens", "Words come once the line is open", "Clip edge fades while it moves", "A line of room under the reply"]);
/// The row taking a new line, as a spring's duration in milliseconds.
pub const LINE_GROW: Knob = Knob::new("Line grow", 40.0, 240.0, 80.0).step(5.0).about("Milliseconds streaming text takes to open a line");
/// Soft edge's fade, in points.
pub const SOFT_EDGE: Knob = Knob::new("Soft edge", 2.0, 24.0, 8.0).step(1.0).about("Points the soft clip edge fades over");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reveal {
    Clip,
    GrowThenShow,
    SoftEdge,
    HeldRoom,
}

impl Reveal {
    pub const ALL: [Reveal; 4] = [Reveal::Clip, Reveal::GrowThenShow, Reveal::SoftEdge, Reveal::HeldRoom];

    /// Its value on [`NEW_LINE_REVEAL`].
    pub fn knob(self) -> f32 {
        Reveal::ALL.iter().position(|&r| r == self).unwrap_or(0) as f32
    }
}

pub fn reveal(k: &Knobs) -> Reveal {
    Reveal::ALL.get(k.get(&NEW_LINE_REVEAL).round() as usize).copied().unwrap_or(Reveal::Clip)
}

pub fn line_grow(k: &Knobs) -> Spring {
    Spring::new(k.get(&LINE_GROW) / 1000.0, 0.0)
}

/// The feather on the row's moving edge, in points: none but for Soft edge.
pub fn feather(k: &Knobs) -> f32 {
    if reveal(k) == Reveal::SoftEdge { k.get(&SOFT_EDGE) } else { 0.0 }
}

/// The room Held room keeps under a streaming reply, over Working: a line of reply text.
pub(crate) const CUSHION: f32 = 24.0;

/// Grow, then show: how much of each streaming text part shows, by "message:part". Its words
/// come a line-grow behind the server's, while its row already takes their height.
#[derive(Default)]
pub struct Hold {
    /// When each part's text reached each length, oldest first.
    seen: HashMap<String, VecDeque<(f64, usize)>>,
    /// The bytes of each part that show.
    shown: HashMap<String, usize>,
}

impl Hold {
    pub fn shown(&self, message: &str, part: usize) -> Option<usize> {
        if self.shown.is_empty() {
            return None;
        }
        self.shown.get(&format!("{message}:{part}")).copied()
    }
}

/// Keeps [`Hold`] up with the open session's streaming text.
pub fn poll<S: TimelineHost>(s: &mut S, now: f64, tx: &mut Tx) -> bool {
    if reveal(s.knobs()) != Reveal::GrowThenShow {
        let had = !s.line_hold().shown.is_empty();
        if had {
            *s.line_hold_mut() = Hold::default();
        }
        return had;
    }
    let delay = f64::from(s.knobs().get(&LINE_GROW)) / 1000.0;
    let mut hold = std::mem::take(s.line_hold_mut());
    let mut lens: Vec<(String, usize)> = Vec::new();
    let s_ref: &S = &*s;
    if let Some(sid) = s_ref.active_session_id()
        && s_ref.is_session_live(sid)
    {
        s_ref.with_transcript(sid, |t| {
            if let Some(opencode_kit::model::Message::Assistant(a)) = t.messages.last() {
                for (i, part) in a.parts.iter().enumerate() {
                    if let opencode_kit::model::Part::Text { text } = part {
                        lens.push((format!("{}:{i}", a.id), text.len()));
                    }
                }
            }
        });
    }
    let live: std::collections::HashSet<String> = lens.iter().map(|(k, _)| k.clone()).collect();
    let Hold { seen, shown } = &mut hold;
    for (key, len) in lens {
        let at = seen.entry(key).or_insert_with(|| VecDeque::from([(now - delay, 0)]));
        match at.back() {
            Some(&(_, last)) if len < last => *at = VecDeque::from([(now - delay, len)]),
            Some(&(_, last)) if len == last => {}
            _ => at.push_back((now, len)),
        }
    }
    let (mut changed, mut next) = (false, f64::INFINITY);
    seen.retain(|key, at| {
        // What showed a line-grow ago shows now; once it's all shown, the part shows whole.
        while at.len() > 1 && at[1].0 <= now - delay + 1e-9 {
            at.pop_front();
        }
        if let Some(&(t, _)) = at.get(1) {
            next = next.min(t + delay - now);
        }
        let held = (at.len() > 1).then_some(at[0].1);
        if shown.get(key).copied() != held {
            changed = true;
            match held {
                Some(n) => shown.insert(key.clone(), n),
                None => shown.remove(key),
            };
        }
        // Its last length is the base for the next words while it streams.
        at.len() > 1 || live.contains(key)
    });
    *s.line_hold_mut() = hold;
    if next.is_finite() {
        tx.wake_after(std::time::Duration::from_secs_f64(next.max(0.0)));
    }
    changed
}

/// A reply's words (`md` of the text and its word fade) in `body`, its padded column. For Grow,
/// then show, the column takes the whole text's height from an unseen copy in the same grid cell
/// while only the first `shown` bytes draw, so the row opens a line before its words come. The
/// words stay its first child either way, keeping their place and their names.
pub fn words(body: El, md: impl Fn(&str, f32) -> El, text: &str, fade: f32, shown: Option<usize>) -> El {
    match shown.filter(|&n| n < text.len() && text.is_char_boundary(n)) {
        None => body.child(md(text, fade)),
        Some(n) => body
            .equal_columns(1)
            .child(md(&text[..n], fade).grid_cell(0))
            .child(col().key("words-size").w_full().grid_cell(0).opacity(0.0).inert().child(md(text, 0.0))),
    }
}

/// The room over Working: [`CUSHION`] for Held room, else none.
pub fn room<S: TimelineHost>(s: &S) -> f32 {
    if reveal(s.knobs()) == Reveal::HeldRoom { CUSHION } else { 0.0 }
}
