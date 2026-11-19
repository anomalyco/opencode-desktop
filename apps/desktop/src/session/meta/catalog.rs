//! Row lookup and family costs for a large session catalog.

pub use opencode_runtime::catalog::*;

pub(crate) static CATALOG: opencode_kernel::Slot<Catalog> = opencode_kernel::Slot::new();
