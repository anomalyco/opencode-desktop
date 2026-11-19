//! The Web Audio nodes `pin-sound.ts` builds its sounds from, as Chromium runs them: automated
//! parameters (`AudioParam`), the biquad filter (`BiquadFilterNode`), the equal-power stereo
//! panner (`StereoPannerNode`), the convolver's normalisation and partitioned convolution
//! (`ConvolverNode`, `reverb.cc`), and the compressor (`DynamicsCompressorNode`,
//! `dynamics_compressor.cc`).

/// An `AudioParam`'s timeline, in seconds: what the sounds use of it.
#[derive(Clone, Debug)]
pub struct Param {
    default: f64,
    events: Vec<(f64, Ev)>,
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Set(f64),
    Exp(f64),
    Lin(f64),
}

impl Param {
    pub fn new(default: f64) -> Param {
        Param { default, events: Vec::new() }
    }

    pub fn set(mut self, v: f64, at: f64) -> Param {
        self.events.push((at, Ev::Set(v)));
        self
    }

    pub fn exp(mut self, v: f64, at: f64) -> Param {
        self.events.push((at, Ev::Exp(v)));
        self
    }

    pub fn lin(mut self, v: f64, at: f64) -> Param {
        self.events.push((at, Ev::Lin(v)));
        self
    }

    /// The value at time `t`: a ramp runs from the previous event to its own time.
    pub fn at(&self, t: f64) -> f64 {
        let mut v = self.default;
        let mut t0 = 0.0;
        for &(at, ev) in &self.events {
            match ev {
                Ev::Set(x) => {
                    if t < at {
                        return v;
                    }
                    v = x;
                }
                Ev::Exp(x) | Ev::Lin(x) => {
                    if t < at {
                        let u = ((t - t0) / (at - t0)).clamp(0.0, 1.0);
                        return match ev {
                            Ev::Exp(_) if v * x > 0.0 => v * (x / v).powf(u),
                            Ev::Exp(_) => v,
                            _ => v + (x - v) * u,
                        };
                    }
                    v = x;
                }
            }
            t0 = at;
        }
        v
    }

}

/// A biquad's normalised coefficients and state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Bandpass,
    Lowpass,
}

impl Biquad {
    /// Coefficients for `freq` Hz and `q` at `rate` (Chromium's `Biquad::Set*Params`): the
    /// band-pass's Q is linear, the low-pass's in dB.
    pub fn tune(&mut self, kind: Kind, freq: f64, q: f64, rate: f64) {
        let f = (freq / (rate / 2.0)).clamp(0.0, 1.0);
        if f <= 0.0 || f >= 1.0 {
            // Chromium passes a filter tuned at or past Nyquist straight through (low-pass) or to
            // silence (band-pass); none of the sounds go there.
            (self.b0, self.b1, self.b2, self.a1, self.a2) = if kind == Kind::Lowpass && f >= 1.0 { (1.0, 0.0, 0.0, 0.0, 0.0) } else { (0.0, 0.0, 0.0, 0.0, 0.0) };
            return;
        }
        let w0 = std::f64::consts::PI * f;
        let (s, k) = w0.sin_cos();
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Bandpass => {
                let alpha = s / (2.0 * q.max(1e-9));
                (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * k, 1.0 - alpha)
            }
            Kind::Lowpass => {
                let alpha = s / (2.0 * 10f64.powf(q / 20.0));
                let beta = (1.0 - k) / 2.0;
                (beta, 2.0 * beta, beta, 1.0 + alpha, -2.0 * k, 1.0 - alpha)
            }
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        // Chromium flushes denormals; so do we.
        self.y1 = if y.abs() < 1e-30 { 0.0 } else { y };
        self.y1
    }
}

/// The equal-power gains of a mono input panned to `pan` (`StereoPannerNode`).
pub fn pan(pan: f64) -> (f64, f64) {
    let x = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
    let (s, c) = (x * std::f64::consts::FRAC_PI_2).sin_cos();
    (c, s)
}

