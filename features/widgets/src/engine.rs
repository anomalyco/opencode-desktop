//! The runners where they run: the Rust PsychoScript natively, steps here and live programs on
//! threads of their own; in a page none, so every widget there says it needs the desktop app.

use opencode_widget::{StepInput, StepOutput};
use serde_json::Value;

#[cfg(not(target_arch = "wasm32"))]
pub type Program = opencode_widget::Program;
#[cfg(not(target_arch = "wasm32"))]
pub type LiveProgram = opencode_widget::live::LiveProgram;
#[cfg(not(target_arch = "wasm32"))]
pub use opencode_widget::live::{Ended, Grants, Live, Post, Setup};

/// A template version's program, compiled for how it runs.
#[derive(Debug)]
pub struct Compiled {
    pub kind: Kind,
    /// Lines of the program before the template's source (the prelude's): a location in the
    /// program less these is one in the source, as `report` gives it.
    pub offset: u32,
}

#[derive(Debug)]
pub enum Kind {
    Step(Program),
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
/// lines of it come before the template's source.
#[cfg(not(target_arch = "wasm32"))]
pub fn compile_as(program: &str, mode: &str, grants: Grants, offset: u32) -> Result<Compiled, String> {
    let kind = if mode == "live" {
        let program = LiveProgram::compile(program).map_err(|e| e.to_string())?;
        Kind::Live { program: std::sync::Arc::new(program), grants }
    } else {
        Kind::Step(compile(program)?)
    };
    Ok(Compiled { kind, offset })
}

#[cfg(not(target_arch = "wasm32"))]
fn failure(e: opencode_widget::Error) -> Failure {
    let location = match &e {
        opencode_widget::Error::Script { location, .. } => *location,
        _ => None,
    };
    Failure { view: matches!(e, opencode_widget::Error::Output(_)), message: e.to_string(), location }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn step(program: &Program, input: &StepInput) -> Result<StepOutput, Failure> {
    program.step(input).map_err(failure)
}

/// A canvas's frame: `draw(state, data, frame)` under the draw budget.
#[cfg(not(target_arch = "wasm32"))]
pub fn draw(program: &Program, data: &Value, state: &Value, frame: &opencode_widget::Frame) -> Result<Vec<opencode_widget::Command>, Failure> {
    program.draw(data, state, frame).map_err(failure)
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

    pub fn compile(_program: &str) -> Result<Program, String> {
        Err(opencode_i18n::t!("rune.widget.unsupported").to_owned())
    }

    pub fn compile_as(_program: &str, _mode: &str, _grants: Grants, _offset: u32) -> Result<Compiled, String> {
        Err(opencode_i18n::t!("rune.widget.unsupported").to_owned())
    }

    fn unsupported() -> Failure {
        Failure { message: opencode_i18n::t!("rune.widget.unsupported").to_owned(), location: None, view: false }
    }

    pub fn step(_program: &Program, _input: &StepInput) -> Result<StepOutput, Failure> {
        Err(unsupported())
    }

    pub fn draw(_program: &Program, _data: &Value, _state: &Value, _frame: &opencode_widget::Frame) -> Result<Vec<opencode_widget::Command>, Failure> {
        Err(unsupported())
    }
}

#[cfg(target_arch = "wasm32")]
pub use page::*;
