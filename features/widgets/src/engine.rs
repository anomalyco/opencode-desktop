//! The step runner where it runs: the Rust PsychoScript natively; in a page none, so every
//! widget there says it needs the desktop app.

use opencode_widget::{StepInput, StepOutput};

#[cfg(not(target_arch = "wasm32"))]
pub type Program = opencode_widget::Program;

#[cfg(not(target_arch = "wasm32"))]
pub fn compile(program: &str) -> Result<Program, String> {
    Program::compile(program).map_err(|e| e.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn step(program: &Program, input: &StepInput) -> Result<StepOutput, String> {
    program.step(input).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug)]
pub struct Program;

#[cfg(target_arch = "wasm32")]
pub fn compile(_program: &str) -> Result<Program, String> {
    Err(opencode_i18n::t!("rune.widget.unsupported").to_owned())
}

#[cfg(target_arch = "wasm32")]
pub fn step(_program: &Program, _input: &StepInput) -> Result<StepOutput, String> {
    Err(opencode_i18n::t!("rune.widget.unsupported").to_owned())
}