// The convolver ---------------------------------------------------------------------------------

/// `CalculateNormalizationScale` (`reverb.cc`): the impulse response's RMS power, calibrated to
/// -58 dB at 44.1 kHz.
pub fn normalization(channels: &[Vec<f32>], rate: f64) -> f32 {
    let len = channels.first().map_or(0, Vec::len);
    let power: f32 = channels.iter().map(|c| c.iter().map(|v| v * v).sum::<f32>()).sum();
    let mut power = (power / (channels.len() * len) as f32).sqrt();
    if !power.is_finite() || power < 0.000125 {
        power = 0.000125;
    }
    let mut scale = 1.0 / power;
    scale *= 10f32.powf(-58.0 * 0.05);
    scale *= (44100.0 / rate) as f32;
    scale
}

const BLOCK: usize = 128;
const FFT: usize = 2 * BLOCK;

/// One channel's uniformly partitioned overlap-save convolution in 128-frame blocks.
pub struct Convolver {
    /// Each partition's spectrum (`FFT` bins, complex).
    parts: Vec<Vec<[f32; 2]>>,
    /// The last spectra of the input, newest first, as a ring.
    history: Vec<Vec<[f32; 2]>>,
    head: usize,
    last: Vec<f32>,
    acc: Vec<[f32; 2]>,
    work: Vec<[f32; 2]>,
    twiddle: Vec<[f32; 2]>,
}

impl Convolver {
    pub fn new(ir: &[f32], scale: f32) -> Convolver {
        let twiddle = (0..FFT / 2).map(|i| {
            let a = -2.0 * std::f64::consts::PI * i as f64 / FFT as f64;
            [a.cos() as f32, a.sin() as f32]
        }).collect::<Vec<_>>();
        let n = ir.len().div_ceil(BLOCK);
        let mut work = vec![[0.0; 2]; FFT];
        let parts = (0..n).map(|p| {
            work.iter_mut().for_each(|c| *c = [0.0; 2]);
            for (i, v) in ir[p * BLOCK..((p + 1) * BLOCK).min(ir.len())].iter().enumerate() {
                work[i] = [v * scale, 0.0];
            }
            fft(&mut work, &twiddle, false);
            work.clone()
        }).collect();
        Convolver { parts, history: vec![vec![[0.0; 2]; FFT]; n], head: 0, last: vec![0.0; BLOCK], acc: vec![[0.0; 2]; FFT], work, twiddle }
    }

    /// Convolves one block of 128 frames in place.
    pub fn block(&mut self, io: &mut [f32]) {
        for (i, &sample) in io[..BLOCK].iter().enumerate() {
            self.work[i] = [self.last[i], 0.0];
            self.work[BLOCK + i] = [sample, 0.0];
        }
        self.last.copy_from_slice(&io[..BLOCK]);
        fft(&mut self.work, &self.twiddle, false);
        self.head = (self.head + self.history.len() - 1) % self.history.len();
        self.history[self.head].copy_from_slice(&self.work);
        self.acc.iter_mut().for_each(|c| *c = [0.0; 2]);
        let n = self.parts.len();
        for p in 0..n {
            let x = &self.history[(self.head + p) % n];
            for ((a, x), h) in self.acc.iter_mut().zip(x).zip(&self.parts[p]) {
                a[0] += x[0] * h[0] - x[1] * h[1];
                a[1] += x[0] * h[1] + x[1] * h[0];
            }
        }
        fft(&mut self.acc, &self.twiddle, true);
        for (out, acc) in io[..BLOCK].iter_mut().zip(&self.acc[BLOCK..]) {
            *out = acc[0] / FFT as f32;
        }
    }
}

