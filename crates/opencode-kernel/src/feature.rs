//! Feature registration contract.

use crate::KernelBuilder;

/// A self-contained feature module that registers its state slots, overlays, commands, and
/// lifecycle hooks into a [`KernelBuilder`] without editing central switchboards.
pub trait Feature<S> {
    /// Stable identifier for this feature (used for deduplication and diagnostics).
    fn id(&self) -> &'static str;

    /// Registers this feature's state slots, hooks, commands, and overlays.
    fn register(&self, builder: &mut KernelBuilder<S>);
}
