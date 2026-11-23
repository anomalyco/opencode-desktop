//! Runs a widget template's program on the Rust PsychoScript as a native host (`widgets.md`, Native
//! hosts, protocol 2): compiled once per program (a widget template's `name@version`) as
//! `<program>\nreturn __widget_native_step(__input())`, with `__input` and `__widget_emit` the host
//! globals and functions crossing as markers, and run once per step under a 250 ms budget. The
//! prelude emits the view as built, which is made into the tree clients get here
//! ([`crate::tree`]); the run's result is the rest of the step output. A canvas's draw steps run on
//! the same compiled program under the 8 ms / 2,000-command draw budget, and emit their commands.
//! A program built for another protocol is refused without running anything.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use psychoscript::{Compiled, DiagnosticKind, HostFunction, HostValue, Limits, Runtime, Translate, TranslateFunctions};
use serde_json::Value;

use crate::draw::{Command, DRAW_TIMEOUT_MS, DrawOutput, Frame};
use crate::step::{Step, StepInput, StepOutput};

/// The contract's step budget; a client may use less.
pub const STEP_TIMEOUT_MS: u64 = 250;

/// The heap a program may hold, steps and live runs alike (`widgets-live.md`, Budgets: "capped
/// by the host, default 32 MB", as iOS and Android cap it), in PsychoScript's size model of what
/// it can still reach, not the process's memory. Over it is a program error.
pub const MAX_HEAP_BYTES: u64 = 32 * 1024 * 1024;

/// How deep a value may nest where it crosses to or from a widget program (`widgets.md`, Native
/// hosts): the deepest value of a view of 100 levels, 1 + 2 × 99 + 3, which a live render's
/// `{ type: "render", node }` reaches (PsychoScript's default is 32).
pub const MAX_VALUE_DEPTH: usize = 202;

/// What a program that went over [`MAX_HEAP_BYTES`] shows and reports, in the mobile clients' words.
pub fn heap_exceeded() -> String {
    format!("It used more than {} MB of memory", MAX_HEAP_BYTES >> 20)
}

/// What runs a step after the widget template's program.
const TAIL: &str = "\nreturn __widget_native_step(__input())";

/// The prelude<->host contract this host implements (`widgets.md`, Protocol).
pub const PROTOCOL: u64 = 2;
/// Why a program built for another contract is refused: a program error for an older one, a
/// client error for a newer one.
pub const OLDER: &str = "This widget was built with an older widget runtime; ask the agent to post it again";
pub const NEWER: &str = "This widget needs a newer version of this app";

/// The contract `program` was built for: its first line's `const __widget_protocol = N`, or 1.
pub fn protocol(program: &str) -> u64 {
    let line = program.split('\n').next().unwrap_or("");
    match line.strip_prefix("const __widget_protocol = ") {
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => n.parse().unwrap_or(u64::MAX),
        _ => 1,
    }
}

/// Why `program`'s protocol isn't run here, if it isn't.
pub(crate) fn refused(program: &str) -> Option<Error> {
    match protocol(program) {
        PROTOCOL => None,
        p if p < PROTOCOL => Some(Error::Older),
        _ => Some(Error::Newer),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The program doesn't parse or compile.
    Compile(String),
    /// The step threw; `location` is the 1-based line and column in the program, when known.
    Script { message: String, location: Option<(u32, u32)> },
    /// The step ran past its budget.
    Timeout,
    /// A draw step returned more commands than a frame may draw.
    Budget(String),
    /// The step returned something that isn't a step output (no view, an invalid view).
    Output(String),
    /// The input given to [`step_json`] isn't a step input.
    Input(String),
    /// Built for an older protocol: a program error ([`OLDER`]).
    Older,
    /// Built for a newer protocol: a client error ([`NEWER`]), never reported.
    Newer,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Compile(m) => write!(f, "the widget's program doesn't compile: {m}"),
            // The interpreter's message already says where.
            Error::Script { message, .. } => f.write_str(message),
            Error::Timeout => write!(f, "the widget took longer than {STEP_TIMEOUT_MS} ms to respond"),
            Error::Output(m) | Error::Input(m) | Error::Budget(m) => f.write_str(m),
            Error::Older => f.write_str(OLDER),
            Error::Newer => f.write_str(NEWER),
        }
    }
}

