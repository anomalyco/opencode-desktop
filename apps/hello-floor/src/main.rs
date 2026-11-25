//! One window, one line of text, nothing else: the least a Rune app holds, to read its memory
//! against a bare Direct3D window below it and the desktop app above it.
//!
//! `HELLO_FLOOR_MEMORY_POLICY` sets Rune's GPU suballocation policy before the device is made
//! (`memory-usage`, `performance`, or `manual:<min>-<max>` in KiB). `HELLO_FLOOR_WARM` names the effect
//! pipelines Rune compiles ahead of their first use (`none`, `all`, `backdrop`).
#![cfg_attr(windows, windows_subsystem = "windows")]

use rune::{AppSpec, Cx, El, col, text};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().w_full().h_full().center().child(text("Hello, world"))
}

pub fn run() {
    let app = rune::App::new(AppSpec::new("Hello floor", (960.0, 640.0), || (), view));
    let app = match rune::env::var("HELLO_FLOOR_MEMORY_POLICY").as_deref().and_then(policy) {
        Some(policy) => app.gpu_memory_policy(policy),
        None => app,
    };
    // `HELLO_FLOOR_WARM`: the effect pipelines compiled ahead of use (`none`, `all`, `backdrop`); unset keeps Rune's default.
    let app = match rune::env::var("HELLO_FLOOR_WARM").as_deref() {
        Some("none") => app.prewarm_effects(rune::PrewarmEffects::NONE),
        Some("all") => app.prewarm_effects(rune::PrewarmEffects::ALL),
        Some("backdrop") => app.prewarm_effects(rune::PrewarmEffects::BACKDROP_BLUR),
        _ => app,
    };
    app.run();
}

fn policy(raw: &str) -> Option<rune::GpuMemoryPolicy> {
    match raw {
        "memory-usage" => Some(rune::GpuMemoryPolicy::MemoryUsage),
        "performance" => Some(rune::GpuMemoryPolicy::Performance),
        _ => {
            let (min, max) = raw.strip_prefix("manual:")?.split_once('-')?;
            let kib = |s: &str| s.parse::<u64>().ok().map(|n| n * 1024);
            Some(rune::GpuMemoryPolicy::Manual { block_size: kib(min)?..kib(max)? })
        }
    }
}

#[allow(dead_code)]
fn main() {
    run();
}
