//! Settings › Interface layout: each tab flies on its own straight path between the titlebar
//! and the sidebar, under the panel, while the panel and the sidebar move in step.

use rune::Runtime;

use super::{Circ, Run};
use crate::App;

/// Settings open over the window in top tabs, recorded from there.
fn open(r: &mut Run) {
    r.rt.update(|s, tx| {
        tx.instant();
        s.open_settings();
    });
    r.idle(1.0);
    r.restart();
}

/// The motion gate's run: to side tabs and back.
pub fn run(r: &mut Run) {
    open(r);
    r.act("side tabs", |rt| Run::tap(rt, "interface-mode-side"));
    r.play(0.8);
    r.act("top tabs", |rt| Run::tap(rt, "interface-mode-top"));
    r.play_out(0.8);
}

/// One frame of a switch: each tab's drawn rect and the opacity its title is drawn at, and the
/// panel's and the sidebar's rects.
struct Shot {
    tabs: Vec<(String, [f32; 4], f32)>,
    panel: [f32; 4],
    sidebar: [f32; 4],
    /// Tabs drawn above the panel where the panel covers their centre.
    over_panel: Vec<String>,
}

fn shot(rt: &Runtime<App>) -> Shot {
    let els = rt.elements();
    let rect = |key: &str| els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(key)).map_or([0.0; 4], |e| e.rect);
    let panel = rect("panel");
    let mut tabs = Vec::new();
    let mut over_panel = Vec::new();
    for e in els.iter().filter(|e| !e.exiting && e.role == "tab" && e.key.as_deref().is_some_and(|k| k.starts_with("t-"))) {
        let key = e.key.clone().unwrap();
        let title = els.iter().find(|l| l.key.as_deref() == Some("label") && l.path.starts_with(&format!("{}/", e.path))).map_or(1.0, |l| l.opacity);
        tabs.push((key.clone(), e.rect, title));
        let [x, y, w, h] = e.rect;
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let inside = cx > panel[0] + 2.0 && cx < panel[0] + panel[2] - 2.0 && cy > panel[1] + 2.0 && cy < panel[1] + panel[3] - 2.0;
        if inside && rt.element_at(cx, cy).is_some_and(|p| !p.path().contains("/panel")) {
            over_panel.push(key);
        }
    }
    Shot { tabs, panel, sidebar: rect("sidebar"), over_panel }
}

/// Taps `key` and records the switch it starts until it has long settled, the frame before
/// the tap first.
fn switch(r: &mut Run, key: &str) -> Vec<Shot> {
    let mut shots = vec![shot(&r.rt)];
    r.act(key, |rt| Run::tap(rt, key));
    shots.push(shot(&r.rt));
    for _ in 0..96 {
        r.step();
        shots.push(shot(&r.rt));
    }
    shots
}

/// What's wrong with one switch; empty when every tab flew as it should.
fn judge(shots: &[Shot]) -> Vec<String> {
    let mut out = Vec::new();
    let (first, last) = (&shots[0], &shots[shots.len() - 1]);
    for (i, s) in shots.iter().enumerate() {
        // One copy of each tab in every frame: never half the old layout and half the new.
        if s.tabs.len() != first.tabs.len() {
            out.push(format!("frame {i}: {} tabs, {} before the switch", s.tabs.len(), first.tabs.len()));
        }
        for (key, _, title) in &s.tabs {
            if *title < 0.99 {
                out.push(format!("frame {i}: {key}'s title drawn at {title:.2}"));
            }
        }
        for key in &s.over_panel {
            out.push(format!("frame {i}: {key} drawn over the page"));
        }
        // The panel keeps its 8 pt from the sidebar's edge the whole way.
        let gap = s.panel[0] - (s.sidebar[0] + s.sidebar[2]);
        if (gap - 8.0).abs() > 1.0 {
            out.push(format!("frame {i}: the panel sits {gap:.1} pt from the sidebar's edge"));
        }
    }
    for (key, from, _) in &first.tabs {
        let Some(to) = last.tabs.iter().find(|t| &t.0 == key).map(|t| t.1) else {
            out.push(format!("{key} is gone after the switch"));
            continue;
        };
        let path: Vec<[f32; 4]> = shots.iter().filter_map(|s| s.tabs.iter().find(|t| &t.0 == key).map(|t| t.1)).collect();
        let (d, len) = ([to[0] - from[0], to[1] - from[1]], ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt());
        if len < 20.0 {
            out.push(format!("{key} didn't move: {from:?} to {to:?}"));
            continue;
        }
        // It glides there over frames, never in one jump.
        let ts: Vec<f32> = path.iter().map(|r| ((r[0] - from[0]) * d[0] + (r[1] - from[1]) * d[1]) / (len * len)).collect();
        let in_flight = ts.iter().filter(|&&t| t > 0.02 && t < 0.98).count();
        if in_flight < 8 {
            out.push(format!("{key} is between its slots for only {in_flight} frames"));
        }
        if let Some((i, w)) = ts.windows(2).enumerate().find(|(_, w)| w[1] - w[0] > 0.25) {
            out.push(format!("{key} frame {}: jumps {:.0}% of its path at once", i + 1, (w[1] - w[0]) * 100.0));
        }
        let mut along = 0.0_f32;
        for (i, r) in path.iter().enumerate() {
            let v = [r[0] - from[0], r[1] - from[1]];
            // Off the straight line from where it was to where it lands, or back along it.
            let off = (v[0] * d[1] - v[1] * d[0]).abs() / len;
            if off > 2.0 {
                out.push(format!("{key} frame {i}: {off:.1} pt off its path"));
                break;
            }
            let t = (v[0] * d[0] + v[1] * d[1]) / (len * len);
            if t < along - 0.01 || t > 1.01 {
                out.push(format!("{key} frame {i}: turns back or overshoots ({t:.3} after {along:.3})"));
                break;
            }
            along = along.max(t);
        }
        let end = path[path.len() - 1];
        if (0..4).any(|k| (end[k] - to[k]).abs() > 0.5) {
            out.push(format!("{key} ends at {end:?}, not {to:?}"));
        }
    }
    out
}

#[test]
fn each_tab_flies_its_own_path_under_the_page() {
    let mut r = Run::new(Circ::default());
    open(&mut r);
    for (key, layout) in [("interface-mode-side", "side"), ("interface-mode-top", "top")] {
        let shots = switch(&mut r, key);
        assert!(shots[0].tabs.len() >= 4, "the fixture opens with tabs: {}", shots[0].tabs.len());
        let wrong = judge(&shots);
        assert!(wrong.is_empty(), "to {layout} tabs:\n{}", wrong.join("\n"));
    }
}
