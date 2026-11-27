//! A window that shows a session nothing is reading, and the loading row's Retry (`session_pages.poll`).

impl crate::App {
    /// A window that shows a session with no messages and nothing reading them (a window restored
    /// or imported whose open never ran) reads them now, as a click on its tab does. Its loading
    /// row then ends in the messages or a stated error with Retry, never in a spinner that lasts.
    /// Once per session while it stays shown: the row's Retry asks again (`retry_open`). The
    /// session's mark goes when the route leaves it or its messages land, so coming back to it, or
    /// its pages being evicted, reads it again instead of leaving the page without messages.
    pub(super) fn read_shown_session(&mut self, tx: &mut rune::Tx) -> bool {
        crate::launch::saved::page_tab::left(self);
        let shown = match &*self.route {
            crate::Route::Session { id, .. } => Some(id.as_str()),
            _ => None,
        };
        if self.launch.shown_read.as_deref().is_some_and(|read| shown != Some(read) || self.transcripts.contains_key(read)) {
            self.launch.shown_read = None;
        }
        let crate::Route::Session { id, server } = &*self.route else { return false };
        if *server != *crate::servers::runtime::active(self) || !self.live() || self.pages.pool.is_none() || self.opening.is_some() || self.transcripts.contains_key(id) {
            return false;
        }
        if self.launch.shown_read.as_deref() == Some(id.as_str()) {
            return false;
        }
        let id = id.clone();
        self.launch.shown_read = Some(id.clone());
        self.begin_open(&id, tx);
        true
    }

    /// The loading row's Retry: the session's messages are read again.
    pub fn retry_open(&mut self, id: &str, tx: &mut rune::Tx) {
        *self.opening = None;
        self.launch.shown_read = None;
        self.begin_open(id, tx);
    }
}
