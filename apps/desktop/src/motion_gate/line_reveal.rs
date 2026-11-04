//! The Streaming line growth run (`line_grow_story.rs`) at a slow pace, once for each New line
//! reveal (`line_reveal`): a pinned timeline climbs one quick line at a time as the answer wraps.

use super::Run;
use crate::line_grow_story::{SLOW, play, script};
use crate::line_reveal::{NEW_LINE_REVEAL, Reveal};

fn stream(r: &mut Run, reveal: Reveal) {
    r.rt.update(|s, _| s.knobs.set(&NEW_LINE_REVEAL, reveal.knob()));
    r.act("the run starts", |rt| rt.update(|s, _| play(s, SLOW)));
    r.play(script(SLOW).last().map_or(0.0, |e| e.0) + 1.0);
}

pub fn clip(r: &mut Run) {
    stream(r, Reveal::Clip);
}

pub fn grow_then_show(r: &mut Run) {
    stream(r, Reveal::GrowThenShow);
}

pub fn soft_edge(r: &mut Run) {
    stream(r, Reveal::SoftEdge);
}

pub fn held_room(r: &mut Run) {
    stream(r, Reveal::HeldRoom);
}
