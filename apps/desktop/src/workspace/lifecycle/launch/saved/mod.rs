//! What a launch draws from the app's saved state before its server answers, each kept in the
//! boot record so the first frame has it with no file read of its own: the session list, the
//! models' names and the scroll bars' gutters; and the tab a restored subagent's page belongs to.

pub mod catalog;
pub mod gutters;
pub mod list;
pub mod page_tab;
