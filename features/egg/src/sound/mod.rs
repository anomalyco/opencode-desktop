//! The egg's sounds (Desktop's `new-session/pin-sound.ts`): stone sounds synthesised from
//! filtered noise, a few with a low thud, through a small room and a limiter. Nothing is loaded.
//!
//! The graph renders on its own thread into the system's default output (`wasapi.rs` on
//! Windows, `audioqueue.rs` on macOS), started by the first sound and gone, with everything it
//! held, once the last one's echo has died away. A browser plays the same graph with Web Audio (`web.rs`).
//!
//! `--egg-audio <file.wav>` renders what would be played into the file instead, on the window's
//! own clock (virtual in a headless run), and plays nothing: how the sounds are proved without a
//! speaker. Its random numbers are the egg's (`--egg-seed` seeds them for a proof run).

#[cfg(not(target_arch = "wasm32"))]
mod dsp;
#[cfg(not(target_arch = "wasm32"))]
mod synth;
#[cfg(target_os = "macos")]
mod audioqueue;
#[cfg(target_os = "windows")]
mod wasapi;
#[cfg(target_arch = "wasm32")]
mod web;

use super::rng::{self, Fork};

/// One sound, with what it depends on: `x` and `width` place it left to right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    /// The wordmark rising in or packing away: a gritty stone slide sweeping across.
    Sweep { duration: f64, reverse: bool },
    /// A click that knocks pins out: one solid stone clack.
    Knock { x: f64, width: f64 },
    /// The ball smashing pins: a clack whose ring climbs the scale with each hit before the paddle.
    Brick { x: f64, width: f64, combo: u32 },
    /// The ball landing on the paddle: a soft, low knock.
    Paddle { x: f64, width: f64 },
    /// A loose pin bouncing off the composer or the floor.
    Bounce { x: f64, width: f64, speed: f64 },
    /// A pin seating back into its socket; pitch rises across the wordmark.
    Slot { column: f64, columns: f64, x: f64, width: f64 },
}

/// A sound to play, with the random numbers Desktop's `pin-sound.ts` draws for it: `random` replays
/// them; `tables` replays the first sound's, which made the room and the noise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sound {
    pub cue: Cue,
    pub random: Fork,
    pub tables: Fork,
}

/// The sample rate the graph renders at (the system resamples), and so how many numbers the room
/// and the noise take: two 1.2 s channels and a second.
pub const RATE: f64 = 48000.0;
const TABLE_DRAWS: usize = 2 * (RATE * 1.2) as usize + RATE as usize;

struct Gate {
    on: bool,
    tables: Option<Fork>,
    /// Recent rate-limited sounds, in milliseconds (`allow`).
    recent: Vec<f64>,
}

thread_local! {
    static GATE: std::cell::RefCell<Gate> = const { std::cell::RefCell::new(Gate { on: false, tables: None, recent: Vec::new() }) };
}

/// Whether sounds play (the interface-sounds setting, a window with a person at it or a capture).
/// Off, a sound draws nothing, as Desktop's `sound()` gate skips the call.
pub fn enable(on: bool) {
    GATE.with_borrow_mut(|g| g.on = on);
}

/// Desktop's `PinSound` call at `now` (seconds): its random numbers come out of the egg's stream
/// here, in its order, so the pins' next numbers are the ones Desktop's draw. None when sounds are
/// off or a burst is capped (`allow`).
pub fn ask(cue: Cue, now: f64) -> Option<Sound> {
    GATE.with_borrow_mut(|g| {
        if !g.on {
            return None;
        }
        // `createAudio`, on the first sound: the room's impulse, then the noise.
        let tables = *g.tables.get_or_insert_with(|| rng::take(TABLE_DRAWS));
        // Caps bursts of collision sounds so a crumble stays a patter rather than a roar.
        if matches!(cue, Cue::Bounce { .. } | Cue::Slot { .. }) {
            let ms = now * 1000.0;
            g.recent.retain(|&t| ms - t <= 80.0);
            if g.recent.len() >= 5 {
                return None;
            }
            g.recent.push(ms);
        }
        let draws = match cue {
            // Each of 22 pebbles: where, pitch, level, decay, then its noise offset.
            Cue::Sweep { .. } => 22 * 5,
            Cue::Brick { .. } | Cue::Bounce { .. } => 2,
            Cue::Knock { .. } | Cue::Paddle { .. } | Cue::Slot { .. } => 1,
        };
        Some(Sound { cue, random: rng::take(draws), tables })
    })
}