impl std::error::Error for Error {}

/// A widget's `data` as its program reads it: converted once per widget, then copied into each
/// step's and draw's input as it is, never serialized and parsed again.
#[derive(Clone)]
pub struct Data(Arc<HostValue>);

impl Data {
    pub fn new(data: &Value) -> Data {
        Data(Arc::new(HostValue::from_json(data)))
    }
}

impl fmt::Debug for Data {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Data")
    }
}

/// A step's input object (`{ phase, data, ... }`) around `data`, the rest of it from JSON.
fn input(phase: &str, data: &Data, rest: Vec<(&str, HostValue)>) -> HostValue {
    let mut fields = vec![("phase".into(), HostValue::String(phase.into())), ("data".into(), (*data.0).clone())];
    fields.extend(rest.into_iter().map(|(k, v)| (k.into(), v)));
    HostValue::Object(fields)
}

/// A compiled widget template program, ready to step. Steps on one program run one at a time.
pub struct Program {
    runtime: Runtime,
    /// The same program under the draw budget.
    drawing: Runtime,
    /// `Err`: refused for its protocol, never compiled.
    compiled: Result<Compiled, Box<Error>>,
    /// What `__input()` returns to the step running now (once: the step's tail reads it once).
    input: Arc<Mutex<HostValue>>,
    /// What the step running now emitted.
    emitted: Arc<Mutex<Emitted>>,
    /// Held across a step, so concurrent steps don't read each other's input.
    running: Mutex<()>,
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program").field("bytes", &self.compiled.as_ref().map_or(0, |c| c.code().len())).finish()
    }
}

impl Program {
    /// Compiles `program` (the server's `prelude + "\n" + source`) under the default budget.
    pub fn compile(program: &str) -> Result<Program, Error> {
        Program::compile_with(program, STEP_TIMEOUT_MS)
    }

    /// [`Program::compile`] with a step budget of `timeout_ms` (at least 1, at most the contract's).
    pub fn compile_with(program: &str, timeout_ms: u64) -> Result<Program, Error> {
        let input: Arc<Mutex<HostValue>> = Arc::new(Mutex::new(HostValue::Null));
        let emitted: Arc<Mutex<Emitted>> = Arc::default();
        let make = |timeout_ms: u64, draws: bool| {
            let (reads, out) = (input.clone(), emitted.clone());
            Runtime::make()
                .typed_arrays(true)
                .translate(Translate { functions: TranslateFunctions::Mark, ..Default::default() })
                .globals(move |_run| {
                    let (reads, out) = (reads.clone(), out.clone());
                    [
                        ("__input", HostFunction::new(move |_args, _call| std::mem::take(&mut *reads.lock().unwrap_or_else(PoisonError::into_inner)))),
                        ("__widget_emit", HostFunction::new(move |args: Vec<HostValue>, _call| emit(&out, args, draws))),
                    ]
                })
                .limits(Limits { timeout_ms: Some(timeout_ms), max_heap_bytes: Some(MAX_HEAP_BYTES), max_value_depth: Some(MAX_VALUE_DEPTH), ..Default::default() })
        };
        let runtime = make(timeout_ms.clamp(1, STEP_TIMEOUT_MS), false);
        let drawing = make(DRAW_TIMEOUT_MS.min(timeout_ms.max(1)), true);
        let compiled = match refused(program) {
            Some(e) => Err(Box::new(e)),
            None => Ok(runtime.compile(&format!("{program}{TAIL}")).map_err(|d| Error::Compile(d.message_lossy()))?),
        };
        Ok(Program { runtime, drawing, compiled, input, emitted, running: Mutex::new(()) })
    }

    /// Whether it's refused for its protocol (`widgets.md`, Protocol): no step of it runs, and no
    /// retry could.
    pub fn refused(&self) -> bool {
        self.compiled.is_err()
    }

