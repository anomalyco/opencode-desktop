//! Runs the shared GPU fixtures (`fixtures/gpu`, opencode `packages/widget/test/fixtures/gpu`;
//! `widgets-gpu.md`) as their driver does, on a real GPU: each fixture's program as a live run with
//! `gpu: { available, format: "rgba8unorm" }`, its canvas sizes first, then while it asks for frames
//! (at most 10) each frame's inputs, the frame, and the inputs after it; stopped when it settles
//! without asking. Its batches (ops, bytes, whether they rode a frame request; not compared for a
//! fixture whose batches are too big to record, `"batches": "from-source"`), its GPU errors (the
//! host checks' exact messages and the canvases they stopped) and how it ended must be the
//! fixture's. Replay runs on the device too, so valid fixtures must replay without an error.
//! The repo has no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run --release -p opencode-widget --example gpu_check [name]` (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use opencode_widget::gpu;
use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Frame, WidgetEvent, fixtures};
use serde_json::{Value, json};
use widget_gpu::{Arg, Stops};

fn device() -> gpu::Device {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::PRIMARY, ..wgpu::InstanceDescriptor::new_without_display_handle() });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("an adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: widget_gpu::device_limits(), ..Default::default() })).expect("a device");
    gpu::Device { device, queue, format: wgpu::TextureFormat::Rgba8Unorm, handoff: None }
}

fn to_json(a: &Arg) -> Value {
    match a {
        Arg::Null => Value::Null,
        Arg::Bool(b) => json!(b),
        // As the fixtures write a number JSON can't hold.
        Arg::Num(n) if !n.is_finite() => json!({ "$num": if n.is_nan() { "NaN" } else if *n > 0.0 { "Infinity" } else { "-Infinity" } }),
        Arg::Num(n) => json!(n),
        Arg::Str(s) => json!(s),
        Arg::List(l) => Value::Array(l.iter().map(to_json).collect()),
        Arg::Obj(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), to_json(v))).collect()),
    }
}

/// JSON values alike, numbers by value.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, a)| y.get(k).is_some_and(|b| same(a, b))),
        _ => a == b,
    }
}

struct Run {
    batches: Vec<Value>,
    errors: Vec<Value>,
    end: String,
}

fn run(f: &Value, batches: &Arc<Mutex<Vec<Value>>>) -> Run {
    batches.lock().unwrap().clear();
    let program = Arc::new(LiveProgram::compile(&fixtures::served("live", f["source"].as_str().unwrap())).expect("compiles"));
    let setup = Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) };
    let live = Live::start(program, f.get("data").cloned().unwrap_or(Value::Null), setup);
    let mut errors = Vec::new();
    let mut wanted = false;
    let mut take = |live: &Live, wanted: &mut bool| {
        for p in live.take() {
            match p {
                Post::Frame => *wanted = true,
                Post::Gpu(e) => errors.push(json!({ "message": e.message, "canvases": match e.canvases { Stops::All => json!("all"), Stops::Canvases(k) => json!(k) } })),
                _ => {}
            }
        }
    };
    let canvas = |live: &Live, c: &Value| {
        let (w, h) = widget_gpu::clamp_size(c["width"].as_u64().unwrap() as u32, c["height"].as_u64().unwrap() as u32);
        live.canvas(c["key"].as_str().unwrap(), w, h);
    };
    for c in f["canvases"].as_array().unwrap() {
        canvas(&live, c);
    }
    // Until it asks for a frame or ends: at most 10 looks, 50 ms apart.
    let settle = |live: &Live, wanted: &mut bool, take: &mut dyn FnMut(&Live, &mut bool)| {
        // Long enough for a cold pipeline compile on a real GPU.
        let end = Instant::now() + Duration::from_millis(3000);
        while Instant::now() < end && !*wanted && live.ended().is_none() {
            take(live, wanted);
            std::thread::sleep(Duration::from_millis(1));
        }
        take(live, wanted);
    };
    let inputs = f.get("inputs").and_then(Value::as_array).cloned().unwrap_or_default();
    for i in 0..10u64 {
        settle(&live, &mut wanted, &mut take);
        if live.ended().is_some() || !wanted {
            break;
        }
        let send = |after: bool| {
            for entry in inputs.iter().filter(|e| e["frame"].as_u64() == Some(i) && e["after"].as_bool().unwrap_or(false) == after) {
                let input = &entry["input"];
                match input["type"].as_str() {
                    Some("event") => live.event(&serde_json::from_value::<WidgetEvent>(input["event"].clone()).expect("an event")),
                    Some("canvas") => canvas(&live, input),
                    Some("gpu") => gpu::lose(gpu::generation().expect("a device"), input["lost"].as_str().unwrap_or("unknown"), None),
                    _ => {}
                }
            }
        };
        send(false);
        wanted = false;
        live.frame(&Frame { t: i as f64 / 60.0, dt: 1.0 / 60.0, width: 300.0, height: 200.0, scale: 2.0, ..Default::default() });
        send(true);
    }
    settle(&live, &mut wanted, &mut take);
    let end = match live.ended() {
        Some(Ended::Returned) => "return".to_owned(),
        Some(Ended::Failed(m)) => format!("error: {m}"),
        Some(other) => format!("{other:?}"),
        None => {
            live.stop();
            "stop".to_owned()
        }
    };
    take(&live, &mut wanted);
    Run { batches: batches.lock().unwrap().clone(), errors, end }
}

