//! `pin-sound.ts`'s audio graph, rendered: every sound is band-passed noise (some with a low
//! thud) through a gain envelope and a panner onto one bus, which goes through a 1.2 s room at
//! 0.18 and a 0.9 master into a limiter. Random numbers are drawn in Desktop's order (the room,
//! then the noise, then each sound's own), so with the same random source both render the same.

use super::Cue;
use super::dsp::{self, Biquad, Compressor, Convolver, Kind, Param, QUANTUM};

const PENTATONIC: [i32; 5] = [0, 2, 4, 7, 9];

/// `note`: the pentatonic scale from MIDI 74.
fn note(step: i32) -> f64 {
    440.0 * 2f64.powf((74 + 12 * step.div_euclid(5) + PENTATONIC[step.rem_euclid(5) as usize] - 69) as f64 / 12.0)
}

fn pan_of(x: f64, width: f64) -> f64 {
    if width != 0.0 { (x / width) * 1.6 - 0.8 } else { 0.0 }
}

/// A source of `Math.random`.
pub trait Random {
    fn next(&mut self) -> f64;
}

/// One pebble or the sweep's slide: noise from `offset` seconds, filtered, enveloped and panned.
struct Voice {
    start: f64,
    stop: f64,
    /// Read position in the noise buffer, in frames, at `start`.
    offset: f64,
    looping: bool,
    band: Biquad,
    band_kind: Kind,
    freq: Param,
    q: f64,
    tuned: Option<(f64, f64)>,
    lowpass: Option<Biquad>,
    env: Param,
    pan: Param,
    thud: Option<Thud>,
    /// Time the voice is silent from (its envelopes and the filters' ring are done).
    end: f64,
}

struct Thud {
    freq: Param,
    gain: Param,
    start: f64,
    stop: f64,
    phase: f64,
}

pub struct Engine {
    pub rate: f64,
    /// Frames rendered so far: the context's `currentTime` is `frame / rate`.
    pub frame: u64,
    noise: Vec<f32>,
    voices: Vec<Voice>,
    room: [Convolver; 2],
    limiter: Compressor,
    bus: [Vec<f32>; 2],
    /// When the last voice went quiet.
    last_end: f64,
}

impl Engine {
    /// `createAudio`: the room's impulse (two channels of decaying noise), then a second of noise.
    pub fn new(rate: f64, random: &mut dyn Random) -> Engine {
        let len = (rate * 1.2).floor() as usize;
        let ir: Vec<Vec<f32>> = (0..2).map(|_| (0..len).map(|i| ((random.next() * 2.0 - 1.0) * (1.0 - i as f64 / len as f64).powf(3.2)) as f32).collect()).collect();
        let scale = dsp::normalization(&ir, rate);
        let noise = (0..rate as usize).map(|_| (random.next() * 2.0 - 1.0) as f32).collect();
        Engine {
            rate,
            frame: 0,
            noise,
            voices: Vec::with_capacity(64),
            room: [Convolver::new(&ir[0], scale), Convolver::new(&ir[1], scale)],
            limiter: Compressor::new(rate as f32, -24.0, 4.0),
            bus: [vec![0.0; QUANTUM], vec![0.0; QUANTUM]],
            last_end: 0.0,
        }
    }

    pub fn now(&self) -> f64 {
        self.frame as f64 / self.rate
    }

    /// Nothing sounds any more, the room's tail included.
    pub fn silent(&self) -> bool {
        self.voices.is_empty() && self.last_end + 1.3 < self.now()
    }

    /// Plays `cue` now, drawing its random numbers from `random` (burst caps are the asker's).
    pub fn play(&mut self, cue: Cue, random: &mut dyn Random) {
        let at = self.now();
        match cue {
            Cue::Sweep { duration, reverse } => self.sweep(at + 0.02, duration, reverse, random),
            Cue::Knock { x, width } => self.pebble(at, 1100.0, 2.2, 0.14, 0.05, pan_of(x, width), Some(95.0), random),
            Cue::Brick { x, width, combo } => {
                self.pebble(at, 1300.0, 2.0, 0.1, 0.04, pan_of(x, width), Some(110.0), random);
                self.pebble(at + 0.004, note(combo.min(14) as i32), 16.0, 0.06, 0.09, pan_of(x, width), None, random);
            }
            Cue::Paddle { x, width } => self.pebble(at, 520.0, 1.4, 0.12, 0.05, pan_of(x, width), Some(72.0), random),
            Cue::Bounce { x, width, speed } => {
                let freq = 2200.0 + random.next() * 1400.0;
                self.pebble(at, freq, 6.0, (0.012 + speed / 40000.0).min(0.07), 0.02, pan_of(x, width), None, random);
            }
            Cue::Slot { column, columns, x, width } => {
                self.pebble(at, note(((column / columns) * 10.0).floor() as i32), 14.0, 0.05, 0.045, pan_of(x, width), None, random);
            }
        }
    }