    /// Runs one step: `init` or an event.
    pub fn step(&self, input: &StepInput) -> Result<StepOutput, Error> {
        if matches!(input, StepInput::Draw { .. }) {
            return Err(Error::Input("a draw step returns commands: run it with Program::draw".into()));
        }
        let input = serde_json::to_value(input).map_err(|e| Error::Input(e.to_string()))?;
        let (rest, view) = self.run(&self.runtime, HostValue::from_json(&input))?;
        output(rest, view)
    }

    /// Runs one step on a widget's `data` as converted once ([`Data`]): `init` or an event.
    pub fn step_with(&self, data: &Data, step: &Step) -> Result<StepOutput, Error> {
        let input = match step {
            Step::Init => input("init", data, vec![]),
            Step::Event { state, event } => {
                let event = serde_json::to_value(event).map_err(|e| Error::Input(e.to_string()))?;
                input("event", data, vec![("state", HostValue::from_json(state)), ("event", HostValue::from_json(&event))])
            }
        };
        let (rest, view) = self.run(&self.runtime, input)?;
        output(rest, view)
    }

    /// Draws a canvas's frame: the widget template's `draw(state, data, frame)`, under the 8 ms budget,
    /// its commands held to 2,000. A widget template without `draw` draws nothing.
    pub fn draw(&self, data: &Value, state: &Value, frame: &Frame) -> Result<Vec<Command>, Error> {
        let input = serde_json::json!({ "phase": "draw", "data": data, "state": state, "frame": frame });
        self.drawn(HostValue::from_json(&input))
    }

    /// [`Program::draw`] on a widget's `data` as converted once ([`Data`]).
    pub fn draw_with(&self, data: &Data, state: &Value, frame: &Frame) -> Result<Vec<Command>, Error> {
        let frame = serde_json::to_value(frame).map_err(|e| Error::Input(e.to_string()))?;
        self.drawn(input("draw", data, vec![("state", HostValue::from_json(state)), ("frame", HostValue::from_json(&frame))]))
    }

    fn drawn(&self, input: HostValue) -> Result<Vec<Command>, Error> {
        let (_, commands) = self.run(&self.drawing, input)?;
        DrawOutput::from_host(commands.as_ref()).map(|d| d.commands).map_err(|e| if e.contains("over the budget") { Error::Budget(e) } else { Error::Output(e) })
    }

    /// Runs one step on a raw input, returning its output unchecked: the run's result, with the
    /// emitted view's tree as `view`; a draw's emitted commands as `{ commands }`.
    pub fn step_value(&self, input: Value) -> Result<Value, Error> {
        if input["phase"] == "draw" {
            let (_, commands) = self.run(&self.drawing, HostValue::from_json(&input))?;
            return Ok(serde_json::json!({ "commands": commands.and_then(|c| c.to_json()) }));
        }
        let (mut rest, view) = self.run(&self.runtime, HostValue::from_json(&input))?;
        if let (Value::Object(m), Some(view)) = (&mut rest, view) {
            m.insert("view".into(), view.to_json().unwrap_or(Value::Null));
        }
        Ok(rest)
    }

