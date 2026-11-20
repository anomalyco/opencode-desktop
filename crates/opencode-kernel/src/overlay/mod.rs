//! Overlay stack state machine ([`OverlayStack`]) and declarative layer/key/Escape dispatcher
//! ([`OverlayDispatcher`]).

mod dispatcher;
mod resolve;
mod stack;

pub use dispatcher::{
    EscapeActionFn, EscapeBand, EscapeLayer, EscapePredicateFn, KeyBand, OverlayDismissFn,
    OverlayDispatcher, OverlayKeyFn, OverlayLayer, OverlayRegistration, OverlayRenderFn,
};
pub use stack::{OverlayEffect, OverlayEntry, OverlayEvent, OverlayKind, OverlayStack, OverlayState, step};
