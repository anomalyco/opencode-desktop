//! Checks the native tree building (`opencode_widget::tree`, `widgets.md` Native hosts) against
//! the wire conformance fixtures (opencode `packages/widget/test/fixtures/wire/*.json`): each
//! fixture's `built` view (functions as `"$function"`, read as markers) and its real native step
//! (`prelude + source` on `input`, PsychoScript marking functions) must give the fixture's `view`
//! as the same JSON text, its `handlers` and its `canvases`, and read as the same `Node`. Views the
//! fixtures don't hold (odd keys and children) must build as the prelude builds them. The repo has
//! no automated tests (AGENTS.md), so this is a program to run by hand:
//! `cargo run -p opencode-widget --example native_tree_check -- <fixtures/wire dir>` (exits
//! non-zero on a difference).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::print_stdout)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use opencode_widget::{Node, fixtures, tree};
use psychoscript::{HostFunction, HostValue, Runtime, Translate, TranslateFunctions};
use serde_json::Value;

/// Views the fixtures don't hold, built by the prelude's own tree building and natively; the two
/// must give the same JSON (or both fail).
const EXTRA: &[&str] = &[
    r#"ui.row([ui.button("a", { key: NaN }), ui.button("b", { key: Infinity }), ui.button("c", { key: -Infinity }), ui.button("d", { key: true }), ui.button("e", { key: false }), ui.button("f", { key: ["x", 1, null] })])"#,
    r#"ui.column([Infinity, -Infinity, NaN, 0, -0, "", false, null, undefined, "s", 3, true])"#,
    r#"ui.column([ui.text("d", { when: new Date(0) }), new Map(), [[ui.button("deep")]]])"#,
    r#"ui.column({ key: 1e21 }, [ui.canvas({ key: "c", height: 1, wgsl: "x" }), ui.canvas({ key: "c", height: 2, onPointer: () => 1 })])"#,
    r#"ui.column([ui.button("bad", { key: "\uD800" })])"#,
    r#"ui.column({ key: "\uD800" }, [])"#,
];

/// What `code` hands `__widget_emit`, with `__input()` returning `input`, functions marked.
fn emitted(code: &str, input: HostValue) -> Result<HostValue, String> {
    let out: Arc<Mutex<Option<HostValue>>> = Arc::default();
    let seen = out.clone();
    let runtime = Runtime::make().translate(Translate { functions: TranslateFunctions::Mark, ..Default::default() }).globals(move |_run| {
        let (seen, input) = (seen.clone(), input.clone());
        [
            ("__input", HostFunction::new(move |_args: Vec<HostValue>, _call| Ok(input.clone()))),
            ("__widget_emit", HostFunction::new(move |args: Vec<HostValue>, _call| {
                *seen.lock().unwrap() = args.into_iter().next();
                Ok(HostValue::Undefined)
            })),
        ]
    });
    runtime.execute(code).json().map_err(|d| d.message_lossy())?;
    Ok(out.lock().unwrap().take().expect("the program emitted a value"))
}

/// `built` from the fixture's text, as the VM's `JSON.parse` reads its numbers, with each
/// `"$function"` the marker a function crosses as.
fn marked(text: &str) -> HostValue {
    fn mark(v: HostValue) -> HostValue {
        match v {
            HostValue::String(s) if s.as_str() == Some("$function") => HostValue::FunctionMarker,
            HostValue::Array(items) => HostValue::Array(items.into_iter().map(|i| i.map(mark)).collect()),
            HostValue::Object(fields) => HostValue::Object(fields.into_iter().map(|(k, v)| (k, mark(v))).collect()),
            v => v,
        }
    }
    mark(emitted("__widget_emit(JSON.parse(__input()).built)", HostValue::String(text.into())).expect("parses"))
}

/// The view the fixture's program builds in native mode, as `__widget_emit` hands it over.
fn native(source: &str, input: &Value) -> Result<HostValue, String> {
    let code = format!("{}\n{source}\n__widget_emit(__widget_step(__input(), true))", fixtures::PRELUDE);
    match emitted(&code, HostValue::from_json(input))? {
        HostValue::Object(fields) => Ok(fields.into_iter().find(|(k, _)| k.as_str() == Some("view")).map(|(_, v)| v).unwrap_or_default()),
        _ => Err("the step output isn't an object".into()),
    }
}