/// An in-place radix-2 FFT (`inverse` without the 1/n).
fn fft(a: &mut [[f32; 2]], tw: &[[f32; 2]], inverse: bool) {
    let n = a.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            a.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let step = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let w = tw[k * step];
                let w = if inverse { [w[0], -w[1]] } else { w };
                let u = a[start + k];
                let v = a[start + k + len / 2];
                let t = [v[0] * w[0] - v[1] * w[1], v[0] * w[1] + v[1] * w[0]];
                a[start + k] = [u[0] + t[0], u[1] + t[1]];
                a[start + k + len / 2] = [u[0] - t[0], u[1] - t[1]];
            }
        }
        len <<= 1;
    }
}

pub const QUANTUM: usize = BLOCK;

// The compressor --------------------------------------------------------------------------------

const MAX_PRE_DELAY: usize = 1024;
const PRE_DELAY: f32 = 0.006;

fn db(x: f32) -> f32 {
    20.0 * x.log10()
}

fn lin(db: f32) -> f32 {
    10f32.powf(0.05 * db)
}

/// `DynamicsCompressor` with `threshold`, `ratio` and Web Audio's defaults otherwise (knee 30 dB,
/// attack 3 ms, release 250 ms), on stereo.
pub struct Compressor {
    rate: f32,
    detector: f32,
    gain: f32,
    max_attack_diff: f32,
    delay: [Vec<f32>; 2],
    read: usize,
    write: usize,
    linear_threshold: f32,
    knee_threshold: f32,
    db_knee_threshold: f32,
    db_yknee_threshold: f32,
    slope: f32,
    k: f32,
    attack: f32,
    release: f32,
}

impl Compressor {
    pub fn new(rate: f32, threshold: f32, ratio: f32) -> Compressor {
        let knee = 30.0;
        let pre = ((PRE_DELAY * rate) as usize).min(MAX_PRE_DELAY - 1);
        let mut c = Compressor {
            rate,
            detector: 0.0,
            gain: 1.0,
            max_attack_diff: -1.0,
            delay: [vec![0.0; MAX_PRE_DELAY], vec![0.0; MAX_PRE_DELAY]],
            read: 0,
            write: pre,
            linear_threshold: lin(threshold),
            knee_threshold: 0.0,
            db_knee_threshold: 0.0,
            db_yknee_threshold: 0.0,
            slope: 1.0 / ratio,
            k: 0.0,
            attack: 0.003,
            release: 0.25,
        };
        let k = c.k_at_slope(threshold, knee, 1.0 / ratio);
        c.db_knee_threshold = threshold + knee;
        c.knee_threshold = lin(c.db_knee_threshold);
        c.db_yknee_threshold = db(c.knee_curve(c.knee_threshold, k));
        c.k = k;
        c
    }

    fn knee_curve(&self, x: f32, k: f32) -> f32 {
        if x < self.linear_threshold {
            return x;
        }
        self.linear_threshold + (1.0 - (-f64::from(k * (x - self.linear_threshold))).exp() as f32) / k
    }

    fn saturate(&self, x: f32, k: f32) -> f32 {
        if x < self.knee_threshold {
            return self.knee_curve(x, k);
        }
        lin(self.db_yknee_threshold + self.slope * (db(x) - self.db_knee_threshold))
    }

    fn k_at_slope(&self, threshold: f32, knee: f32, desired: f32) -> f32 {
        let db_x = threshold + knee;
        let x = lin(db_x);
        let (mut x2, mut db_x2) = (1.0, 0.0);
        if x >= self.linear_threshold {
            x2 = x * 1.001;
            db_x2 = db(x2);
        }
        let (mut min_k, mut max_k, mut k) = (0.1f32, 10000.0f32, 5.0f32);
        let mut slope = 1.0;
        for _ in 0..15 {
            if x >= self.linear_threshold {
                let db_y = db(self.knee_curve(x, k));
                let db_y2 = db(self.knee_curve(x2, k));
                slope = (db_y2 - db_y) / (db_x2 - db_x);
            }
            if slope < desired { max_k = k } else { min_k = k }
            k = (min_k * max_k).sqrt();
        }
        k
    }

