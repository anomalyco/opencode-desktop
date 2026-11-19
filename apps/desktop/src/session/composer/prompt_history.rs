//! Desktop host adapter for `opencode-feature-composer-input::prompt_history`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::prompt_history::*;

use crate::App;

pub(crate) static PROMPTS: opencode_kernel::Slot<PromptHistory> = opencode_kernel::Slot::new();

impl App {
    /// Runs `send`, and records the prompt in the composer if that sent it (the composer is
    /// empty afterwards). A send that failed or waits leaves the text, and records nothing.
    pub fn record_sent(&mut self, send: impl FnOnce(&mut App)) {
        let (entry, shell) = PromptHistory::capture(&self.composer, self.kernel.slots.get(&crate::add_menu::ADD_MENU).shell());
        send(self);
        self.kernel.slots.get_mut(&PROMPTS).record_if_sent(&self.composer, entry, shell);
    }
}
