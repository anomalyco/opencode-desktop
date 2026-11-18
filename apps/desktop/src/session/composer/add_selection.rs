//! Add selection to context (`context.addSelection`, ⇧⌘L; `use-session-commands.tsx:376-393,
//! 597-606`): the lines selected in the side panel's open file go to the composer as a file
//! card (`editor.tsx:802-830`), and out with the next prompt as `file://<path>?start=&end=`
//! without a mention (`request.ts:187-196`).

use rune::{El, Tx, el, icon};

use crate::App;
use crate::attachments::remove_button;
use crate::keymap::Bind;
use crate::palette::Pal;

pub use opencode_runtime::ContextFile;

impl App {
    /// The side panel's open file, its tab's session, and the lines selected in it (`activeFile`,
    /// `file.selectedLines`).
    fn selected_lines(&self) -> Option<(String, Option<(u32, u32)>)> {
        let (session, path) = self.active_file_tab()?;
        let path = path?;
        let lines = self.files.sessions.get(&session).and_then(|w| w.docs.get(&path)).and_then(|d| d.selection()).map(|s| s.ordered());
        Some((path, lines))
    }

    /// Desktop's `canAddSelectionContext`: a file is open with lines selected.
    pub fn can_add_selection(&self) -> bool {
        self.selected_lines().is_some_and(|(_, lines)| lines.is_some())
    }

    pub fn add_selection(&mut self, tx: &mut Tx) {
        let Some((path, lines)) = self.selected_lines() else { return };
        let Some((start, end)) = lines else {
            return crate::status_toast::say(self, opencode_i18n::t!("toast.context.noLineSelection.title"), opencode_i18n::t!("toast.context.noLineSelection.description"));
        };
        tx.instant();
        let file = ContextFile { path, start, end };
        // One card per file and range (`contextItemKey`).
        if !self.attach.context.contains(&file) {
            self.attach.context.push(file);
        }
    }
}

/// The context cards, after the quotes and before the attachments, each with its hover remove.
pub fn chips(s: &App, p: &Pal) -> Vec<El> {
    s.attach.context.iter().map(|f| chip(p, f)).collect()
}

fn chip(p: &Pal, f: &ContextFile) -> El {
    let name = opencode_kit::path::basename(&f.path);
    let lines = if f.start == f.end { format!(":{}", f.start) } else { format!(":{}-{}", f.start, f.end) };
    let label = format!("{}{lines}", session_view::composer::attachments::type_label(name));
    let card = session_view::composer::attachments::context_card(p, name, icon(rune_theme_opencode::file_icons::file(&f.path), 12.0), label).help(f.path.clone());
    let gone = f.clone();
    let remove = remove_button(p, name, 160.0, move |s: &mut App| s.attach.context.retain(|c| *c != gone));
    el().key(format!("context-{}:{}-{}", f.path, f.start, f.end)).hover_group().child(card).child(remove)
}

/// The cards as the next prompt's files, leaving the composer without them.
pub fn take(s: &mut App) -> Vec<opencode_kit::client::FileInput> {
    let directory = s.composer_directory();
    std::mem::take(&mut s.attach.context).iter().map(|f| crate::mentions::context_input(&directory, &f.path, f.start, f.end)).collect()
}

rune_register::register!(crate::commands::COMMANDS: crate::commands::Cmd = [
    crate::commands::Cmd::new("context.addSelection", opencode_i18n::key!("command.context.addSelection"), |s, tx| s.add_selection(tx))
        .describe(opencode_i18n::key!("command.context.addSelection.description"))
        .keys(Bind::AddSelection)
        .when(App::can_add_selection),
]);
