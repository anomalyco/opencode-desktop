//! `Math.random` for the egg: one stream for the whole egg, as Desktop's pin board, lodger, game
//! and sounds all draw from the page's one `Math.random`. A sound's random numbers come out of it
//! at the moment it's asked for (`sound::ask`), so what the pins draw next is what Desktop's draw.
//!
//! It's xorshift64*, seeded from the clock the first time the egg wakes (a headless run's clock is
//! virtual, so a run plays the same every time). `--egg-seed <n>` (proof runs) makes it the
//! harness's seeded `Math.random` instead (`harness/egg/official.ts`: xorshift32 from `n`), so the
//! two apps draw the same numbers.

use std::cell::Cell;

/// The stream's state: a copy of it replays what the stream would draw next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fork {
    Unseeded,
    X64(u64),
    X32(u32),
}

impl Fork {
    /// A number in [0, 1).
    // Unbounded PRNG draw returning f64 directly, not a fallible Iterator::next.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        match self {
            Fork::X32(s) => {
                let mut x = *s;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *s = if x == 0 { 1 } else { x };
                f64::from(x) / 4_294_967_296.0
            }
            Fork::X64(s) => {
                *s ^= *s >> 12;
                *s ^= *s << 25;
                *s ^= *s >> 27;
                (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
            }
            Fork::Unseeded => {
                *self = Fork::X64(1);
                self.next()
            }
        }
    }
}

thread_local! {
    static STREAM: Cell<Fork> = const { Cell::new(Fork::Unseeded) };
    /// Numbers drawn so far, for `--egg-trace`.
    static DRAWN: Cell<u64> = const { Cell::new(0) };
}

/// How many numbers the stream has given (a proof run's trace compares it with Desktop's).
pub fn drawn() -> u64 {
    DRAWN.with(Cell::get)
}

/// A handle on the egg's stream: every handle draws from the same one.
#[derive(Clone, Copy, Debug)]
pub struct Rng(());

impl Rng {
    /// The stream, seeded from `seed` the first time (later wakes keep drawing where it was).
    pub fn new(seed: u64) -> Rng {
        STREAM.with(|s| {
            if s.get() == Fork::Unseeded {
                let proof = crate::arg("--egg-seed").and_then(|v| v.parse::<u32>().ok());
                s.set(proof.map_or(Fork::X64(seed | 1), Fork::X32));
            }
        });
        Rng(())
    }

    /// A handle on the stream as it is (seeded or not).
    pub fn shared() -> Rng {
        Rng(())
    }

    /// A number in [0, 1).
    // Unbounded PRNG draw returning f64 directly, not a fallible Iterator::next.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f64 {
        STREAM.with(|s| {
            let mut f = s.get();
            let v = f.next();
            s.set(f);
            DRAWN.with(|d| d.set(d.get() + 1));
            v
        })
    }
}

/// The stream as it is now, which then moves on by `n` numbers: what a sound will draw, drawn.
pub fn take(n: usize) -> Fork {
    STREAM.with(|s| {
        let fork = s.get();
        let mut f = fork;
        for _ in 0..n {
            f.next();
        }
        s.set(f);
        DRAWN.with(|d| d.set(d.get() + n as u64));
        fork
    })
}
