//! Desktop host adapter for `opencode-feature-composer-input::prompt_history`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::prompt_history::*;

use crate::App;

impl App {
    /// Runs `send`, and records the prompt in the composer if that sent it (the composer is
    /// empty afterwards). A send that failed or waits leaves the text, and records nothing.
    pub fn record_sent(&mut self, send: impl FnOnce(&mut App)) {
        let (entry, shell) = PromptHistory::capture(&self.composer, self.composer_ext.add.shell());
        send(self);
        self.composer_ext.prompts.record_if_sent(&self.composer, entry, shell);
    }
}
