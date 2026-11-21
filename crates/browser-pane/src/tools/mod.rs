//! Pure-Rust browser agent tool helpers shared between `crates/browser-pane` and desktop's
//! `experimental.browser` v4 RPC executors: deep accessibility snapshots with sensitive-field
//! redaction, extension-filtered frame trees, bounded console/network diagnostics with header
//! redaction, and agent key/chord guards.

pub mod diagnostics;
pub mod field_value;
pub mod frames;
pub mod keys;
pub mod profiling;
pub mod snapshot;
