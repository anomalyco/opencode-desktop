//! Shrinks a failing run to the fewest, simplest steps that still fail the same way: delta
//! debugging (Zeller's ddmin) over the ops, then each wait and restart made shorter.

use super::ops::Op;
use super::oracle::Kind;
use super::world::run;

/// The smallest run of `ops` from `seed` found within `budget` replays that still fails with
/// `kind`.
pub fn shrink(seed: u64, ops: &[Op], kind: Kind, budget: usize) -> Vec<Op> {
    let fails = |c: &[Op]| !c.is_empty() && run(seed, c, true).failure.is_some_and(|f| f.kind == kind);
    let mut cur = ops.to_vec();
    let mut n = 2;
    let mut spent = 0;
    while cur.len() >= 2 && spent < budget {
        let chunk = cur.len().div_ceil(n);
        let mut cut = false;
        for i in 0..n {
            let start = i * chunk;
            if start >= cur.len() || spent >= budget {
                break;
            }
            let end = (start + chunk).min(cur.len());
            let mut candidate = cur[..start].to_vec();
            candidate.extend_from_slice(&cur[end..]);
            spent += 1;
            if fails(&candidate) {
                cur = candidate;
                n = (n - 1).max(2);
                cut = true;
                break;
            }
        }
        if !cut {
            if n >= cur.len() {
                break;
            }
            n = (n * 2).min(cur.len());
        }
    }
    let mut changed = true;
    while changed && spent < budget {
        changed = false;
        for i in 0..cur.len() {
            let Some(simpler) = simpler(cur[i]) else { continue };
            if spent >= budget {
                break;
            }
            let mut candidate = cur.clone();
            candidate[i] = simpler;
            spent += 1;
            if fails(&candidate) {
                cur = candidate;
                changed = true;
            }
        }
    }
    cur
}

fn simpler(op: Op) -> Option<Op> {
    match op {
        Op::Wait(ms) if ms > 100 => Some(Op::Wait(round(ms / 2))),
        Op::Restart(s) if s > 1 => Some(Op::Restart(s / 2)),
        Op::Helper(s) if s > 3 => Some(Op::Helper(s / 2)),
        Op::Queue | Op::HoldEnter => Some(Op::Send),
        Op::Slow => Some(Op::Fine),
        _ => None,
    }
}

/// To two significant figures, so a shrunk replay reads cleanly.
fn round(ms: u32) -> u32 {
    let mut unit = 1;
    while ms / unit >= 100 {
        unit *= 10;
    }
    (ms / unit) * unit
}
