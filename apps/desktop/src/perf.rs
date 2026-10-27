//! CPU-only runtime replay: no window, GPU, network, or saved app state.
use crate::{App, Route, view};
use opencode_kit::model::{Assistant, Message, Part, ToolStatus, Transcript};
use rune::{Env, Runtime};

#[global_allocator]
static ALLOC: procwatch::alloc::Counting = procwatch::alloc::Counting::system();

pub fn fixture() -> App {
    let mut app = App::empty();
    let mut messages = Vec::new();
    for i in 0..800 {
        messages.push(Message::User { id: format!("u{i:04}"), text: format!("Check implementation {i} and explain the result.") });
        messages.push(Message::Assistant(Assistant {
            id: format!("a{i:04}"), completed: Some(1),
            parts: vec![
                Part::Reasoning { text: "Inspect the boundary and its callers.".into(), started: 0, completed: Some(1) },
                Part::Tool { id: format!("c{i}"), name: "read".into(), title: "src/runtime.rs".into(), status: ToolStatus::Completed,
                    input: serde_json::Value::Null, files: vec![], metadata: serde_json::Value::Null },
                Part::Text { text: format!("## Result {i}\n\nThe **boundary** preserves identity while the next update arrives. {}\n\n```rust\nfn update_{i}(value: usize) -> usize {{\n    value.saturating_add(1)\n}}\n```\n\n- Keep the existing state\n- Apply the new value\n", "Some prose with `code` and [a link](https://example.com). ".repeat(3)) },
            ], ..Default::default()
        }));
    }
    messages.push(Message::Assistant(Assistant {
        id: "stream".into(),
        parts: vec![Part::Text { text: "Streaming answer".into() }],
        ..Default::default()
    }));
    app.route = Route::Session("bench".into());
    app.transcripts.insert("bench".into(), Transcript { session: "bench".into(), messages });
    let mut other = app.transcripts["bench"].clone();
    other.session = "other".into();
    app.transcripts.insert("other".into(), other);
    app
}

fn report(name: &str, values: &mut [f64]) {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    println!("METRIC name={name} p50={:.4} p95={:.4} max={:.4} n={}", at(0.5), at(0.95), at(1.0), values.len());
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --ignored --nocapture --test-threads=1"]
fn timeline_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut open = [vec![], vec![]];
    let mut all = [vec![vec![]; 24], vec![vec![]; 24]];
    for sample in 0..8 {
        // Alternate the reference and lazy path to share thermal/cache drift.
        for eager in if sample % 2 == 0 { [true, false] } else { [false, true] } {
            view::EAGER_ROWS.set(eager);
            let variant = usize::from(eager);
            let app = fixture();
            let start = rune::thread_cpu_ms();
            let mut rt = Runtime::new(app, view::app, (1280.0, 820.0), Env::new(true, 2.0));
            rt.frame(0.0);
            let elapsed = rune::thread_cpu_ms() - start;
            if sample > 0 {
                open[variant].push(elapsed);
            }
            for _ in 0..120 {
                if !rt.frame(1.0 / 120.0) {
                    break;
                }
            }
            for mode in 0..4 {
                if mode == 1 {
                    rt.update(|_, tx| tx.scroll_to_bottom_instant("scroll-bench"));
                    rt.frame(0.0);
                    for _ in 0..120 {
                        if !rt.frame(1.0 / 120.0) {
                            break;
                        }
                    }
                }
                for frame in 0..32 {
                    rune::take_frame_phases();
                    let alloc = procwatch::alloc::thread_allocations();
                    let start = rune::thread_cpu_ms();
                    if mode == 0 {
                        rt.wheel(900.0, 350.0, 96.0);
                    } else if mode == 1 {
                        rt.update(|s, _| { s.apply_event("session.text.delta", &serde_json::json!({"sessionID":"bench", "assistantMessageID":"stream", "ordinal":0, "delta":" another word"})); });
                    } else if mode == 2 {
                        rt.update(|s, _| {
                        s.apply_event("session.prompted", &serde_json::json!({"sessionID":"bench", "messageID":format!("arrival{frame}"), "text":"Please inspect the next boundary."}));
                    });
                    } else {
                        rt.update(|s, tx| {
                            s.route = Route::Session(if frame % 2 == 0 { "other" } else { "bench" }.into());
                            tx.settle();
                        });
                    }
                    rt.frame(1.0 / 120.0);
                    let elapsed = rune::thread_cpu_ms() - start;
                    let allocations = (procwatch::alloc::thread_allocations() - alloc) as f64;
                    let phases = rune::take_frame_phases();
                    if sample > 0 && frame > 0 {
                        let p = rune::PHASES;
                        for (i, value) in
                            [elapsed, phases[p], phases[p + 2], phases[p + 3], allocations, phases[p + 1]].into_iter().enumerate()
                        {
                            all[variant][mode * 6 + i].push(value);
                        }
                    }
                }
            }
        }
    }
    view::EAGER_ROWS.set(false);
    for (variant, name) in ["lazy", "eager"].iter().enumerate() {
        report(&format!("{name}_open_cpu_ms"), &mut open[variant]);
        for (mode, label) in ["scroll", "stream", "arrival", "switch"].iter().enumerate() {
            for (i, metric) in ["cpu_ms", "view_ms", "layout_ms", "text_ms", "allocations", "commit_ms"].iter().enumerate() {
                report(&format!("{name}_{label}_{metric}"), &mut all[variant][mode * 6 + i]);
            }
        }
    }
}
