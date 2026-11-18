//! Virtualized diff viewer, state, navigation, and in-diff search.

pub mod chrome;
pub mod find;
pub mod nav;
pub mod state;
pub mod view;

pub use chrome::default_file_header;
pub use find::DiffFind;
pub use state::{DiffDocState, PreparedDiffDoc};
pub use view::DiffViewer;
