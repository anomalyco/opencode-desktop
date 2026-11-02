//! A reply streaming into the pinned timeline at four paces (`stream_pace.rs`): a word every
//! 300 ms, every 50 ms, six a frame, and bursts with pauses. The timeline climbs steadily at
//! each, with no rest and kick per word or line (`stop-start`).

use super::{Run, SID};
use crate::stream_pace::{Pace, queue};

fn stream(r: &mut Run, pace: Pace, words: usize) {
    r.act("the reply starts", |rt| rt.update(|s, _| queue(s, SID, "msg_zzstreampace", pace, words)));
    r.play_out(1.5);
}

pub fn slow(r: &mut Run) {
    stream(r, Pace::Steady(1.0 / 0.3), 40);
}

pub fn medium(r: &mut Run) {
    stream(r, Pace::Steady(20.0), usize::MAX);
}

pub fn fast(r: &mut Run) {
    stream(r, Pace::Burst, usize::MAX);
}

pub fn bursty(r: &mut Run) {
    stream(r, Pace::Bursty, usize::MAX);
}