    /// Compresses one render quantum of stereo frames in place.
    pub fn quantum(&mut self, l: &mut [f32], r: &mut [f32]) {
        let k = self.k;
        let post = (1.0 / self.saturate(1.0, k)).powf(0.6);
        let attack_frames = self.attack.max(0.001) * self.rate;
        let release_frames = self.rate * self.release;
        let sat_release_frames = 0.0025 * self.rate;
        let (z1, z2, z3, z4) = (0.09f32, 0.16f32, 0.42f32, 0.98f32);
        let ka = 0.999_999_999_999_999_8 * z1 + 1.843_221_9e-16 * z2 - 1.937_339_4e-16 * z3 + 8.824_516e-18 * z4;
        let kb = -1.578_832 * z1 + 2.330_583_8 * z2 - 0.914_119_4 * z3 + 0.162_367_75 * z4;
        let kc = 0.533_414_3 * z1 - 1.272_736_8 * z2 + 0.925_885_6 * z3 - 0.186_563_1 * z4;
        let kd = 0.087_834_634 * z1 - 0.169_416_3 * z2 + 0.085_880_58 * z3 - 0.004_298_914 * z4;
        let ke = -0.042_416_88 * z1 + 0.111_569_38 * z2 - 0.097_646_765 * z3 + 0.028_494_263 * z4;
        let (a, b, c, d, e) = (release_frames * ka, release_frames * kb, release_frames * kc, release_frames * kd, release_frames * ke);
        let n = l.len();
        let mut i = 0;
        while i < n {
            if !self.detector.is_finite() {
                self.detector = 1.0;
            }
            let desired = self.detector;
            let scaled = desired.asin() / std::f32::consts::FRAC_PI_2;
            let releasing = scaled > self.gain;
            let mut diff = if scaled == 0.0 { if releasing { -1.0 } else { 1.0 } } else { db(self.gain / scaled) };
            let rate = if releasing {
                self.max_attack_diff = -1.0;
                if !diff.is_finite() {
                    diff = -1.0;
                }
                let x = 0.25 * (diff.clamp(-12.0, 0.0) + 12.0);
                let (x2, x3) = (x * x, x * x * x);
                let frames = a + b * x + c * x2 + d * x3 + e * x2 * x2;
                lin(5.0 / frames)
            } else {
                if !diff.is_finite() {
                    diff = 1.0;
                }
                if self.max_attack_diff == -1.0 || self.max_attack_diff < diff {
                    self.max_attack_diff = diff;
                }
                let eff = self.max_attack_diff.max(0.5);
                1.0 - (0.25 / eff).powf(1.0 / attack_frames)
            };
            let frames = 32.min(n - i);
            for _ in 0..frames {
                let (sl, sr) = (l[i], r[i]);
                self.delay[0][self.write] = sl;
                self.delay[1][self.write] = sr;
                let input = sl.abs().max(sr.abs());
                let shaped = self.saturate(input, k);
                let attenuation = if input <= 0.0001 { 1.0 } else { shaped / input };
                let db_att = (-db(attenuation)).max(2.0);
                let sat_rate = lin(db_att / sat_release_frames) - 1.0;
                let step = if attenuation > self.detector { sat_rate } else { 1.0 };
                self.detector += (attenuation - self.detector) * step;
                self.detector = self.detector.min(1.0);
                if !self.detector.is_finite() {
                    self.detector = 1.0;
                }
                if rate < 1.0 {
                    self.gain += (scaled - self.gain) * rate;
                } else {
                    self.gain = (self.gain * rate).min(1.0);
                }
                let warped = (std::f64::consts::FRAC_PI_2 * f64::from(self.gain)).sin() as f32;
                let total = post * warped;
                l[i] = self.delay[0][self.read] * total;
                r[i] = self.delay[1][self.read] * total;
                self.read = (self.read + 1) & (MAX_PRE_DELAY - 1);
                self.write = (self.write + 1) & (MAX_PRE_DELAY - 1);
                i += 1;
            }
            if self.detector.abs() < 1e-30 {
                self.detector = 0.0;
            }
            if self.gain.abs() < 1e-30 {
                self.gain = 0.0;
            }
        }
    }
}
