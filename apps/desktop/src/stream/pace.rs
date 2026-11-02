//! A reply streaming at any speed, for the storybook (Stream › Text speed) and the motion gate's
//! `stream-pace-*` cells: its words arrive one at a time, on a steady pace or in bursts with
//! pauses between, as models stream. The timeline under it should climb calmly at every pace,
//! and no `**`, `` ` `` or half-written link should show while it does.

use rune::Play;
use rune_knobs::{Group, Knob};
use serde_json::{Value, json};

use crate::App;
use crate::stories::{Story, Trigger, replay, sid};

pub const REPLY: &str = "The reconnect jitter comes from **two retries racing** after a drop: the socket's own `onclose` handler and the [health check](https://example.com/health) both call `reconnect()`, so a short blip schedules two attempts a few milliseconds apart. Whichever lands second sees a half-open socket, gives up, and backs off for the full 30 s.\n\nThe fix is one guard in `net/backoff.ts`: keep a single pending attempt and let later callers *join* it instead of starting their own. I added a test that drops the socket twice in a row and checks that only one attempt runs, and **all 14 unit tests pass**.";

pub const SPEED: Knob = Knob::new("Stream speed", 1.0, 60.0, 3.3).step(0.1).label("Words a second").about("Words a second a steady reply streams");

pub const GROUP: Group = Group::new("Stream", "stream", &[SPEED]).under("Timeline");

rune_register::register!(crate::knobs::GROUP_LIST: Group = [GROUP]);

#[derive(Clone, Copy, Debug)]
pub enum Pace {
    /// This many words a second.
    Steady(f64),
    /// Six words a frame at 60 Hz.
    Burst,
    /// Runs of 4 to 12 words a few ms apart, with pauses of 0.1 to 0.9 s between.
    Bursty,
}

/// When each of `n` words arrives, in seconds from the first.
pub fn times(pace: Pace, n: usize) -> Vec<f64> {
    match pace {
        Pace::Steady(rate) => (0..n).map(|i| i as f64 / rate).collect(),
        Pace::Burst => (0..n).map(|i| (i / 6) as f64 / 60.0).collect(),
        Pace::Bursty => {
            let (mut t, mut out, mut seed) = (0.0, Vec::new(), 7u32);
            let mut rand = || {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (seed >> 16) as f64 / 65_536.0
            };
            while out.len() < n {
                for _ in 0..4 + (rand() * 9.0) as usize {
                    out.push(t);
                    t += 0.012;
                }
                t += 0.1 + rand() * 0.8;
            }
            out.truncate(n);
            out
        }
    }
}

/// Reply `reply` in session `id` streaming the first `words` words of [`REPLY`] at `pace`: each
/// event and its time in seconds from now.
pub fn script(id: &str, reply: &str, pace: Pace, words: usize) -> Vec<(f64, &'static str, Value)> {
    let pieces: Vec<&str> = REPLY.split_inclusive([' ', '\n']).take(words).collect();
    let text: String = pieces.concat();
    let msg = |extra: Value| {
        let mut v = json!({ "sessionID": id, "assistantMessageID": reply });
        v.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        v
    };
    let mut out = vec![
        (0.0, "session.execution.started", msg(json!({}))),
        (0.0, "session.step.started", msg(json!({ "agent": "build", "model": { "id": "claude-opus-5-5", "providerID": "anthropic" } }))),
        (0.0, "session.text.started", msg(json!({ "ordinal": 0 }))),
    ];
    let at = times(pace, pieces.len());
    for (t, piece) in at.iter().zip(&pieces) {
        out.push((0.3 + t, "session.text.delta", msg(json!({ "ordinal": 0, "delta": piece }))));
    }
    let end = 0.3 + at.last().copied().unwrap_or(0.0) + 0.05;
    out.push((end, "session.text.ended", msg(json!({ "ordinal": 0, "text": text }))));
    out.push((end + 0.1, "session.step.ended", msg(json!({ "finish": "stop", "cost": 0.0, "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } } }))));
    out.push((end + 0.1, "session.execution.succeeded", msg(json!({}))));
    out
}

/// Queues [`script`] into the app's playback from now.
pub fn queue(s: &mut App, id: &str, reply: &str, pace: Pace, words: usize) {
    s.running.insert(id.to_owned());
    let now = s.clock;
    for (t, ty, data) in script(id, reply, pace, words) {
        s.schedule(now + t, ty, data);
    }
    s.wake_at_playback();
}

fn open(s: &mut App, _: &mut Play<App>) {
    s.finish_stream();
    let id = sid(s);
    let pace = match s.variant {
        1 => Pace::Steady(20.0),
        2 => Pace::Burst,
        3 => Pace::Bursty,
        _ => Pace::Steady(f64::from(s.knobs.get(&SPEED))),
    };
    queue(s, &id, "msg_zzstreampace", pace, usize::MAX);
}

const KEYS: &[Trigger] = &[replay(open)];
const VARIANTS: &[(&str, usize)] = &[("Steady", 0), ("Medium", 1), ("Fast", 2), ("Bursty", 3)];

rune_register::register!(crate::stories::STORY_LIST: Story = [Story::session("Stream/Text speed", KEYS).knobs(&[GROUP]).variants(VARIANTS)]);
