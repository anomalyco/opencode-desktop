//! How long a click on a Settings page takes to its settled frame, with a large model catalog.

use rune::Runtime;

use crate::App;

const DT: f64 = 1.0 / 120.0;
const PAGES: [&str; 5] = ["general", "providers", "models", "extensions", "appearance"];

fn app() -> Runtime<App> {
    let mut a = App::fixture_session();
    let models: Vec<_> = (0..24).flat_map(|p| (0..25).map(move |m| serde_json::json!({
        "id": format!("model-{m}"), "providerID": format!("provider-{p}"), "name": format!("Model {p}.{m}"),
        "variants": [{ "id": "low" }, { "id": "high" }], "limit": { "context": 200000 },
    }))).collect();
    a.pickers.load_live(&serde_json::Value::Array(models), &serde_json::json!([{ "id": "build" }]));
    let mut rt = a.into_runtime_sized((1280.0, 820.0));
    rt.frame(0.0);
    rt.state.open_settings();
    rt.invalidate();
    settle(&mut rt);
    rt
}

/// Frames until nothing moves; how many it took.
fn settle(rt: &mut Runtime<App>) -> usize {
    (0..240).take_while(|_| rt.frame(DT)).count()
}

#[test]
#[ignore = "CPU benchmark; run --profile perf --features frame-profile --ignored --nocapture --test-threads=1"]
fn settings_page_switch_benchmark() {
    assert!(!cfg!(debug_assertions), "use --profile perf");
    let mut rt = app();
    let mut first: Vec<Vec<f64>> = vec![vec![]; PAGES.len()];
    let mut settled: Vec<Vec<f64>> = vec![vec![]; PAGES.len()];
    let mut frames: Vec<Vec<f64>> = vec![vec![]; PAGES.len()];
    for sample in 0..12 {
        for (at, page) in PAGES.iter().enumerate() {
            let (x, y) = rt.find(&format!("settings-nav-{page}")).expect("the nav row");
            rune::take_frame_phases();
            let start = rune::thread_cpu_ms();
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(DT);
            let one = rune::thread_cpu_ms() - start;
            if sample == 11 {
                let ph = rune::take_frame_phases();
                let p = rune::PHASES;
                println!("METRIC name=switch_to_{page}_phases view={:.3} commit={:.3} layout={:.3} text={:.3}", ph[p], ph[p + 1], ph[p + 2], ph[p + 3]);
            }
            let n = settle(&mut rt);
            if sample > 1 {
                first[at].push(one);
                settled[at].push(rune::thread_cpu_ms() - start);
                frames[at].push(n as f64 + 1.0);
            }
        }
    }
    for (at, page) in PAGES.iter().enumerate() {
        crate::perf::report(&format!("switch_to_{page}_first_frame_cpu_ms"), &mut first[at]);
        crate::perf::report(&format!("switch_to_{page}_settled_cpu_ms"), &mut settled[at]);
        crate::perf::report(&format!("switch_to_{page}_frames"), &mut frames[at]);
    }
}
