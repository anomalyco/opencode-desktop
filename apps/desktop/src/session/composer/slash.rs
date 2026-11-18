//! Desktop host adapter for `opencode-feature-composer-input::slash`
//! (`features/composer-input`).

pub use opencode_feature_composer_input::slash::*;

use opencode_kit::client::Server;
use rune::{Cx, El, TextEdit, Tx};
use session_view::palette::Pal;

use crate::suggestions::Suggest;
use crate::{App, Source};

impl SlashHost for App {
    fn slash(&self) -> &Slash {
        &self.add.slash
    }

    fn slash_mut(&mut self) -> &mut Slash {
        &mut self.add.slash
    }

    fn slash_suggest(&self) -> &Suggest {
        &self.suggest
    }

    fn slash_composer_mut(&mut self) -> &mut TextEdit {
        &mut self.composer
    }

    fn slash_directory(&self) -> String {
        self.composer_directory()
    }

    fn slash_live_server(&self) -> Option<Server> {
        match &*self.source {
            Source::Live { server, .. } => Some(server.clone()),
            Source::Fixture => None,
        }
    }

    fn builtin_slash_commands(&self) -> Vec<BuiltinSlashCommand> {
        let mut own: Vec<BuiltinSlashCommand> = self
            .commands()
            .into_iter()
            .filter(|c| !matches!(c.run, crate::commands::Run::Plugin(_)))
            .filter_map(|c| {
                let name = c.slash?;
                // Desktop's `/new` is `session.new`, which has no keys of its own (`use-session-commands.tsx:446-451`).
                let keys = c.keys.filter(|_| name != "new");
                Some(BuiltinSlashCommand {
                    id: c.id,
                    name: name.into(),
                    title: c.title.into(),
                    description: c.description.into(),
                    chord: keys.and_then(|b| b.chord()),
                    args: false,
                })
            })
            .collect();
        // Plugin commands, hidden ones too (`/btw`), as Desktop's `slashCommands` lists them.
        own.extend(crate::plugin_host::slash_commands(self).into_iter().map(|c| BuiltinSlashCommand {
            id: c.id,
            name: c.name.into(),
            title: c.title.into(),
            description: c.description.into(),
            // A plugin's keys show too, as Desktop's `/terminal` shows Ctrl+` (`editor.tsx:1400-1402`).
            chord: crate::keymap::command_chord(self, c.id),
            args: c.args,
        }));
        own
    }

    fn run_builtin_command(&mut self, id: &str, tx: &mut Tx) {
        if !crate::plugin_host::run_command(self, id, tx) {
            self.run_command(id, tx);
        }
    }

    fn run_slash_command(&mut self, id: &str, args: &str, tx: &mut Tx) {
        crate::plugin_host::run_slash(self, id, args, tx);
    }

    fn refresh_slash_suggestions(&mut self, tx: &mut Tx) {
        self.refresh_suggestions(tx);
    }
}

impl App {
    pub fn slash_query(&self) -> Option<String> {
        opencode_feature_composer_input::slash::slash_query(self)
    }

    pub fn slash_list(&self) -> Vec<Item> {
        opencode_feature_composer_input::slash::slash_list(self)
    }

    pub fn slash_items(&self) -> Vec<Item> {
        opencode_feature_composer_input::slash::slash_items(self)
    }

    pub fn slash_active(&self) -> Option<String> {
        opencode_feature_composer_input::slash::slash_active(self)
    }

    pub fn pick_command(&mut self, id: &str, menu: bool, tx: &mut Tx) {
        opencode_feature_composer_input::slash::pick_command(self, id, menu, tx);
    }

    pub fn run_slash(&mut self, text: &str, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::slash::run_slash(self, text, tx)
    }

    pub fn poll_commands(&mut self, tx: &mut Tx) -> bool {
        opencode_feature_composer_input::slash::poll_commands(self, tx)
    }

    pub fn slash_command(&self, text: &str) -> Option<(String, String)> {
        opencode_feature_composer_input::slash::slash_command(self, text)
    }
}

/// The `/` popover over the composer.
pub fn commands_view(s: &App, p: &Pal, cx: &Cx<App>) -> Option<El> {
    opencode_feature_composer_input::slash::commands_view::<App>(s, p, cx)
}
