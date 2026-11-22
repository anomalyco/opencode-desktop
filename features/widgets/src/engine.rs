//! The runners where they run: the Rust PsychoScript natively, steps here and live programs on
//! threads of their own; in a page none, so every widget there says it needs the desktop app.

use opencode_widget::{Step, StepOutput};
use serde_json::Value;

#[cfg(not(target_arch = "wasm32"))]
pub type Program = opencode_widget::Program;
#[cfg(not(target_arch = "wasm32"))]
pub type LiveProgram = opencode_widget::live::LiveProgram;
#[cfg(not(target_arch = "wasm32"))]
pub use opencode_widget::live::{Ended, Grants, Live, Post, Setup};
/// A widget's `data`, converted once for its program's steps and draws.
#[cfg(not(target_arch = "wasm32"))]
pub type Data = opencode_widget::runner::Data;

/// A widget template version's program, compiled for how it runs.
#[derive(Debug)]
pub struct Compiled {
    pub kind: Kind,
    /// Lines of the program before the widget template's source (the prelude's): a location in the
    /// program less these is one in the source, as `report` gives it.
    pub offset: u32,
}

impl Compiled {
    /// Built for a protocol this client doesn't run: it never runs, so it offers no Try again or
    /// Restart (`widgets.md`, Protocol).
    pub fn refused(&self) -> bool {
        match &self.kind {
            Kind::Step(p) => p.refused(),
            Kind::Live { program, .. } => program.refused(),
        }
    }
}

#[derive(Debug)]
pub enum Kind {
    Step(Box<Program>),
    /// A live program, with the capabilities granted to it.
    Live { program: std::sync::Arc<LiveProgram>, grants: Grants },
}

/// Why a step or a draw failed, and where in the program when known.
#[derive(Clone, Debug)]
pub struct Failure {
    pub message: String,
    pub location: Option<(u32, u32)>,
    /// It returned something that isn't a view.
    pub view: bool,
    /// The widget template's own error (reported); otherwise this client's (logged in dev mode only).
    pub program: bool,
}

/// Why a widget template's program isn't here: its own code didn't compile (a program error, reported),
/// or this client couldn't have it (the `program` fetch failed, no runner here: never reported).
#[derive(Clone, Debug)]
pub enum LoadError {
    Program(String),
    Client(String),
}

impl LoadError {
    pub fn message(&self) -> &str {
        match self {
            LoadError::Program(m) | LoadError::Client(m) => m,
        }
    }
}

/// Lines of `program` before `source`, its tail.
pub fn offset(program: &str, source: &str) -> u32 {
    let head = program.len().saturating_sub(source.len());
    program.get(..head).map_or(0, |h| h.matches('\n').count() as u32)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn compile(program: &str) -> Result<Program, String> {
    Program::compile(program).map_err(|e| e.to_string())
}

/// `program` compiled for `mode` (`"step"` or `"live"`), a live one granted `grants`; `offset`
/// lines of it come before the widget template's source.
#[cfg(not(target_arch = "wasm32"))]
pub fn compile_as(program: &str, mode: &str, grants: Grants, offset: u32) -> Result<Compiled, LoadError> {
    let kind = if mode == "live" {
        let program = LiveProgram::compile(program).map_err(|e| LoadError::Program(e.to_string()))?;
        Kind::Live { program: std::sync::Arc::new(program), grants }
    } else {
        Kind::Step(Box::new(compile(program).map_err(LoadError::Program)?))
    };
    Ok(Compiled { kind, offset })
}

#[cfg(not(target_arch = "wasm32"))]
fn failure(e: opencode_widget::Error) -> Failure {
    let location = match &e {
        opencode_widget::Error::Script { location, .. } => *location,
        _ => None,
    };
    // Only an input this client built wrong, or a program for a newer protocol, is its own error;
    // the rest is the program's.
    let program = !matches!(e, opencode_widget::Error::Input(_) | opencode_widget::Error::Newer);
    Failure { view: matches!(e, opencode_widget::Error::Output(_)), message: e.to_string(), location, program }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn step(program: &Program, data: &Data, step: &Step) -> Result<StepOutput, Failure> {
    program.step_with(data, step).map_err(failure)
}

/// A canvas's frame: `draw(state, data, frame)` under the draw budget.
#[cfg(not(target_arch = "wasm32"))]
pub fn draw(program: &Program, data: &Data, state: &Value, frame: &opencode_widget::Frame) -> Result<Vec<opencode_widget::Command>, Failure> {
    program.draw_with(data, state, frame).map_err(failure)
}

#[cfg(target_arch = "wasm32")]
mod page {
    use super::*;

    #[derive(Debug)]
    pub struct Program;
    #[derive(Debug)]
    pub struct LiveProgram;
    #[derive(Clone, Debug, Default)]
    pub struct Grants {
        pub https: Option<Vec<String>>,
    }

    impl Grants {
        pub fn declared(_capabilities: &Value) -> Grants {
            Grants::default()
        }
    }

    /// A live program never runs in a page.
    pub struct Live;

    #[derive(Clone, Debug)]
    pub struct Data;

    impl Data {
        pub fn new(_data: &Value) -> Data {
            Data
        }
    }

    pub fn compile(_program: &str) -> Result<Program, String> {
        Err(opencode_i18n::t!("rune.widget.unsupported").to_owned())
    }

    pub fn compile_as(_program: &str, _mode: &str, _grants: Grants, _offset: u32) -> Result<Compiled, LoadError> {
        Err(LoadError::Client(opencode_i18n::t!("rune.widget.unsupported").to_owned()))
    }

    fn unsupported() -> Failure {
        Failure { message: opencode_i18n::t!("rune.widget.unsupported").to_owned(), location: None, view: false, program: false }
    }

    pub fn step(_program: &Program, _data: &Data, _step: &Step) -> Result<StepOutput, Failure> {
        Err(unsupported())
    }

    pub fn draw(_program: &Program, _data: &Data, _state: &Value, _frame: &opencode_widget::Frame) -> Result<Vec<opencode_widget::Command>, Failure> {
        Err(unsupported())
    }
}

#[cfg(target_arch = "wasm32")]
pub use page::*;
