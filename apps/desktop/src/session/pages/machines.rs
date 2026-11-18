//! Thin adapter binding `opencode_runtime::machines::pages` to the desktop [`crate::Route`].

pub use opencode_runtime::machines::pages::{
    HOLD, Histories, History, HistoryEffect, HistoryEvent, Key, LATEST, OpenEvent, Stall,
};

pub type Opening = opencode_runtime::machines::pages::Opening<crate::Route>;
