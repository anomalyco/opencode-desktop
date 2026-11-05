//! Finds send, steer, queue and stop bugs by playing seeded random sessions against the real
//! app (t1387). A deterministic server ([`sim`]) runs one session as OpenCode does; a simulated
//! network ([`link`]) stands where `opencode_kit::client::connect` and the page lanes do, slow,
//! dead, dropping a reply or restarting the server; [`ops`] are what Kit does (send, steer,
//! queue, stop, Esc, an empty ↵) through real keys and taps. After every frame [`oracle`]
//! compares what the screen shows with what the server has; a failure is shrunk ([`shrink`]) to
//! the fewest steps and printed as a replay with a timeline of screen vs server ([`trace`]).
//!
//! ```sh
//! # 200 seeds from 0, each failure shrunk and printed (captures/interaction-fuzz/report-<FUZZ_FROM>.txt)
//! FUZZ_SEEDS=200 FUZZ_FROM=0 cargo test -p opencode-desktop interaction_fuzz::tests::fuzz -- --ignored --nocapture
//!
//! # each "name<TAB>ops" line of bugs.tsv replayed once on a real Drive server
//! apps/desktop/scripts/interaction-fuzz/real.sh confirm apps/desktop/src/interaction_fuzz/bugs.tsv
//! ```
//!
//! Every run starts from the fixture's open session on a live source whose channels lead
//! here, so nothing reaches a real server.

mod desk;
mod link;
mod ops;
mod oracle;
mod proxy;
mod real;
#[cfg(test)]
mod recorded_tests;
mod shrink;
mod sim;
#[cfg(test)]
mod tests;
mod trace;
mod world;

use std::cell::Cell;

thread_local! {
    /// The app's wall clock under the fuzzer (virtual ms), so `now_ms` follows its time.
    static WALL_MS: Cell<Option<i64>> = const { Cell::new(None) };
}

/// The fuzzer's virtual wall clock on this thread, when one runs.
pub fn wall_override() -> Option<i64> {
    WALL_MS.with(Cell::get)
}

fn set_wall(ms: Option<i64>) {
    WALL_MS.with(|w| w.set(ms));
}

/// The fuzzer's virtual clock as an `Instant`, for timers that measure with one (the open
/// session's "Still trying" silence).
pub fn instant_override() -> Option<std::time::Instant> {
    static BASE: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let ms = wall_override()?;
    let base = *BASE.get_or_init(std::time::Instant::now);
    Some(base + std::time::Duration::from_millis((ms - sim::T0).max(0) as u64))
}

/// xorshift64*: small, seeded, and the same on every machine.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng(seed ^ 0x2545_f491_4f6c_dd1d);
        if r.0 == 0 {
            r.0 = 1;
        }
        r.next();
        r
    }

    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// In `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + self.below(u64::from(hi - lo + 1)) as u32
    }
}
