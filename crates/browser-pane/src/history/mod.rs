//! Persistent browsing history, frecency omnibox search, and read-only user-explicit browser
//! history import matching upstream `UP-106` (`#65073`, `packages/gui-plugins/src/browser/history/`
//! and `profile/history.ts`).
//!
//! - Stores at most `MAX_PAGES` (`50_000`) `http(s)` web pages in `history.sqlite` with
//!   `PRAGMA secure_delete = ON` so cleared history leaves zero remnants in free pages.
//! - Never records `file://`, `workspace://`, `artifact://`, `oc-preview://`, `about:`, `data:`,
//!   extension pages, credentialed URLs, or private/isolated preview contexts.
//! - Browser history import copies the source SQLite database into a private scratch directory
//!   and opens the copy strictly read-only (`SQLITE_OPEN_READ_ONLY`, `PRAGMA query_only = ON`)
//!   only on explicit user action, never on startup.

pub mod import;
pub mod page;
pub mod search;
pub mod store;

pub use import::{ImportBrowser, ImportEngine, ImportProfile, discover_sources, read_profile_history};
pub use page::{FoundPage, MAX_PAGES, MAX_TITLE_LENGTH, MAX_URL_LENGTH, PageVisit, bare, page_key, page_title, web_page};
pub use store::HistoryStore;
