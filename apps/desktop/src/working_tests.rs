use rune::{ElementInfo, Env, Runtime};
use serde_json::json;

use super::*;
use crate::knobs::{WORKING_PAUSE, WORKING_SPEED};

const DT: f64 = 1.0 / 120.0;

fn runtime(reduced: bool) -> (Runtime<App>, String) {
    let mut app = App::empty();
    app.load_fixture(crate::FIXTURE);
    crate::open_scene(&mut app);
    app.finish_stream();
    let crate::Route::Session(sid) = app.route.clone() else { panic!("no open session") };
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

fn sheen(rt: &Runtime<App>) -> ElementInfo {
    rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("sheen")).expect("the sheen")
}

#[test]
fn a_loop_that_mounts_mid_run_waits_for_its_place_in_the_runs_rhythm() {
    let run = Run { since: 10.0, now: 10.3 };
    assert!((run.delay(1.2) - 0.9).abs() < 1e-9);
    assert_eq!(Run { since: 10.0, now: 10.0 }.delay(1.2), 0.0, "mounting with the run starts at once");
    assert!(Run { since: 10.0, now: 12.4 }.delay(1.2) < 1e-6, "on a boundary, no wait");
}

#[test]
fn the_band_sweeps_at_a_steady_speed_whatever_the_label() {
    let app = App::empty();
    let speed = WORKING_SPEED.default;
    for w in [52.0, 150.0] {
        let b = band(&app, w);
        assert!((((b.x1 - b.x0) / b.period as f32) - speed).abs() < 1e-3, "{w} pt: {b:?}");
        assert!(b.x0 + b.width <= 0.0 && b.x1 >= w, "starts and ends off the label: {b:?}");
    }
    let (mut rt, sid) = runtime(false);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 0.5);
    let a = sheen(&rt).rect[0];
    play(&mut rt, 0.2);
    let b = sheen(&rt).rect[0];
    assert!(((b - a) / 0.2 - speed).abs() < 2.0, "{a} -> {b}");
}

#[test]
fn a_pause_rests_out_of_sight_between_passes() {
    let mut app = App::empty();
    let w = 52.0;
    let moving = band(&app, w);
    app.knobs.set(&WORKING_PAUSE, 1.0);
    let paused = band(&app, w);
    assert!((paused.period - moving.period - 1.0).abs() < 1e-6);
    assert!((((paused.x1 - paused.x0) / paused.period as f32) - ((moving.x1 - moving.x0) / moving.period as f32)).abs() < 1e-3, "same speed");
}

#[test]
fn a_new_label_keeps_the_sweeps_phase() {
    let (mut rt, sid) = runtime(false);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 0.55);
    let before: Vec<(String, f64)> = loops(&rt).into_iter().map(|e| (e.path, e.loop_phase.unwrap())).collect();
    assert!(!before.is_empty());
    rt.update(|s, _| {
        let now = s.clock;
        s.working.relabel("Running the tests", now);
    });
    rt.frame(DT);
    let after = loops(&rt);
    for (path, p) in &before {
        let e = after.iter().find(|e| &e.path == path).unwrap_or_else(|| panic!("{path} went"));
        let step = (e.loop_phase.unwrap() - p).rem_euclid(1.0);
        assert!(step < 0.02, "{path} stepped {step}");
    }
}

/// A sweep a new label interrupts carries on from where it was, then sweeps the new label: on
/// the words the band never jumps, and never changes speed by more than it must to clear the
/// longer words in the pass. A row that mounts again afterwards keeps the rhythm.
#[test]
fn a_sweep_glides_through_a_new_label() {
    let (mut rt, sid) = runtime(false);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 0.55);
    let label = |rt: &Runtime<App>| rt.elements().into_iter().find(|e| !e.exiting && e.key.as_deref() == Some("label") && e.path.contains("/working/")).unwrap().rect;
    // Where the band is, and whether more than its faded tip is on the words.
    let at = |rt: &Runtime<App>| {
        let (s, l) = (sheen(rt).rect, label(rt));
        (s[0], s[0] + s[2] > l[0] + 2.0 && s[0] < l[0] + l[2] - 2.0)
    };
    let mut xs = vec![at(&rt)];
    for k in 0..600 {
        if k == 1 {
            rt.update(|s, _| {
                let now = s.clock;
                s.working.relabel("Searching the codebase", now);
            });
        }
        rt.frame(DT);
        xs.push(at(&rt));
    }
    for (k, w) in xs.windows(2).enumerate() {
        if w[0].1 && w[1].1 {
            assert!((w[1].0 - w[0].0).abs() < 4.0, "at frame {k}: stepped {} pt on the words", w[1].0 - w[0].0);
        }
    }
    assert!(rt.state.working.relabeled.is_none(), "the new label's sweep took over");
    let running = sheen(&rt).loop_phase.unwrap();
    // Off screen and back: the same row, mounted again.
    rt.update(|s, _| s.reduced_motion = true);
    rt.frame(DT);
    rt.update(|s, _| s.reduced_motion = false);
    rt.frame(DT);
    play(&mut rt, 0.6);
    let b = band(&rt.state, crate::working::width("Searching the codebase"));
    let again = sheen(&rt).loop_phase.unwrap();
    let want = (running + (3.0 * DT + 0.6) / b.period).rem_euclid(1.0);
    assert!((again - want).abs() < 0.01, "remounted at {again}, the rhythm says {want}");
}

#[test]
fn the_sweep_stops_asking_for_frames_when_the_run_ends() {
    let (mut rt, sid) = runtime(false);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 1.0);
    assert!(!loops(&rt).is_empty(), "runs while working");
    event(&mut rt, "session.execution.succeeded", &sid);
    assert!((0..120).any(|_| !rt.frame(DT)), "still animating 1 s after the run");
    assert!(loops(&rt).is_empty(), "left a loop");
    assert!((0..60).all(|_| !rt.frame(DT)), "woke again");
}

#[test]
fn reduced_motion_draws_the_label_still() {
    let (mut rt, sid) = runtime(true);
    event(&mut rt, "session.execution.started", &sid);
    play(&mut rt, 0.5);
    assert!(loops(&rt).is_empty(), "loops under reduced motion");
    let label = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label") && e.path.contains("/working/")).expect("the label");
    assert!(label.opacity > 0.99);
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
