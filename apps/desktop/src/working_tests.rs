use rune::{ElementInfo, Env, Runtime};
use serde_json::json;

use super::*;
use crate::knobs::{WORKING_SPEED, WORKING_STYLE};

const DT: f64 = 1.0 / 120.0;

fn runtime(style: usize, reduced: bool) -> (Runtime<App>, String) {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    let crate::Route::Session(sid) = app.route.clone() else { panic!("no open session") };
    app.knobs.set(&WORKING_STYLE, style as f32);
    app.reduced_motion = reduced;
    // Only this session runs, so a quiet frame is the row's doing.
    app.running.retain(|id| *id == sid);
    let mut rt = Runtime::new(app, crate::view::app, (1280.0, 820.0), Env::new(true, 1.0));
    rt.poll = Some(crate::poll);
    rt.frame(0.0);
    (rt, sid)
}

fn event(rt: &mut Runtime<App>, ty: &str, sid: &str) {
    let at = rt.state.clock + 0.001;
    rt.state.playback.push_back((at, ty.to_owned(), json!({ "sessionID": sid, "assistantMessageID": "msg_zzzzworking" })));
}

fn play(rt: &mut Runtime<App>, secs: f64) {
    for _ in 0..(secs / DT).round() as usize {
        rt.frame(DT);
    }
}

/// The Working label's looping elements (the spinner's aside).
fn loops(rt: &Runtime<App>) -> Vec<ElementInfo> {
    rt.elements().into_iter().filter(|e| !e.exiting && e.path.contains("/working/") && !e.path.contains("/ring") && e.loop_phase.is_some()).collect()
}

fn phase(rt: &Runtime<App>, key: &str) -> f64 {
    let els = rt.elements();
    els.iter().find(|e| !e.exiting && e.key.as_deref() == Some(key) && e.path.contains("/working/")).and_then(|e| e.loop_phase).unwrap_or_else(|| panic!("no {key} loop"))
}

#[test]
fn a_loop_that_mounts_mid_run_waits_for_its_place_in_the_runs_rhythm() {
    let run = Run { since: 10.0, now: 10.3 };
    assert!((run.delay(1.2, 0.0) - 0.9).abs() < 1e-9);
    assert_eq!(Run { since: 10.0, now: 10.0 }.delay(1.2, 0.0), 0.0, "mounting with the run starts at once");
    assert!(Run { since: 10.0, now: 12.4 }.delay(1.2, 0.0) < 1e-6, "on a boundary, no wait");
    // A cycle reads the clock from the run's start, whenever it mounted.
    let c = Cycle { period: 1.2, phase: run.phase(1.2, 0.25), stops: &[(0.0, 0.0), (1.0, 1.0)], still: 0.0, fps: 120.0 };
    for t in [10.0_f64, 10.3, 11.7, 25.0] {
        let want = ((t - 10.0) / 1.2 + 0.25).rem_euclid(1.0) as f32;
        assert!((c.sample(t) - want).abs() < 1e-4, "at {t}: {} vs {want}", c.sample(t));
    }
}

#[test]
fn desktop_is_the_default_and_sweeps_its_band_once_every_1_2_s() {
    assert_eq!(rune_knobs::Knobs::default().get(&WORKING_STYLE), 0.0);
    let (mut rt, sid) = runtime(DESKTOP, false);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 0.5);
    let a = phase(&rt, "sheen");
    play(&mut rt, 0.3);
    let b = phase(&rt, "sheen");
    assert!(((b - a).rem_euclid(1.0) - 0.3 / 1.2).abs() < 1e-6, "{a} -> {b}");
}

#[test]
fn a_new_label_keeps_every_styles_phase() {
    for style in 0..STYLES.len() {
        let (mut rt, sid) = runtime(style, false);
        event(&mut rt, "session.execution.started", &sid);
        play(&mut rt, 0.55);
        let before: Vec<(String, f64)> = loops(&rt).into_iter().map(|e| (e.path, e.loop_phase.unwrap())).collect();
        assert!(!before.is_empty(), "{} has loops", STYLES[style]);
        rt.update(|s, _| {
            let now = s.clock;
            s.working.relabel("Running the tests", now);
        });
        rt.frame(DT);
        let after = loops(&rt);
        for (path, p) in &before {
            // A glyph past the shorter label goes; the rest step on by one frame.
            let Some(e) = after.iter().find(|e| &e.path == path) else { continue };
            let step = (e.loop_phase.unwrap() - p).rem_euclid(1.0);
            assert!(step < 0.02, "{}: {path} stepped {step}", STYLES[style]);
        }
    }
}

