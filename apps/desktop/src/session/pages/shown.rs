//! Session opening lifecycle, shown-session recovery, stalled-load cutoff, and loading-row Retry.

use crate::pickers::PickerHost as _;

use super::machines::{HOLD, LOAD_TIMEOUT, OpenEvent, Opening};

impl crate::App {
    pub fn cancel_open(&mut self, tx: &mut rune::Tx) {
        if let Some(opening) = self.opening.take() {
            *self.route = opening.from;
            let _ = self.show_draft();
            session_view::jump::settle(self, tx);
        }
    }

    pub fn begin_open(&mut self, id: &str, tx: &mut rune::Tx) {
        self.close_picker(false);
        crate::subagents::shells::set_open(self, false);
        *self.kernel.slots.get_mut(&crate::todos::TODO_OPEN) = false;
        let from = self.opening.as_ref().filter(|o| !o.loading()).map_or_else(|| (*self.route).clone(), |o| o.from.clone());
        *self.opening = Some(Opening::new(id, from, self.transcripts.contains_key(id), *self.clock));
        self.open_now(id, tx);
        self.open_page(id, tx);
        self.poll_open(tx);
        if self.opening.is_some() { tx.wake_after(HOLD); }
    }

    pub(super) fn poll_open(&mut self, tx: &mut rune::Tx) -> bool {
        let Some(opening) = &*self.opening else { return false };
        if self.route != opening.from && self.route != crate::Route::here(opening.id.clone()) {
            *self.opening = None;
            return false;
        }
        let id = opening.id.clone();
        let ready = opening.received() && self.transcripts.get(&id).is_some_and(|t| self.composer_ext.attach.transcript_ready(t));
        if ready {
            *self.opening = None;
            // Mount the finished transcript at its end, or where it was read, without
            // resetting input or focus.
            session_view::jump::settle(self, tx);
            self.land_place(&id, tx);
            if let Some(page) = self.pages.pool.as_ref().and_then(|p| p.peek(&id)) { self.schedule_history(&page, tx); }
            return true;
        }
        let expired = opening.expired(self.live(), *self.clock);
        let stalled = opening.loading_stalled(self.live(), *self.clock);
        if expired && self.opening.as_mut().is_some_and(|o| o.step(OpenEvent::Expired)) {
            tx.wake_after(LOAD_TIMEOUT);
            session_view::jump::settle(self, tx);
            return true;
        }
        if stalled && self.opening.as_mut().is_some_and(|o| o.step(OpenEvent::Stalled)) {
            return true;
        }
        false
    }

    /// A window that shows a session with no messages and nothing reading them (a window restored
    /// or imported whose open never ran) reads them now, as a click on its tab does. Its loading
    /// row then ends in the messages or a stated error with Retry, never in a spinner that lasts.
    /// Once per session: the row's Retry asks again (`retry_open`).
    pub(super) fn read_shown_session(&mut self, tx: &mut rune::Tx) -> bool {
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

    /// True while session `id`'s initial message read is actively in flight and not stalled:
    /// turns off the loading spinner once `Opening` stalls past `LOAD_TIMEOUT` or the page lane
    /// stalls past `STALL_AFTER`.
    pub fn loading_in_flight_session(&self, id: &str) -> bool {
        if self.pages.is_stalled() {
            return false;
        }
        self.opening.as_ref().is_some_and(|o| o.id == id && o.actively_loading())
    }
}