fn main() {
    let only = std::env::args().nth(1);
    let d = device();
    println!("device: {} ({:?})", d.device.adapter_info().name, d.device.adapter_info().backend);
    gpu::provide(move || Some(d.clone()));
    let batches: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = batches.clone();
    opencode_widget::live::gpu::tap(Some(Box::new(move |ops: &Arg, bytes: Option<&[u8]>, frame: bool| {
        let bytes = bytes.map(|b| base64::engine::general_purpose::STANDARD.encode(b));
        sink.lock().unwrap().push(json!({ "frame": frame, "bytes": bytes, "ops": to_json(ops) }));
    })));
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/gpu");
    let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    let (mut bad, mut ran) = (0, 0);
    for path in files {
        let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let name = f["name"].as_str().unwrap();
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        ran += 1;
        let started = Instant::now();
        let r = run(&f, &batches);
        let mut why = Vec::new();
        let recorded = f["batches"].as_array();
        if !(recorded.is_some() || f["batches"].is_null() || f["batches"] == "from-source") {
            why.push(format!("batches {}, want a list or \"from-source\"", f["batches"]));
        }
        if let Some(want) = recorded {
            if r.batches.len() != want.len() {
                why.push(format!("{} batches, want {}", r.batches.len(), want.len()));
            }
            for (i, (got, want)) in r.batches.iter().zip(want).enumerate() {
                if !same(got, want) {
                    why.push(format!("batch {i} differs: got {} want {}", got.to_string().chars().take(300).collect::<String>(), want.to_string().chars().take(300).collect::<String>()));
                    break;
                }
            }
        }
        if !same(&json!(r.errors), &f["errors"]) {
            why.push(format!("errors {:?}, want {}", json!(r.errors).to_string(), f["errors"]));
        }
        if r.end != f["end"].as_str().unwrap() {
            why.push(format!("ended {}, want {}", r.end, f["end"]));
        }
        if why.is_empty() {
            let from = if f["batches"] == "from-source" { ", from source" } else { "" };
            println!("ok   {name} ({} batches{from}, {} errors, {}; {} ms)", r.batches.len(), r.errors.len(), r.end, started.elapsed().as_millis());
        } else {
            bad += 1;
            println!("FAIL {name}\n    {}", why.join("\n    "));
        }
    }
    println!("{ran} fixtures: {bad} differ");
    if bad > 0 {
        std::process::exit(1);
    }
}
