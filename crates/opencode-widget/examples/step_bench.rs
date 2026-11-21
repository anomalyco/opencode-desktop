//! Host cost of a step's input: the built-in `table` at 200 rows, `init`, a press, and one
//! sparkline draw. `cargo run --release -p opencode-widget --example step_bench`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::time::Instant;

use opencode_widget::{Frame, Program, StepInput, WidgetEvent, fixtures};
use serde_json::{Value, json};

const SMALL: &str = "function init(data) { return { n: data.rows.length } }\nfunction view(state, data) { return ui.text(String(state.n)) }";

fn first(v: &Value, ty: &str) -> Option<String> {
    match v {
        Value::Object(m) => {
            if m.get("type").and_then(Value::as_str) == Some(ty)
                && let Some(id) = m.get("id").and_then(Value::as_str)
            {
                return Some(id.to_owned());
            }
            m.values().find_map(|c| first(c, ty))
        }
        Value::Array(a) => a.iter().find_map(|c| first(c, ty)),
        _ => None,
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn time(n: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..3 {
        f();
    }
    median((0..n).map(|_| {
        let t = Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1e6
    }).collect())
}

fn main() {
    let mut data = fixtures::example("table").expect("table example");
    let rows = data["rows"].as_array().cloned().unwrap_or_default();
    data["rows"] = Value::Array((0..200).map(|i| {
        let mut r = rows[i % rows.len()].clone();
        r["cells"][0] = json!(format!("GET /endpoint/{i}"));
        r
    }).collect());
    println!("data: {} bytes of JSON", serde_json::to_string(&data).map_or(0, |s| s.len()));
    let program = Program::compile_with(&fixtures::program("table").expect("table"), 250).expect("compiles");
    let prepared = opencode_widget::runner::Data::new(&data);

    let out = program.step_with(&prepared, &opencode_widget::Step::Init).expect("init");
    let view = serde_json::to_value(&out.view).unwrap_or_default();
    let press = first(&view, "button").expect("a button");
    let canvas = first(&view, "canvas").expect("a canvas");
    let state = out.state.clone();
    let event = WidgetEvent::press(press);
    let frame = Frame { id: Some(canvas), t: 0.0, dt: 0.0, width: 80.0, height: 20.0, scale: 2.0, pointer: None, dark: true, fullscreen: false };

    let n = 40;
    let init_before = time(n, || drop(program.step(&StepInput::Init { data: data.clone() }).expect("init")));
    let init_after = time(n, || drop(program.step_with(&prepared, &opencode_widget::Step::Init).expect("init")));
    let ev = || opencode_widget::Step::Event { state: state.clone(), event: event.clone() };
    let event_before = time(n, || drop(program.step(&StepInput::Event { data: data.clone(), state: state.clone(), event: event.clone() }).expect("event")));
    let event_after = time(n, || drop(program.step_with(&prepared, &ev()).expect("event")));
    let draw_before = time(n * 5, || drop(program.draw(&data, &state, &frame).expect("draw")));
    let draw_after = time(n * 5, || drop(program.draw_with(&prepared, &state, &frame).expect("draw")));
    // The same 200 rows to a widget template that barely reads them: what's left is the input's cost.
    let small = Program::compile_with(&format!("{}\n{SMALL}", fixtures::PRELUDE), 250).expect("compiles");
    let s_state = small.step_with(&prepared, &opencode_widget::Step::Init).expect("init").state;
    let s_event = WidgetEvent::press("none");
    let small_init_before = time(n * 10, || drop(small.step(&StepInput::Init { data: data.clone() }).expect("init")));
    let small_init_after = time(n * 10, || drop(small.step_with(&prepared, &opencode_widget::Step::Init).expect("init")));
    let small_event_before = time(n * 10, || drop(small.step(&StepInput::Event { data: data.clone(), state: s_state.clone(), event: s_event.clone() }).expect("event")));
    let small_event_after = time(n * 10, || drop(small.step_with(&prepared, &opencode_widget::Step::Event { state: s_state.clone(), event: s_event.clone() }).expect("event")));
    println!("median µs          before    after");
    println!("table init      {init_before:9.0} {init_after:8.0}");
    println!("table press     {event_before:9.0} {event_after:8.0}");
    println!("table draw      {draw_before:9.0} {draw_after:8.0}");
    println!("small init      {small_init_before:9.0} {small_init_after:8.0}");
    println!("small press     {small_event_before:9.0} {small_event_after:8.0}");
}
