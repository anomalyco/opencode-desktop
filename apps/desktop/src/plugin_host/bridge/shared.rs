//! What the host keeps for plugins across their reloads: what the app knows that plugins ask
//! about, and the images they draw. Their stored values are [`super::stores`].

use std::cell::RefCell;

#[derive(Default)]
pub struct Shared {
    pub facts: Facts,
    /// Images plugins handed over or loaded (`images`), drawn from renders too.
    pub images: RefCell<crate::plugin_host::images::Images>,
}

/// What the app knows that plugins ask about.
#[derive(Default)]
pub struct Facts {
    /// The app's own server's version, once it answered.
    pub server_version: Option<String>,
    /// Contributed servers' versions once they answered healthy, by `<plugin>:<id>`.
    pub health: std::collections::HashMap<String, String>,
}