#[cfg(not(target_arch = "wasm32"))]
impl synth::Random for Fork {
    fn next(&mut self) -> f64 {
        Fork::next(self)
    }
}

/// Where sounds go: the speakers, or a capture file.
pub fn capturing() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return capture::path().is_some();
    #[cfg(target_arch = "wasm32")]
    false
}

/// Plays `sound` now. `now` is the window's clock in seconds.
pub fn play(sound: Sound, now: f64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if capture::path().is_some() {
            return capture::play(sound, now);
        }
        #[cfg(target_os = "windows")]
        wasapi::play(sound);
        #[cfg(target_os = "macos")]
        audioqueue::play(sound);
    }
    #[cfg(target_arch = "wasm32")]
    web::play(sound);
    let _ = now;
}

/// Keeps a capture's render up with the clock; true while it still has sound to write.
pub fn tick(now: f64) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if capture::path().is_some() {
        return capture::tick(now);
    }
    let _ = now;
    false
}
#[cfg(not(target_arch = "wasm32"))]
mod capture {
    //! `--egg-audio`: the graph rendered on the window's clock into a WAV file, with a log of
    //! each sound beside it (`<file>.cues.jsonl`).
    use std::cell::RefCell;
    use std::io::Write;

    use super::synth::Engine;
    use super::{RATE, Sound};

    pub fn path() -> Option<String> {
        thread_local! {
            static PATH: Option<String> = crate::arg("--egg-audio");
        }
        PATH.with(Clone::clone)
    }

    struct Capture {
        engine: Engine,
        samples: Vec<f32>,
        /// The cue log, a line per sound as it plays.
        cues: Option<std::fs::File>,
        /// The clock at which the engine's time started.
        origin: f64,
        dirty: bool,
    }

    thread_local! {
        static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    }

    fn with<T>(now: f64, sound: Sound, f: impl FnOnce(&mut Capture) -> T) -> T {
        CAPTURE.with_borrow_mut(|c| {
            let c = c.get_or_insert_with(|| {
                let engine = Engine::new(RATE, &mut { sound.tables });
                let cues = path().and_then(|p| std::fs::File::create(format!("{p}.cues.jsonl")).ok());
                Capture { engine, samples: Vec::new(), cues, origin: now, dirty: false }
            });
            f(c)
        })
    }

    /// Renders up to `now`, quantum by quantum, as a context's clock runs.
    fn advance(c: &mut Capture, now: f64) {
        let mut l = [0.0f32; 128];
        let mut r = [0.0f32; 128];
        while c.engine.now() + 128.0 / RATE <= now - c.origin {
            c.engine.quantum(&mut l, &mut r);
            for i in 0..128 {
                c.samples.push(l[i]);
                c.samples.push(r[i]);
            }
        }
    }

    pub fn play(sound: Sound, now: f64) {
        let cue = sound.cue;
        with(now, sound, |c| {
            advance(c, now);
            if let Some(f) = c.cues.as_mut() {
                writeln!(f, "{{\"at\":{now:.6},\"t\":{:.6},\"ctx\":{:.6},\"cue\":\"{cue:?}\"}}", now - c.origin, c.engine.now()).ok();
            }
            c.engine.play(cue, &mut { sound.random });
            c.dirty = true;
        });
    }

    pub fn tick(now: f64) -> bool {
        CAPTURE.with_borrow_mut(|c| {
            let Some(c) = c.as_mut() else { return false };
            advance(c, now);
            if c.dirty && c.engine.silent() {
                c.dirty = false;
                write(c);
            }
            c.dirty
        })
    }

    fn write(c: &Capture) {
        let Some(path) = path() else { return };
        let data: Vec<u8> = c.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut out = Vec::with_capacity(44 + data.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&(RATE as u32).to_le_bytes());
        out.extend_from_slice(&(RATE as u32 * 8).to_le_bytes());
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        std::fs::write(&path, out).ok();
    }
}
