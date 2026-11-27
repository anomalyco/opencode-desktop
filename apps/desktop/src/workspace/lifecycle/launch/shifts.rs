//! `--launch-trace-shifts`: the layout shifts of a launch, for `perf/content.bench.ts`. From the
//! first frame on, an element a person could see in one frame that stands somewhere else in the
//! next has shifted: its place or size changed while it stayed on screen. Elements that appear
//! or leave are not shifts; the ones they push are. The trace's line gets `shifts` (frames with
//! one, elements moved, and of those the ones whose parent didn't move the same way: the roots
//! of each shift, with the first of them named), and stderr a `launch-shift` line per such frame.
//!
//! `seen` counts the roots a person saw move: the ones that draw something (words, a fill, an
//! icon), or carry something that does. A box that draws nothing and only grew around children
//! that stayed where they were (a list getting rows above the ones on screen, a row's padding
//! closing over a bubble that held its place) is a root and not a seen one. A border alone isn't
//! known to the walk, so a box with nothing but a border counts as drawing nothing.
//!
//! Every frame's elements are walked for it, so a run that times the launch leaves it off.

use std::collections::HashMap;
use std::sync::Mutex;

use rune::time::Instant;

use crate::App;

/// How far (in points) an edge moves before it counts: a subpixel settle of text doesn't.
const SLACK: f32 = 0.5;
/// How many shifted roots the trace names.
const NAMED: usize = 24;

#[derive(Default)]
struct Shifts {
    /// Each window's last frame: every visible element's rect, by its path.
    prev: HashMap<String, HashMap<String, [f32; 4]>>,
    frames: u32,
    elements: u32,
    roots: u32,
    seen: u32,
    named: Vec<String>,
    last: Option<Instant>,
}

static SHIFTS: Mutex<Option<Shifts>> = Mutex::new(None);

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::flag("--launch-trace-shifts"))
}

fn moved(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a.iter().zip(b).any(|(x, y)| (x - y).abs() > SLACK)
}

/// After a frame drew: compares what's on screen with the window's last frame.
pub fn drawn(rt: &rune::Runtime<App>, since_main_ms: u128) {
    if !on() {
        return;
    }
    let elements = rt.elements();
    // On screen: some of it is left by its ancestors' clips (a row scrolled out of its list isn't).
    let seen = |e: &rune::ElementInfo| e.opacity > 0.0 && !e.exiting && e.visible[2] > 0.0 && e.visible[3] > 0.0;
    let now: HashMap<String, [f32; 4]> = elements.iter().filter(|e| seen(e)).map(|e| (e.path.clone(), e.rect)).collect();
    let draws: std::collections::HashSet<&str> =
        elements.iter().filter(|e| e.text.is_some() || e.fill.a > 0.0 || !matches!(e.role, "" | "group")).map(|e| e.path.as_str()).collect();
    let parents: HashMap<&str, &str> = elements.iter().filter_map(|e| Some((e.path.as_str(), elements.get(e.parent?)?.path.as_str()))).collect();
    let mut guard = SHIFTS.lock().unwrap_or_else(|e| e.into_inner());
    let all = guard.get_or_insert_with(Shifts::default);
    let window = rt.state.window.id.to_string();
    if let Some(before) = all.prev.get(&window) {
        let shifted: Vec<(&String, &[f32; 4], &[f32; 4])> = now.iter().filter_map(|(path, rect)| before.get(path).filter(|old| moved(old, rect)).map(|old| (path, old, rect))).collect();
        if !shifted.is_empty() {
            // A root: its parent stayed, or moved another way (a child carried by its parent isn't one).
            let delta = |old: &[f32; 4], new: &[f32; 4]| [new[0] - old[0], new[1] - old[1]];
            let roots: Vec<&(&String, &[f32; 4], &[f32; 4])> = shifted
                .iter()
                .filter(|(path, old, new)| {
                    let Some(parent) = parents.get(path.as_str()) else { return true };
                    let (Some(p_old), Some(p_new)) = (before.get(*parent), now.get(*parent)) else { return true };
                    let (d, p) = (delta(old, new), delta(p_old, p_new));
                    (d[0] - p[0]).abs() > SLACK || (d[1] - p[1]).abs() > SLACK || (old[2] - new[2]).abs() > SLACK || (old[3] - new[3]).abs() > SLACK
                })
                .collect();
            // Seen: the root draws, or something that moved with it (or inside it) does.
            let seen_root = |root: &str| draws.contains(root) || shifted.iter().any(|(path, ..)| draws.contains(path.as_str()) && path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/')));
            let seen = roots.iter().filter(|(path, ..)| seen_root(path)).count();
            all.frames += 1;
            all.elements += shifted.len() as u32;
            all.roots += roots.len() as u32;
            all.seen += seen as u32;
            all.last = Some(Instant::now());
            eprintln!("launch-shift {since_main_ms} ms after main, window {window}: {} elements moved, {} of them on their own, {seen} of those seen", shifted.len(), roots.len());
            let mut lines: Vec<String> = roots.iter().map(|(path, old, new)| format!("{path} {old:?} -> {new:?}{}", if seen_root(path) { "" } else { " (draws nothing)" })).collect();
            lines.sort();
            for line in lines.iter().take(8) {
                eprintln!("  {line}");
            }
            let room = NAMED.saturating_sub(all.named.len());
            all.named.extend(lines.into_iter().take(room).map(|l| format!("{since_main_ms} ms: {l}")));
        }
    }
    all.prev.insert(window, now);
}

/// The trace's `shifts`: none when the run didn't count them.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn json(ns: &dyn Fn(Instant) -> u64) -> serde_json::Value {
    let guard = SHIFTS.lock().unwrap_or_else(|e| e.into_inner());
    match &*guard {
        Some(s) if on() => serde_json::json!({ "frames": s.frames, "elements": s.elements, "roots": s.roots, "seen": s.seen, "last_ns": s.last.map(ns), "first": s.named }),
        _ => serde_json::Value::Null,
    }
}
