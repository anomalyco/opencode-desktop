//! Checks Rune as a native host (`widgets.md`, Native hosts, protocol 2) against the wire
//! conformance fixtures (`fixtures/wire/*.json`, copied from opencode `packages/widget/test/fixtures/wire`):
//! - the real step (`Program`, as Rune runs it) on each fixture's `prelude + source` and `input`
//!   gives its `view` as the same JSON text and its `result`, or exactly its `error` (`view: true`:
//!   a program error in phase `view`; `view: false`: the step's run failure);
//! - the walk alone (`tree::build`) on its `built` view gives the same view or failure;
//! - the walk on the view a real native step emits gives its `handlers` and `canvases`.
//!
//! A function error from the real step must carry the call's ` (line N, col M)`; the walk alone,
//! with no call, gives the same message without it. Any other difference in location is listed
//! as a gap, not a pass.
//! The repo has no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run -p opencode-widget --example native_tree_check` (exits non-zero on a difference).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use opencode_widget::runner::Error;
use opencode_widget::tree::{self, Fail};
use opencode_widget::{Program, fixtures};
use psychoscript::{HostFunction, HostValue, Runtime, Translate, TranslateFunctions};
use serde_json::Value;

/// What `code` hands `__widget_emit` (its first argument), `__input()` returning `input`,
/// functions marked.
fn emitted(code: &str, input: HostValue) -> Result<HostValue, String> {
    let out: Arc<Mutex<Option<HostValue>>> = Arc::default();
    let seen = out.clone();
    let limits = psychoscript::Limits { max_value_depth: Some(opencode_widget::runner::MAX_VALUE_DEPTH), ..Default::default() };
    let runtime = Runtime::make().limits(limits).translate(Translate { functions: TranslateFunctions::Mark, ..Default::default() }).globals(move |_run| {
        let (seen, input) = (seen.clone(), input.clone());
        [
            ("__input", HostFunction::new(move |_args: Vec<HostValue>, _call| Ok(input.clone()))),
            ("__widget_emit", HostFunction::new(move |args: Vec<HostValue>, _call| {
                let mut seen = seen.lock().unwrap();
                if seen.is_none() {
                    *seen = args.into_iter().next();
                }
                Ok(HostValue::Undefined)
            })),
        ]
    });
    runtime.execute(code).json().map_err(|d| d.message_lossy())?;
    Ok(out.lock().unwrap().take().unwrap_or_default())
}

/// `built` from the fixture's text, as the VM's `JSON.parse` reads its numbers, with each
/// `"$function"` the marker a function crosses as and `"$NaN"`, `"$Infinity"`, `"$-Infinity"` those
/// numbers.
fn built(text: &str) -> HostValue {
    fn mark(v: HostValue) -> HostValue {
        match v {
            HostValue::String(s) => match s.as_str() {
                Some("$function") => HostValue::FunctionMarker,
                Some("$NaN") => HostValue::Number(f64::NAN),
                Some("$Infinity") => HostValue::Number(f64::INFINITY),
                Some("$-Infinity") => HostValue::Number(f64::NEG_INFINITY),
                _ => HostValue::String(s),
            },
            HostValue::Array(items) => HostValue::Array(items.into_iter().map(|i| i.map(mark)).collect()),
            HostValue::Object(fields) => HostValue::Object(fields.into_iter().map(|(k, v)| (k, mark(v))).collect()),
            v => v,
        }
    }
    mark(emitted("__widget_emit(JSON.parse(__input()).built)", HostValue::String(text.into())).expect("parses"))
}

/// JSON values alike, numbers by value (the fixtures' numbers are JavaScript's).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| same(a, b)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().zip(y).all(|((ka, a), (kb, b))| ka == kb && same(a, b)),
        _ => a == b,
    }
}

/// Whether `got` is `want` but for the ` (line N, col M)` `want` ends with.
fn but_location(got: &str, want: &str) -> bool {
    want.rsplit_once(" (line ").is_some_and(|(head, tail)| head == got && tail.ends_with(')') && tail.contains(", col "))
}

enum Verdict {
    Ok,
    Gap(String),
    Fail(String),
}

