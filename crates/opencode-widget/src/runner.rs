//! Runs a template's program on the Rust PsychoScript: compiled once per program (a template's
//! `name@version`) as `<program>\nreturn __widget_step(__input())`, with `__input` the only host
//! global, and run once per step under a 250 ms budget; a canvas's draw steps run on the same
//! compiled program under the 8 ms / 2,000-command draw budget.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};

use psychoscript::{Compiled, DiagnosticKind, HostFunction, HostValue, Limits, Runtime};
use serde_json::Value;

use crate::draw::{Command, DRAW_TIMEOUT_MS, DrawOutput, Frame};
use crate::step::{Step, StepInput, StepOutput};

/// The contract's step budget; a client may use less.
pub const STEP_TIMEOUT_MS: u64 = 250;

/// What runs a step after the template's program.
const TAIL: &str = "\nreturn __widget_step(__input())";

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
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Compile(m) => write!(f, "the widget's program doesn't compile: {m}"),
            // The interpreter's message already says where.
            Error::Script { message, .. } => f.write_str(message),
            Error::Timeout => write!(f, "the widget took longer than {STEP_TIMEOUT_MS} ms to respond"),
            Error::Output(m) | Error::Input(m) | Error::Budget(m) => f.write_str(m),
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

/// A compiled template program, ready to step. Steps on one program run one at a time.
pub struct Program {
    runtime: Runtime,
    /// The same program under the draw budget.
    drawing: Runtime,
    compiled: Compiled,
    /// What `__input()` returns to the step running now (once: the step's tail reads it once).
    input: Arc<Mutex<HostValue>>,
    /// Held across a step, so concurrent steps don't read each other's input.
    running: Mutex<()>,
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Program").field("bytes", &self.compiled.code().len()).finish()
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
        let make = |timeout_ms: u64| {
            let reads = input.clone();
            Runtime::make()
                .globals(move |_run| {
                    let reads = reads.clone();
                    [("__input", HostFunction::new(move |_args, _call| std::mem::take(&mut *reads.lock().unwrap_or_else(PoisonError::into_inner))))]
                })
                .limits(Limits { timeout_ms: Some(timeout_ms), ..Default::default() })
        };
        let runtime = make(timeout_ms.clamp(1, STEP_TIMEOUT_MS));
        let drawing = make(DRAW_TIMEOUT_MS.min(timeout_ms.max(1)));
        let code = format!("{program}{TAIL}");
        let compiled = runtime.compile(&code).map_err(|d| Error::Compile(d.message_lossy()))?;
        Ok(Program { runtime, drawing, compiled, input, running: Mutex::new(()) })
    }

    /// Runs one step: `init` or an event.
    pub fn step(&self, input: &StepInput) -> Result<StepOutput, Error> {
        if matches!(input, StepInput::Draw { .. }) {
            return Err(Error::Input("a draw step returns commands: run it with Program::draw".into()));
        }
        let input = serde_json::to_value(input).map_err(|e| Error::Input(e.to_string()))?;
        StepOutput::from_value(self.step_value(input)?).map_err(Error::Output)
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
        StepOutput::from_value(self.run(&self.runtime, input)?).map_err(Error::Output)
    }

    /// Draws a canvas's frame: the template's `draw(state, data, frame)`, under the 8 ms budget,
    /// its commands held to 2,000. A template without `draw` draws nothing.
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
        let out = self.run(&self.drawing, input)?;
        DrawOutput::from_value(&out).map(|d| d.commands).map_err(|e| if e.contains("over the budget") { Error::Budget(e) } else { Error::Output(e) })
    }

    /// Runs one step on a raw input, returning the program's raw value unchecked.
    pub fn step_value(&self, input: Value) -> Result<Value, Error> {
        self.run(&self.runtime, HostValue::from_json(&input))
    }

    fn run(&self, runtime: &Runtime, input: HostValue) -> Result<Value, Error> {
        let _running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
        *self.input.lock().unwrap_or_else(PoisonError::into_inner) = input;
        let outcome = runtime.execute(&self.compiled);
        *self.input.lock().unwrap_or_else(PoisonError::into_inner) = HostValue::Null;
        outcome.json().map_err(|d| match d.kind {
            DiagnosticKind::TimeoutExceeded => Error::Timeout,
            DiagnosticKind::ParseError | DiagnosticKind::UnsupportedSyntax if d.location.is_none() => Error::Compile(d.message_lossy()),
            _ => Error::Script { message: d.message_lossy(), location: d.location },
        })
    }
}

/// Programs [`step_json`] compiled lately, by a hash of their text: a few templates are live at once.
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
