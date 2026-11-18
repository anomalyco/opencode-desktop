//! Thin adapter over `opencode_runtime::machines::send_failed` for [`super::Sent`].

use rune::machine::Effects;

pub use opencode_runtime::machines::send_failed::{
    AfterCheck, Effect, Event, Found, Operation, Phase, Verdict, step_fields,
};

use super::Sent;

pub fn step(s: &mut Sent, event: Event) -> Effects<Effect> {
    step_fields(&mut s.phase, &mut s.operation, &mut s.handoffs, s.journaled, event)
}