/// The expected error against the one we got.
fn error(want: &Value, view: bool, got: &str) -> Verdict {
    let message = want["message"].as_str().unwrap();
    match (want["view"].as_bool() == Some(view), got == message) {
        (true, true) => Verdict::Ok,
        (true, false) if but_location(got, message) => Verdict::Gap(format!("{got:?}\n    want {message:?}")),
        _ => Verdict::Fail(format!("error (view: {view}) {got:?}\n    want (view: {}) {message:?}", want["view"])),
    }
}

/// The real step, as Rune runs it.
fn step(f: &Value) -> Verdict {
    let program = Program::compile(&format!("{}\n{}", fixtures::PRELUDE, f["source"].as_str().unwrap())).expect("compiles");
    match (program.step_value(f["input"].clone()), f.get("error")) {
        (Err(Error::Output(m)), Some(want)) => error(want, true, &m),
        (Err(e), Some(want)) => error(want, false, &e.to_string()),
        (Err(e), None) => Verdict::Fail(format!("failed: {e:?}")),
        (Ok(v), Some(want)) => Verdict::Fail(format!("ran: {v}\n    want error {want}")),
        (Ok(mut v), None) => {
            let view = v.as_object_mut().and_then(|m| m.shift_remove("view")).unwrap_or_default();
            let (got, want) = (serde_json::to_string(&view).unwrap(), serde_json::to_string(&f["view"]).unwrap());
            if got != want {
                return Verdict::Fail(format!("view\n    got  {got}\n    want {want}"));
            }
            if !same(&v, &f["result"]) {
                return Verdict::Fail(format!("result\n    got  {v}\n    want {}", f["result"]));
            }
            // And the step reads as the fixture's view reads (a bare string child fails both).
            let read = program.step(&serde_json::from_value(f["input"].clone()).unwrap()).map(|o| o.view).map_err(|e| e.to_string());
            let want = opencode_widget::Node::from_value(&f["view"]).map_err(|e| format!("invalid step output: {}", e.trim_start_matches("invalid view: ")));
            if read != want {
                return Verdict::Fail(format!("step output\n    got  {read:?}\n    want {want:?}"));
            }
            Verdict::Ok
        }
    }
}

/// The walk alone, on `view` (the fixture's `built`, or a real step's emitted view).
fn walk(f: &Value, view: HostValue, tables: bool) -> Verdict {
    // A failure of the run's result (a typed array in the state) isn't the view's.
    let expected = f.get("error").filter(|e| e["view"] == true || e["message"].as_str().is_some_and(|m| m.contains("contains a function")));
    if expected.is_none() && f.get("error").is_some() {
        return match tree::build(view, "Argument 1 to __widget_emit") {
            Ok(_) => Verdict::Ok,
            Err(e) => Verdict::Fail(format!("failed: {e:?}")),
        };
    }
    match (tree::build(view, "Argument 1 to __widget_emit"), expected) {
        (Err(Fail::View(m)), Some(want)) => error(want, true, &m),
        // The walk alone has no call to locate its error at: the message without ` (line N, col M)`.
        (Err(Fail::Thrown(HostValue::Located(e))), Some(want)) => match error(want, false, &format!("{}: {}", e.name.to_string_lossy(), e.message.to_string_lossy())) {
            Verdict::Gap(_) => Verdict::Ok,
            v => v,
        },
        (Err(e), _) => Verdict::Fail(format!("failed: {e:?}")),
        (Ok(t), Some(want)) => Verdict::Fail(format!("built: {:?}\n    want error {want}", t.view.to_json())),
        (Ok(t), None) => {
            let (got, want) = (serde_json::to_string(&t.view.to_json().unwrap_or_default()).unwrap(), serde_json::to_string(&f["view"]).unwrap());
            if got != want {
                return Verdict::Fail(format!("view\n    got  {got}\n    want {want}"));
            }
            if tables {
                let handlers: BTreeMap<String, String> = t.handlers.iter().map(|(k, v)| (k.clone(), (*v).to_owned())).collect();
                let want: BTreeMap<String, String> = serde_json::from_value(f["handlers"].clone()).unwrap();
                if handlers != want {
                    return Verdict::Fail(format!("handlers\n    got  {handlers:?}\n    want {want:?}"));
                }
                let canvases: BTreeMap<String, String> = t.canvases.iter().map(|(k, (id, _))| (k.clone(), id.clone())).collect();
                let want: BTreeMap<String, String> = serde_json::from_value(f["canvases"].clone()).unwrap();
                if canvases != want {
                    return Verdict::Fail(format!("canvases\n    got  {canvases:?}\n    want {want:?}"));
                }
            }
            Verdict::Ok
        }
    }
}