/// A sweep a new label interrupts carries on from where it was, then sweeps the new label: the
/// band never jumps, only wraps out of sight, and never changes speed by more than it must to
/// clear the longer words in the pass.
#[test]
fn a_sweep_glides_through_a_new_label() {
    for style in [DESKTOP, SMOOTH, GLINT] {
        let (mut rt, sid) = runtime(style, false);
        event(&mut rt, "session.execution.started", &sid);
        play(&mut rt, 0.55);
        let x = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("sheen")).map(|e| e.rect[0]).unwrap();
        let mut xs = vec![x(&rt)];
        for k in 0..360 {
            if k == 1 {
                rt.update(|s, _| {
                    let now = s.clock;
                    s.working.relabel("Searching the codebase", now);
                });
            }
            rt.frame(DT);
            xs.push(x(&rt));
        }
        let steps: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        for (k, pair) in steps.windows(2).enumerate() {
            if pair.iter().any(|d| d.abs() > 40.0) {
                continue; // a wrap
            }
            assert!(pair[1].abs() < 4.0, "{} at frame {k}: stepped {} pt", STYLES[style], pair[1]);
        }
        assert!(rt.state.working.relabeled.is_none(), "{}: the new label's sweep took over", STYLES[style]);
    }
}

#[test]
fn every_style_stops_asking_for_frames_when_the_run_ends() {
    for style in 0..STYLES.len() {
        let (mut rt, sid) = runtime(style, false);
        event(&mut rt, "session.execution.started", &sid);
        play(&mut rt, 1.0);
        assert!(!loops(&rt).is_empty(), "{} runs while working", STYLES[style]);
        event(&mut rt, "session.execution.succeeded", &sid);
        let mut quiet = None;
        for k in 0..120 {
            if !rt.frame(DT) {
                quiet = Some(k);
                break;
            }
        }
        assert!(quiet.is_some(), "{} still animating 1 s after the run: running {:?} busy {:?} loops {:?}", STYLES[style], rt.state.running, rt.state.transcripts.get(&sid).map(|t| t.busy()), rt.elements().into_iter().filter(|e| e.loop_phase.is_some()).map(|e| e.path.chars().rev().take(50).collect::<String>().chars().rev().collect::<String>()).take(6).collect::<Vec<_>>());
        assert!(loops(&rt).is_empty(), "{} left a loop", STYLES[style]);
        assert!((0..60).all(|_| !rt.frame(DT)), "{} woke again", STYLES[style]);
    }
}

#[test]
fn reduced_motion_draws_every_style_still() {
    for style in 0..STYLES.len() {
        let (mut rt, sid) = runtime(style, true);
        event(&mut rt, "session.execution.started", &sid);
        play(&mut rt, 0.5);
        assert!(loops(&rt).is_empty(), "{} loops under reduced motion", STYLES[style]);
        let label = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label") && e.path.contains("/working/")).expect("the label");
        assert!(label.opacity > 0.99);
    }
}

#[test]
fn picking_a_style_loads_its_preset_and_desktop_resets_them() {
    let mut app = App::empty();
    app.knobs.set(&WORKING_STYLE, 4.0);
    assert!(sync(&mut app));
    assert_eq!(app.knobs.get(&WORKING_SPEED), 260.0);
    assert!(!sync(&mut app), "once");
    app.knobs.set(&WORKING_SPEED, 300.0);
    assert!(!sync(&mut app), "a tweak stays");
    assert_eq!(app.knobs.get(&WORKING_SPEED), 300.0);
    app.knobs.set(&WORKING_STYLE, 0.0);
    sync(&mut app);
    assert_eq!(app.knobs.get(&WORKING_SPEED), WORKING_SPEED.default);
}

#[test]
fn desktops_colours_are_its_tokens_blended_in_srgb() {
    let p = crate::palette::pal(false);
    assert_eq!(colors(p).0.hex(), "#8f8f8fff");
    assert_eq!(colors(p).1.hex(), "#171717ff");
    let d = crate::palette::pal(true);
    let (weak, strong) = colors(d);
    let enc = |c: Color| srgb_encode(c.r);
    let bg = srgb_encode(d.bg_base.r);
    assert!((enc(weak) - (bg + (1.0 - bg) * 0.422)).abs() < 1e-4);
    assert!((enc(strong) - (bg + (1.0 - bg) * 0.936)).abs() < 1e-4);
}


