//! What happens in a run: Kit at the keyboard, the network, the server and other clients.
//! `Debug` prints each as the Rust a replay is written in (`replay(seed, &[Send, Wait(500)])`).

use super::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Types the next prompt (`p1`, `p2`…) and presses ↵: sends when idle, steers when busy.
    Send,
    /// Types the next prompt and presses ⌘↵: queues when busy.
    Queue,
    /// Taps the composer's Send/Stop button with nothing typed.
    Stop,
    /// Esc in the composer.
    Esc,
    /// ↵ with nothing typed: stops a run that shows Stop (#342).
    EnterEmpty,
    /// ↵ held down with nothing typed: key repeats never stop a run (t1347).
    HoldEnter,
    /// Lets `ms` pass.
    Wait(u32),
    /// The network drops every packet until `Fine` or `Slow`.
    Dead,
    /// Two seconds each way.
    Slow,
    /// The network is back to normal.
    Fine,
    /// The server takes the next write but its reply never arrives.
    DropResponse,
    /// The next write never reaches the server.
    LoseWrite,
    /// The server restarts and is gone for `secs`.
    Restart(u32),
    /// Another client sends a prompt to this session.
    Other,
    /// A background helper starts under this session and runs `secs`.
    Helper(u32),
}

pub use Op::*;

/// A random run of `n` steps for `seed`, mostly starting with a send so the session is busy.
pub fn generate(seed: u64) -> Vec<Op> {
    let mut rng = Rng::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed);
    let n = rng.range(6, 24) as usize;
    let mut ops = Vec::with_capacity(n + 1);
    if rng.chance(0.8) {
        ops.push(Send);
    }
    while ops.len() < n {
        ops.push(pick(&mut rng));
    }
    ops
}

fn pick(rng: &mut Rng) -> Op {
    const WEIGHTS: &[(u64, u8)] = &[(20, 0), (6, 1), (8, 2), (3, 3), (4, 4), (1, 5), (30, 6), (4, 7), (3, 8), (6, 9), (2, 10), (2, 11), (2, 12), (2, 13), (1, 14)];
    let total: u64 = WEIGHTS.iter().map(|w| w.0).sum();
    let mut roll = rng.below(total);
    let mut which = 0;
    for (w, op) in WEIGHTS {
        if roll < *w {
            which = *op;
            break;
        }
        roll -= w;
    }
    match which {
        0 => Send,
        1 => Queue,
        2 => Stop,
        3 => Esc,
        4 => EnterEmpty,
        5 => HoldEnter,
        6 => Wait(wait_ms(rng)),
        7 => Dead,
        8 => Slow,
        9 => Fine,
        10 => DropResponse,
        11 => LoseWrite,
        12 => Restart(rng.range(1, 10)),
        13 => Other,
        _ => Helper(rng.range(3, 40)),
    }
}

/// Mostly short, sometimes past a stop's 5 s confirm, a write's 15 s timeout, or the event
/// stream's 45 s silence.
fn wait_ms(rng: &mut Rng) -> u32 {
    match rng.below(10) {
        0..=2 => rng.range(50, 500),
        3..=6 => rng.range(500, 5_000),
        7 | 8 => rng.range(5_000, 20_000),
        _ => rng.range(20_000, 60_000),
    }
}

/// Ops written as a replay lists them (`Send, Wait(500), Dead`).
pub fn parse(text: &str) -> Result<Vec<Op>, String> {
    let text = text.trim().trim_start_matches("&[").trim_start_matches('[').trim_end_matches(']');
    let mut ops = vec![];
    for word in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        let (name, arg) = match word.split_once('(') {
            Some((name, rest)) => (name, Some(rest.trim_end_matches(')').trim().parse::<u32>().map_err(|e| format!("{word}: {e}"))?)),
            None => (word, None),
        };
        ops.push(match (name, arg) {
            ("Send", None) => Send,
            ("Queue", None) => Queue,
            ("Stop", None) => Stop,
            ("Esc", None) => Esc,
            ("EnterEmpty", None) => EnterEmpty,
            ("HoldEnter", None) => HoldEnter,
            ("Dead", None) => Dead,
            ("Slow", None) => Slow,
            ("Fine", None) => Fine,
            ("DropResponse", None) => DropResponse,
            ("LoseWrite", None) => LoseWrite,
            ("Other", None) => Other,
            ("Wait", Some(n)) => Wait(n),
            ("Restart", Some(n)) => Restart(n),
            ("Helper", Some(n)) => Helper(n),
            _ => return Err(format!("not an op: {word}")),
        });
    }
    Ok(ops)
}

/// The ops as a replay call.
pub fn replay_code(seed: u64, ops: &[Op]) -> String {
    let list: Vec<String> = ops.iter().map(|o| format!("{o:?}")).collect();
    format!("replay(0x{seed:x}, &[{}])", list.join(", "))
}