/// Keys `encodeURIComponent` can't encode, which the fixtures don't hold: the step fails with the
/// prelude's `URIError`, not as a view.
const SURROGATES: &[&str] = &[r#"ui.column([ui.button("bad", { key: "\uD800" })])"#, r#"ui.column({ key: "\uD800" }, [])"#];

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/wire");
    let mut files: Vec<_> = std::fs::read_dir(dir).expect("reads the fixtures").map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    let (mut bad, mut gaps) = (0, 0);
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        // Views of 100 levels nest deeper than serde_json reads by default.
        let mut de = serde_json::Deserializer::from_str(&text);
        de.disable_recursion_limit();
        let f = <Value as serde::Deserialize>::deserialize(&mut de).unwrap();
        let name = f["name"].as_str().unwrap_or("?").to_owned();
        let source = format!("{}\n{}\n__widget_native_step(__input())", fixtures::PRELUDE, f["source"].as_str().unwrap());
        let real = emitted(&source, HostValue::from_json(&f["input"]));
        let mut checks = vec![("step", step(&f))];
        // A view too deep to cross has no `built`: the prelude emits its failure instead.
        if f.get("built").is_some() {
            checks.push(("built", walk(&f, built(&text), true)));
        }
        if f.get("error").is_none() {
            checks.push(("emitted", walk(&f, real.expect("the step runs"), true)));
        }
        for (how, verdict) in checks {
            match verdict {
                Verdict::Ok => {}
                Verdict::Gap(d) => {
                    gaps += 1;
                    println!("GAP  {name} ({how}): no location\n    got  {d}");
                }
                Verdict::Fail(d) => {
                    bad += 1;
                    println!("FAIL {name} ({how}): {d}");
                }
            }
        }
    }
    for expr in SURROGATES {
        let program = Program::compile(&format!("{}\nfunction view(state, data) {{ return {expr} }}", fixtures::PRELUDE)).expect("compiles");
        match program.step_value(serde_json::json!({ "phase": "init", "data": null })) {
            Err(Error::Script { message, .. }) if message.starts_with("URIError: encodeURIComponent received malformed URI data") => println!("ok   {expr}: {message}"),
            other => {
                bad += 1;
                println!("FAIL {expr}: {other:?}");
            }
        }
    }
    // A draw step emits its commands, read as JSON reads them: a function in a paint is left out,
    // a non-finite coordinate fails as an invalid draw.
    let draw = |commands: &str| {
        let program = Program::compile(&format!("{}\nfunction view() {{ return ui.canvas({{ key: \"c\", height: 10 }}) }}\nfunction draw() {{ return {commands} }}", fixtures::PRELUDE)).expect("compiles");
        let frame = opencode_widget::Frame { id: Some("0".into()), width: 10.0, height: 10.0, scale: 2.0, ..Default::default() };
        program.draw(&Value::Null, &Value::Null, &frame).map_err(|e| e.to_string())
    };
    for (commands, want) in [
        (r##"[["rect", 0, 0, 4, 4, { fill: "#fff", f: () => 1 }]]"##, Ok(r##"[["rect",0.0,0.0,4.0,4.0,{"fill":"#ffffffff"}]]"##.to_owned())),
        (r##"[["rect", 0, 0, Infinity, 4, { fill: "#fff" }]]"##, Err(r#"draw command 0: "rect" argument 3 must be a finite number"#.to_owned())),
        ("undefined", Ok("[]".to_owned())),
    ] {
        let got = draw(commands).map(|c| serde_json::to_string(&c.iter().map(opencode_widget::Command::to_value).collect::<Vec<_>>()).unwrap());
        if got == want {
            println!("ok   draw {commands}: {got:?}");
        } else {
            bad += 1;
            println!("FAIL draw {commands}\n    got  {got:?}\n    want {want:?}");
        }
    }
    println!("{} fixtures: {} differ, {gaps} differ only by the location of a function error", files.len(), bad);
    if bad > 0 {
        std::process::exit(1);
    }
}