    /// One pebble click: band-passed noise with a resonant body and an optional low thud.
    #[allow(clippy::too_many_arguments)]
    fn pebble(&mut self, at: f64, freq: f64, q: f64, gain: f64, decay: f64, pan: f64, thud: Option<f64>, random: &mut dyn Random) {
        let env = Param::new(1.0).set(0.0001, at).exp(gain, at + 0.002).exp(0.0001, at + decay);
        let offset = random.next() * 0.8 * self.rate;
        let thud = thud.map(|f| Thud {
            freq: Param::new(440.0).set(f * 1.6, at).exp(f, at + 0.03),
            gain: Param::new(1.0).set(0.0001, at).exp(gain * 0.5, at + 0.003).exp(0.0001, at + decay * 2.5),
            start: at,
            stop: at + decay * 2.5 + 0.02,
            phase: 0.0,
        });
        let end = at + decay * if thud.is_some() { 2.5 } else { 1.0 } + 0.1;
        self.voices.push(Voice {
            start: at,
            stop: at + decay + 0.02,
            offset,
            looping: false,
            band: Biquad::default(),
            band_kind: Kind::Bandpass,
            freq: Param::new(freq),
            q,
            tuned: None,
            lowpass: None,
            env,
            pan: Param::new(pan.clamp(-0.9, 0.9)),
            thud,
            end,
        });
    }

    /// The wordmark rising in: a gritty stone slide sweeping across with pebbles settling under it.
    fn sweep(&mut self, at: f64, duration: f64, reverse: bool, random: &mut dyn Random) {
        let end = at + duration;
        let (from, to) = if reverse { (1500.0, 260.0) } else { (260.0, 1500.0) };
        let mut lowpass = Biquad::default();
        lowpass.tune(Kind::Lowpass, 2400.0, 1.0, self.rate);
        self.voices.push(Voice {
            start: at,
            stop: end + 0.15,
            offset: 0.0,
            looping: true,
            band: Biquad::default(),
            band_kind: Kind::Bandpass,
            freq: Param::new(350.0).set(from, at).exp(to, end),
            q: 1.4,
            tuned: None,
            lowpass: Some(lowpass),
            env: Param::new(1.0).set(0.0001, at).exp(0.03, at + 0.12).set(0.03, end - 0.18).exp(0.0001, end + 0.12),
            pan: Param::new(0.0).set(-0.7, at).lin(0.7, end),
            thud: None,
            end: end + 0.25,
        });
        let clicks = 22;
        for i in 0..clicks {
            let k = (f64::from(i) + random.next() * 0.6) / f64::from(clicks);
            let freq = 1800.0 + random.next() * 1600.0;
            let gain = 0.02 + random.next() * 0.018;
            let decay = 0.02 + random.next() * 0.015;
            self.pebble(at + k * (duration - 0.1) + 0.06, freq, 5.0, gain, decay, k * 1.4 - 0.7, None, random);
        }
    }

    /// Renders one quantum (128 frames) into `l` and `r`.
    pub fn quantum(&mut self, l: &mut [f32], r: &mut [f32]) {
        let rate = self.rate;
        let t0 = self.now();
        for b in &mut self.bus {
            b.fill(0.0);
        }
        let noise = &self.noise;
        let n = noise.len() as f64;
        for v in &mut self.voices {
            for i in 0..QUANTUM {
                let t = t0 + i as f64 / rate;
                if t < v.start {
                    continue;
                }
                // The source: silent outside [start, stop), the buffer read with linear interpolation.
                let src = if t >= v.start && t < v.stop {
                    let mut pos = v.offset + (t - v.start) * rate;
                    if v.looping {
                        pos %= n;
                    }
                    if pos < n - 1.0 {
                        let k = pos.floor();
                        let f = pos - k;
                        let a = f64::from(noise[k as usize]);
                        let b = f64::from(noise[(k as usize + 1) % noise.len()]);
                        a + (b - a) * f
                    } else if pos < n {
                        f64::from(noise[noise.len() - 1])
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                let freq = v.freq.at(t);
                if v.tuned != Some((freq, v.q)) {
                    v.band.tune(v.band_kind, freq, v.q, rate);
                    v.tuned = Some((freq, v.q));
                }
                let mut s = v.band.run(src);
                if let Some(lp) = &mut v.lowpass {
                    s = lp.run(s);
                }
                let mut out = s * v.env.at(t);
                if let Some(th) = &mut v.thud
                    && t >= th.start
                    && t < th.stop
                {
                    out += th.phase.sin() * th.gain.at(t);
                    th.phase += 2.0 * std::f64::consts::PI * th.freq.at(t) / rate;
                }
                let (gl, gr) = dsp::pan(v.pan.at(t));
                self.bus[0][i] += (out * gl) as f32;
                self.bus[1][i] += (out * gr) as f32;
            }
        }
        let end = t0 + QUANTUM as f64 / rate;
        let before = self.voices.len();
        self.voices.retain(|v| v.end > end);
        if self.voices.len() != before {
            self.last_end = end;
        }
        // The room, from the bus, at 0.18, then the 0.9 master and the limiter.
        let mut wet = [[0.0f32; QUANTUM]; 2];
        for (wet, (bus, room)) in wet.iter_mut().zip(self.bus.iter().zip(&mut self.room)) {
            wet.copy_from_slice(bus);
            room.block(wet);
        }
        for i in 0..QUANTUM {
            l[i] = (self.bus[0][i] + wet[0][i] * 0.18) * 0.9;
            r[i] = (self.bus[1][i] + wet[1][i] * 0.18) * 0.9;
        }
        self.limiter.quantum(l, r);
        self.frame += QUANTUM as u64;
    }
}
