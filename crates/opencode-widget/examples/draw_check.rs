//! Checks draw commands against the shared fixtures (`fixtures/draw.json`, opencode
//! `packages/widget/test/fixtures/draw.json`; `widgets.md`, Canvas, Draw command checks): each case
//! gives its normalised commands or fails with exactly its error, four ways: the checks on its JSON
//! reading and on the value as it crossed, a draw step, and a live `widget.draw` (which ends the run
//! on a failure). The normalised commands are compared as Rune holds them: the fixture's `valid`
//! read by Rune equals what the case gives. The repo has no automated tests (AGENTS.md), so this
//! is a program to run by hand: `cargo run --release -p opencode-widget --example draw_check`
//! (exits non-zero on a failure).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use opencode_widget::live::{Ended, Grants, Live, LiveProgram, Post, Setup};
use opencode_widget::{Command, DrawOutput, Frame, Program, fixtures};
use psychoscript::HostValue;
use serde_json::{Value, json};

/// The fixture's stand-ins for what JSON can't hold.
fn special(s: &str) -> Option<HostValue> {
    Some(match s {
        "$NaN" => HostValue::Number(f64::NAN),
        "$Infinity" => HostValue::Number(f64::INFINITY),
        "$-Infinity" => HostValue::Number(f64::NEG_INFINITY),
        "$undefined" => HostValue::Undefined,
        _ => return None,
    })
}

/// A case's commands as they crossed to the host.
fn crossed(v: HostValue) -> HostValue {
    match v {
        HostValue::String(s) => s.as_str().and_then(special).unwrap_or(HostValue::String(s)),
        HostValue::Array(items) => HostValue::Array(items.into_iter().map(|i| i.map(crossed)).collect()),
        HostValue::Object(fields) => HostValue::Object(fields.into_iter().map(|(k, v)| (k, crossed(v))).collect()),
        v => v,
    }
}

/// A case's commands as JSON reads them: non-finite numbers and undefined items null, undefined
/// fields left out.
fn json_read(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) if special(s).is_some() => (s != "$undefined").then_some(Value::Null),
        Value::Array(items) => Some(Value::Array(items.iter().map(|i| json_read(i).unwrap_or(Value::Null)).collect())),
        Value::Object(fields) => Some(Value::Object(fields.iter().filter_map(|(k, v)| Some((k.clone(), json_read(v)?))).collect())),
        v => Some(v.clone()),
    }
}

/// The case's commands, a `repeat` made into its copies.
fn commands(case: &Value) -> Value {
    match &case["repeat"] {
        Value::Null => case["commands"].clone(),
        r => Value::Array(vec![r["command"].clone(); r["count"].as_u64().unwrap() as usize]),
    }
}

/// A JavaScript expression that makes the case's commands.
fn expression(case: &Value) -> String {
    let js = |v: &Value| {
        let mut s = v.to_string();
        for t in ["NaN", "Infinity", "-Infinity", "undefined"] {
            s = s.replace(&format!("\"${t}\""), t);
        }
        s
    };
    match &case["repeat"] {
        Value::Null => js(&case["commands"]),
        r => format!("Array.from({{ length: {} }}, () => {})", r["count"], js(&r["command"])),
    }
}

fn expected(case: &Value) -> Result<Vec<Command>, String> {
    if let Some(e) = case["error"].as_str() {
        return Err(e.to_owned());
    }
    let valid = &case["valid"];
    let valid = if valid.get("repeat").is_some() { commands(valid) } else { valid.clone() };
    Ok(valid.as_array().unwrap().iter().map(|c| Command::from_value(c).expect("a valid case's normalised command reads")).collect())
}

fn step(case: &Value) -> Result<Vec<Command>, String> {
    let source = format!("{}\nfunction view() {{ return ui.canvas({{ key: \"c\", height: 1 }}) }}\nfunction draw() {{ return {} }}", fixtures::PRELUDE, expression(case));
    let frame = Frame { id: Some("0".into()), width: 10.0, height: 10.0, scale: 1.0, ..Default::default() };
    Program::compile(&source).expect("compiles").draw(&Value::Null, &json!({}), &frame).map_err(|e| e.to_string())
}

fn live(case: &Value) -> Result<Vec<Command>, String> {
    let source = fixtures::served("live", &format!("async function main() {{\n  widget.render(ui.canvas({{ key: \"c\", height: 1 }}))\n  widget.draw(\"c\", {})\n}}", expression(case)));
    let setup = Setup { grants: Grants::default(), transport: None, allow_local_network: false, notify: Arc::new(|| {}) };
    let live = Live::start(Arc::new(LiveProgram::compile(&source).expect("compiles")), json!({}), setup);
    let (end, mut drawn) = (Instant::now() + Duration::from_secs(10), Vec::new());
    loop {
        let ended = live.ended();
        for p in live.take() {
            if let Post::Draw { commands, .. } = p {
                drawn = commands;
            }
        }
        match ended {
            Some(Ended::Returned) => return Ok(drawn),
            Some(Ended::Failed(m)) => return Err(m),
            Some(other) => return Err(format!("ended {other:?}")),
            None if Instant::now() > end => return Err("never ended".into()),
            None => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn main() {
    let fixture: Value = serde_json::from_str(include_str!("../fixtures/draw.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut bad = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let want = expected(case);
        let input = commands(case);
        let ways: [(&str, Result<Vec<Command>, String>); 4] = [
            ("json", DrawOutput::from_value(&json!({ "commands": json_read(&input) })).map(|d| d.commands)),
            ("host", DrawOutput::from_host(Some(&crossed(HostValue::from_json(&input)))).map(|d| d.commands)),
            ("step", step(case)),
            ("live", live(case)),
        ];
        for (way, got) in ways {
            if got != want {
                bad += 1;
                let short = |r: &Result<Vec<Command>, String>| format!("{:?}", r.as_ref().map(|c| c.iter().map(Command::to_value).collect::<Vec<_>>())).chars().take(400).collect::<String>();
                println!("FAIL {name} ({way})\n    got  {}\n    want {}", short(&got), short(&want));
            }
        }
    }
    let errors = cases.iter().filter(|c| c["error"].is_string()).count();
    println!("{} cases ({} valid, {errors} errors), 4 ways each: {bad} differ", cases.len(), cases.len() - errors);
    if bad > 0 {
        std::process::exit(1);
    }
}
