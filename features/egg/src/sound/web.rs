//! The egg's sound in a browser: Desktop's own Web Audio graph (`pin-sound.ts`), node for node,
//! so the page plays exactly what Desktop's Chromium plays, with the random numbers each sound drew
//! from the egg's stream (`sound::ask`) where Desktop's call `Math.random`. The context is made on the first
//! sound after the page has had a person's input, and never while the page is hidden.

use std::cell::{Cell, RefCell};

use wasm_bindgen::JsValue;
use web_sys::{AudioBuffer, AudioContext, AudioContextState, AudioNode, BiquadFilterType, GainNode};

use super::super::rng::Fork;
use super::{Cue, Sound};

const PENTATONIC: [i32; 5] = [0, 2, 4, 7, 9];

struct Audio {
    context: AudioContext,
    bus: GainNode,
    noise: AudioBuffer,
}

thread_local! {
    static SHARED: RefCell<Option<Audio>> = const { RefCell::new(None) };
    /// The numbers the sound being built draws.
    static DRAWS: Cell<Fork> = const { Cell::new(Fork::Unseeded) };
}

pub fn play(s: Sound) {
    SHARED.with_borrow_mut(|shared| {
        DRAWS.with(|d| d.set(s.tables));
        let Some(a) = audio(shared) else { return };
        DRAWS.with(|d| d.set(s.random));
        let _ = sound(a, s.cue);
    });
}

/// A running graph, or nothing when the browser would block it or the page hasn't been used.
fn audio(shared: &mut Option<Audio>) -> Option<&mut Audio> {
    let window = web_sys::window()?;
    if window.document().is_none_or(|d| d.hidden()) {
        return None;
    }
    // `navigator.userActivation?.hasBeenActive`: absent in older browsers, which then stay quiet.
    let activation = js_sys::Reflect::get(&window.navigator(), &"userActivation".into()).ok().filter(|v| v.is_object())?;
    if !js_sys::Reflect::get(&activation, &"hasBeenActive".into()).ok().is_some_and(|v| v.is_truthy()) {
        return None;
    }
    if shared.is_none() {
        *shared = create().ok();
    }
    let a = shared.as_mut()?;
    if a.context.state() == AudioContextState::Suspended {
        let _ = a.context.resume();
    }
    (a.context.state() == AudioContextState::Running).then_some(a)
}

fn create() -> Result<Audio, JsValue> {
    let context = AudioContext::new()?;
    let limiter = context.create_dynamics_compressor()?;
    limiter.threshold().set_value(-24.0);
    limiter.ratio().set_value(4.0);
    limiter.connect_with_audio_node(&context.destination())?;
    let master = context.create_gain()?;
    master.gain().set_value(0.9);
    master.connect_with_audio_node(&limiter)?;
    let bus = context.create_gain()?;
    bus.connect_with_audio_node(&master)?;
    let room = context.create_convolver()?;
    room.set_buffer(Some(&impulse(&context, 1.2, 3.2)?));
    let wet = context.create_gain()?;
    wet.gain().set_value(0.18);
    bus.connect_with_audio_node(&room)?;
    room.connect_with_audio_node(&wet)?;
    wet.connect_with_audio_node(&master)?;
    let rate = context.sample_rate();
    let noise = context.create_buffer(1, rate as u32, rate)?;
    let data: Vec<f32> = (0..rate as usize).map(|_| (random() * 2.0 - 1.0) as f32).collect();
    noise.copy_to_channel(&data, 0)?;
    Ok(Audio { context, bus, noise })
}

fn impulse(context: &AudioContext, seconds: f64, decay: f64) -> Result<AudioBuffer, JsValue> {
    let rate = context.sample_rate();
    let length = (f64::from(rate) * seconds).floor() as u32;
    let buffer = context.create_buffer(2, length, rate)?;
    for channel in 0..2 {
        let data: Vec<f32> = (0..length).map(|i| ((random() * 2.0 - 1.0) * (1.0 - f64::from(i) / f64::from(length)).powf(decay)) as f32).collect();
        buffer.copy_to_channel(&data, channel)?;
    }
    Ok(buffer)
}

fn random() -> f64 {
    DRAWS.with(|d| {
        let mut f = d.get();
        let v = f.next();
        d.set(f);
        v
    })
}

struct Pebble {
    freq: f64,
    q: f64,
    gain: f64,
    decay: f64,
    pan: f64,
    thud: Option<f64>,
}

