//! Pure, host-independent data models for code and diff surfaces:
//! - [`review`]: review change modes, server & fixture diff loading, off-main-thread patch parsing,
//!   depth-first directory-first path sorting, tree building, and file filtering.
//! - [`delimited`]: decoded CSV / TSV table document and `SlabFind` cell index.
//! - [`label_find`]: cell/label find coordinator over [`opencode_search::SlabFind`] and
//!   [`crate::DocFind`].

pub mod delimited;
pub mod label_find;
pub mod review;

/// Bigger rendered previews (Markdown, CSV/TSV tables, inert HTML, Mermaid diagrams) fall back
/// to the virtualized source viewer above this byte threshold (1 MiB).
pub const RENDER_LIMIT: usize = 1024 * 1024;

pub use delimited::Table;
pub use label_find::{Document, LabelDoc, LabelFind, TableFind};
pub use review::{
    ChangeList, ChangeMode, FetchedDiffs, PreparedReview, ReviewFetchSpec, ReviewFileMeta,
    build_change_tree, collapsed_folders, encode_fs_path, encode_uri_component, fetch_live_diffs,
    fetch_live_file_text, filter_review_files, insert_folder_nodes, listed_paths,
    load_fixture_diffs, normalize_path, parse_diff_entry, parse_diff_list, prepare_narrow_paths,
    sort_review_paths,
};
