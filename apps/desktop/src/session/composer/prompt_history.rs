//! Desktop host adapter for `opencode-feature-composer-input::prompt_history`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::prompt_history::*;

use crate::App;

/// Binds desktop's document storage callbacks to `opencode-feature-composer-input::prompt_history`.
pub fn init() {
    configure(StorageOps {
        is_ready: crate::storage::is_ready,
        doc: |key| match crate::storage::doc(key) {
            opencode_storage::Lookup::Ready(v) => DocLookup::Ready(v),
            opencode_storage::Lookup::Pending => DocLookup::Pending,
        },
        load_docs: crate::storage::load_docs,
        set_doc: crate::storage::set_doc,
    });
}

impl App {
    /// Runs `send`, and records the prompt in the composer if that sent it (the composer is
    /// empty afterwards). A send that failed or waits leaves the text, and records nothing.
    pub fn record_sent(&mut self, send: impl FnOnce(&mut App)) {
        let (entry, shell) = PromptHistory::capture(&self.composer, self.add.shell());
        send(self);
        self.prompts.record_if_sent(&self.composer, entry, shell);
    }
}