/// One pebble click: band-passed noise with a resonant body and an optional low thud.
fn pebble(a: &Audio, at: f64, p: Pebble) -> Result<(), JsValue> {
    let c = &a.context;
    let source = c.create_buffer_source()?;
    source.set_buffer(Some(&a.noise));
    let band = c.create_biquad_filter()?;
    band.set_type(BiquadFilterType::Bandpass);
    band.frequency().set_value(p.freq as f32);
    band.q().set_value(p.q as f32);
    let env = c.create_gain()?;
    env.gain().set_value_at_time(0.0001, at)?;
    env.gain().exponential_ramp_to_value_at_time(p.gain as f32, at + 0.002)?;
    env.gain().exponential_ramp_to_value_at_time(0.0001, at + p.decay)?;
    let pan = c.create_stereo_panner()?;
    pan.pan().set_value(p.pan.clamp(-0.9, 0.9) as f32);
    chain(&[&source, &band, &env, &pan, &a.bus])?;
    source.start_with_when_and_grain_offset_and_grain_duration(at, random() * 0.8, p.decay + 0.02)?;
    let Some(thud) = p.thud else { return Ok(()) };
    let body = c.create_oscillator()?;
    body.frequency().set_value_at_time((thud * 1.6) as f32, at)?;
    body.frequency().exponential_ramp_to_value_at_time(thud as f32, at + 0.03)?;
    let gain = c.create_gain()?;
    gain.gain().set_value_at_time(0.0001, at)?;
    gain.gain().exponential_ramp_to_value_at_time((p.gain * 0.5) as f32, at + 0.003)?;
    gain.gain().exponential_ramp_to_value_at_time(0.0001, at + p.decay * 2.5)?;
    chain(&[&body, &gain, &pan])?;
    body.start_with_when(at)?;
    body.stop_with_when(at + p.decay * 2.5 + 0.02)
}

fn chain(nodes: &[&AudioNode]) -> Result<(), JsValue> {
    nodes.windows(2).try_for_each(|w| w[0].connect_with_audio_node(w[1]).map(|_| ()))
}

fn note(step: i32) -> f64 {
    440.0 * 2f64.powf(f64::from(74 + 12 * step.div_euclid(5) + PENTATONIC[step.rem_euclid(5) as usize] - 69) / 12.0)
}

fn pan_of(x: f64, width: f64) -> f64 {
    if width != 0.0 { (x / width) * 1.6 - 0.8 } else { 0.0 }
}

fn sound(a: &Audio, cue: Cue) -> Result<(), JsValue> {
    let now = a.context.current_time();
    match cue {
        Cue::Sweep { duration, reverse } => sweep(a, duration, reverse),
        Cue::Knock { x, width } => pebble(a, now, Pebble { freq: 1100.0, q: 2.2, gain: 0.14, decay: 0.05, pan: pan_of(x, width), thud: Some(95.0) }),
        Cue::Brick { x, width, combo } => {
            pebble(a, now, Pebble { freq: 1300.0, q: 2.0, gain: 0.1, decay: 0.04, pan: pan_of(x, width), thud: Some(110.0) })?;
            pebble(a, now + 0.004, Pebble { freq: note(combo.min(14) as i32), q: 16.0, gain: 0.06, decay: 0.09, pan: pan_of(x, width), thud: None })
        }
        Cue::Paddle { x, width } => pebble(a, now, Pebble { freq: 520.0, q: 1.4, gain: 0.12, decay: 0.05, pan: pan_of(x, width), thud: Some(72.0) }),
        Cue::Bounce { x, width, speed } => {
            pebble(a, now, Pebble { freq: 2200.0 + random() * 1400.0, q: 6.0, gain: (0.012 + speed / 40000.0).min(0.07), decay: 0.02, pan: pan_of(x, width), thud: None })
        }
        Cue::Slot { column, columns, x, width } => {
            pebble(a, now, Pebble { freq: note(((column / columns) * 10.0).floor() as i32), q: 14.0, gain: 0.05, decay: 0.045, pan: pan_of(x, width), thud: None })
        }
    }
}

/// The wordmark rising in: a gritty stone slide sweeping across with pebbles settling under it.
fn sweep(a: &Audio, duration: f64, reverse: bool) -> Result<(), JsValue> {
    let c = &a.context;
    let at = c.current_time() + 0.02;
    let end = at + duration;
    let source = c.create_buffer_source()?;
    source.set_buffer(Some(&a.noise));
    source.set_loop(true);
    let grit = c.create_biquad_filter()?;
    grit.set_type(BiquadFilterType::Bandpass);
    grit.q().set_value(1.4);
    grit.frequency().set_value_at_time(if reverse { 1500.0 } else { 260.0 }, at)?;
    grit.frequency().exponential_ramp_to_value_at_time(if reverse { 260.0 } else { 1500.0 }, end)?;
    let body = c.create_biquad_filter()?;
    body.set_type(BiquadFilterType::Lowpass);
    body.frequency().set_value(2400.0);
    let env = c.create_gain()?;
    env.gain().set_value_at_time(0.0001, at)?;
    env.gain().exponential_ramp_to_value_at_time(0.03, at + 0.12)?;
    env.gain().set_value_at_time(0.03, end - 0.18)?;
    env.gain().exponential_ramp_to_value_at_time(0.0001, end + 0.12)?;
    let pan = c.create_stereo_panner()?;
    pan.pan().set_value_at_time(-0.7, at)?;
    pan.pan().linear_ramp_to_value_at_time(0.7, end)?;
    chain(&[&source, &grit, &body, &env, &pan, &a.bus])?;
    source.start_with_when(at)?;
    web_sys::AudioScheduledSourceNode::stop_with_when(&source, end + 0.15)?;
    let clicks = 22;
    for i in 0..clicks {
        let k = (f64::from(i) + random() * 0.6) / f64::from(clicks);
        let freq = 1800.0 + random() * 1600.0;
        let gain = 0.02 + random() * 0.018;
        let decay = 0.02 + random() * 0.015;
        pebble(a, at + k * (duration - 0.1) + 0.06, Pebble { freq, q: 5.0, gain, decay, pan: k * 1.4 - 0.7, thud: None })?;
    }
    Ok(())
}
