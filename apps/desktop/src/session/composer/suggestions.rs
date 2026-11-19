//! Desktop host adapter for `opencode-feature-composer-input::suggestions`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::suggestions::*;

use rune::{KeyEvent, TextEdit, Tx};

use crate::App;
use crate::prompt_history::PromptHistory;

/// `App`-specialized history popover wrapper.
pub mod history {
    use rune::{Cx, El};
    use session_view::palette::Pal;

    use crate::App;

    pub fn view(s: &App, p: &Pal, cx: &Cx<App>) -> Option<El> {
        opencode_feature_composer_input::suggestions::history::view::<App>(s, p, cx)
    }
}

impl SuggestionsHost for App {
    fn suggest(&self) -> &Suggest {
        &self.composer_ext.suggest
    }

    fn suggest_mut(&mut self) -> &mut Suggest {
        &mut self.composer_ext.suggest
    }

    fn composer(&self) -> &TextEdit {
        &self.composer
    }

    fn composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn prompts(&self) -> &PromptHistory {
        &self.composer_ext.prompts
    }

    fn shell_mode(&self) -> bool {
        self.composer_ext.add.shell()
    }

    fn has_composer(&self) -> bool {
        App::has_composer(self)
    }

    fn history_key_blocked(&self) -> bool {
        self.composer_ext.add.open() || self.composer_ext.pickers.open().is_some() || self.kernel.slots.get(&crate::switcher::SWITCHER).is_open() || self.settings.open
    }

    fn composer_has_attachments_or_quotes(&self) -> bool {
        self.composer_ext.attach.any() || self.composer_ext.quote.any()
    }

    fn auto_focus_composer(&self, tx: &Tx) -> bool {
        crate::auto_focus_composer(tx)
    }

    fn mention_suggestion_ids(&self, query: &str) -> Vec<String> {
        let stale = !matches!(*self.source, crate::Source::Fixture) && self.composer_ext.mentions.search.files_stale(query);
        self.mention_items()
            .into_iter()
            .filter(|i| !(stale && i.kind == crate::mentions::Kind::File && !i.recent))
            .map(|i| i.key)
            .collect()
    }

    fn slash_suggestion_ids(&self) -> Vec<String> {
        self.slash_items().into_iter().map(|c| c.id).collect()
    }

    fn mention_whitespace_empty(&self, query: &str) -> bool {
        !self.composer_ext.mentions.search.sessions_pending(query, self.mention_server()) && self.mention_items().is_empty()
    }

    fn pick_mention(&mut self, key: &str) {
        App::pick_mention(self, key);
    }

    fn pick_command(&mut self, id: &str, menu: bool, tx: &mut Tx) {
        App::pick_command(self, id, menu, tx);
    }

    fn sync_mention_search(&mut self, tx: &mut Tx) {
        App::sync_mention_search(self, tx);
    }

    fn close_mention_search(&mut self) {
        let m = &mut *self.composer_ext.mentions;
        crate::mentions::search::step(&mut m.seq, &mut m.search, crate::mentions::search::Event::Closed);
    }

    fn on_command_trigger(&mut self) {
        crate::plugin_host::dormant::wake_kind(self, "commands", &["enabled", "hidden"]);
    }
}

impl App {
    pub fn suggest(&mut self, event: machine::Event<'_>, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::suggestions::suggest(self, event, tx)
    }

    pub fn suggestion_ids(&self) -> Vec<String> {
        opencode_feature_composer_input::suggestions::suggestion_ids(self)
    }

    pub fn refresh_suggestions(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::refresh_suggestions(self, tx);
    }

    pub fn suggest_input(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::suggest_input(self, tx);
    }

    pub fn suggest_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::suggestions::suggest_key(self, ev, tx)
    }

    pub fn history_key(&mut self, ev: &KeyEvent, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::suggestions::history_key(self, ev, tx)
    }

    pub fn open_commands(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::open_commands(self, tx);
    }

    pub fn open_context(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::open_context(self, tx);
    }

    pub fn suggest_mode(&mut self) {
        opencode_feature_composer_input::suggestions::suggest_mode(self);
    }

    pub fn suggest_drop(&mut self) {
        opencode_feature_composer_input::suggestions::suggest_drop(self);
    }

    pub fn suggest_edited(&mut self) {
        opencode_feature_composer_input::suggestions::suggest_edited(self);
    }

    pub fn suggest_query(&mut self, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::suggest_query(self, tx);
    }

    pub fn suggest_pointed(&mut self, id: &str, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::suggest_pointed(self, id, tx);
    }

    pub fn suggest_pick(&mut self, id: &str, tx: &mut Tx) {
        opencode_feature_composer_input::suggestions::suggest_pick(self, id, tx);
    }

    pub fn poll_suggestions(&mut self, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::suggestions::poll_suggestions(self, tx)
    }

    pub fn history_rows(&self, query: &str) -> Vec<(String, String)> {
        opencode_feature_composer_input::suggestions::history_rows(self, query)
    }
}