    /// Runs one step: the run's result as JSON (functions left out), and the view it emitted, as
    /// the tree clients get. A view that broke Keys and depth fails it (phase `view`), before the
    /// run's own failure.
    fn run(&self, runtime: &Runtime, input: HostValue) -> Result<(Value, Option<HostValue>), Error> {
        let compiled = self.compiled.as_ref().map_err(|e| (**e).clone())?;
        let _running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        *self.input.lock().unwrap_or_else(PoisonError::into_inner) = input;
        *self.emitted.lock().unwrap_or_else(PoisonError::into_inner) = Emitted::default();
        let outcome = runtime.execute(compiled);
        *self.input.lock().unwrap_or_else(PoisonError::into_inner) = HostValue::Null;
        let Emitted { view, failure } = std::mem::take(&mut *self.emitted.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(failure) = failure {
            return Err(Error::Output(failure));
        }
        let rest = outcome.json().map_err(|d| match d.kind {
            DiagnosticKind::TimeoutExceeded => Error::Timeout,
            DiagnosticKind::MemoryLimitExceeded => Error::Script { message: heap_exceeded(), location: None },
            DiagnosticKind::ParseError | DiagnosticKind::UnsupportedSyntax if d.location.is_none() => Error::Compile(d.message_lossy()),
            _ => Error::Script { message: d.message_lossy(), location: d.location },
        })?;
        Ok((rest, view))
    }
}

/// What a step emitted: its view's tree (a draw: its commands), or why its view failed (phase
/// `view`).
#[derive(Default)]
struct Emitted {
    view: Option<HostValue>,
    failure: Option<String>,
}

/// `__widget_emit(view, failure?)`: the view as built, made into its tree; a string `failure` (a
/// view too deep to cross) or a view that breaks Keys and depth fails the step after the run. A
/// function where none may go throws, from this call. A draw step's (`draws`) emits its commands,
/// kept as they are: they're read as JSON reads them, functions left out.
fn emit(out: &Mutex<Emitted>, mut args: Vec<HostValue>, draws: bool) -> Result<HostValue, HostValue> {
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    if draws {
        out.view = Some(if args.is_empty() { HostValue::Undefined } else { args.swap_remove(0) });
        return Ok(HostValue::Undefined);
    }
    if let Some(HostValue::String(failure)) = args.get(1) {
        out.failure = Some(failure.to_string_lossy().into_owned());
        return Ok(HostValue::Undefined);
    }
    let view = if args.is_empty() { HostValue::Undefined } else { args.swap_remove(0) };
    match crate::tree::build(view, "Argument 1 to __widget_emit") {
        Ok(tree) => out.view = Some(tree.view),
        Err(crate::tree::Fail::View(m)) => out.failure = Some(m),
        Err(crate::tree::Fail::Thrown(e)) => return Err(e),
    }
    Ok(HostValue::Undefined)
}

/// A step's output from its run's result and its emitted view.
fn output(rest: Value, view: Option<HostValue>) -> Result<StepOutput, Error> {
    let view = view.ok_or_else(|| Error::Output("The step gave no view".into()))?;
    let view = crate::host_value::read::<crate::Node>(&view).map_err(|e| Error::Output(format!("invalid step output: {e}")))?;
    StepOutput::from_parts(rest, view).map_err(Error::Output)
}

/// Programs [`step_json`] compiled lately, by a hash of their text: a few widget templates are live at once.
const CACHED: usize = 8;

type Cache = Vec<(u64, Arc<str>, Arc<Program>)>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: std::sync::OnceLock<Mutex<Cache>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// `program`, compiled once and kept among the last [`CACHED`] programs used.
pub fn cached(program: &str) -> Result<Arc<Program>, Error> {
    let mut h = DefaultHasher::new();
    program.hash(&mut h);
    let hash = h.finish();
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(at) = cache.iter().position(|(k, text, _)| *k == hash && &**text == program) {
        let hit = cache.remove(at);
        let p = hit.2.clone();
        cache.push(hit);
        return Ok(p);
    }
    let p = Arc::new(Program::compile(program)?);
    if cache.len() >= CACHED {
        cache.remove(0);
    }
    cache.push((hash, program.into(), p.clone()));
    Ok(p)
}

/// JSON in, JSON out, for hosts across an FFI boundary: runs step `input_json` (a [`StepInput`])
/// of `program` and returns its [`StepOutput`] as JSON, or for a `draw` phase its
/// [`DrawOutput`] (`{ commands }`, checked and in compact form). The program compiles once and
/// stays cached while it's in use.
pub fn step_json(program: &str, input_json: &str) -> Result<String, Error> {
    let input: StepInput = serde_json::from_str(input_json).map_err(|e| Error::Input(format!("invalid step input: {e}")))?;
    let program = cached(program)?;
    let out = match &input {
        StepInput::Draw { data, state, frame } => serde_json::to_string(&DrawOutput { commands: program.draw(data, state, frame)? }),
        _ => serde_json::to_string(&program.step(&input)?),
    };
    out.map_err(|e| Error::Output(e.to_string()))
}