/// What differs between `tree` and the fixture `f`, if anything.
fn differences(f: &Value, tree: &tree::Tree) -> Vec<String> {
    let mut out = Vec::new();
    let view = tree.view.to_json().unwrap_or_default();
    let (got, want) = (serde_json::to_string(&view).unwrap(), serde_json::to_string(&f["view"]).unwrap());
    if got != want {
        out.push(format!("view\n    got  {got}\n    want {want}"));
    }
    let handlers: BTreeMap<String, String> = tree.handlers.iter().map(|(k, v)| (k.clone(), (*v).to_owned())).collect();
    let want: BTreeMap<String, String> = serde_json::from_value(f["handlers"].clone()).unwrap();
    if handlers != want {
        out.push(format!("handlers\n    got  {handlers:?}\n    want {want:?}"));
    }
    let canvases: BTreeMap<String, String> = tree.canvases.iter().map(|(k, (id, _))| (k.clone(), id.clone())).collect();
    let want: BTreeMap<String, String> = serde_json::from_value(f["canvases"].clone()).unwrap();
    if canvases != want {
        out.push(format!("canvases\n    got  {canvases:?}\n    want {want:?}"));
    }
    let (read, json) = (format!("{:?}", Node::from_host(&tree.view)), format!("{:?}", Node::from_value(&f["view"])));
    if read != json {
        out.push(format!("node\n    got  {read}\n    want {json}"));
    }
    out
}

/// The view of `expr` as the prelude builds it, and as built natively then walked here, as JSON.
fn both(expr: &str) -> (Result<String, String>, Result<String, String>) {
    let source = format!("function view(state, data) {{ return {expr} }}");
    let input = serde_json::json!({ "phase": "init", "data": null });
    let prelude = {
        let code = format!("{}\n{source}\nreturn __widget_step(__input())", fixtures::PRELUDE);
        let input = HostValue::from_json(&input);
        let runtime = Runtime::make().globals(move |_run| {
            let input = input.clone();
            [("__input", HostFunction::new(move |_args: Vec<HostValue>, _call| Ok(input.clone())))]
        });
        runtime.execute(code.as_str()).json().map(|v| v["view"].to_string()).map_err(|d| d.message_lossy())
    };
    let walked = native(&source, &input).and_then(tree::build).map(|t| t.view.to_json().unwrap_or_default().to_string());
    (prelude, walked)
}

fn main() {
    let dir = std::env::args().nth(1).expect("the fixtures/wire directory");
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("reads the directory").map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect();
    files.sort();
    let mut bad = 0;
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let f: Value = serde_json::from_str(&text).unwrap();
        let name = f["name"].as_str().unwrap_or("?").to_owned();
        let built = marked(&text);
        let native = native(f["source"].as_str().unwrap(), &f["input"]).expect("the native step runs");
        for (how, view) in [("built", built), ("native", native)] {
            let d = match tree::build(view) {
                Ok(t) => differences(&f, &t),
                Err(e) => vec![format!("error: {e}")],
            };
            if !d.is_empty() {
                bad += 1;
                println!("FAIL {name} ({how}):\n  {}", d.join("\n  "));
            }
        }
    }
    println!("{} fixtures, built and native: {}", files.len(), if bad == 0 { "all identical".to_owned() } else { format!("{bad} differ") });
    for expr in EXTRA {
        match both(expr) {
            (Ok(a), Ok(b)) if a == b => println!("ok   {expr}"),
            (Err(a), Err(b)) => println!("ok   both fail: {expr}\n       prelude: {a}\n       native:  {b}"),
            (a, b) => {
                bad += 1;
                println!("FAIL {expr}\n    prelude {a:?}\n    native  {b:?}");
            }
        }
    }
    if bad > 0 {
        std::process::exit(1);
    }
}
