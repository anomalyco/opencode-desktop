//! Tracks the most recent workspace file dragged out of a Files/Review tree row so dropping it
//! back onto the composer inserts an inline `@<rel>` mention pill (`file/drag.ts:29-31`,
//! `composer/attachments/attachments.ts:217-223`) while external drops still attach as files.

use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct DragTracker(RefCell<Option<(String, String, Instant)>>);

impl DragTracker {
    pub fn record(&self, physical: &str, rel: &str) {
        *self.0.borrow_mut() = Some((crate::pin_key(physical), rel.to_owned(), Instant::now()));
    }

    pub fn take(&self, path: &Path) -> Option<String> {
        let (dragged, rel, at) = self.0.borrow_mut().take()?;
        if at.elapsed() > Duration::from_secs(30) {
            return None;
        }
        (dragged == crate::pin_key(&path.to_string_lossy())).then_some(rel)
    }
}
